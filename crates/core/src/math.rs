//! Math: `glam` types plus the small geometric helpers every module needs (bounds, rays,
//! frustums), smoothing functions, a deterministic RNG, and gradient noise for procedural
//! content.

pub use glam::{
    BVec3, DAffine3, DMat3, DMat4, DQuat, DVec2, DVec3, DVec4, EulerRot, IVec2, IVec3, IVec4, Mat2,
    Mat3, Mat4, Quat, UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4, dvec2, dvec3, ivec2, ivec3,
    uvec2, uvec3, vec2, vec3, vec4,
};
use serde::{Deserialize, Serialize};

pub use std::f32::consts::{PI, TAU};

/// Linear interpolation.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Where `v` falls between `a` and `b` (0..1, unclamped).
pub fn inverse_lerp(a: f32, b: f32, v: f32) -> f32 {
    if (b - a).abs() < f32::EPSILON {
        0.0
    } else {
        (v - a) / (b - a)
    }
}

/// Map `v` from `[a0, a1]` to `[b0, b1]`.
pub fn remap(v: f32, a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    lerp(b0, b1, inverse_lerp(a0, a1, v))
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Frame-rate independent exponential smoothing toward `target`.
/// `rate` is roughly "how many times per second the gap shrinks by e".
pub fn damp(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    lerp(current, target, 1.0 - (-rate * dt).exp())
}

pub fn damp_vec3(current: Vec3, target: Vec3, rate: f32, dt: f32) -> Vec3 {
    current.lerp(target, 1.0 - (-rate * dt).exp())
}

pub fn damp_dvec3(current: DVec3, target: DVec3, rate: f32, dt: f32) -> DVec3 {
    current.lerp(target, (1.0 - (-rate * dt).exp()) as f64)
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn from_center_half_extents(center: Vec3, half: Vec3) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut b = Self::EMPTY;
        for p in points {
            b.extend(p);
        }
        b
    }

    pub fn extend(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Bounding sphere radius around the center.
    pub fn radius(&self) -> f32 {
        self.half_extents().length()
    }

    /// Bounds of this box after an affine transform.
    pub fn transformed(&self, m: &Mat4) -> Aabb {
        let c = m.transform_point3(self.center());
        let h = self.half_extents();
        let abs = Mat3::from_cols(
            m.x_axis.truncate().abs(),
            m.y_axis.truncate().abs(),
            m.z_axis.truncate().abs(),
        );
        Aabb::from_center_half_extents(c, abs * h)
    }
}

/// A ray with an `f64` origin (large worlds) and a unit `f32` direction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ray {
    pub origin: DVec3,
    pub direction: Vec3,
}

impl Ray {
    pub fn new(origin: DVec3, direction: Vec3) -> Self {
        Self {
            origin,
            direction: direction.normalize_or_zero(),
        }
    }

    pub fn at(&self, t: f64) -> DVec3 {
        self.origin + self.direction.as_dvec3() * t
    }
}

/// Plane `n·p + d = 0` with `n` normalized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f32,
}

impl Plane {
    pub fn from_vec4(v: Vec4) -> Self {
        let len = v.truncate().length().max(1e-12);
        Self {
            normal: v.truncate() / len,
            d: v.w / len,
        }
    }

    pub fn signed_distance(&self, p: Vec3) -> f32 {
        self.normal.dot(p) + self.d
    }
}

/// View frustum for culling, in camera-relative space.
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    /// left, right, bottom, top, near, (far — may be at infinity)
    pub planes: [Plane; 6],
}

