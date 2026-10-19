//! Seeded 3D gradient noise in `f64` with analytic derivatives, for planet-scale terrain.
//!
//! Inputs are meters (or meters divided by a wavelength), so a planet 6 371 km in radius gets
//! centimeter detail with no precision loss. Lattice hashing is a 64-bit integer mix (no
//! permutation table), so the noise never repeats.

use engine_core::math::{DMat3, DVec3};

/// Integer lattice hash (splitmix-style finalizer).
#[inline]
fn hash3(x: i64, y: i64, z: i64, seed: u64) -> u32 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 32) as u32
}

/// 64 well-mixed bits for a lattice point (three 21-bit random numbers and change).
#[inline]
fn hash3_wide(x: i64, y: i64, z: i64, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 32)
}

/// `(sin x, cos x)` to about 1e-7, several times faster than the libm pair (for phases of a
/// few turns; the reduction loses precision far from zero).
#[inline]
pub fn sin_cos_fast(x: f64) -> (f64, f64) {
    use std::f64::consts::{FRAC_2_PI, FRAC_PI_2};
    let k = (x * FRAC_2_PI).round();
    let y = x - k * FRAC_PI_2;
    let y2 = y * y;
    let s = y * (1.0 + y2 * (-1.0 / 6.0 + y2 * (1.0 / 120.0 + y2 * (-1.0 / 5040.0))));
    let c = 1.0 + y2 * (-0.5 + y2 * (1.0 / 24.0 + y2 * (-1.0 / 720.0 + y2 / 40_320.0)));
    match (k as i64) & 3 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// The 12 cube-edge gradients (Perlin's improved noise), padded to 16.
const GRADIENTS: [[f64; 3]; 16] = [
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
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [0.0, -1.0, -1.0],
];

#[inline]
fn grad(x: i64, y: i64, z: i64, seed: u64) -> DVec3 {
    let g = GRADIENTS[(hash3(x, y, z, seed) & 15) as usize];
    DVec3::new(g[0], g[1], g[2])
}

/// Gradient noise at `p` (about `[-1, 1]`) and its gradient (∂/∂x, ∂/∂y, ∂/∂z).
///
/// Quintic interpolation, derivatives after Iñigo Quilez ("gradient noise derivatives").
pub fn gradient3(p: DVec3, seed: u64) -> (f64, DVec3) {
    let i = p.floor();
    let f = p - i;
    let (ix, iy, iz) = (i.x as i64, i.y as i64, i.z as i64);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);

    let ga = grad(ix, iy, iz, seed);
    let gb = grad(ix + 1, iy, iz, seed);
    let gc = grad(ix, iy + 1, iz, seed);
    let gd = grad(ix + 1, iy + 1, iz, seed);
    let ge = grad(ix, iy, iz + 1, seed);
    let gf = grad(ix + 1, iy, iz + 1, seed);
    let gg = grad(ix, iy + 1, iz + 1, seed);
    let gh = grad(ix + 1, iy + 1, iz + 1, seed);

    let va = ga.dot(f);
    let vb = gb.dot(f - DVec3::new(1.0, 0.0, 0.0));
    let vc = gc.dot(f - DVec3::new(0.0, 1.0, 0.0));
    let vd = gd.dot(f - DVec3::new(1.0, 1.0, 0.0));
    let ve = ge.dot(f - DVec3::new(0.0, 0.0, 1.0));
    let vf = gf.dot(f - DVec3::new(1.0, 0.0, 1.0));
    let vg = gg.dot(f - DVec3::new(0.0, 1.0, 1.0));
    let vh = gh.dot(f - DVec3::new(1.0, 1.0, 1.0));

    let value = va
        + u.x * (vb - va)
        + u.y * (vc - va)
        + u.z * (ve - va)
        + u.x * u.y * (va - vb - vc + vd)
        + u.y * u.z * (va - vc - ve + vg)
        + u.z * u.x * (va - vb - ve + vf)
        + u.x * u.y * u.z * (-va + vb + vc - vd + ve - vf - vg + vh);

    let uyzx = DVec3::new(u.y, u.z, u.x);
    let uzxy = DVec3::new(u.z, u.x, u.y);
    let d = ga
        + u.x * (gb - ga)
        + u.y * (gc - ga)
        + u.z * (ge - ga)
        + u.x * u.y * (ga - gb - gc + gd)
        + u.y * u.z * (ga - gc - ge + gg)
        + u.z * u.x * (ga - gb - ge + gf)
        + u.x * u.y * u.z * (-ga + gb + gc - gd + ge - gf - gg + gh)
        + du * (DVec3::new(vb - va, vc - va, ve - va)
            + uyzx * DVec3::new(va - vb - vc + vd, va - vc - ve + vg, va - vb - ve + vf)
            + uzxy * DVec3::new(va - vb - ve + vf, va - vb - vc + vd, va - vc - ve + vg)
            + uyzx * uzxy * (-va + vb + vc - vd + ve - vf - vg + vh));
    (value, d)
}

