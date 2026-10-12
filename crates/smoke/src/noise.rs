//! The detail noise smoke is drawn with: a tiling 3D texture of billowy noise, made once.
//!
//! Red is Perlin-Worley noise (soft Perlin turbulence pinched into cauliflower lumps by Worley
//! cells); green, blue and alpha are inverted Worley noise at 2×, 4× and 8× its cell count
//! (round billows, finer and finer). Everything wraps, so the texture tiles seamlessly.

use engine_core::math::{IVec3, Vec3};

/// Hash of a lattice point (wrapped to `period`) and a channel, 0..1.
fn hash(p: IVec3, period: i32, salt: u32) -> f32 {
    let w = p.rem_euclid(IVec3::splat(period));
    let mut h = (w.x as u32).wrapping_mul(0x8da6_b343)
        ^ (w.y as u32).wrapping_mul(0xd816_3841)
        ^ (w.z as u32).wrapping_mul(0xcb1a_b31f)
        ^ salt.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn gradient(p: IVec3, period: i32) -> Vec3 {
    let a = hash(p, period, 1) * std::f32::consts::TAU;
    let z = hash(p, period, 2) * 2.0 - 1.0;
    let r = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(r * a.cos(), r * a.sin(), z)
}

fn fade(t: Vec3) -> Vec3 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Periodic gradient noise at `p` (in cells), roughly -1..1.
fn perlin(p: Vec3, period: i32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let i = i.as_ivec3();
    let u = fade(f);
    let mut sum = 0.0;
    for corner in 0..8 {
        let o = IVec3::new(corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
        let w = (if o.x == 1 { u.x } else { 1.0 - u.x })
            * (if o.y == 1 { u.y } else { 1.0 - u.y })
            * (if o.z == 1 { u.z } else { 1.0 - u.z });
        sum += w * gradient(i + o, period).dot(f - o.as_vec3());
    }
    sum * 1.4
}

/// Periodic inverted Worley noise at `p` (in cells): 1 at a feature point, falling to 0 between.
fn worley(p: Vec3, period: i32) -> f32 {
    let i = p.floor().as_ivec3();
    let mut nearest = f32::MAX;
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                let c = i + IVec3::new(x, y, z);
                let feature = c.as_vec3()
                    + Vec3::new(hash(c, period, 3), hash(c, period, 4), hash(c, period, 5));
                nearest = nearest.min(feature.distance_squared(p));
            }
        }
    }
    (1.0 - nearest.sqrt()).clamp(0.0, 1.0)
}

/// Three octaves of Worley noise starting at `cells` per tile.
fn worley_fbm(uvw: Vec3, cells: i32) -> f32 {
    worley(uvw * cells as f32, cells) * 0.625
        + worley(uvw * (cells * 2) as f32, cells * 2) * 0.25
        + worley(uvw * (cells * 4) as f32, cells * 4) * 0.125
}

fn remap(x: f32, lo: f32, hi: f32, new_lo: f32, new_hi: f32) -> f32 {
    new_lo + (x - lo) / (hi - lo).max(1e-6) * (new_hi - new_lo)
}

/// One texel of the detail texture at `uvw` (0..1 across the tile).
fn texel(uvw: Vec3) -> [u8; 4] {
    // Cells per tile of the coarsest noise; the finest Worley octave is 32× that.
    let cells = 2;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    // Perlin turbulence, three octaves.
    let mut turb = 0.0;
    let mut amp = 0.5;
    let mut freq = cells;
    for _ in 0..3 {
        turb += perlin(uvw * freq as f32, freq) * amp;
        amp *= 0.5;
        freq *= 2;
    }
    let turb = turb * 0.5 + 0.5;
    let billows = worley_fbm(uvw, cells);
    let pw = remap(turb, billows - 1.0, 1.0, 0.0, 1.0);
    [
        byte(pw),
        byte(worley_fbm(uvw, cells * 2)),
        byte(worley_fbm(uvw, cells * 4)),
        byte(worley_fbm(uvw, cells * 8)),
    ]
}

/// The detail texture, `size`³ RGBA8 texels (x fastest), tiling in every direction. Slices
/// are made in parallel.
pub fn detail_noise(size: u32) -> Vec<[u8; 4]> {
    let slice = (size * size) as usize;
    let mut out = vec![[0u8; 4]; slice * size as usize];
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(size as usize)
        .max(1);
    let per = (size as usize).div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk, slices) in out.chunks_mut(slice * per).enumerate() {
            scope.spawn(move || {
                for (i, t) in slices.iter_mut().enumerate() {
                    let z = chunk * per + i / slice;
                    let y = (i % slice) / size as usize;
                    let x = i % size as usize;
                    let uvw = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / size as f32;
                    *t = texel(uvw);
                }
            });
        }
    });
    // Stretch every channel to the full 0..255 range.
    for ch in 0..4 {
        let (lo, hi) = out
            .iter()
            .fold((255u8, 0u8), |(lo, hi), t| (lo.min(t[ch]), hi.max(t[ch])));
        let span = (hi.saturating_sub(lo)).max(1) as f32;
        for t in &mut out {
            t[ch] = ((t[ch] - lo) as f32 * 255.0 / span).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_and_spans_its_range() {
        let n = 16;
        let tex = detail_noise(n);
        assert_eq!(tex.len(), (n * n * n) as usize);
        for ch in 0..4 {
            let (lo, hi) = tex
                .iter()
                .fold((255u8, 0u8), |(lo, hi), t| (lo.min(t[ch]), hi.max(t[ch])));
            assert!(hi - lo > 100, "channel {ch} spans {lo}..{hi}");
        }
        // Opposite faces meet: the noise just past the last texel continues from the first.
        let p = Vec3::new(0.3, 0.7, 0.999);
        let q = Vec3::new(0.3, 0.7, -0.001);
        assert!((worley(p * 8.0, 8) - worley(q * 8.0, 8)).abs() < 0.05);
        assert!((perlin(p * 4.0, 4) - perlin(q * 4.0, 4)).abs() < 0.05);
    }
}