impl Frustum {
    /// Extract planes from a view-projection matrix (wgpu clip space, z in 0..1, works for
    /// reverse-Z and infinite far).
    pub fn from_view_proj(m: &Mat4) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        Self {
            planes: [
                Plane::from_vec4(r3 + r0),
                Plane::from_vec4(r3 - r0),
                Plane::from_vec4(r3 + r1),
                Plane::from_vec4(r3 - r1),
                Plane::from_vec4(r2),
                Plane::from_vec4(r3 - r2),
            ],
        }
    }

    pub fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes.iter().all(|p| {
            // Degenerate planes (infinite far) have near-zero normals: skip them.
            p.normal.length_squared() < 0.5 || p.signed_distance(center) >= -radius
        })
    }

    pub fn intersects_aabb(&self, b: &Aabb) -> bool {
        let c = b.center();
        let h = b.half_extents();
        self.planes.iter().all(|p| {
            if p.normal.length_squared() < 0.5 {
                return true;
            }
            let r = h.dot(p.normal.abs());
            p.signed_distance(c) >= -r
        })
    }
}

/// Small, fast, deterministic RNG (PCG32). Same seed ⇒ same sequence on every platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (seed << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed ^ 0x853c_49e6_748f_ea9b);
        rng.next_u32();
        rng
    }

    /// Derive an independent stream (e.g. per chunk or per entity).
    pub fn fork(&mut self, salt: u64) -> Rng {
        Rng::new(self.next_u64() ^ hash_u64(salt))
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    pub fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | self.next_u32() as u64
    }

    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.f32()
    }

    pub fn range_i32(&mut self, min: i32, max_exclusive: i32) -> i32 {
        if max_exclusive <= min {
            return min;
        }
        min + (self.next_u32() % (max_exclusive - min) as u32) as i32
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    /// Uniform point in the unit disk.
    pub fn in_unit_disk(&mut self) -> Vec2 {
        let r = self.f32().sqrt();
        let a = self.f32() * TAU;
        Vec2::new(r * a.cos(), r * a.sin())
    }

    /// Uniform direction on the unit sphere.
    pub fn unit_vector(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let a = self.f32() * TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), r * a.sin(), z)
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[self.next_u32() as usize % items.len()])
        }
    }
}