/// Gradient noise value only.
#[inline]
pub fn noise3(p: DVec3, seed: u64) -> f64 {
    gradient3(p, seed).0
}

/// Per-octave rotation so octaves don't align on the lattice (removes grid artifacts).
#[inline]
fn twist(p: DVec3) -> DVec3 {
    // A fixed rotation (about 37° around a skew axis) followed by the 2x frequency step.
    DVec3::new(
        0.80 * p.x - 0.36 * p.y + 0.48 * p.z,
        0.60 * p.x + 0.48 * p.y - 0.64 * p.z,
        0.00 * p.x + 0.80 * p.y + 0.60 * p.z,
    ) * 2.02
}

/// The inverse-transpose companion of [`twist`] for carrying gradients back.
#[inline]
fn twist_grad(g: DVec3) -> DVec3 {
    // Rotation matrix R above is orthonormal, so dG/dp = 2.02 * R^T * g.
    DVec3::new(
        0.80 * g.x + 0.60 * g.y + 0.00 * g.z,
        -0.36 * g.x + 0.48 * g.y + 0.80 * g.z,
        0.48 * g.x - 0.64 * g.y + 0.60 * g.z,
    ) * 2.02
}

/// Fractal sum of `octaves` noise layers starting at wavelength 1 (in units of `p`), each
/// half the wavelength and `gain` times the amplitude. Returns about `[-1, 1]`.
pub fn fbm(p: DVec3, seed: u64, octaves: u32, gain: f64) -> f64 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let mut q = p;
    for i in 0..octaves {
        sum += amp * noise3(q, seed.wrapping_add(i as u64 * 0x51ED));
        norm += amp;
        amp *= gain;
        q = twist(q);
    }
    sum / norm.max(1e-9)
}

/// Fractal noise whose last octave may be fractional (`octaves` = 4.3 adds 30% of a fifth
/// layer), so detail fades in smoothly as the level of detail increases.
pub fn fbm_smooth(p: DVec3, seed: u64, octaves: f64, gain: f64) -> f64 {
    let whole = octaves.floor().max(0.0) as u32;
    let frac = octaves - whole as f64;
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let mut q = p;
    for i in 0..=whole {
        let w = if i == whole { frac } else { 1.0 };
        if w <= 0.0 {
            break;
        }
        sum += amp * w * noise3(q, seed.wrapping_add(i as u64 * 0x51ED));
        norm += amp * w;
        amp *= gain;
        q = twist(q);
    }
    sum / norm.max(1e-9)
}

/// Ridged multifractal: sharp crests like mountain ridges, about `[0, 1]`. `octaves` may be
/// fractional (see [`fbm_smooth`]).
pub fn ridged(p: DVec3, seed: u64, octaves: f64) -> f64 {
    ridged_worn(p, seed, octaves, 0.0)
}

/// [`ridged`] with its crests worn round: `wear` 0 is exactly [`ridged`], 1 leaves broad,
/// rounded massifs with fainter small ridges on them (mountains on an airless world, worn
/// down by billions of years of small impacts instead of cut by water and ice).
pub fn ridged_worn(p: DVec3, seed: u64, octaves: f64, wear: f64) -> f64 {
    let whole = octaves.floor().max(0.0) as u32;
    let frac = octaves - whole as f64;
    let (mut sum, mut amp, mut norm, mut weight) = (0.0, 1.0, 0.0, 1.0f64);
    // A crest is the fold `1 - |n|`; `1 - sqrt(n² + k²)` rounds it over a width of about k.
    let k = 0.3 * wear;
    let gain = 0.5 - 0.15 * wear;
    let mut q = p;
    for i in 0..=whole {
        let w = if i == whole { frac } else { 1.0 };
        if w <= 0.0 {
            break;
        }
        let v = noise3(q, seed.wrapping_add(i as u64 * 0x3C6E));
        let n = (1.0 - (v * v + k * k).sqrt()) / (1.0 - k);
        let n = n * n * weight;
        weight = (n * 1.8).clamp(0.0, 1.0);
        sum += n * amp * w;
        norm += amp * w;
        amp *= gain;
        q = twist(q);
    }
    sum / norm.max(1e-9)
}

