//! A small BC7 encoder for the virtual texture cache: mode 6 only (one subset, RGBA endpoints
//! of 7 bits plus a shared low bit each, 16 interpolation steps). Endpoints come from the
//! principal axis of the block's colors and are refined by least squares, so smooth gradients
//! and the noisy detail of scans both keep their look at a quarter of the memory of RGBA8.
//! Runs on the tile loader threads.

use super::residency::SLOT;

const WEIGHTS: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Compress the concatenated RGBA8 levels of a tile slot (`SLOT`², then halved) to BC7 blocks,
/// level after level, blocks in rows.
pub fn compress_levels(levels: &[u8], count: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offset = 0;
    for l in 0..count {
        let size = SLOT >> l;
        let bytes = (size * size * 4) as usize;
        let Some(level) = levels.get(offset..offset + bytes) else {
            break;
        };
        out.extend_from_slice(&compress_image(level, size, size));
        offset += bytes;
    }
    out
}

/// Compress a `w`×`h` RGBA8 image (both multiples of 4) to BC7 blocks in rows.
pub fn compress_image(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w / 4 * h / 4 * 16) as usize);
    let mut block = [[0u8; 4]; 16];
    for by in 0..h / 4 {
        for bx in 0..w / 4 {
            for (i, px) in block.iter_mut().enumerate() {
                let x = bx * 4 + i as u32 % 4;
                let y = by * 4 + i as u32 / 4;
                let o = ((y * w + x) * 4) as usize;
                px.copy_from_slice(&rgba[o..o + 4]);
            }
            out.extend_from_slice(&encode_block(&block));
        }
    }
    out
}

fn interpolate(e0: [u8; 4], e1: [u8; 4], i: usize) -> [u8; 4] {
    let w = WEIGHTS[i];
    let mut c = [0u8; 4];
    for k in 0..4 {
        c[k] = (((64 - w) * e0[k] as u32 + w * e1[k] as u32 + 32) >> 6) as u8;
    }
    c
}

fn dist(a: [u8; 4], b: [u8; 4]) -> u32 {
    (0..4)
        .map(|k| {
            let d = a[k] as i32 - b[k] as i32;
            (d * d) as u32
        })
        .sum()
}

/// Best indices for the endpoints and the total squared error.
fn fit(block: &[[u8; 4]; 16], e0: [u8; 4], e1: [u8; 4]) -> ([u8; 16], u32) {
    let palette: [[u8; 4]; 16] = std::array::from_fn(|i| interpolate(e0, e1, i));
    let mut idx = [0u8; 16];
    let mut err = 0;
    for (p, slot) in block.iter().zip(idx.iter_mut()) {
        let (best, d) = palette
            .iter()
            .enumerate()
            .map(|(i, c)| (i, dist(*p, *c)))
            .min_by_key(|&(_, d)| d)
            .unwrap_or((0, 0));
        *slot = best as u8;
        err += d;
    }
    (idx, err)
}

/// Quantize a float endpoint to 7 bits + the shared low bit `p`.
fn quantize(e: [f32; 4], p: u8) -> [u8; 4] {
    e.map(|v| {
        let q = ((v - p as f32) / 2.0).round().clamp(0.0, 127.0) as u8;
        q * 2 + p
    })
}

/// The best quantized endpoints near the float ones (trying all four low-bit pairs).
fn best_quantized(
    block: &[[u8; 4]; 16],
    a: [f32; 4],
    b: [f32; 4],
) -> ([u8; 4], [u8; 4], [u8; 16], u32) {
    let mut best = ([0; 4], [0; 4], [0; 16], u32::MAX);
    for p0 in 0..2 {
        for p1 in 0..2 {
            let e0 = quantize(a, p0);
            let e1 = quantize(b, p1);
            let (idx, err) = fit(block, e0, e1);
            if err < best.3 {
                best = (e0, e1, idx, err);
            }
        }
    }
    best
}

