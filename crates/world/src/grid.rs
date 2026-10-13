//! Regular grids over world space, used for every environment field (wind, wetness, heat,
//! snow depth, fuel, smoke density…).
//!
//! Grids have an `f64` world origin so they work anywhere in a large world, and can be
//! re-centered ([`Grid2::recenter`]) to follow the player: cells that stay inside the new
//! window keep their values, new cells get the default.

use engine_core::math::{DVec2, DVec3, IVec2, IVec3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Values that can be bilinearly/trilinearly blended.
pub trait Blend: Copy {
    fn blend(a: Self, b: Self, t: f32) -> Self;
}

impl Blend for f32 {
    fn blend(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}
impl Blend for Vec2 {
    fn blend(a: Self, b: Self, t: f32) -> Self {
        a.lerp(b, t)
    }
}
impl Blend for Vec3 {
    fn blend(a: Self, b: Self, t: f32) -> Self {
        a.lerp(b, t)
    }
}

/// A 2D grid on the horizontal (XZ) plane. Cell `(0,0)` covers
/// `[origin.x, origin.x + cell) × [origin.y, origin.y + cell)` in world X/Z.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grid2<T> {
    /// World X/Z of the grid's minimum corner.
    pub origin: DVec2,
    /// Cell size in meters.
    pub cell: f32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<T>,
    pub default: T,
}

impl<T: Copy> Grid2<T> {
    pub fn new(origin: DVec2, cell: f32, width: u32, height: u32, default: T) -> Self {
        Self {
            origin,
            cell,
            width,
            height,
            data: vec![default; (width * height) as usize],
            default,
        }
    }

    /// A grid of `size` meters square centered on `center` (world X/Z).
    pub fn centered(center: DVec2, size: f32, cell: f32, default: T) -> Self {
        let n = (size / cell).ceil().max(1.0) as u32;
        let origin = center - DVec2::splat(n as f64 * cell as f64 * 0.5);
        Self::new(snap(origin, cell), cell, n, n, default)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn index(&self, c: IVec2) -> Option<usize> {
        (c.x >= 0 && c.y >= 0 && (c.x as u32) < self.width && (c.y as u32) < self.height)
            .then(|| (c.y as u32 * self.width + c.x as u32) as usize)
    }

    pub fn get(&self, c: IVec2) -> Option<T> {
        self.index(c).map(|i| self.data[i])
    }

    pub fn get_mut(&mut self, c: IVec2) -> Option<&mut T> {
        self.index(c).map(move |i| &mut self.data[i])
    }

    pub fn set(&mut self, c: IVec2, v: T) {
        if let Some(i) = self.index(c) {
            self.data[i] = v;
        }
    }

    /// Cell containing world position (X/Z of `p`).
    pub fn cell_at(&self, p: DVec3) -> IVec2 {
        let local = (DVec2::new(p.x, p.z) - self.origin) / self.cell as f64;
        IVec2::new(local.x.floor() as i32, local.y.floor() as i32)
    }

    /// World X/Z center of a cell.
    pub fn cell_center(&self, c: IVec2) -> DVec2 {
        self.origin + (c.as_dvec2() + DVec2::splat(0.5)) * self.cell as f64
    }

    /// World position of a cell center at height `y`.
    pub fn cell_center3(&self, c: IVec2, y: f64) -> DVec3 {
        let xz = self.cell_center(c);
        DVec3::new(xz.x, y, xz.y)
    }

    /// Value at the cell containing `p`, or the default outside the grid.
    pub fn sample_nearest(&self, p: DVec3) -> T {
        self.get(self.cell_at(p)).unwrap_or(self.default)
    }

    pub fn contains(&self, p: DVec3) -> bool {
        self.index(self.cell_at(p)).is_some()
    }

    pub fn fill(&mut self, v: T) {
        self.data.fill(v);
    }

    /// Iterate `(cell, &value)`.
    pub fn iter(&self) -> impl Iterator<Item = (IVec2, &T)> {
        let w = self.width;
        self.data
            .iter()
            .enumerate()
            .map(move |(i, v)| (IVec2::new((i as u32 % w) as i32, (i as u32 / w) as i32), v))
    }

    /// Move the grid by whole cells so it is centered on `center`, keeping overlapping values.
    /// Returns true if the grid moved.
    pub fn recenter(&mut self, center: DVec2) -> bool {
        let half = DVec2::new(self.width as f64, self.height as f64) * self.cell as f64 * 0.5;
        // Whole cells from the current origin, so values stay where they were in the world
        // even when the origin is off the global cell lattice (after a world shift).
        let shift = ((center - half - self.origin) / self.cell as f64).round();
        if shift == DVec2::ZERO {
            return false;
        }
        let new_origin = self.origin + shift * self.cell as f64;
        // A jump past the whole grid (a trip to another planet) keeps nothing, and its cell
        // count may not even fit in an i32.
        if shift.x.abs() >= self.width as f64 || shift.y.abs() >= self.height as f64 {
            self.data.fill(self.default);
            self.origin = new_origin;
            return true;
        }
        let (dx, dy) = (shift.x as i32, shift.y as i32);
        let mut next = vec![self.default; self.data.len()];
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                if let Some(v) = self.get(IVec2::new(x + dx, y + dy)) {
                    next[(y as u32 * self.width + x as u32) as usize] = v;
                }
            }
        }
        self.data = next;
        self.origin = new_origin;
        true
    }
}