/// "Eroded" fractal noise: each octave is damped by the slope accumulated so far, so slopes
/// stay smooth and detail gathers on flats and crests, which reads like weathered terrain
/// (after Iñigo Quilez). `tangent` removes the radial part of gradients on a sphere (pass the
/// surface normal; `None` for flat terrain). Returns about `[-1, 1]`.
pub fn eroded(p: DVec3, seed: u64, octaves: f64, gain: f64, tangent: Option<DVec3>) -> f64 {
    let whole = octaves.floor().max(0.0) as u32;
    let frac = octaves - whole as f64;
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    let mut slope = DVec3::ZERO;
    let mut q = p;
    // Chain rule for the twisted coordinates: d(octave)/dp = J^T g.
    let mut jac_scale = 1.0;
    for i in 0..=whole {
        let w = if i == whole { frac } else { 1.0 };
        if w <= 0.0 {
            break;
        }
        let (n, mut g) = gradient3(q, seed.wrapping_add(i as u64 * 0x7A3B));
        // Bring the gradient back to `p` space.
        for _ in 0..i {
            g = twist_grad(g) / 2.02;
        }
        g *= jac_scale;
        if let Some(t) = tangent {
            g -= t * g.dot(t);
        }
        slope += g * amp;
        let damp = 1.0 / (1.0 + slope.length_squared() * 0.35);
        sum += amp * w * n * damp;
        norm += amp * w;
        amp *= gain;
        q = twist(q);
        jac_scale *= 2.02;
    }
    sum / norm.max(1e-9)
}

/// `twist` as a matrix acting on gradients: the gradient of `f(twist(p))` is `TWIST_T * ∇f`.
const TWIST_T: DMat3 = DMat3::from_cols(
    DVec3::new(0.80 * 2.02, -0.36 * 2.02, 0.48 * 2.02),
    DVec3::new(0.60 * 2.02, 0.48 * 2.02, -0.64 * 2.02),
    DVec3::new(0.00, 0.80 * 2.02, 0.60 * 2.02),
);

/// Weight of octave `i` when `octaves` (possibly fractional) are wanted.
#[inline]
fn octave_weight(i: u32, octaves: f64) -> f64 {
    (octaves - i as f64).clamp(0.0, 1.0)
}

/// [`fbm_smooth`] of `p / wavelength`, with gradients (per unit of `p`) for steering.
///
/// The value sums `octaves` layers (fractional for level of detail, exactly like
/// [`fbm_smooth`]). Gradient `k` sums the first `grad_octaves` layers whose wavelength is at
/// least `cutoffs[k]` (in units of `p`): a low-passed slope whose direction varies no faster
/// than that, and which never changes with the level of detail.
pub fn fbm_d<const N: usize>(
    p: DVec3,
    wavelength: f64,
    seed: u64,
    octaves: f64,
    grad_octaves: u32,
    gain: f64,
    cutoffs: &[f64; N],
) -> (f64, [DVec3; N]) {
    let n = (octaves.ceil().max(0.0) as u32).max(grad_octaves);
    let (mut sum, mut norm, mut amp) = (0.0, 0.0, 1.0);
    let mut grads = [DVec3::ZERO; N];
    let mut gnorm = 0.0;
    let mut q = p / wavelength;
    let mut jac = DMat3::IDENTITY / wavelength;
    let mut wl = wavelength;
    for i in 0..n {
        let (v, g) = gradient3(q, seed.wrapping_add(i as u64 * 0x51ED));
        let w = octave_weight(i, octaves);
        sum += amp * w * v;
        norm += amp * w;
        if i < grad_octaves {
            let dg = jac * g * amp;
            for (grad, cutoff) in grads.iter_mut().zip(cutoffs) {
                if wl >= *cutoff {
                    *grad += dg;
                }
            }
            gnorm += amp;
        }
        amp *= gain;
        q = twist(q);
        jac *= TWIST_T;
        wl /= 2.02;
    }
    let inv = 1.0 / gnorm.max(1e-9);
    (sum / norm.max(1e-9), grads.map(|g| g * inv))
}