/// Stateless 64-bit hash (SplitMix64 finalizer). Handy for per-cell randomness.
pub fn hash_u64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Hash integer grid coordinates (+ seed) to a float in `[0, 1)`.
pub fn hash01(x: i64, y: i64, z: i64, seed: u64) -> f32 {
    let h = hash_u64(
        (x as u64).wrapping_mul(0x8da6_b343)
            ^ (y as u64).wrapping_mul(0xd816_3841)
            ^ (z as u64).wrapping_mul(0xcb1a_b31f)
            ^ seed,
    );
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Gradient noise and fractal sums, seeded and deterministic.
pub mod noise {
    use super::hash_u64;

    fn grad2(h: u64, x: f64, y: f64) -> f64 {
        let a = (h >> 32) as f64 / u32::MAX as f64 * std::f64::consts::TAU;
        a.cos() * x + a.sin() * y
    }

    fn grad3(h: u64, x: f64, y: f64, z: f64) -> f64 {
        match h & 15 {
            0 => x + y,
            1 => -x + y,
            2 => x - y,
            3 => -x - y,
            4 => x + z,
            5 => -x + z,
            6 => x - z,
            7 => -x - z,
            8 => y + z,
            9 => -y + z,
            10 => y - z,
            11 => -y - z,
            12 => x + y,
            13 => -x + y,
            14 => -y + z,
            _ => -y - z,
        }
    }

    fn fade(t: f64) -> f64 {
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    fn lerp(a: f64, b: f64, t: f64) -> f64 {
        a + (b - a) * t
    }

    fn h2(x: i64, y: i64, seed: u64) -> u64 {
        hash_u64((x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed)
    }

    fn h3(x: i64, y: i64, z: i64, seed: u64) -> u64 {
        hash_u64(
            (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
                ^ seed,
        )
    }

    /// 2D Perlin noise, roughly in `[-1, 1]`. `f64` input so it works at planet scale.
    pub fn perlin2(x: f64, y: f64, seed: u64) -> f32 {
        let (xi, yi) = (x.floor() as i64, y.floor() as i64);
        let (xf, yf) = (x - xi as f64, y - yi as f64);
        let (u, v) = (fade(xf), fade(yf));
        let n00 = grad2(h2(xi, yi, seed), xf, yf);
        let n10 = grad2(h2(xi + 1, yi, seed), xf - 1.0, yf);
        let n01 = grad2(h2(xi, yi + 1, seed), xf, yf - 1.0);
        let n11 = grad2(h2(xi + 1, yi + 1, seed), xf - 1.0, yf - 1.0);
        (lerp(lerp(n00, n10, u), lerp(n01, n11, u), v) * 1.414) as f32
    }

    /// 3D Perlin noise, roughly in `[-1, 1]`.
    pub fn perlin3(x: f64, y: f64, z: f64, seed: u64) -> f32 {
        let (xi, yi, zi) = (x.floor() as i64, y.floor() as i64, z.floor() as i64);
        let (xf, yf, zf) = (x - xi as f64, y - yi as f64, z - zi as f64);
        let (u, v, w) = (fade(xf), fade(yf), fade(zf));
        let g = |dx: i64, dy: i64, dz: i64| {
            grad3(
                h3(xi + dx, yi + dy, zi + dz, seed),
                xf - dx as f64,
                yf - dy as f64,
                zf - dz as f64,
            )
        };
        let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
        let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
        let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
        let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
        (lerp(lerp(x00, x10, v), lerp(x01, x11, v), w) * 0.95) as f32
    }

    /// Fractal Brownian motion over [`perlin2`]: `octaves` layers, each `lacunarity`× the
    /// frequency and `gain`× the amplitude of the previous. Normalized to about `[-1, 1]`.
    pub fn fbm2(x: f64, y: f64, seed: u64, octaves: u32, lacunarity: f64, gain: f32) -> f32 {
        let (mut sum, mut amp, mut norm, mut freq) = (0.0f32, 1.0f32, 0.0f32, 1.0f64);
        for i in 0..octaves {
            sum += amp * perlin2(x * freq, y * freq, seed.wrapping_add(i as u64 * 131));
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        sum / norm.max(1e-6)
    }

    /// Fractal Brownian motion over [`perlin3`].
    pub fn fbm3(x: f64, y: f64, z: f64, seed: u64, octaves: u32, lacunarity: f64, gain: f32) -> f32 {
        let (mut sum, mut amp, mut norm, mut freq) = (0.0f32, 1.0f32, 0.0f32, 1.0f64);
        for i in 0..octaves {
            sum += amp
                * perlin3(
                    x * freq,
                    y * freq,
                    z * freq,
                    seed.wrapping_add(i as u64 * 131),
                );
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        sum / norm.max(1e-6)
    }

    /// Ridged multifractal (sharp mountain ridges), in about `[0, 1]`.
    pub fn ridged2(x: f64, y: f64, seed: u64, octaves: u32) -> f32 {
        let (mut sum, mut amp, mut norm, mut freq) = (0.0f32, 1.0f32, 0.0f32, 1.0f64);
        let mut weight = 1.0f32;
        for i in 0..octaves {
            let n = 1.0 - perlin2(x * freq, y * freq, seed.wrapping_add(i as u64 * 977)).abs();
            let n = n * n * weight;
            weight = (n * 2.0).clamp(0.0, 1.0);
            sum += n * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm.max(1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        let v = Rng::new(1).f32();
        assert!((0.0..1.0).contains(&v));
    }

    #[test]
    fn noise_is_bounded_and_continuous() {
        let mut max = 0.0f32;
        for i in 0..1000 {
            let x = i as f64 * 0.137;
            let n = noise::perlin2(x, x * 0.7, 7);
            max = max.max(n.abs());
            let d = (noise::perlin2(x + 1e-4, x * 0.7, 7) - n).abs();
            assert!(d < 0.01);
        }
        assert!(max <= 1.2 && max > 0.2);
    }

    #[test]
    fn frustum_culls_behind_camera() {
        #[allow(deprecated)]
        let proj = Mat4::perspective_infinite_reverse_rh(1.0, 1.0, 0.1);
        let f = Frustum::from_view_proj(&proj);
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, -10.0), 1.0));
        assert!(!f.intersects_sphere(Vec3::new(0.0, 0.0, 10.0), 1.0));
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, -1.0e7), 1.0));
    }
}
