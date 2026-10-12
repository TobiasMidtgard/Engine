//! Procedural, seamlessly tiling PBR textures.
//!
//! Noise functions take a position in **cells** and a `period` in cells, and repeat every
//! period, so a texture built from `uv * period` tiles without seams. [`bake`] evaluates a
//! [`Surface`] function over the unit square (on all cores) and produces the four images a
//! [`StandardMaterial`](engine_render::StandardMaterial) takes: base color (sRGB), normal
//! (from the height field), metallic-roughness (glTF: G = roughness, B = metallic) and
//! occlusion (R).

use engine_core::math::{Vec2, Vec3};
use engine_render::Image;

/// What a material looks like at one point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    /// Linear RGB albedo, 0..1.
    pub albedo: Vec3,
    /// Height in meters (relative; only its slope matters). Drives the normal map.
    pub height: f32,
    pub roughness: f32,
    pub metallic: f32,
    /// Ambient occlusion, 0 (fully occluded) .. 1.
    pub ao: f32,
}

impl Surface {
    pub fn new(albedo: Vec3, roughness: f32) -> Self {
        Self {
            albedo,
            height: 0.0,
            roughness,
            metallic: 0.0,
            ao: 1.0,
        }
    }
}

/// The images of one baked material.
#[derive(Clone)]
pub struct TextureSet {
    pub base_color: Image,
    pub normal: Image,
    pub metallic_roughness: Image,
    pub occlusion: Image,
}

/// Bakes `f(uv)` (uv in 0..1, tiling) into a `size`×`size` [`TextureSet`]. `meters` is how
/// many meters the texture covers (so height slopes become correct normals), and
/// `normal_strength` scales them.
pub fn bake(
    size: u32,
    meters: f32,
    normal_strength: f32,
    f: impl Fn(Vec2) -> Surface + Sync,
) -> TextureSet {
    let n = size.max(4) as usize;
    let mut samples = vec![Surface::new(Vec3::ZERO, 1.0); n * n];
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .min(16);
    let rows_per = n.div_ceil(threads);
    std::thread::scope(|s| {
        for (chunk_i, chunk) in samples.chunks_mut(rows_per * n).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, out) in chunk.iter_mut().enumerate() {
                    let idx = chunk_i * rows_per * n + i;
                    let (x, y) = (idx % n, idx / n);
                    let uv = Vec2::new((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
                    *out = f(uv);
                }
            });
        }
    });
    let texel = meters / n as f32;
    let h = |x: usize, y: usize| samples[(y % n) * n + (x % n)].height;
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let srgb = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    };
    let mut base = Vec::with_capacity(n * n * 4);
    let mut normal = Vec::with_capacity(n * n * 4);
    let mut mr = Vec::with_capacity(n * n * 4);
    let mut ao = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let s = samples[y * n + x];
            base.extend_from_slice(&[
                to8(srgb(s.albedo.x)),
                to8(srgb(s.albedo.y)),
                to8(srgb(s.albedo.z)),
                255,
            ]);
            // Central differences with wrap-around (tiling). Rows run top to bottom (+v);
            // tangent-space +Y points up the image (glTF/OpenGL).
            let dhdu = (h(x + 1, y) - h(x + n - 1, y)) / (2.0 * texel);
            let dhdv = (h(x, y + 1) - h(x, y + n - 1)) / (2.0 * texel);
            let nrm = Vec3::new(-dhdu * normal_strength, dhdv * normal_strength, 1.0).normalize();
            normal.extend_from_slice(&[
                to8(nrm.x * 0.5 + 0.5),
                to8(nrm.y * 0.5 + 0.5),
                to8(nrm.z * 0.5 + 0.5),
                255,
            ]);
            mr.extend_from_slice(&[255, to8(s.roughness), to8(s.metallic), 255]);
            let a = to8(s.ao);
            ao.extend_from_slice(&[a, a, a, 255]);
        }
    }
    let size = n as u32;
    TextureSet {
        base_color: Image::new(size, size, base, true),
        normal: Image::new(size, size, normal, false),
        metallic_roughness: Image::new(size, size, mr, false),
        occlusion: Image::new(size, size, ao, false),
    }
}

// ------------------------------------------------------------------------ noise

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h
}

/// A pseudo-random number in 0..1 for an integer lattice point.
pub fn rand2(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) & 0x00ff_ffff) as f32 / 16_777_216.0
}