impl<T: Blend> Grid2<T> {
    /// Bilinear sample at world X/Z (values are at cell centers).
    pub fn sample(&self, p: DVec3) -> T {
        let local = (DVec2::new(p.x, p.z) - self.origin) / self.cell as f64 - DVec2::splat(0.5);
        // Far outside, every corner is the default; clamping keeps `base + 1` from
        // overflowing i32 (a point on another planet is ~1e11 cells away).
        let size = DVec2::new(self.width as f64, self.height as f64);
        let local = local.clamp(DVec2::splat(-3.0), size + 3.0);
        let base = IVec2::new(local.x.floor() as i32, local.y.floor() as i32);
        let f = (local - base.as_dvec2()).as_vec2();
        let g = |dx, dy| {
            let c = (base + IVec2::new(dx, dy)).clamp(
                IVec2::ZERO,
                IVec2::new(self.width as i32 - 1, self.height as i32 - 1),
            );
            if !self.contains_cell_loose(base + IVec2::new(dx, dy)) {
                return self.default;
            }
            self.data[(c.y as u32 * self.width + c.x as u32) as usize]
        };
        let a = T::blend(g(0, 0), g(1, 0), f.x);
        let b = T::blend(g(0, 1), g(1, 1), f.x);
        T::blend(a, b, f.y)
    }

    fn contains_cell_loose(&self, c: IVec2) -> bool {
        c.x >= -1 && c.y >= -1 && c.x <= self.width as i32 && c.y <= self.height as i32
    }
}

/// A 3D grid (for wind, smoke, cloud density). Cell `(0,0,0)` starts at `origin`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grid3<T> {
    pub origin: DVec3,
    pub cell: f32,
    pub dims: [u32; 3],
    pub data: Vec<T>,
    pub default: T,
}

impl<T: Copy> Grid3<T> {
    pub fn new(origin: DVec3, cell: f32, dims: [u32; 3], default: T) -> Self {
        Self {
            origin,
            cell,
            dims,
            data: vec![default; (dims[0] * dims[1] * dims[2]) as usize],
            default,
        }
    }

    pub fn index(&self, c: IVec3) -> Option<usize> {
        let [w, h, d] = self.dims;
        (c.x >= 0
            && c.y >= 0
            && c.z >= 0
            && (c.x as u32) < w
            && (c.y as u32) < h
            && (c.z as u32) < d)
            .then(|| ((c.z as u32 * h + c.y as u32) * w + c.x as u32) as usize)
    }

    pub fn get(&self, c: IVec3) -> Option<T> {
        self.index(c).map(|i| self.data[i])
    }

    pub fn get_mut(&mut self, c: IVec3) -> Option<&mut T> {
        self.index(c).map(move |i| &mut self.data[i])
    }

    pub fn cell_at(&self, p: DVec3) -> IVec3 {
        let l = (p - self.origin) / self.cell as f64;
        IVec3::new(l.x.floor() as i32, l.y.floor() as i32, l.z.floor() as i32)
    }

    pub fn cell_center(&self, c: IVec3) -> DVec3 {
        self.origin + (c.as_dvec3() + DVec3::splat(0.5)) * self.cell as f64
    }

    pub fn sample_nearest(&self, p: DVec3) -> T {
        self.get(self.cell_at(p)).unwrap_or(self.default)
    }
}