/// [`ridged`] of `p / wavelength`, with low-passed gradients for steering as in [`fbm_d`].
///
/// The gradients are rounded off at the crests (as if they were a tenth of a wavelength
/// wide), so they turn continuously from one side of a ridge to the other: directions taken
/// from them (which way gullies run) never jump.
///
/// Also returns how close `p` is to a crest of the first layer (1 on it, 0 a tenth of a
/// wavelength away), where those directions turn fastest.
pub fn ridged_d<const N: usize>(
    p: DVec3,
    wavelength: f64,
    seed: u64,
    octaves: f64,
    grad_octaves: u32,
    cutoffs: &[f64; N],
) -> (f64, [DVec3; N], f64) {
    let mut crest = 0.0;
    let n = (octaves.ceil().max(0.0) as u32).max(grad_octaves);
    let (mut sum, mut norm, mut amp) = (0.0, 0.0, 1.0);
    let mut grads = [DVec3::ZERO; N];
    let mut gnorm = 0.0;
    let (mut weight, mut dweight) = (1.0f64, DVec3::ZERO);
    let mut q = p / wavelength;
    let mut jac = DMat3::IDENTITY / wavelength;
    let mut wl = wavelength;
    for i in 0..n {
        let (v, g) = gradient3(q, seed.wrapping_add(i as u64 * 0x3C6E));
        let a = 1.0 - v.abs();
        let da = jac * g * (-v / (v * v + 0.03).sqrt());
        if i == 0 {
            crest = (-(v * v) / 0.06).exp();
        }
        let nv = a * a * weight;
        let dn = da * (2.0 * a * weight) + dweight * (a * a);
        let w = octave_weight(i, octaves);
        sum += nv * amp * w;
        norm += amp * w;
        if i < grad_octaves {
            for (grad, cutoff) in grads.iter_mut().zip(cutoffs) {
                if wl >= *cutoff {
                    *grad += dn * amp;
                }
            }
            gnorm += amp;
        }
        let k = nv * 1.8;
        // The weight saturates at 1; its gradient fades out just before, not at once.
        (weight, dweight) = if k <= 0.0 {
            (0.0, DVec3::ZERO)
        } else if k >= 1.0 {
            (1.0, DVec3::ZERO)
        } else {
            let fade = ((1.0 - k) * 6.0).min(1.0);
            (k, dn * (1.8 * fade))
        };
        amp *= 0.5;
        q = twist(q);
        jac *= TWIST_T;
        wl /= 2.02;
    }
    let inv = 1.0 / gnorm.max(1e-9);
    (sum / norm.max(1e-9), grads.map(|g| g * inv), crest)
}

/// Gully stripes: parallel waves that run along the slope, with a random phase per lattice
/// cell, blended across cells so they fork and merge like a drainage network ("phacelle"
/// noise, after Rune Skovbo Johansen, in 3D so it works on a sphere).
///
/// `across` is a unit tangent across the slope (perpendicular to downhill); the waves
/// repeat `freq` times per cell along it, so their crests run downhill. Returns the wave as
/// `(cos φ, sin φ)` with length ≤ 1: 1 almost everywhere, shrinking toward 0 only right at
/// a fork, where neighbouring cells disagree. `φ = 0` is a spur crest, `φ = ±π` a gully
/// floor; φ grows by 2π per `1 / freq` cells along `across`.
pub fn stripes(q: DVec3, across: DVec3, seed: u64, freq: f64) -> (f64, f64) {
    let i = q.floor();
    let f = q - i;
    let (ix, iy, iz) = (i.x as i64, i.y as i64, i.z as i64);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let k = std::f64::consts::TAU * freq;
    let (mut c, mut s) = (0.0, 0.0);
    for corner in 0..8u32 {
        let (ox, oy, oz) = (corner & 1, (corner >> 1) & 1, corner >> 2);
        let w = (if ox == 1 { u.x } else { 1.0 - u.x })
            * (if oy == 1 { u.y } else { 1.0 - u.y })
            * (if oz == 1 { u.z } else { 1.0 - u.z });
        if w < 1e-7 {
            continue;
        }
        let h = hash3_wide(ix + ox as i64, iy + oy as i64, iz + oz as i64, seed);
        // A random point in the cell around the corner; the waves are measured from it, so
        // flipping `across` (the other side of a crest) gives the mirror image, not a jump.
        let jitter = DVec3::new(
            (h & 0x1F_FFFF) as f64,
            ((h >> 21) & 0x1F_FFFF) as f64,
            ((h >> 42) & 0x1F_FFFF) as f64,
        ) * (1.0 / 2_097_152.0)
            - 0.5;
        let d = f - DVec3::new(ox as f64, oy as f64, oz as f64) - jitter;
        let (sn, cs) = sin_cos_fast(k * d.dot(across));
        c += w * cs;
        s += w * sn;
    }
    // Normalize (full contrast) but softly, so forks stay continuous.
    let inv = 1.0 / (c * c + s * s + 0.03).sqrt();
    (c * inv, s * inv)
}

