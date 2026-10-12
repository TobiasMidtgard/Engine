//! Fast, seeded noise for baking body textures: f32 gradient noise on a permutation table,
//! fractal sums with a rotation between octaves (no grid-aligned artifacts), integer hashes for
//! scattering craters and storms, and helpers for points on the unit sphere.

use engine_core::math::Vec3;

/// Seeded 3D gradient (Perlin) noise. Values are roughly in `[-1, 1]` (σ ≈ 0.25 per octave).
#[derive(Clone)]
pub struct Noise {
    perm: [u8; 512],
}

/// A fixed rotation applied between octaves so lattice axes never line up across octaves.
const OCTAVE_ROT: [[f32; 3]; 3] = [
    [0.000, 0.800, 0.600],
    [-0.800, 0.360, -0.480],
    [-0.600, -0.480, 0.640],
];

#[inline]
fn rot(p: Vec3) -> Vec3 {
    Vec3::new(
        OCTAVE_ROT[0][0] * p.x + OCTAVE_ROT[0][1] * p.y + OCTAVE_ROT[0][2] * p.z,
        OCTAVE_ROT[1][0] * p.x + OCTAVE_ROT[1][1] * p.y + OCTAVE_ROT[1][2] * p.z,
        OCTAVE_ROT[2][0] * p.x + OCTAVE_ROT[2][1] * p.y + OCTAVE_ROT[2][2] * p.z,
    )
}