/// Encode one 4×4 block (texels in rows) as BC7 mode 6.
pub fn encode_block(block: &[[u8; 4]; 16]) -> [u8; 16] {
    // Mean and principal axis (power iteration on the covariance).
    let mut mean = [0.0f32; 4];
    for p in block {
        for k in 0..4 {
            mean[k] += p[k] as f32 / 16.0;
        }
    }
    let mut cov = [[0.0f32; 4]; 4];
    for p in block {
        let d: [f32; 4] = std::array::from_fn(|k| p[k] as f32 - mean[k]);
        for i in 0..4 {
            for j in 0..4 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let mut axis = [1.0f32, 1.0, 1.0, 0.25];
    for _ in 0..8 {
        let mut n = [0.0f32; 4];
        for i in 0..4 {
            for j in 0..4 {
                n[i] += cov[i][j] * axis[j];
            }
        }
        let len = n.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len < 1e-6 {
            break;
        }
        axis = n.map(|v| v / len);
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in block {
        let t: f32 = (0..4).map(|k| (p[k] as f32 - mean[k]) * axis[k]).sum();
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let mut a: [f32; 4] = std::array::from_fn(|k| (mean[k] + axis[k] * lo).clamp(0.0, 255.0));
    let mut b: [f32; 4] = std::array::from_fn(|k| (mean[k] + axis[k] * hi).clamp(0.0, 255.0));
    let mut best = best_quantized(block, a, b);
    // Least-squares refinement of the endpoints for the chosen indices.
    for _ in 0..2 {
        if best.3 == 0 {
            break;
        }
        let (mut aa, mut ab, mut bb) = (0.0f32, 0.0f32, 0.0f32);
        let mut ax = [0.0f32; 4];
        let mut bx = [0.0f32; 4];
        for (p, &i) in block.iter().zip(best.2.iter()) {
            let w = WEIGHTS[i as usize] as f32 / 64.0;
            let (u, v) = (1.0 - w, w);
            aa += u * u;
            ab += u * v;
            bb += v * v;
            for k in 0..4 {
                ax[k] += u * p[k] as f32;
                bx[k] += v * p[k] as f32;
            }
        }
        let det = aa * bb - ab * ab;
        if det.abs() < 1e-6 {
            break;
        }
        a = std::array::from_fn(|k| ((bb * ax[k] - ab * bx[k]) / det).clamp(0.0, 255.0));
        b = std::array::from_fn(|k| ((aa * bx[k] - ab * ax[k]) / det).clamp(0.0, 255.0));
        let refined = best_quantized(block, a, b);
        if refined.3 < best.3 {
            best = refined;
        } else {
            break;
        }
    }
    pack(best.0, best.1, best.2)
}

/// Pack mode 6: the first index must have its top bit clear (swap the endpoints otherwise).
fn pack(mut e0: [u8; 4], mut e1: [u8; 4], mut idx: [u8; 16]) -> [u8; 16] {
    if idx[0] >= 8 {
        std::mem::swap(&mut e0, &mut e1);
        for i in &mut idx {
            *i = 15 - *i;
        }
    }
    let mut bits = 0u128;
    let mut pos = 0;
    let mut put = |value: u128, n: u32| {
        bits |= (value & ((1u128 << n) - 1)) << pos;
        pos += n;
    };
    put(1 << 6, 7);
    for k in 0..4 {
        put((e0[k] >> 1) as u128, 7);
        put((e1[k] >> 1) as u128, 7);
    }
    put((e0[0] & 1) as u128, 1);
    put((e1[0] & 1) as u128, 1);
    for (i, &v) in idx.iter().enumerate() {
        put(v as u128, if i == 0 { 3 } else { 4 });
    }
    bits.to_le_bytes()
}

/// Decode a mode 6 block (tests).
#[cfg(test)]
pub fn decode_block(block: [u8; 16]) -> [[u8; 4]; 16] {
    let bits = u128::from_le_bytes(block);
    let mut pos = 0;
    let mut get = |n: u32| {
        let v = (bits >> pos) & ((1u128 << n) - 1);
        pos += n;
        v as u8
    };
    assert_eq!(get(7), 1 << 6, "mode 6");
    // Channel by channel: R0 R1 G0 G1 B0 B1 A0 A1, then the two p-bits.
    let (mut e0, mut e1) = ([0u8; 4], [0u8; 4]);
    for (a, b) in e0.iter_mut().zip(e1.iter_mut()) {
        *a = get(7) << 1;
        *b = get(7) << 1;
    }
    let (p0, p1) = (get(1), get(1));
    e0.iter_mut().for_each(|c| *c |= p0);
    e1.iter_mut().for_each(|c| *c |= p1);
    std::array::from_fn(|i| interpolate(e0, e1, get(if i == 0 { 3 } else { 4 }) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psnr(a: &[[u8; 4]; 16], b: &[[u8; 4]; 16]) -> f64 {
        let mse = a
            .iter()
            .zip(b)
            .map(|(x, y)| dist(*x, *y) as f64)
            .sum::<f64>()
            / 64.0;
        if mse == 0.0 {
            99.0
        } else {
            10.0 * (255.0f64 * 255.0 / mse).log10()
        }
    }

    #[test]
    fn blocks_round_trip_closely() {
        let flat = [[90u8, 120, 30, 255]; 16];
        assert!(psnr(&flat, &decode_block(encode_block(&flat))) > 45.0);
        let ramp: [[u8; 4]; 16] =
            std::array::from_fn(|i| [(i * 16) as u8, 200 - (i * 8) as u8, 40, 255]);
        assert!(psnr(&ramp, &decode_block(encode_block(&ramp))) > 38.0);
        // Noise around a color (a scan's grain).
        let mut s = 12345u32;
        let noisy: [[u8; 4]; 16] = std::array::from_fn(|_| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (s >> 24) as u8 / 4;
            [100 + n, 80 + n / 2, 60 + n, 255]
        });
        assert!(psnr(&noisy, &decode_block(encode_block(&noisy))) > 34.0);
        // Alpha survives.
        let cut: [[u8; 4]; 16] =
            std::array::from_fn(|i| [50, 90, 40, if i % 2 == 0 { 0 } else { 255 }]);
        let d = decode_block(encode_block(&cut));
        assert!(
            d.iter()
                .enumerate()
                .all(|(i, p)| (p[3] > 128) == (i % 2 == 1))
        );
    }

    #[test]
    fn tile_levels_compress_to_a_quarter() {
        let levels = vec![77u8; ((SLOT * SLOT + SLOT * SLOT / 4 + SLOT * SLOT / 16) * 4) as usize];
        let out = compress_levels(&levels, 3);
        assert_eq!(out.len() * 4, levels.len());
    }
}
