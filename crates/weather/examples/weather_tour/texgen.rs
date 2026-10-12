//! Procedural textures, painted on the CPU: meadow ground, rock, bark, leaf clusters, conifer
//! fronds, grass blades and the farm track. Each produces linear albedo + alpha, a height
//! field (turned into a tangent-space normal map) and roughness/occlusion (an ORM image:
//! R = occlusion, G = roughness, B = metallic).

use engine_core::math::Vec3;
use engine_render::Image;

use crate::noise::{Rng, fbm, gradient, smoothstep32 as smoothstep, value, worley};

/// One painted texel: (linear rgba, height, roughness, ambient occlusion).
pub type Texel = ([f32; 4], f32, f32, f32);

/// A float canvas: linear RGB + alpha, height, roughness and occlusion per pixel.
#[derive(Clone)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub color: Vec<[f32; 4]>,
    pub height: Vec<f32>,
    pub rough: Vec<f32>,
    pub ao: Vec<f32>,
    /// Coordinates wrap around (tiling texture) instead of clipping.
    pub wrap: bool,
}

/// The three images of a material.
pub struct TextureSet {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
    /// Mean linear albedo of the opaque pixels.
    pub mean: Vec3,
}

impl Canvas {
    pub fn new(w: usize, h: usize, wrap: bool) -> Self {
        Self {
            w,
            h,
            color: vec![[0.0, 0.0, 0.0, 0.0]; w * h],
            height: vec![0.0; w * h],
            rough: vec![0.9; w * h],
            ao: vec![1.0; w * h],
            wrap,
        }
    }