fn wrap(i: i32, period: i32) -> i32 {
    i.rem_euclid(period.max(1))
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Tiling gradient (Perlin) noise, about −1..1. `p` in cells, repeats every `period` cells.
pub fn perlin(p: Vec2, period: i32, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (ix, iy) = (i.x as i32, i.y as i32);
    let grad = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let a = rand2(wrap(cx, period), wrap(cy, period), seed) * std::f32::consts::TAU;
        a.cos() * dx + a.sin() * dy
    };
    let n00 = grad(ix, iy, f.x, f.y);
    let n10 = grad(ix + 1, iy, f.x - 1.0, f.y);
    let n01 = grad(ix, iy + 1, f.x, f.y - 1.0);
    let n11 = grad(ix + 1, iy + 1, f.x - 1.0, f.y - 1.0);
    let (u, v) = (fade(f.x), fade(f.y));
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 1.414
}

/// Tiling value noise, 0..1.
pub fn value(p: Vec2, period: i32, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (ix, iy) = (i.x as i32, i.y as i32);
    let r = |x: i32, y: i32| rand2(wrap(x, period), wrap(y, period), seed);
    let (u, v) = (fade(f.x), fade(f.y));
    let a = r(ix, iy) + (r(ix + 1, iy) - r(ix, iy)) * u;
    let b = r(ix, iy + 1) + (r(ix + 1, iy + 1) - r(ix, iy + 1)) * u;
    a + (b - a) * v
}

/// Fractal (fBm) Perlin noise, about −1..1: `octaves` layers, each twice the frequency and
/// `gain` the amplitude of the last. Tiles every `period` cells of the first octave.
pub fn fbm(p: Vec2, period: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut q, mut per) = (p, period);
    for o in 0..octaves {
        sum += perlin(q, per, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= gain;
        q *= 2.0;
        per *= 2;
    }
    sum / norm.max(1e-6)
}

/// Ridged fBm, 0..1 (sharp creases: cracks, veins, scratches).
pub fn ridged(p: Vec2, period: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let (mut q, mut per) = (p, period);
    for o in 0..octaves {
        let v = 1.0 - perlin(q, per, seed.wrapping_add(o * 131)).abs();
        sum += v * v * amp;
        norm += amp;
        amp *= 0.5;
        q *= 2.0;
        per *= 2;
    }
    sum / norm.max(1e-6)
}

/// Tiling Worley (cellular) noise: distances to the nearest and second-nearest feature
/// points (in cells) and a random 0..1 id of the nearest cell.
pub fn worley(p: Vec2, period: i32, seed: u32) -> (f32, f32, f32) {
    let i = p.floor();
    let (ix, iy) = (i.x as i32, i.y as i32);
    let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, 0.0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix + dx, iy + dy);
            let (wx, wy) = (wrap(cx, period), wrap(cy, period));
            let fp = Vec2::new(
                cx as f32 + rand2(wx, wy, seed),
                cy as f32 + rand2(wx, wy, seed ^ 0x9e37),
            );
            let d = fp.distance(p);
            if d < d1 {
                d2 = d1;
                d1 = d;
                id = rand2(wx, wy, seed ^ 0x51ed);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2, id)
}

/// Smooth 0..1 step between `a` and `b`.
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear RGB from an sRGB hex color (`0xrrggbb`).
pub fn hex(rgb: u32) -> Vec3 {
    let c = |v: u32| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3::new(c((rgb >> 16) & 255), c((rgb >> 8) & 255), c(rgb & 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles() {
        for p in [Vec2::new(0.3, 0.7), Vec2::new(2.9, 1.1)] {
            let q = p + Vec2::new(8.0, 0.0);
            assert!((perlin(p, 8, 1) - perlin(q, 8, 1)).abs() < 1e-5);
            assert!((fbm(p, 8, 4, 0.5, 1) - fbm(q, 8, 4, 0.5, 1)).abs() < 1e-5);
            assert!((worley(p, 8, 1).0 - worley(q, 8, 1).0).abs() < 1e-5);
            assert!((value(p, 8, 1) - value(q, 8, 1)).abs() < 1e-5);
        }
    }

    #[test]
    fn bake_makes_four_images_and_flat_normals_for_flat_height() {
        let set = bake(16, 1.0, 1.0, |_| Surface::new(Vec3::splat(0.5), 0.7));
        assert_eq!(set.base_color.width, 16);
        let n = set.normal.pixel(3, 3);
        assert_eq!((n[0], n[1], n[2]), (128, 128, 255));
        let mr = set.metallic_roughness.pixel(0, 0);
        assert_eq!(mr[1], (0.7f32 * 255.0 + 0.5) as u8);
        assert!(set.base_color.srgb && !set.normal.srgb);
    }

    #[test]
    fn a_ramp_tilts_the_normal_away_from_the_slope() {
        // Height rising with u: the normal leans toward −u (red below 128).
        let set = bake(32, 1.0, 1.0, |uv| Surface {
            height: (uv.x * std::f32::consts::TAU).sin() * 0.01,
            ..Surface::new(Vec3::ONE, 0.5)
        });
        let n = set.normal.pixel(1, 5); // near u = 0, where the slope is steepest upward
        assert!(n[0] < 125, "{n:?}");
    }
}