impl<T: Blend> Grid3<T> {
    /// Trilinear sample (values at cell centers); default outside.
    pub fn sample(&self, p: DVec3) -> T {
        let l = (p - self.origin) / self.cell as f64 - DVec3::splat(0.5);
        // As in `Grid2::sample`: far outside is the default, and `b + 1` must not overflow.
        let l = l.clamp(
            DVec3::splat(-3.0),
            DVec3::new(
                self.dims[0] as f64,
                self.dims[1] as f64,
                self.dims[2] as f64,
            ) + 3.0,
        );
        let b = IVec3::new(l.x.floor() as i32, l.y.floor() as i32, l.z.floor() as i32);
        let f = (l - b.as_dvec3()).as_vec3();
        let g = |dx, dy, dz| self.get(b + IVec3::new(dx, dy, dz)).unwrap_or(self.default);
        let x00 = T::blend(g(0, 0, 0), g(1, 0, 0), f.x);
        let x10 = T::blend(g(0, 1, 0), g(1, 1, 0), f.x);
        let x01 = T::blend(g(0, 0, 1), g(1, 0, 1), f.x);
        let x11 = T::blend(g(0, 1, 1), g(1, 1, 1), f.x);
        T::blend(T::blend(x00, x10, f.y), T::blend(x01, x11, f.y), f.z)
    }
}

fn snap(p: DVec2, cell: f32) -> DVec2 {
    let c = cell as f64;
    DVec2::new((p.x / c).floor() * c, (p.y / c).floor() * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_between_cells() {
        let mut g = Grid2::new(DVec2::ZERO, 1.0, 2, 1, 0.0f32);
        g.set(IVec2::new(0, 0), 0.0);
        g.set(IVec2::new(1, 0), 1.0);
        let v = g.sample(DVec3::new(1.0, 0.0, 0.5));
        assert!((v - 0.5).abs() < 1e-5, "{v}");
    }

    #[test]
    fn recenter_keeps_overlap() {
        let mut g = Grid2::centered(DVec2::ZERO, 10.0, 1.0, 0.0f32);
        let p = DVec3::new(2.5, 0.0, 2.5);
        let c = g.cell_at(p);
        g.set(c, 7.0);
        assert!(g.recenter(DVec2::new(3.0, 0.0)));
        assert_eq!(g.sample_nearest(p), 7.0);
    }

    #[test]
    fn recenter_across_planets_clears_without_overflow() {
        let mut g = Grid2::centered(DVec2::ZERO, 10.0, 1.0, 0.0f32);
        g.fill(5.0);
        // Earth to Mars is ~1e11 m: more cells than an i32 holds.
        let far = DVec2::new(7.8e10, -2.3e11);
        assert!(g.recenter(far));
        assert!(g.data.iter().all(|&v| v == 0.0));
        let back = g.cell_center(IVec2::new(5, 5));
        assert!((back - far).length() < 1.0, "{back} vs {far}");
        // A jump just past the edge also clears; one just inside keeps the overlap.
        let mut g = Grid2::centered(DVec2::ZERO, 10.0, 1.0, 0.0f32);
        g.fill(1.0);
        assert!(g.recenter(DVec2::new(10.0, 0.0)));
        assert!(g.data.iter().all(|&v| v == 0.0));
        g.fill(2.0);
        assert!(g.recenter(DVec2::new(19.0, 0.0)));
        assert_eq!(g.data.iter().filter(|&&v| v == 2.0).count(), 10);
    }

    #[test]
    fn sampling_on_another_planet_is_the_default() {
        let far = DVec3::new(2.3e11, 1e9, -7.8e10);
        let mut g2 = Grid2::centered(DVec2::ZERO, 10.0, 1.0, 3.0f32);
        g2.fill(1.0);
        assert_eq!(g2.sample(far), 3.0);
        assert_eq!(g2.sample(-far), 3.0);
        let mut g3 = Grid3::new(DVec3::ZERO, 1.0, [4, 4, 4], 3.0f32);
        g3.data.fill(1.0);
        assert_eq!(g3.sample(far), 3.0);
        assert_eq!(g3.sample(-far), 3.0);
        // Clamping leaves nearby samples alone.
        assert_eq!(g2.sample(DVec3::new(0.2, 0.0, 0.3)), 1.0);
        assert_eq!(g3.sample(DVec3::splat(2.0)), 1.0);
    }
}
