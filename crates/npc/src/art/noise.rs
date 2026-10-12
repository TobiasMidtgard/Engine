//! Deterministic noise for procedural meshes and textures: value and gradient noise in 2D and
//! 3D, fractal sums, ridged noise and cellular (Worley) noise. 2D functions take a `period`
//! (cells) so textures tile seamlessly; pass 0 for no tiling.

use engine_core::math::hash_u64;
use engine_core::prelude::*;

fn h3(x: i32, y: i32, z: i32, seed: u32) -> u64 {
    hash_u64(
        (x as u32 as u64)
            ^ ((y as u32 as u64) << 21)
            ^ ((z as u32 as u64) << 42)
            ^ ((seed as u64) << 13).rotate_left(29),
    )
}

/// Uniform 0..1 from integer coordinates.
pub fn rand3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    (h3(x, y, z, seed) >> 40) as f32 / (1u64 << 24) as f32
}

fn wrap(i: i32, period: i32) -> i32 {
    if period > 0 { i.rem_euclid(period) } else { i }
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn grad2(x: i32, y: i32, seed: u32) -> Vec2 {
    let a = rand3(x, y, 0, seed) * std::f32::consts::TAU;
    Vec2::new(a.cos(), a.sin())
}

fn grad3(x: i32, y: i32, z: i32, seed: u32) -> Vec3 {
    let u = rand3(x, y, z, seed) * 2.0 - 1.0;
    let a = rand3(x, y, z, seed ^ 0x51ed) * std::f32::consts::TAU;
    let r = (1.0 - u * u).max(0.0).sqrt();
    Vec3::new(r * a.cos(), r * a.sin(), u)
}

/// Gradient (Perlin-style) noise, about −0.7..0.7, tiling every `period` cells (0 = never).
pub fn perlin2(p: Vec2, period: i32, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (x0, y0) = (i.x as i32, i.y as i32);
    let g = |dx: i32, dy: i32| {
        let gr = grad2(wrap(x0 + dx, period), wrap(y0 + dy, period), seed);
        gr.dot(f - Vec2::new(dx as f32, dy as f32))
    };
    let (u, v) = (fade(f.x), fade(f.y));
    let a = g(0, 0) + (g(1, 0) - g(0, 0)) * u;
    let b = g(0, 1) + (g(1, 1) - g(0, 1)) * u;
    a + (b - a) * v
}

/// 3D gradient noise, about −0.8..0.8.
pub fn perlin3(p: Vec3, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (x0, y0, z0) = (i.x as i32, i.y as i32, i.z as i32);
    let g = |dx: i32, dy: i32, dz: i32| {
        grad3(x0 + dx, y0 + dy, z0 + dz, seed).dot(f - Vec3::new(dx as f32, dy as f32, dz as f32))
    };
    let (u, v, w) = (fade(f.x), fade(f.y), fade(f.z));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

/// Fractal sum of `octaves` layers of [`perlin2`], roughly −1..1. Each octave doubles the
/// frequency (and the period, so it keeps tiling) and scales the amplitude by `gain`.
pub fn fbm2(p: Vec2, period: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        let per = if period > 0 { period * freq as i32 } else { 0 };
        sum += amp * perlin2(p * freq, per, seed.wrapping_add(o * 101));
        norm += amp;
        amp *= gain;
        freq *= 2.0;
    }
    sum / norm * 1.4
}

/// Fractal sum of [`perlin3`], roughly −1..1.
pub fn fbm3(p: Vec3, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += amp * perlin3(p * freq, seed.wrapping_add(o * 101));
        norm += amp;
        amp *= gain;
        freq *= 2.0;
    }
    sum / norm * 1.3
}

/// Ridged fractal noise 0..1 (sharp crests: cracks, veins, wrinkles).
pub fn ridged2(p: Vec2, period: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        let per = if period > 0 { period * freq as i32 } else { 0 };
        let n = 1.0 - (perlin2(p * freq, per, seed.wrapping_add(o * 37)) * 1.4).abs();
        sum += amp * n * n;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// Cellular noise: distances to the nearest and second-nearest feature point (in cells) and
/// a random 0..1 id of the nearest cell. Tiles every `period` cells (0 = never).
pub fn worley2(p: Vec2, period: i32, seed: u32) -> (f32, f32, f32) {
    let i = p.floor();
    let (x0, y0) = (i.x as i32, i.y as i32);
    let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, 0.0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (wrap(x0 + dx, period), wrap(y0 + dy, period));
            let jitter = Vec2::new(rand3(cx, cy, 1, seed), rand3(cx, cy, 2, seed));
            let q = Vec2::new((x0 + dx) as f32, (y0 + dy) as f32) + jitter;
            let d = q.distance(p);
            if d < d1 {
                d2 = d1;
                d1 = d;
                id = rand3(cx, cy, 3, seed);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2, id)
}

/// 3D cellular noise: distance to the nearest feature point and its cell's random id.
pub fn worley3(p: Vec3, seed: u32) -> (f32, f32) {
    let i = p.floor();
    let (x0, y0, z0) = (i.x as i32, i.y as i32, i.z as i32);
    let (mut d1, mut id) = (f32::MAX, 0.0);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (cx, cy, cz) = (x0 + dx, y0 + dy, z0 + dz);
                let q = Vec3::new(cx as f32, cy as f32, cz as f32)
                    + Vec3::new(
                        rand3(cx, cy, cz, seed),
                        rand3(cx, cy, cz, seed ^ 7),
                        rand3(cx, cy, cz, seed ^ 13),
                    );
                let d = q.distance(p);
                if d < d1 {
                    d1 = d;
                    id = rand3(cx, cy, cz, seed ^ 99);
                }
            }
        }
    }
    (d1, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_and_stays_in_range() {
        for k in 0..200 {
            let p = Vec2::new(k as f32 * 0.137, k as f32 * 0.071);
            let a = fbm2(p, 8, 4, 0.5, 3);
            let b = fbm2(p + Vec2::new(8.0, 0.0), 8, 4, 0.5, 3);
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
            assert!(a.abs() <= 1.5);
            let (d1, d2, id) = worley2(p * 3.0, 6, 1);
            let (e1, _, eid) = worley2(p * 3.0 + Vec2::new(0.0, 6.0), 6, 1);
            assert!((d1 - e1).abs() < 1e-4 && id == eid && d2 >= d1);
            assert!((0.0..=1.0).contains(&ridged2(p, 4, 3, 2)));
        }
    }
}