/// Value in `[0, 1)` for an integer cell (for scattering features like craters).
#[inline]
pub fn cell_random(x: i64, y: i64, z: i64, seed: u64, salt: u64) -> f64 {
    hash3(x, y, z, seed ^ salt.wrapping_mul(0xA24B_AED4_963E_E407)) as f64 / 4_294_967_296.0
}

/// Value in `[0, 1)` for an integer cell, in 32-bit arithmetic with 24-bit results so the
/// terrain shader (`crater_hash` in terrain.wgsl) computes exactly the same value: features
/// scattered with it (craters) can be drawn by the shader where vertices are too far apart.
#[inline]
pub fn cell_random32(x: i32, y: i32, z: i32, seed: u32, salt: u32) -> f64 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ (z as u32).wrapping_mul(0xCB1A_B31F)
        ^ seed
        ^ salt.wrapping_mul(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h >> 8) as f64 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The terrain shader repeats this hash (`crater_hash`) to draw the same craters: pinned
    /// values catch a change on one side only.
    #[test]
    fn cell_random32_is_pinned() {
        assert_eq!(cell_random32(1, -2, 3, 7, 2), 12_697_250.0 / 16_777_216.0);
        assert_eq!(
            cell_random32(-40_000, 12, 99_999, 0xBEEF, 5),
            16_668_524.0 / 16_777_216.0
        );
    }

    #[test]
    fn noise_is_bounded_and_deterministic() {
        let mut max = 0.0f64;
        for i in 0..2000 {
            let p = DVec3::new(i as f64 * 0.137, i as f64 * 0.071 - 3.0, i as f64 * 0.013);
            let v = noise3(p, 7);
            assert_eq!(v, noise3(p, 7));
            max = max.max(v.abs());
        }
        assert!(max <= 1.2 && max > 0.3, "{max}");
    }

    #[test]
    fn analytic_gradient_matches_finite_differences() {
        for i in 0..50 {
            let p = DVec3::new(
                0.31 + i as f64 * 0.77,
                1.7 - i as f64 * 0.23,
                i as f64 * 0.41,
            );
            let (_, g) = gradient3(p, 3);
            let e = 1e-6;
            let fd = DVec3::new(
                (noise3(p + DVec3::X * e, 3) - noise3(p - DVec3::X * e, 3)) / (2.0 * e),
                (noise3(p + DVec3::Y * e, 3) - noise3(p - DVec3::Y * e, 3)) / (2.0 * e),
                (noise3(p + DVec3::Z * e, 3) - noise3(p - DVec3::Z * e, 3)) / (2.0 * e),
            );
            assert!((g - fd).length() < 1e-4, "{g:?} vs {fd:?}");
        }
    }

    #[test]
    fn works_at_planet_scale() {
        // Centimeter steps 6 000 km from the origin still change the noise smoothly.
        let base = DVec3::new(6.36e6, 1.0e5, -2.0e6);
        let a = fbm(base, 1, 4, 0.5);
        let b = fbm(base + DVec3::X * 0.01, 1, 4, 0.5);
        assert!((a - b).abs() < 0.1);
        assert!(a.is_finite());
    }

    #[test]
    fn fractional_octaves_are_continuous() {
        let p = DVec3::new(12.3, 4.5, 6.7);
        let a = fbm_smooth(p, 9, 3.999, 0.5);
        let b = fbm_smooth(p, 9, 4.0, 0.5);
        assert!((a - b).abs() < 1e-2);
    }
}