    /// Fill every pixel from `f(u, v)` (u, v in 0..1) -> (rgba, height, roughness, ao).
    pub fn fill(&mut self, f: impl Fn(f32, f32) -> Texel + Sync) {
        let (w, h) = (self.w, self.h);
        let rows: Vec<Vec<Texel>> = par_map(h, |y| {
            (0..w)
                .map(|x| f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32))
                .collect()
        });
        for (y, row) in rows.into_iter().enumerate() {
            for (x, (c, ht, r, a)) in row.into_iter().enumerate() {
                let i = y * w + x;
                self.color[i] = c;
                self.height[i] = ht;
                self.rough[i] = r;
                self.ao[i] = a;
            }
        }
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if self.wrap {
            let x = x.rem_euclid(self.w as i32) as usize;
            let y = y.rem_euclid(self.h as i32) as usize;
            Some(y * self.w + x)
        } else if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            None
        } else {
            Some(y as usize * self.w + x as usize)
        }
    }

    /// Paint a tapered, curved stroke (a quadratic Bézier from `a` through `ctrl` to `b`, in
    /// pixels) with width `w0` at `a` and `w1` at `b`. `shade(t, across)` (t 0..1 along,
    /// across -1..1) returns (linear rgb, height, roughness) or `None` to skip the pixel.
    /// Pixels are written where the stroke's height is above what is there (a height test),
    /// alpha is the antialiased coverage (max-combined).
    #[allow(clippy::too_many_arguments)]
    pub fn stroke(
        &mut self,
        a: (f32, f32),
        ctrl: (f32, f32),
        b: (f32, f32),
        w0: f32,
        w1: f32,
        shade: &mut dyn FnMut(f32, f32) -> Option<([f32; 3], f32, f32)>,
    ) {
        const N: usize = 8;
        let mut pts = [(0.0f32, 0.0f32); N + 1];
        for (k, p) in pts.iter_mut().enumerate() {
            let t = k as f32 / N as f32;
            let s = 1.0 - t;
            *p = (
                s * s * a.0 + 2.0 * s * t * ctrl.0 + t * t * b.0,
                s * s * a.1 + 2.0 * s * t * ctrl.1 + t * t * b.1,
            );
        }
        let wmax = w0.max(w1) * 0.5 + 1.5;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &pts {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
        let (xa, ya) = ((x0 - wmax).floor() as i32, (y0 - wmax).floor() as i32);
        let (xb, yb) = ((x1 + wmax).ceil() as i32, (y1 + wmax).ceil() as i32);
        for py in ya..=yb {
            for px in xa..=xb {
                let Some(i) = self.index(px, py) else {
                    continue;
                };
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                // Closest point on the polyline.
                let mut best = (f32::MAX, 0.0f32, 0.0f32);
                for k in 0..N {
                    let (p, q) = (pts[k], pts[k + 1]);
                    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
                    let len2 = (dx * dx + dy * dy).max(1e-6);
                    let s = (((fx - p.0) * dx + (fy - p.1) * dy) / len2).clamp(0.0, 1.0);
                    let (cx, cy) = (p.0 + dx * s, p.1 + dy * s);
                    let d2 = (fx - cx) * (fx - cx) + (fy - cy) * (fy - cy);
                    if d2 < best.0 {
                        // Signed side for "across".
                        let side = (dx * (fy - p.1) - dy * (fx - p.0)).signum();
                        best = (d2, (k as f32 + s) / N as f32, side);
                    }
                }
                let t = best.1;
                let half = (w0 + (w1 - w0) * t) * 0.5;
                let d = best.0.sqrt();
                let coverage = (half + 0.5 - d).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let across = (d / half.max(0.3)).min(1.0) * best.2;
                let Some((rgb, ht, rough)) = shade(t, across) else {
                    continue;
                };
                if ht < self.height[i] && self.color[i][3] > 0.5 {
                    continue;
                }
                let c = &mut self.color[i];
                if coverage >= 0.999 || c[3] <= 0.0 {
                    *c = [rgb[0], rgb[1], rgb[2], c[3].max(coverage)];
                } else {
                    // Blend the antialiased edge over what is there.
                    for k in 0..3 {
                        c[k] = c[k] + (rgb[k] - c[k]) * coverage;
                    }
                    c[3] = c[3].max(coverage);
                }
                if coverage > 0.5 {
                    self.height[i] = ht;
                    self.rough[i] = rough;
                }
            }
        }
    }

    /// Tangent-space normal map from the height field (`strength` = height units per pixel
    /// that make a 45° slope).
    pub fn normal_image(&self, strength: f32) -> Image {
        let (w, h) = (self.w as i32, self.h as i32);
        let get = |x: i32, y: i32| -> f32 {
            let (x, y) = if self.wrap {
                (x.rem_euclid(w), y.rem_euclid(h))
            } else {
                (x.clamp(0, w - 1), y.clamp(0, h - 1))
            };
            self.height[(y * w + x) as usize]
        };
        let mut data = Vec::with_capacity(self.w * self.h * 4);
        for y in 0..h {
            for x in 0..w {
                // Sobel.
                let dx = (get(x + 1, y - 1) + 2.0 * get(x + 1, y) + get(x + 1, y + 1))
                    - (get(x - 1, y - 1) + 2.0 * get(x - 1, y) + get(x - 1, y + 1));
                let dy = (get(x - 1, y + 1) + 2.0 * get(x, y + 1) + get(x + 1, y + 1))
                    - (get(x - 1, y - 1) + 2.0 * get(x, y - 1) + get(x + 1, y - 1));
                let n = Vec3::new(-dx / 8.0 / strength, dy / 8.0 / strength, 1.0).normalize();
                data.extend_from_slice(&[
                    ((n.x * 0.5 + 0.5) * 255.0).round() as u8,
                    ((n.y * 0.5 + 0.5) * 255.0).round() as u8,
                    ((n.z * 0.5 + 0.5) * 255.0).round() as u8,
                    255,
                ]);
            }
        }
        Image::new(self.w as u32, self.h as u32, data, false)
    }

    /// Fill the color of transparent pixels from their opaque neighbours (so mipmaps and
    /// filtering don't bleed black into alpha-tested edges).
    pub fn dilate_color(&mut self) {
        let (w, h) = (self.w as i32, self.h as i32);
        let mut known: Vec<bool> = self.color.iter().map(|c| c[3] > 0.02).collect();
        let mut sum = [0.0f32; 3];
        let mut n = 0.0;
        for c in self.color.iter().filter(|c| c[3] > 0.5) {
            for k in 0..3 {
                sum[k] += c[k];
            }
            n += 1.0;
        }
        let mean = if n > 0.0 {
            [sum[0] / n, sum[1] / n, sum[2] / n]
        } else {
            [0.05, 0.08, 0.03]
        };
        for _ in 0..6 {
            let prev = self.color.clone();
            let prev_known = known.clone();
            for y in 0..h {
                for x in 0..w {
                    let i = (y * w + x) as usize;
                    if prev_known[i] {
                        continue;
                    }
                    let mut acc = [0.0f32; 3];
                    let mut cnt = 0.0;
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        if let Some(j) = self.index(x + dx, y + dy)
                            && prev_known[j]
                        {
                            for k in 0..3 {
                                acc[k] += prev[j][k];
                            }
                            cnt += 1.0;
                        }
                    }
                    if cnt > 0.0 {
                        let a = self.color[i][3];
                        self.color[i] = [acc[0] / cnt, acc[1] / cnt, acc[2] / cnt, a];
                        known[i] = true;
                    }
                }
            }
        }
        for (c, k) in self.color.iter_mut().zip(&known) {
            if !k {
                c[0] = mean[0];
                c[1] = mean[1];
                c[2] = mean[2];
            }
        }
    }

    /// Mean linear albedo over pixels with alpha > 0.5.
    pub fn mean_albedo(&self) -> Vec3 {
        let mut sum = Vec3::ZERO;
        let mut n = 0.0;
        for c in self.color.iter().filter(|c| c[3] > 0.5) {
            sum += Vec3::new(c[0], c[1], c[2]);
            n += 1.0;
        }
        if n > 0.0 { sum / n } else { Vec3::splat(0.5) }
    }

    /// Encode as albedo (sRGB) / normal / ORM images. `alpha_cutoff` makes coverage-preserving
    /// mipmaps for alpha-tested cards.
    pub fn finish(mut self, normal_strength: f32, alpha_cutoff: Option<f32>) -> TextureSet {
        if alpha_cutoff.is_some() {
            self.dilate_color();
        }
        let mean = self.mean_albedo();
        let mut albedo = Vec::with_capacity(self.w * self.h * 4);
        let mut orm = Vec::with_capacity(self.w * self.h * 4);
        for i in 0..self.w * self.h {
            let c = self.color[i];
            albedo.extend_from_slice(&[
                linear_to_srgb8(c[0]),
                linear_to_srgb8(c[1]),
                linear_to_srgb8(c[2]),
                (c[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            ]);
            orm.extend_from_slice(&[
                (self.ao[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                (self.rough[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                0,
                255,
            ]);
        }
        let mut albedo = Image::new(self.w as u32, self.h as u32, albedo, true);
        if let Some(cutoff) = alpha_cutoff {
            coverage_preserving_mips(&mut albedo, cutoff);
        }
        TextureSet {
            normal: self.normal_image(normal_strength),
            albedo,
            orm: Image::new(self.w as u32, self.h as u32, orm, false),
            mean,
        }
    }
}

/// Run `f(row)` for rows `0..n` on all cores, in order.
pub fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<(usize, T)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        out.push((i, f(i)));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("texture worker"))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, t)| t).collect()
}

/// Mipmaps whose alpha is rescaled so each level keeps the share of pixels above `cutoff`
/// that level 0 has (alpha-tested foliage otherwise thins out with distance).
pub fn coverage_preserving_mips(image: &mut Image, cutoff: f32) {
    let cut = (cutoff * 255.0) as u32;
    let coverage = |data: &[u8], scale: f32| -> f32 {
        let mut n = 0u32;
        let total = data.len() / 4;
        for px in data.as_chunks::<4>().0 {
            if (px[3] as f32 * scale) as u32 >= cut {
                n += 1;
            }
        }
        n as f32 / total.max(1) as f32
    };
    let target = coverage(&image.data, 1.0);
    let mut mips = image.mip_chain(image.srgb);
    for level in mips.iter_mut() {
        let (mut lo, mut hi) = (0.25f32, 8.0f32);
        for _ in 0..14 {
            let mid = (lo + hi) * 0.5;
            if coverage(level, mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let s = (lo + hi) * 0.5;
        for px in level.as_chunks_mut::<4>().0 {
            px[3] = (px[3] as f32 * s).min(255.0) as u8;
        }
    }
    image.mips = mips;
}

/// Linear 0..1 to an sRGB byte.
pub fn linear_to_srgb8(v: f32) -> u8 {
    crate::terrain::linear_to_srgb8(v)
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

// ------------------------------------------------------------------ meadow ground

/// Meters covered by one tile of the meadow texture.
pub const GRASS_TILE_M: f32 = 3.0;

/// Meadow seen from above: tufts of blades bursting from clumps over thatch and soil, with a
/// little clover. Tiles seamlessly.
pub fn meadow(size: usize) -> TextureSet {
    let mut c = Canvas::new(size, size, true);
    let s = size as f32;
    // Soil and thatch underneath.
    c.fill(|u, v| {
        let n = fbm(u * 8.0, v * 8.0, 5, 0.5, [8, 8], 11) * 0.5 + 0.5;
        let soil = lerp3([0.045, 0.034, 0.022], [0.075, 0.058, 0.035], n);
        ([soil[0], soil[1], soil[2], 1.0], n * 0.15, 0.95, 0.7)
    });
    let mut rng = Rng::new(42);
    let px_per_m = s / GRASS_TILE_M;
    // Dead thatch: short straw strokes lying flat.
    for _ in 0..(size * size / 90) {
        let (x, y) = (rng.f32() * s, rng.f32() * s);
        let a = rng.f32() * std::f32::consts::TAU;
        let len = rng.range(0.02, 0.06) * px_per_m;
        let (dx, dy) = (a.cos() * len, a.sin() * len);
        let tone = rng.range(0.6, 1.1);
        let col = scale3([0.20, 0.165, 0.09], tone);
        let base = rng.range(0.1, 0.25);
        c.stroke(
            (x, y),
            (
                x + dx * 0.5 + rng.signed() * 2.0,
                y + dy * 0.5 + rng.signed() * 2.0,
            ),
            (x + dx, y + dy),
            rng.range(1.5, 2.5),
            1.0,
            &mut |_, across| Some((col, base + 0.05 * (1.0 - across.abs()), 0.85)),
        );
    }
    // Clumps: blades radiate from a center, as grass tufts look from above.
    let clumps = (GRASS_TILE_M * GRASS_TILE_M * 380.0) as usize;
    for _ in 0..clumps {
        let (cx, cy) = (rng.f32() * s, rng.f32() * s);
        // Clump color family.
        let family = rng.f32();
        let green = lerp3([0.035, 0.085, 0.018], [0.075, 0.13, 0.03], rng.f32());
        let tint = if family < 0.12 {
            lerp3(green, [0.16, 0.15, 0.06], rng.range(0.4, 0.9))
        } else if family < 0.35 {
            lerp3(green, [0.10, 0.14, 0.035], 0.6)
        } else {
            green
        };
        let blades = rng.range(10.0, 26.0) as usize;
        let radius = rng.range(0.03, 0.08) * px_per_m;
        for _ in 0..blades {
            let a = rng.f32() * std::f32::consts::TAU;
            let len = radius * rng.range(0.5, 1.4);
            let start = rng.range(0.0, 0.25) * radius;
            let (x0, y0) = (cx + a.cos() * start, cy + a.sin() * start);
            let bend = rng.signed() * 0.5;
            let (ex, ey) = (x0 + (a + bend).cos() * len, y0 + (a + bend).sin() * len);
            let (mx, my) = (x0 + a.cos() * len * 0.55, y0 + a.sin() * len * 0.55);
            let shade = rng.range(0.75, 1.25);
            let dry = rng.chance(0.06);
            let col = if dry {
                [0.19, 0.17, 0.09]
            } else {
                scale3(tint, shade)
            };
            let top = rng.range(0.6, 1.0);
            c.stroke(
                (x0, y0),
                (mx, my),
                (ex, ey),
                rng.range(2.2, 3.6),
                0.8,
                &mut |t, across| {
                    // Darker toward the base (self shadowing), a light midrib.
                    let ao = 0.55 + 0.45 * t.powf(0.6);
                    let rib = 1.0 + 0.12 * (1.0 - across.abs() * 3.0).max(0.0);
                    Some((
                        scale3(col, ao * rib),
                        0.3 + top * (1.0 - (1.0 - t) * (1.0 - t)) * 0.7
                            + 0.04 * (1.0 - across * across),
                        0.88,
                    ))
                },
            );
        }
    }
    // Clover patches.
    for _ in 0..(GRASS_TILE_M * GRASS_TILE_M * 14.0) as usize {
        let (px, py) = (rng.f32() * s, rng.f32() * s);
        if value(px / s * 6.0, py / s * 6.0, [6, 6], 77) < 0.55 {
            continue;
        }
        let leaf = rng.range(0.008, 0.014) * px_per_m;
        let rot = rng.f32() * std::f32::consts::TAU;
        let col = scale3([0.05, 0.11, 0.03], rng.range(0.85, 1.2));
        for k in 0..3 {
            let a = rot + k as f32 * std::f32::consts::TAU / 3.0;
            let (ex, ey) = (px + a.cos() * leaf * 2.0, py + a.sin() * leaf * 2.0);
            c.stroke(
                (px, py),
                ((px + ex) * 0.5, (py + ey) * 0.5),
                (ex, ey),
                leaf * 1.2,
                leaf * 2.2,
                &mut |t, across| {
                    let mark = if (0.35..0.6).contains(&t) && across.abs() < 0.7 {
                        1.25
                    } else {
                        1.0
                    };
                    Some((scale3(col, mark), 0.55 + 0.25 * t, 0.6))
                },
            );
        }
    }
    // A few small flowers.
    for _ in 0..(GRASS_TILE_M * GRASS_TILE_M * 6.0) as usize {
        let (px, py) = (rng.f32() * s, rng.f32() * s);
        let col = if rng.chance(0.6) {
            [0.75, 0.72, 0.62]
        } else {
            [0.70, 0.55, 0.05]
        };
        let r = rng.range(0.004, 0.007) * px_per_m;
        c.stroke(
            (px - r * 0.3, py),
            (px, py),
            (px + r * 0.3, py),
            r * 2.0,
            r * 2.0,
            &mut |_, _| Some((col, 1.05, 0.6)),
        );
    }
    // Ambient occlusion from height: low pixels between clumps are shadowed.
    for i in 0..c.w * c.h {
        c.ao[i] = 0.65 + 0.35 * c.height[i].clamp(0.0, 1.0);
        c.color[i][3] = 1.0;
    }
    c.finish(0.12, None)
}

// ------------------------------------------------------------------ rock

/// Meters covered by one tile of the rock texture.
pub const ROCK_TILE_M: f32 = 2.0;

/// Weathered granite-like rock with cracks, speckles and lichen. Tiles seamlessly.
pub fn rock(size: usize) -> TextureSet {
    let mut c = Canvas::new(size, size, true);
    c.fill(|u, v| {
        let p = 6;
        let large = fbm(u * 3.0, v * 3.0, 6, 0.55, [3, 3], 21);
        let fine = fbm(u * 24.0, v * 24.0, 4, 0.5, [24, 24], 22);
        let cells = worley(u * p as f32, v * p as f32, [p, p], 1.0, 23);
        let crack = smoothstep(0.0, 0.06, cells.f2 - cells.f1);
        let cells2 = worley(u * 17.0, v * 17.0, [17, 17], 1.0, 24);
        let crack2 = smoothstep(0.0, 0.04, cells2.f2 - cells2.f1);
        // Each plate is tilted a little (cleaved faces).
        let tilt_a = (cells.id & 0xff) as f32 / 255.0 - 0.5;
        let tilt_b = ((cells.id >> 8) & 0xff) as f32 / 255.0 - 0.5;
        let facet = (cells.to_center.0 * tilt_a + cells.to_center.1 * tilt_b) * 0.35;
        let height = 0.45 + large * 0.25 + fine * 0.05 + facet
            - (1.0 - crack) * 0.35
            - (1.0 - crack2) * 0.08;
        let tone = 0.5 + large * 0.6 + fine * 0.25;
        let mut col = lerp3([0.16, 0.152, 0.14], [0.36, 0.345, 0.32], tone);
        // Mineral speckles.
        let speck = value(u * 300.0, v * 300.0, [300, 300], 25);
        if speck > 0.86 {
            col = scale3(col, 1.45);
        } else if speck < 0.1 {
            col = scale3(col, 0.55);
        }
        col = scale3(col, 0.55 + 0.45 * crack * (0.7 + 0.3 * crack2));
        // Lichen: pale green-grey crusts and a few orange spots.
        let lichen_field = fbm(u * 5.0, v * 5.0, 4, 0.55, [5, 5], 26);
        let spots = worley(u * 40.0, v * 40.0, [40, 40], 1.0, 27);
        let spot = smoothstep(0.42, 0.3, spots.f1) * smoothstep(0.05, 0.35, lichen_field);
        let crust = smoothstep(0.2, 0.45, lichen_field + fine * 0.2);
        let mut rough = 0.78 + 0.1 * fine;
        if crust > 0.0 {
            col = lerp3(col, [0.30, 0.32, 0.25], crust * 0.65);
            rough = rough + (0.95 - rough) * crust;
        }
        if spot > 0.0 {
            let orange = (spots.id & 7) == 0;
            let lc = if orange {
                [0.42, 0.24, 0.06]
            } else {
                [0.40, 0.42, 0.33]
            };
            col = lerp3(col, lc, spot * 0.8);
        }
        let ao = 0.55 + 0.45 * crack * (0.8 + 0.2 * crack2);
        ([col[0], col[1], col[2], 1.0], height, rough, ao)
    });
    c.finish(0.0035, None)
}

// ------------------------------------------------------------------ bark

/// Meters around / along one tile of the bark texture.
pub const BARK_TILE_M: (f32, f32) = (0.8, 1.6);

/// Furrowed bark: vertical plates between deep fissures, with lichen. Tiles seamlessly.
pub fn bark(w: usize, h: usize) -> TextureSet {
    let mut c = Canvas::new(w, h, true);
    c.fill(|u, v| {
        // Plates elongated along the trunk (v).
        let (pu, pv) = (7, 5);
        let warp = fbm(u * 4.0, v * 2.0, 3, 0.5, [4, 2], 31) * 0.35;
        let cells = worley(u * pu as f32 + warp, v * pv as f32, [pu, pv], 0.9, 32);
        let fissure = smoothstep(0.0, 0.22, cells.f2 - cells.f1);
        let fine = fbm(u * 16.0, v * 6.0, 4, 0.5, [16, 6], 33);
        let streak = gradient(u * 40.0, v * 3.0, [40, 3], 34);
        let height = fissure * 0.8 + fine * 0.12 + streak * 0.05;
        let tone = 0.55 + fine * 0.5 + streak * 0.2;
        let plate = lerp3([0.13, 0.11, 0.09], [0.26, 0.235, 0.20], tone);
        let deep = [0.035, 0.028, 0.022];
        let mut col = lerp3(deep, plate, fissure.powf(0.7));
        let lichen = fbm(u * 3.0, v * 2.0, 4, 0.5, [3, 2], 35);
        let l = smoothstep(0.15, 0.4, lichen) * fissure;
        col = lerp3(col, [0.22, 0.25, 0.17], l * 0.7);
        let rough = 0.85 + 0.12 * (1.0 - fissure);
        let ao = 0.35 + 0.65 * fissure;
        ([col[0], col[1], col[2], 1.0], height, rough, ao)
    });
    c.finish(0.004, None)
}

/// Birch bark: chalky white with dark horizontal lenticels and black patches.
pub fn birch_bark(w: usize, h: usize) -> TextureSet {
    let mut c = Canvas::new(w, h, true);
    c.fill(|u, v| {
        let n = fbm(u * 4.0, v * 8.0, 5, 0.5, [4, 8], 41);
        let streaks = fbm(u * 2.0, v * 40.0, 3, 0.5, [2, 40], 42);
        // Lenticels: short dark horizontal dashes.
        let cell = worley(u * 6.0, v * 30.0, [6, 30], 0.8, 43);
        let (ox, oy) = cell.to_center;
        let dash = smoothstep(0.55, 0.3, (ox * ox * 0.08 + oy * oy * 4.0).sqrt())
            * if cell.id.is_multiple_of(3) { 1.0 } else { 0.0 };
        let patches = smoothstep(0.3, 0.45, fbm(u * 3.0, v * 3.0, 4, 0.55, [3, 3], 44));
        let white = lerp3(
            [0.55, 0.53, 0.48],
            [0.72, 0.70, 0.65],
            n * 0.5 + 0.5 + streaks * 0.2,
        );
        let mut col = lerp3(white, [0.05, 0.045, 0.04], dash.max(patches * 0.9));
        col = lerp3(col, [0.42, 0.38, 0.30], smoothstep(0.2, 0.5, streaks) * 0.3);
        let height = 0.5 + n * 0.1 - dash * 0.3 - patches * 0.2;
        (
            [col[0], col[1], col[2], 1.0],
            height,
            0.7 + patches * 0.2,
            1.0 - dash * 0.4,
        )
    });
    c.finish(0.004, None)
}

// ------------------------------------------------------------------ foliage cards

/// Broadleaf cluster: a twig with alternate ovate leaves, on a transparent card. The twig
/// enters at the bottom center; leaves fan out upward.
pub fn leaf_cluster(size: usize, seed: u64, palette: [[f32; 3]; 3]) -> TextureSet {
    let mut c = Canvas::new(size, size, false);
    let s = size as f32;
    let mut rng = Rng::new(seed);
    // Main twig and side twigs.
    let twig_col = [0.08, 0.06, 0.04];
    let mut leaves: Vec<((f32, f32), f32, f32)> = Vec::new();
    let mut twig = |c: &mut Canvas,
                    rng: &mut Rng,
                    a: (f32, f32),
                    dir: f32,
                    len: f32,
                    width: f32,
                    depth: u32| {
        let b = (a.0 + dir.cos() * len, a.1 - dir.sin() * len);
        let mid = (
            (a.0 + b.0) * 0.5 + rng.signed() * len * 0.08,
            (a.1 + b.1) * 0.5 + rng.signed() * len * 0.08,
        );
        c.stroke(a, mid, b, width, width * 0.5, &mut |_, across| {
            Some((scale3(twig_col, 1.0 - 0.3 * across.abs()), 0.3, 0.8))
        });
        // Leaves along this twig, alternating sides, plus one at the tip.
        let n = 3 + depth as usize * 2;
        for k in 0..n {
            let t = 0.3 + 0.7 * (k as f32 + 0.5) / n as f32;
            let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let ang = dir + side * rng.range(0.6, 1.1);
            leaves.push((p, ang, rng.range(0.75, 1.0)));
        }
        leaves.push((b, dir + rng.signed() * 0.2, 1.0));
        b
    };
    let base = (s * 0.5, s * 0.99);
    let up = std::f32::consts::FRAC_PI_2;
    let main_dir = up + rng.signed() * 0.1;
    let main_len = s * 0.62;
    twig(&mut c, &mut rng, base, main_dir, main_len, s * 0.016, 3);
    // Side twigs alternate up the main twig and fan out to fill the card.
    for k in 0..6 {
        let t = 0.18 + 0.68 * k as f32 / 5.0;
        let side = if k % 2 == 0 { 1.0 } else { -1.0 };
        let p = (
            base.0 + main_dir.cos() * main_len * t,
            base.1 - main_dir.sin() * main_len * t,
        );
        let dir = main_dir + side * rng.range(0.55, 0.95) * (1.0 - 0.3 * t);
        let len = s * rng.range(0.24, 0.34) * (1.0 - 0.35 * t);
        twig(&mut c, &mut rng, p, dir, len, s * 0.01, 1);
    }
    // Leaves, drawn far to near (random order) with a height test.
    let leaf_len = s * 0.145;
    for (p, ang, scale) in leaves {
        let len = leaf_len * scale * rng.range(0.85, 1.15);
        let width = len * rng.range(0.42, 0.52);
        let petiole = len * 0.12;
        let a = (p.0 + ang.cos() * petiole, p.1 - ang.sin() * petiole);
        let b = (a.0 + ang.cos() * len, a.1 - ang.sin() * len);
        let curl = rng.signed() * 0.25;
        let ctrl = (
            (a.0 + b.0) * 0.5 + (ang + 1.57).cos() * len * curl * 0.3,
            (a.1 + b.1) * 0.5 - (ang + 1.57).sin() * len * curl * 0.3,
        );
        // Leaf color: palette mix with per-leaf variation.
        let k = rng.f32();
        let col = if k < 0.6 {
            lerp3(palette[0], palette[1], rng.f32())
        } else if k < 0.9 {
            lerp3(palette[1], palette[2], rng.f32() * 0.6)
        } else {
            lerp3(palette[0], palette[2], 0.7)
        };
        let col = scale3(col, rng.range(0.85, 1.15));
        let layer = rng.f32() * 0.5;
        // Petiole.
        c.stroke(
            p,
            ((p.0 + a.0) * 0.5, (p.1 + a.1) * 0.5),
            a,
            s * 0.006,
            s * 0.006,
            &mut |_, _| Some(([0.06, 0.07, 0.03], 0.5 + layer, 0.7)),
        );
        // The blade: wide near the base, pointed at the tip.
        let mut shade = |t: f32, across: f32| {
            let profile = (std::f32::consts::PI * t.clamp(0.0, 1.0).powf(0.75))
                .sin()
                .max(0.0)
                .powf(0.85);
            let edge = across.abs();
            if profile.is_nan() || profile <= 1e-3 || edge > profile {
                return None;
            }
            let rel = edge / profile.max(1e-3);
            // Veins: midrib and side veins angled toward the tip.
            let midrib = smoothstep(0.08, 0.0, rel);
            let side_vein = {
                let ph = (t * 9.0 - rel * 1.6).fract();
                smoothstep(0.1, 0.0, (ph - 0.5).abs()) * (1.0 - rel) * 0.6
            };
            let vein = midrib.max(side_vein);
            let light = 1.0 + 0.25 * vein - 0.15 * rel * rel;
            let mut cc = scale3(col, light);
            cc = lerp3(
                cc,
                [cc[0] * 1.3 + 0.01, cc[1] * 1.15, cc[2] * 0.8],
                vein * 0.4,
            );
            // Folded along the midrib (V-shaped cross-section), cupped toward the tip.
            let h = 0.6 + layer + 0.25 * (1.0 - rel) - 0.08 * t;
            Some((cc, h, 0.55 + 0.1 * rel))
        };
        c.stroke(a, ctrl, b, width, width, &mut shade);
    }
    for i in 0..c.w * c.h {
        c.ao[i] = 0.75 + 0.25 * c.height[i].clamp(0.0, 1.2) / 1.2;
    }
    c.finish(0.012, Some(0.5))
}

/// Conifer frond: a woody axis with side shoots densely covered in needles. The axis runs
/// from the bottom center (where it attaches) to the top (the tip).
pub fn conifer_frond(w: usize, h: usize, seed: u64) -> TextureSet {
    let mut c = Canvas::new(w, h, false);
    let (sw, sh) = (w as f32, h as f32);
    let mut rng = Rng::new(seed);
    let dark = [0.018, 0.040, 0.022];
    let mid = [0.030, 0.062, 0.030];
    let light = [0.075, 0.115, 0.040];
    let axis_x = sw * 0.5;
    let (y0, y1) = (sh * 0.99, sh * 0.03);
    // Needles first (lower), then the axis on top.
    let shoots = 19;
    for k in 0..shoots {
        let t = (k as f32 + 0.5) / shoots as f32;
        let y = y0 + (y1 - y0) * t;
        let reach = sw * 0.48 * (1.0 - t * 0.75) * rng.range(0.8, 1.05);
        for side in [-1.0f32, 1.0] {
            let ang: f32 = rng.range(0.55, 0.85); // radians from the axis toward the tip
            let ex = axis_x + side * ang.sin() * reach;
            let ey = y - ang.cos() * reach * 1.1;
            let n_needles = (reach / 1.6) as usize;
            for j in 0..n_needles {
                let tt = j as f32 / n_needles as f32;
                let px = axis_x + (ex - axis_x) * tt;
                let py = y + (ey - y) * tt;
                let tip_t = (t * 0.6 + tt * 0.6).min(1.0);
                for nside in [-1.0f32, 1.0] {
                    let nlen = sw * rng.range(0.045, 0.062) * (1.0 - 0.4 * tt);
                    let na = rng.range(0.6, 1.0);
                    // Needles point forward along the shoot.
                    let dir = ((ex - axis_x) * side, ey - y);
                    let dl = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-3);
                    let (fx, fy) = (dir.0 / dl * side, dir.1 / dl);
                    let (nx, ny) = (-fy * nside, fx * nside);
                    let (dx, dy) = (fx * na.cos() + nx * na.sin(), fy * na.cos() + ny * na.sin());
                    let col = lerp3(
                        lerp3(dark, mid, rng.f32()),
                        light,
                        smoothstep(0.75, 1.0, tip_t) * rng.f32(),
                    );
                    let layer = rng.f32() * 0.3;
                    c.stroke(
                        (px, py),
                        (px + dx * nlen * 0.5, py + dy * nlen * 0.5),
                        (px + dx * nlen, py + dy * nlen),
                        2.8,
                        1.2,
                        &mut |t, _| Some((scale3(col, 0.7 + 0.4 * t), 0.4 + layer + 0.3 * t, 0.6)),
                    );
                }
            }
            c.stroke(
                (axis_x, y),
                ((axis_x + ex) * 0.5, (y + ey) * 0.5),
                (ex, ey),
                2.5,
                1.2,
                &mut |_, _| Some(([0.07, 0.05, 0.03], 0.35, 0.8)),
            );
        }
    }
    c.stroke(
        (axis_x, y0),
        (axis_x + rng.signed() * 3.0, (y0 + y1) * 0.5),
        (axis_x, y1),
        6.0,
        2.0,
        &mut |_, across| {
            Some((
                scale3([0.09, 0.065, 0.04], 1.0 - 0.3 * across.abs()),
                0.45,
                0.8,
            ))
        },
    );
    for i in 0..c.w * c.h {
        c.ao[i] = 0.7 + 0.3 * c.height[i].clamp(0.0, 1.0);
    }
    c.finish(0.01, Some(0.5))
}

/// Grass blades for near-camera tufts: two variants stacked vertically (plain meadow grass
/// on top, grass with seed heads and flowers below). Blades grow from the bottom of each half.
pub fn grass_blades(w: usize, h: usize) -> TextureSet {
    let mut c = Canvas::new(w, h, false);
    let (sw, sh) = (w as f32, h as f32);
    let half = sh * 0.5;
    let mut rng = Rng::new(99);
    for variant in 0..2 {
        let bottom = half * (variant + 1) as f32 - 1.0;
        let blades = 110;
        for _ in 0..blades {
            let x0 = rng.range(0.06, 0.94) * sw;
            let height = half * rng.range(0.35, 0.97);
            let lean = rng.signed() * 0.35 + (x0 / sw - 0.5) * 0.5;
            let curve = lean * rng.range(0.6, 1.6);
            let top = (
                x0 + lean * height,
                bottom - height * (1.0 - lean.abs() * 0.3),
            );
            let ctrl = (x0 + lean * height * 0.2, bottom - height * 0.55);
            let tip = (
                top.0 + curve * height * 0.25,
                top.1 + curve.abs() * height * 0.1,
            );
            let dry = rng.chance(0.08);
            let base = if dry {
                [0.09, 0.07, 0.035]
            } else {
                [0.022, 0.045, 0.012]
            };
            let top_col = if dry {
                [0.30, 0.25, 0.12]
            } else {
                lerp3([0.07, 0.13, 0.03], [0.11, 0.16, 0.04], rng.f32())
            };
            let width = rng.range(4.0, 7.5) * sw / 512.0;
            let layer = rng.f32();
            c.stroke((x0, bottom), ctrl, tip, width, 0.7, &mut |t, across| {
                let col = lerp3(base, top_col, t.powf(0.7));
                let rib = 1.0 + 0.15 * (1.0 - across.abs() * 2.5).max(0.0);
                Some((scale3(col, rib), layer + t * 0.2, 0.55))
            });
            if variant == 1 && rng.chance(0.18) {
                // Seed head or flower at the tip.
                let flower = rng.chance(0.5);
                let col = if !flower {
                    [0.22, 0.17, 0.08]
                } else if rng.chance(0.5) {
                    [0.80, 0.78, 0.70]
                } else {
                    [0.75, 0.58, 0.04]
                };
                let r = if flower { 5.0 } else { 3.5 } * sw / 512.0;
                let len = if flower { r } else { r * 4.0 };
                c.stroke(
                    (tip.0, tip.1 + len * 0.5),
                    (tip.0, tip.1),
                    (tip.0, tip.1 - len * 0.5),
                    r * 2.0,
                    r * 1.6,
                    &mut |_, _| Some((col, 2.0, 0.6)),
                );
            }
        }
    }
    for i in 0..c.w * c.h {
        c.ao[i] = 1.0;
    }
    c.finish(0.02, Some(0.45))
}

// ------------------------------------------------------------------ farm track

/// Meters along one tile of the track texture (it spans the track's width across).
pub const TRACK_TILE_M: f32 = 8.0;
/// Track width, m.
pub const TRACK_WIDTH_M: f32 = 3.2;

/// Rut centers across the track, m from its left edge, before they wander.
pub const TRACK_RUTS_M: [f32; 2] = [0.86, 2.34];

/// How far the ruts wander sideways at `v` (0..1 along one tile), m.
fn track_wander(v: f32) -> f32 {
    fbm(0.5, v * 3.0, 3, 0.5, [0, 3], 51) * 0.12
}

/// Farm track: two wheel ruts of compacted, damp earth with tire tread, crumbly edges and a
/// few embedded pebbles, grass creeping in, and bare patches in the middle strip. Everything
/// else is transparent so the meadow (and its tufts) shows through. U runs across, V along.
pub fn track(w: usize, h: usize) -> TextureSet {
    let mut c = Canvas::new(w, h, true);
    let per_m_u = w as f32 / TRACK_WIDTH_M;
    c.fill(|u, v| {
        let xm = u * TRACK_WIDTH_M;
        let wander = track_wander(v);
        let width_var = fbm(1.5, v * 5.0, 2, 0.5, [0, 5], 57) * 0.08;
        let edge = fbm(u * 24.0, v * 60.0, 4, 0.55, [24, 60], 52) * 0.09;
        let d = TRACK_RUTS_M
            .iter()
            .map(|&r| (xm - r - wander).abs())
            .fold(f32::MAX, f32::min);
        let half = 0.32 + width_var;
        let rut = smoothstep(half + 0.04, half - 0.04, d + edge * 0.5);
        // The whole track is worn: bare soil to its ragged outer edges, the middle strip
        // bare in patches with grass between.
        let from_center = (xm - TRACK_WIDTH_M * 0.5 - wander).abs();
        let outer = smoothstep(1.42, 1.3, from_center + edge);
        let patch_n = fbm(u * 7.0, v * 18.0, 3, 0.5, [7, 18], 58);
        let middle = smoothstep(0.55, 0.3, from_center) * smoothstep(-0.05, 0.12, patch_n + edge);
        let shoulder = outer * smoothstep(0.0, 0.25, from_center - 1.0 + patch_n * 0.3);
        let alpha = rut.max(smoothstep(0.05, 0.15, middle + shoulder) * outer);
        let mid_bare = 1.0 - rut;
        // Grass creeping into the ruts in a few places.
        let creep_n = fbm(u * 6.0, v * 14.0, 3, 0.5, [6, 14], 59);
        let creep = smoothstep(0.35, 0.55, creep_n) * smoothstep(half * 0.5, half, d);
        let alpha = alpha * (1.0 - creep);
        // Soil: compacted and damp at the bottom of the ruts, loose and dusty at the edges.
        let fine = fbm(u * 64.0, v * 160.0, 4, 0.5, [64, 160], 54);
        let depth = smoothstep(half, 0.0, d);
        let damp_n = fbm(u * 3.0, v * 8.0, 2, 0.5, [3, 8], 60) * 0.5 + 0.5;
        let damp = depth * smoothstep(0.35, 0.7, damp_n);
        let dusty = [0.20, 0.155, 0.10];
        let earth = [0.105, 0.080, 0.052];
        let wet = [0.050, 0.038, 0.026];
        let mut col = lerp3(dusty, earth, 0.2 + 0.8 * depth);
        col = lerp3(col, wet, damp * 0.6);
        col = scale3(col, 0.85 + 0.3 * fine);
        // Tire tread: chevrons across the rut, worn away in places.
        let phase = v * 50.0 + d * 5.0;
        let lug = smoothstep(0.32, 0.42, phase.fract()) * smoothstep(0.68, 0.58, phase.fract());
        let worn = smoothstep(
            0.2,
            0.6,
            fbm(u * 4.0, v * 30.0, 2, 0.5, [4, 30], 61) * 0.5 + 0.5,
        );
        let tread = lug * smoothstep(half * 0.85, half * 0.4, d) * worn;
        col = scale3(col, 1.0 - 0.18 * tread);
        // Crumbs and clods along the rut edges.
        let clods = worley(u * 30.0, v * 75.0, [30, 75], 0.9, 55);
        let rim = smoothstep(half * 0.45, half * 0.9, d);
        let clod =
            smoothstep(0.38, 0.18, clods.f1) * rim * clods.id.is_multiple_of(3) as u32 as f32;
        col = lerp3(col, scale3(dusty, 1.15), clod * 0.6);
        // A few pebbles pressed into the soil.
        let peb = worley(u * 48.0, v * 120.0, [48, 120], 1.0, 53);
        let pebble = smoothstep(0.3, 0.15, peb.f1) * peb.id.is_multiple_of(6) as u32 as f32;
        let tone = ((peb.id >> 4) & 0xff) as f32 / 255.0;
        col = lerp3(
            col,
            lerp3([0.13, 0.12, 0.11], [0.28, 0.26, 0.23], tone),
            pebble * 0.85,
        );
        // Between and beside the ruts: dusty soil with trampled grass and moss.
        let grassy = smoothstep(0.0, 0.5, patch_n * 0.5 + 0.5 - 0.35) * mid_bare;
        col = lerp3(
            col,
            lerp3(
                [0.085, 0.078, 0.042],
                [0.045, 0.062, 0.022],
                fine * 0.5 + 0.5,
            ),
            grassy * 0.55,
        );
        let height = 0.55 - 0.06 * depth + 0.05 * fine - 0.03 * tread
            + 0.25 * clod * (1.0 - clods.f1)
            + 0.3 * pebble * (1.0 - peb.f1);
        let rough = 0.95 - 0.3 * damp - 0.15 * pebble;
        let ao = 0.8 + 0.2 * (1.0 - depth * 0.5) - 0.1 * tread;
        ([col[0], col[1], col[2], alpha], height, rough, ao)
    });
    // Grass blades overhanging the rut edges (cut the alpha with blade shapes).
    let mut rng = Rng::new(56);
    let sh = h as f32;
    for _ in 0..(TRACK_TILE_M * 110.0) as usize {
        let y0 = rng.f32() * sh;
        let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
        let x0 =
            (TRACK_WIDTH_M * 0.5 + track_wander(y0 / sh) + side * rng.range(1.25, 1.42)) * per_m_u;
        let len = rng.range(0.05, 0.14) * per_m_u;
        let ang: f32 = rng.signed() * 0.7;
        let dx = -side * ang.cos() * len;
        let dy = ang.sin() * len * 1.6;
        let col = lerp3([0.055, 0.095, 0.028], [0.12, 0.14, 0.05], rng.f32());
        c.stroke(
            (x0, y0),
            (x0 + dx * 0.5, y0 + dy * 0.5),
            (x0 + dx, y0 + dy),
            3.0,
            0.8,
            &mut |t, _| Some((scale3(col, 0.7 + 0.4 * t), 0.7, 0.7)),
        );
    }
    c.finish(0.012, Some(0.5))
}