/// `x.floor()` as an integer without a library call (baseline x86-64 has no rounding
/// instruction, so `f32::floor` is a call to `floorf`). For |x| < 2³¹.
#[inline(always)]
pub fn floor_i(x: f32) -> i32 {
    let i = x as i32;
    i - (x < i as f32) as i32
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Perlin's 16 gradient directions (12 cube edges, 4 repeated), as a table: no branches.
const GRAD: [[f32; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0],
];

#[inline]
fn grad(h: u8, x: f32, y: f32, z: f32) -> f32 {
    let g = &GRAD[(h & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

/// SplitMix64 step.
pub fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Hash of integer cell coordinates and a seed.
#[inline]
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add((x as u32).wrapping_mul(0x85eb_ca6b))
        ^ (y as u32).wrapping_mul(0xc2b2_ae35)
        ^ (z as u32).wrapping_mul(0x27d4_eb2f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

/// `[0, 1)` from a hash, picking 8 bits at `slot` (0..3) mixed with the rest.
#[inline]
pub fn unit(h: u32, slot: u32) -> f32 {
    let v = h.rotate_left(slot * 11).wrapping_mul(0x9e37_79b9);
    (v >> 8) as f32 / (1u32 << 24) as f32
}

impl Noise {
    pub fn new(seed: u64) -> Self {
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut s = seed;
        for i in (1..256usize).rev() {
            s = mix64(s);
            let j = (s % (i as u64 + 1)) as usize;
            p.swap(i, j);
        }
        let mut perm = [0u8; 512];
        for (i, v) in perm.iter_mut().enumerate() {
            *v = p[i & 255];
        }
        Self { perm }
    }

    /// Gradient noise at `p`, about `[-1, 1]`.
    #[inline]
    pub fn at(&self, p: Vec3) -> f32 {
        let (ix, iy, iz) = (floor_i(p.x), floor_i(p.y), floor_i(p.z));
        let (x, y, z) = (p.x - ix as f32, p.y - iy as f32, p.z - iz as f32);
        let xi = (ix & 255) as usize;
        let yi = (iy & 255) as usize;
        let zi = (iz & 255) as usize;
        let (u, v, w) = (fade(x), fade(y), fade(z));
        let pm = &self.perm;
        let a = pm[xi] as usize + yi;
        let aa = pm[a] as usize + zi;
        let ab = pm[a + 1] as usize + zi;
        let b = pm[xi + 1] as usize + yi;
        let ba = pm[b] as usize + zi;
        let bb = pm[b + 1] as usize + zi;
        lerp(
            lerp(
                lerp(grad(pm[aa], x, y, z), grad(pm[ba], x - 1.0, y, z), u),
                lerp(
                    grad(pm[ab], x, y - 1.0, z),
                    grad(pm[bb], x - 1.0, y - 1.0, z),
                    u,
                ),
                v,
            ),
            lerp(
                lerp(
                    grad(pm[aa + 1], x, y, z - 1.0),
                    grad(pm[ba + 1], x - 1.0, y, z - 1.0),
                    u,
                ),
                lerp(
                    grad(pm[ab + 1], x, y - 1.0, z - 1.0),
                    grad(pm[bb + 1], x - 1.0, y - 1.0, z - 1.0),
                    u,
                ),
                v,
            ),
            w,
        )
    }

    /// Fractal sum of `octaves` layers (frequency ×2, amplitude ×`gain` each), normalized so
    /// the result stays about `[-1, 1]` with σ ≈ 0.3.
    #[inline]
    pub fn fbm(&self, p: Vec3, octaves: u32, gain: f32) -> f32 {
        let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
        let mut q = p;
        for _ in 0..octaves {
            sum += amp * self.at(q);
            norm += amp;
            amp *= gain;
            q = rot(q) * 2.0 + Vec3::new(5.3, 1.7, 9.1);
        }
        sum / norm * 1.6
    }

    /// Ridged fractal (sharp crests where the noise crosses zero), about `[0, 1]`.
    #[inline]
    pub fn ridged(&self, p: Vec3, octaves: u32) -> f32 {
        let (mut sum, mut amp, mut norm, mut weight) = (0.0, 1.0, 0.0, 1.0f32);
        let mut q = p;
        for _ in 0..octaves {
            let n = 1.0 - self.at(q).abs() * 1.4;
            let n = n.max(0.0);
            let n = n * n * weight;
            weight = (n * 1.6).clamp(0.0, 1.0);
            sum += n * amp;
            norm += amp;
            amp *= 0.5;
            q = rot(q) * 2.0 + Vec3::new(2.9, 7.3, 4.1);
        }
        sum / norm
    }
}

/// Unit direction for latitude/longitude (radians): +Y north, longitude 0 toward +Z, east
/// toward +X (as `dir_from_lat_lon`).
#[inline]
pub fn dir(lat: f32, lon: f32) -> Vec3 {
    let (sl, cl) = lat.sin_cos();
    let (so, co) = lon.sin_cos();
    Vec3::new(cl * so, sl, cl * co)
}

/// Latitude and longitude (radians) of a unit direction.
#[inline]
pub fn lat_lon(d: Vec3) -> (f32, f32) {
    (d.y.clamp(-1.0, 1.0).asin(), d.x.atan2(d.z))
}

/// Wrap an angle to `(-π, π]`.
#[inline]
pub fn wrap_pi(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    // Most angles are at most a turn away: skip the division.
    if a.abs() <= PI {
        return a;
    }
    let b = if a > 0.0 { a - TAU } else { a + TAU };
    if b.abs() <= PI {
        return b;
    }
    let a = (a + PI).rem_euclid(TAU) - PI;
    if a <= -PI { a + TAU } else { a }
}

/// Smoothstep.
#[inline]
pub fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Great-circle angle between two unit vectors (radians), accurate for small angles.
#[inline]
pub fn angle_between(a: Vec3, b: Vec3) -> f32 {
    let c = a.cross(b).length();
    let d = a.dot(b);
    c.atan2(d)
}

/// Local east/north offsets (radians) of `d` from the point `center` (`center_lat`), for
/// shapes drawn around a point: the tangent-plane coordinates of the gnomonic projection.
#[inline]
pub fn local_offset(d: Vec3, center: Vec3) -> (f32, f32) {
    let east = Vec3::Y.cross(center).try_normalize().unwrap_or(Vec3::X);
    let north = center.cross(east);
    let c = d.dot(center).max(1e-3);
    (d.dot(east) / c, d.dot(north) / c)
}

/// Map coordinates of a latitude/longitude (radians): `u` 0..1 west to east from longitude
/// −180°, `v` 0..1 from the north pole to the south pole.
#[inline]
pub fn uv(lat: f32, lon: f32) -> (f32, f32) {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    ((lon + PI) / TAU, (FRAC_PI_2 - lat) / PI)
}

/// A low-resolution latitude/longitude grid of values sampled bilinearly: smooth fields
/// (warps, climate) are computed on it once instead of per texel.
pub struct Grid<const N: usize> {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; N]>,
}

impl<const N: usize> Grid<N> {
    /// Evaluate `f(direction)` at every cell center (rows from north to south), in parallel.
    pub fn bake(width: usize, height: usize, f: impl Fn(Vec3) -> [f32; N] + Sync) -> Self {
        Self::bake_lat_lon(width, height, |d, _, _| f(d))
    }

    /// Evaluate `f(direction, latitude, longitude)` at every cell center, in parallel.
    pub fn bake_lat_lon(
        width: usize,
        height: usize,
        f: impl Fn(Vec3, f32, f32) -> [f32; N] + Sync,
    ) -> Self {
        use std::f32::consts::{FRAC_PI_2, PI, TAU};
        let mut data = vec![[0.0; N]; width * height];
        super::bake::par_rows(&mut data, width, |y, row| {
            let lat = FRAC_PI_2 - (y as f32 + 0.5) / height as f32 * PI;
            for (x, out) in row.iter_mut().enumerate() {
                let lon = (x as f32 + 0.5) / width as f32 * TAU - PI;
                *out = f(dir(lat, lon), lat, lon);
            }
        });
        Self {
            width,
            height,
            data,
        }
    }

    /// Bilinear sample at a latitude/longitude (radians).
    #[inline]
    pub fn at(&self, lat: f32, lon: f32) -> [f32; N] {
        let (u, v) = uv(lat, lon);
        self.sample(u, v)
    }

    /// Bilinear sample at a unit direction.
    #[inline]
    pub fn at_dir(&self, d: Vec3) -> [f32; N] {
        let (lat, lon) = lat_lon(d);
        self.at(lat, lon)
    }

    /// Bilinear sample at map coordinates `u` (0..1 west to east, wraps) and `v` (0..1 north
    /// to south, clamps).
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> [f32; N] {
        let x = u * self.width as f32 - 0.5;
        let y = (v * self.height as f32 - 0.5).clamp(0.0, self.height as f32 - 1.0);
        let (x0, y0) = (floor_i(x), floor_i(y));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let w = self.width as i32;
        // Wrap around in longitude (a mask for the usual power-of-two widths).
        let (xa, xb) = if w & (w - 1) == 0 {
            ((x0 & (w - 1)) as usize, ((x0 + 1) & (w - 1)) as usize)
        } else {
            (x0.rem_euclid(w) as usize, (x0 + 1).rem_euclid(w) as usize)
        };
        let ya = y0.max(0) as usize;
        let yb = (ya + 1).min(self.height - 1);
        let (a, b) = (
            &self.data[ya * self.width + xa],
            &self.data[ya * self.width + xb],
        );
        let (c, d) = (
            &self.data[yb * self.width + xa],
            &self.data[yb * self.width + xb],
        );
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_smooth_and_seeded() {
        let a = Noise::new(1);
        let b = Noise::new(2);
        let mut max = 0.0f32;
        let mut differs = false;
        for i in 0..2000 {
            let p = Vec3::new(
                i as f32 * 0.137,
                i as f32 * 0.071 - 3.0,
                i as f32 * 0.029 + 1.0,
            );
            let v = a.at(p);
            max = max.max(v.abs());
            differs |= (v - b.at(p)).abs() > 1e-3;
            // Continuous: a tiny step changes it a tiny bit.
            assert!((a.at(p + Vec3::splat(1e-3)) - v).abs() < 0.02);
            let f = a.fbm(p, 5, 0.5);
            assert!(f.abs() < 2.0, "{f}");
            let r = a.ridged(p, 4);
            assert!((0.0..=1.0).contains(&r), "{r}");
        }
        assert!(max > 0.4 && max <= 1.2, "{max}");
        assert!(differs);
    }

    #[test]
    fn lat_lon_round_trips_and_grids_interpolate() {
        let d = dir(0.3, -2.0);
        let (lat, lon) = lat_lon(d);
        assert!((lat - 0.3).abs() < 1e-5 && (lon + 2.0).abs() < 1e-5);
        assert!((wrap_pi(3.5) - (3.5 - std::f32::consts::TAU)).abs() < 1e-5);
        let g = Grid::<1>::bake(64, 32, |d| [d.y]);
        // v = 0.25 is latitude 45°.
        let s = g.sample(0.3, 0.25)[0];
        assert!((s - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "{s}");
    }
}
