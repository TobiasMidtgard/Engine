//! Building blocks for generated texture sets: a [`Tile`] (pixels and the meters they cover),
//! parallel per-pixel evaluation, wrapping blurs, normal / occlusion / curvature maps derived
//! from a height field in meters, and packing into GPU-ready [`Image`]s with mip chains.
//!
//! Everything wraps at the tile border, so textures built from tileable noise stay seamless
//! through every step (blurs, normals, occlusion, mips).

use engine_assets::{Assets, Handle};
use engine_core::math::{Vec2, Vec3};
use engine_render::Image;

use super::noise::{self, Voronoi};

/// A texture's pixel grid and the patch of surface it covers (meters). Texels are square:
/// `size.y / size.x == h / w`.
#[derive(Debug, Clone, Copy)]
pub struct Tile {
    pub w: usize,
    pub h: usize,
    /// Meters covered by the whole texture.
    pub size: Vec2,
}

impl Tile {
    /// A `w`×`h` texture covering `width_m` meters across (the height in meters follows from
    /// square texels).
    pub fn new(w: usize, h: usize, width_m: f32) -> Self {
        let w = w.max(4);
        let h = h.max(4);
        Self {
            w,
            h,
            size: Vec2::new(width_m, width_m * h as f32 / w as f32),
        }
    }

    /// Number of texels.
    pub fn texels(&self) -> usize {
        self.w * self.h
    }

    /// Edge length of one texel in meters.
    pub fn texel_m(&self) -> f32 {
        self.size.x / self.w as f32
    }

    /// Meters to texels (at least 1).
    pub fn px(&self, meters: f32) -> usize {
        ((meters / self.texel_m()).round() as usize).max(1)
    }

    /// Lattice period (cells per tile per axis) for features about `scale_m` meters across.
    pub fn period(&self, scale_m: f32) -> [i32; 2] {
        [
            ((self.size.x / scale_m).round() as i32).max(1),
            ((self.size.y / scale_m).round() as i32).max(1),
        ]
    }

    /// A tile-unit offset from meters.
    pub fn off(&self, m: Vec2) -> Vec2 {
        m / self.size
    }

    /// Gradient noise (about -1..1) with features about `scale_m` across.
    pub fn noise(&self, p: Vec2, scale_m: f32, seed: u32) -> f32 {
        noise::gradient(p, self.period(scale_m), seed)
    }

    /// Value noise (0..1) with features about `scale_m` across.
    pub fn value(&self, p: Vec2, scale_m: f32, seed: u32) -> f32 {
        noise::value(p, self.period(scale_m), seed)
    }

    /// Fractal gradient noise whose largest features are about `scale_m` across.
    pub fn fbm(&self, p: Vec2, scale_m: f32, octaves: u32, gain: f32, seed: u32) -> f32 {
        noise::fbm(p, self.period(scale_m), octaves, gain, seed)
    }

    /// Ridged fractal noise (0..1).
    pub fn ridged(&self, p: Vec2, scale_m: f32, octaves: u32, gain: f32, seed: u32) -> f32 {
        noise::ridged(p, self.period(scale_m), octaves, gain, seed)
    }

    /// Voronoi cells about `scale_m` across.
    pub fn voronoi(&self, p: Vec2, scale_m: f32, jitter: f32, seed: u32) -> Voronoi {
        noise::voronoi(p, self.period(scale_m), jitter, seed)
    }

    /// Voronoi cells with the distance to the cell border.
    pub fn voronoi_edge(&self, p: Vec2, scale_m: f32, jitter: f32, seed: u32) -> Voronoi {
        noise::voronoi_edge(p, self.period(scale_m), jitter, seed)
    }

    /// Smoothly blended Voronoi dishes about `scale_m` across (see [`noise::smooth_cells`]).
    pub fn smooth_cells(&self, p: Vec2, scale_m: f32, jitter: f32, k: f32, seed: u32) -> f32 {
        noise::smooth_cells(p, self.period(scale_m), jitter, k, seed)
    }

    /// Calls `f(id, r)` for the 3×3 cells (about `scale_m` across) around `p`; `r` is the
    /// vector from each cell's point to `p` in meters.
    pub fn cells3(&self, p: Vec2, scale_m: f32, seed: u32, mut f: impl FnMut(u32, Vec2)) {
        let period = self.period(scale_m);
        let cell = self.size / Vec2::new(period[0] as f32, period[1] as f32);
        noise::cells3(p, period, 1.0, seed, |id, r| f(id, r * cell));
    }

    /// Domain warp: `p` displaced by up to about `amp_m` meters by fractal noise of size
    /// `scale_m`.
    pub fn warp(&self, p: Vec2, scale_m: f32, amp_m: f32, octaves: u32, seed: u32) -> Vec2 {
        let period = self.period(scale_m);
        let d = Vec2::new(
            noise::fbm(p, period, octaves, 0.5, noise::sub(seed, 1)),
            noise::fbm(p, period, octaves, 0.5, noise::sub(seed, 2)),
        );
        p + d * amp_m / self.size
    }

    /// Evaluate `f(p, x, y)` for every texel in parallel; `p` is the texel center in tile
    /// units.
    pub fn par<T: Send + Clone + Default>(
        &self,
        f: impl Fn(Vec2, usize, usize) -> T + Sync,
    ) -> Vec<T> {
        let (w, h) = (self.w as f32, self.h as f32);
        let mut out = vec![T::default(); self.texels()];
        par_fill(&mut out, self.w, |x, y| {
            f(Vec2::new((x as f32 + 0.5) / w, (y as f32 + 0.5) / h), x, y)
        });
        out
    }

    /// Evaluate `f(i)` for every texel index in parallel.
    pub fn par_idx<T: Send + Clone + Default>(&self, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
        let mut out = vec![T::default(); self.texels()];
        let w = self.w;
        par_fill(&mut out, w, |x, y| f(y * w + x));
        out
    }

    /// Value at texel (x, y), wrapping.
    #[inline(always)]
    pub fn at<T: Copy>(&self, data: &[T], x: isize, y: isize) -> T {
        let x = x.rem_euclid(self.w as isize) as usize;
        let y = y.rem_euclid(self.h as isize) as usize;
        data[y * self.w + x]
    }

    /// Bilinear sample at tile-unit position `p`, wrapping.
    pub fn sample(&self, data: &[f32], p: Vec2) -> f32 {
        let x = p.x * self.w as f32 - 0.5;
        let y = p.y * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.at(data, x0, y0);
        let b = self.at(data, x0 + 1, y0);
        let c = self.at(data, x0, y0 + 1);
        let d = self.at(data, x0 + 1, y0 + 1);
        let top = a + (b - a) * fx;
        let bottom = c + (d - c) * fx;
        top + (bottom - top) * fy
    }

    /// Blur with wrap-around: two box passes per axis (a tent about `radius_m` wide on each
    /// side). Wide blurs run on a downsampled copy.
    pub fn blur(&self, data: &[f32], radius_m: f32) -> Vec<f32> {
        let r_px = self.px(radius_m);
        let mut k = 1;
        while r_px / (k * 2) >= 6
            && self.w.is_multiple_of(k * 2)
            && self.h.is_multiple_of(k * 2)
            && self.w / (k * 2) >= 16
        {
            k *= 2;
        }
        let (w, h) = (self.w / k, self.h / k);
        let mut a = if k == 1 {
            data.to_vec()
        } else {
            let mut d = vec![0.0; w * h];
            let inv = 1.0 / (k * k) as f32;
            par_fill(&mut d, w, |x, y| {
                let mut s = 0.0;
                for yy in 0..k {
                    let row = &data[(y * k + yy) * self.w + x * k..][..k];
                    s += row.iter().sum::<f32>();
                }
                s * inv
            });
            d
        };
        let r = (r_px / k / 2).max(1);
        for _ in 0..2 {
            box_rows(&mut a, w, r);
        }
        let mut t = transpose(&a, w, h);
        for _ in 0..2 {
            box_rows(&mut t, h, r);
        }
        let low = transpose(&t, h, w);
        if k == 1 {
            return low;
        }
        let up = Coarse {
            data: low,
            w,
            h,
            sx: 1.0 / k as f32,
            sy: 1.0 / k as f32,
        };
        self.par(|_, x, y| up.at(x, y))
    }

    /// Evaluate a smooth field on a grid `factor` times coarser than the texture (use it for
    /// noise whose smallest features span many texels) and read it back with
    /// [`Coarse::at`]. Saves most of the cost of low-frequency layers.
    pub fn coarse<T>(&self, factor: usize, f: impl Fn(Vec2) -> T + Sync) -> Coarse<T>
    where
        T: Send
            + Clone
            + Default
            + Copy
            + std::ops::Add<Output = T>
            + std::ops::Mul<f32, Output = T>,
    {
        let factor = factor.max(1);
        let w = (self.w / factor).max(2);
        let h = (self.h / factor).max(2);
        let small = Tile {
            w,
            h,
            size: self.size,
        };
        Coarse {
            data: small.par(|p, _, _| f(p)),
            w,
            h,
            sx: w as f32 / self.w as f32,
            sy: h as f32 / self.h as f32,
        }
    }
}

/// A field on a coarse grid, sampled bilinearly (wrapping) at full-resolution texels.
pub struct Coarse<T> {
    data: Vec<T>,
    w: usize,
    h: usize,
    sx: f32,
    sy: f32,
}

impl<T> Coarse<T>
where
    T: Copy + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
    /// Value at full-resolution texel (x, y).
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> T {
        let fx = (x as f32 + 0.5) * self.sx - 0.5;
        let fy = (y as f32 + 0.5) * self.sy - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let wrap = |i: f32, n: usize| -> usize {
            let i = i as isize;
            if i < 0 {
                (i + n as isize) as usize
            } else if i as usize >= n {
                i as usize - n
            } else {
                i as usize
            }
        };
        let xa = wrap(x0, self.w);
        let xb = wrap(x0 + 1.0, self.w);
        let ya = wrap(y0, self.h);
        let yb = wrap(y0 + 1.0, self.h);
        let d = &self.data;
        let top = d[ya * self.w + xa] * (1.0 - tx) + d[ya * self.w + xb] * tx;
        let bottom = d[yb * self.w + xa] * (1.0 - tx) + d[yb * self.w + xb] * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// Fill `out` (rows of `w` texels) with `f(x, y)` on all cores.
pub fn par_fill<T: Send>(out: &mut [T], w: usize, f: impl Fn(usize, usize) -> T + Sync) {
    let h = out.len() / w.max(1);
    let threads = threads();
    let rows = (h / (threads * 6)).max(1);
    let fill = |y0: usize, chunk: &mut [T]| {
        for (r, row) in chunk.chunks_mut(w).enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                *v = f(x, y0 + r);
            }
        }
    };
    if threads == 1 || h <= rows {
        fill(0, out);
        return;
    }
    let chunks = std::sync::Mutex::new(out.chunks_mut(rows * w).enumerate());
    let fill = &fill;
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let next = chunks.lock().map(|mut c| c.next()).unwrap_or(None);
                    let Some((ci, chunk)) = next else { break };
                    fill(ci * rows, chunk);
                }
            });
        }
    });
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 32)
}

/// Run `f(row)` over each row of `w` values on all cores.
fn par_rows(data: &mut [f32], w: usize, f: impl Fn(&mut [f32]) + Sync) {
    let threads = threads();
    let h = data.len() / w;
    let rows = (h / (threads * 4)).max(1);
    let chunks = std::sync::Mutex::new(data.chunks_mut(rows * w));
    let f = &f;
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let next = chunks.lock().map(|mut c| c.next()).unwrap_or(None);
                    let Some(chunk) = next else { break };
                    for row in chunk.chunks_mut(w) {
                        f(row);
                    }
                }
            });
        }
    });
}

/// Box blur of radius `r` along each row, wrapping.
fn box_rows(data: &mut [f32], w: usize, r: usize) {
    par_rows(data, w, |row| {
        let n = row.len();
        let r = r.min(n.saturating_sub(1) / 2).max(1);
        // Row padded by `r + 1` on both sides so the running sum never wraps an index.
        let mut src = Vec::with_capacity(n + 2 * r + 2);
        src.extend_from_slice(&row[n - r - 1..]);
        src.extend_from_slice(row);
        src.extend_from_slice(&row[..r + 1]);
        let inv = 1.0 / (2 * r + 1) as f64;
        let mut sum: f64 = src[1..2 * r + 2].iter().map(|v| *v as f64).sum();
        for (x, v) in row.iter_mut().enumerate() {
            *v = (sum * inv) as f32;
            // Window for x is src[x + 1 ..= x + 2r + 1].
            sum += src[x + 2 * r + 2] as f64 - src[x + 1] as f64;
        }
    });
}

/// Rows become columns: `out[x * h + y] = data[y * w + x]`.
fn transpose(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; data.len()];
    par_fill(&mut out, h, |y, x| data[y * w + x]);
    out
}

/// Maps derived from a height field.
pub struct Relief {
    /// Unit tangent-space normals (+X right / +u, +Y up the image / -v, +Z out).
    pub normal: Vec<Vec3>,
    /// Ambient occlusion, 0 (fully occluded) .. 1 (open).
    pub ao: Vec<f32>,
    /// Concavity: positive in pits and grooves, negative on ridges and edges (roughly the
    /// slope toward the surroundings at the curvature radius).
    pub curv: Vec<f32>,
}

/// Normals, occlusion and curvature from `height` (meters).
///
/// - `ao_radii_m`: horizon search distances; the occlusion of each is how far the
///   surroundings rise above the texel at that distance.
/// - `ao_strength`: 1 is physically motivated (cos² of the horizon elevation), more darkens.
/// - `curv_radius_m`: scale of the concavity measure.
pub fn relief(
    tile: &Tile,
    height: &[f32],
    ao_radii_m: &[f32],
    ao_strength: f32,
    curv_radius_m: f32,
) -> Relief {
    let t = tile.texel_m();
    let (w, hh) = (tile.w, tile.h);
    let normal = tile.par(|_, x, y| {
        let xm = if x == 0 { w - 1 } else { x - 1 };
        let xp = if x + 1 == w { 0 } else { x + 1 };
        let ym = if y == 0 { hh - 1 } else { y - 1 };
        let yp = if y + 1 == hh { 0 } else { y + 1 };
        let h = |xx: usize, yy: usize| height[yy * w + xx];
        // Sobel gradient (smoother than central differences on noisy height).
        let gx =
            (h(xp, ym) + 2.0 * h(xp, y) + h(xp, yp)) - (h(xm, ym) + 2.0 * h(xm, y) + h(xm, yp));
        let gy =
            (h(xm, yp) + 2.0 * h(x, yp) + h(xp, yp)) - (h(xm, ym) + 2.0 * h(x, ym) + h(xp, ym));
        let dhdx = gx / (8.0 * t);
        let dhdrow = gy / (8.0 * t);
        // +Y points up the image (toward decreasing rows).
        Vec3::new(-dhdx, dhdrow, 1.0).normalize()
    });
    let blurred: Vec<(f32, Vec<f32>)> = ao_radii_m
        .iter()
        .map(|&r| (r, tile.blur(height, r)))
        .collect();
    let ao = tile.par_idx(|i| {
        let h = height[i];
        let mut max_t = 0.0f32;
        let mut sum_t = 0.0f32;
        for (r, b) in &blurred {
            // Mean distance of the tent-weighted ring is about 0.4 r.
            let tan = ((b[i] - h) / (0.45 * r)).max(0.0);
            max_t = max_t.max(tan);
            sum_t += tan;
        }
        let tan = max_t * 0.6 + sum_t / blurred.len().max(1) as f32 * 0.4;
        let cos2 = 1.0 / (1.0 + tan * tan * ao_strength);
        cos2.clamp(0.0, 1.0)
    });
    // Reuse an occlusion blur of the same radius when there is one.
    let own;
    let cb = match blurred
        .iter()
        .find(|(r, _)| (r - curv_radius_m).abs() < 1e-6)
    {
        Some((_, b)) => b,
        None => {
            own = tile.blur(height, curv_radius_m);
            &own
        }
    };
    let curv = tile.par_idx(|i| (cb[i] - height[i]) / curv_radius_m);
    Relief { normal, ao, curv }
}

/// Normals from a height field (meters) plus an extra slope per texel (for glints and
/// sparkle that should not show in the occlusion).
pub fn add_slope(normal: &mut [Vec3], slope: &[Vec2]) {
    for (n, s) in normal.iter_mut().zip(slope) {
        let g = Vec2::new(n.x / n.z, n.y / n.z) + *s;
        *n = Vec3::new(g.x, g.y, 1.0).normalize();
    }
}

/// A finished texture set in linear values, ready to pack.
pub struct Maps {
    pub tile: Tile,
    /// Linear albedo.
    pub albedo: Vec<Vec3>,
    /// Optional opacity (stored in the albedo alpha).
    pub alpha: Option<Vec<f32>>,
    /// Unit tangent-space normals.
    pub normal: Vec<Vec3>,
    pub ao: Vec<f32>,
    /// Perceptual roughness.
    pub rough: Vec<f32>,
    pub metal: Vec<f32>,
    /// Linear emission color 0..1 (scaled by the material's emissive strength).
    pub emissive: Option<Vec<Vec3>>,
}

/// Everything a material says about one texel (besides its normal).
#[derive(Debug, Clone, Copy, Default)]
pub struct Texel {
    /// Linear albedo.
    pub albedo: Vec3,
    /// Perceptual roughness.
    pub rough: f32,
    pub metal: f32,
    pub ao: f32,
    /// Linear emission 0..1.
    pub emissive: Vec3,
    pub alpha: f32,
}

impl Texel {
    /// An opaque dielectric texel.
    pub fn new(albedo: Vec3, rough: f32, ao: f32) -> Self {
        Self {
            albedo,
            rough,
            metal: 0.0,
            ao,
            emissive: Vec3::ZERO,
            alpha: 1.0,
        }
    }
}

impl Maps {
    /// Maps from per-texel values. `emissive` and `alpha` say whether those images are needed.
    pub fn new(
        tile: Tile,
        normal: Vec<Vec3>,
        texels: &[Texel],
        emissive: bool,
        alpha: bool,
    ) -> Self {
        Self {
            tile,
            albedo: texels.iter().map(|t| t.albedo).collect(),
            alpha: alpha.then(|| texels.iter().map(|t| t.alpha).collect()),
            normal,
            ao: texels.iter().map(|t| t.ao).collect(),
            rough: texels.iter().map(|t| t.rough.clamp(0.02, 1.0)).collect(),
            metal: texels.iter().map(|t| t.metal).collect(),
            emissive: emissive.then(|| texels.iter().map(|t| t.emissive).collect()),
        }
    }
}

/// Per-texel white noise in 0..1 (stable for a texel index and key).
#[inline(always)]
pub fn white(i: usize, k: u32) -> f32 {
    noise::unit(noise::mix32(
        (i as u32).wrapping_mul(0x9e37_79b9) ^ k.wrapping_mul(0x85eb_ca6b),
    ))
}

/// The images of a texture set.
pub struct TextureImages {
    /// sRGB albedo (alpha = opacity).
    pub albedo: Image,
    /// Linear tangent-space normal map.
    pub normal: Image,
    /// Linear occlusion (R), roughness (G), metalness (B).
    pub orm: Image,
    /// sRGB emission.
    pub emissive: Option<Image>,
}

/// Handles of an uploaded texture set.
pub struct TextureHandles {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub orm: Handle<Image>,
    pub emissive: Option<Handle<Image>>,
}

impl TextureImages {
    /// Add every image to `images`.
    pub fn add_to(self, images: &mut Assets<Image>) -> TextureHandles {
        TextureHandles {
            albedo: images.add(self.albedo),
            normal: images.add(self.normal),
            orm: images.add(self.orm),
            emissive: self.emissive.map(|e| images.add(e)),
        }
    }

    /// Bytes on the GPU and CPU for these images including mips (generated mips counted too).
    pub fn bytes(&self) -> usize {
        let one = |i: &Image| i.data.len() * 4 / 3;
        one(&self.albedo)
            + one(&self.normal)
            + one(&self.orm)
            + self.emissive.as_ref().map_or(0, one)
    }
}

/// Linear 0..1 to an sRGB byte.
pub fn srgb_u8(v: f32) -> u8 {
    static LUT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..4096)
            .map(|i| {
                let l = i as f32 / 4095.0;
                let s = if l <= 0.003_130_8 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect()
    });
    lut[(v.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize]
}

/// 0..1 to a linear byte.
pub fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

impl Maps {
    /// Pack into images. Normal and ORM images get their own mip chains: normals are averaged
    /// without renormalizing, and roughness grows where averaged normals disagree (Toksvig),
    /// so bumpy surfaces stay correctly rough in the distance instead of sparkling.
    pub fn images(&self) -> TextureImages {
        let (w, h) = (self.tile.w as u32, self.tile.h as u32);
        let n = self.tile.texels();
        let albedo = encode(n, |i| {
            let c = self.albedo[i];
            let a = self.alpha.as_ref().map_or(1.0, |a| a[i]);
            [srgb_u8(c.x), srgb_u8(c.y), srgb_u8(c.z), unorm(a)]
        });
        let albedo = Image::new(w, h, albedo, true);

        // Normal and ORM chains in floats.
        let mut normals = self.normal.clone();
        let mut ao = self.ao.clone();
        let mut alpha2: Vec<f32> = self.rough.iter().map(|r| r.powi(4)).collect();
        let mut metal = self.metal.clone();
        let (mut lw, mut lh) = (self.tile.w, self.tile.h);
        let encode_normal = |ns: &[Vec3]| {
            encode(ns.len(), |i| {
                let v = ns[i];
                [
                    unorm(v.x * 0.5 + 0.5),
                    unorm(v.y * 0.5 + 0.5),
                    unorm(v.z * 0.5 + 0.5),
                    255,
                ]
            })
        };
        let encode_orm = |ao: &[f32], a2: &[f32], m: &[f32], ns: &[Vec3]| {
            encode(ao.len(), |i| {
                // Toksvig: alpha'^2 = alpha^2 + 2 (1 - |n|) / |n| for averaged normals.
                let len = ns[i].length().clamp(1e-3, 1.0);
                let a2 = a2[i] + 2.0 * (1.0 - len) / len;
                [unorm(ao[i]), unorm(a2.sqrt().sqrt()), unorm(m[i]), 255]
            })
        };
        let mut normal_img = Image::new(w, h, encode_normal(&normals), false);
        let mut orm_img = Image::new(w, h, encode_orm(&ao, &alpha2, &metal, &normals), false);
        while lw > 1 || lh > 1 {
            let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
            normals = down(&normals, lw, lh, nw, nh);
            ao = down(&ao, lw, lh, nw, nh);
            alpha2 = down(&alpha2, lw, lh, nw, nh);
            metal = down(&metal, lw, lh, nw, nh);
            normal_img.mips.push(encode_normal(&normals));
            orm_img
                .mips
                .push(encode_orm(&ao, &alpha2, &metal, &normals));
            (lw, lh) = (nw, nh);
        }

        let emissive = self.emissive.as_ref().map(|e| {
            let data = encode(n, |i| {
                let c = e[i];
                [srgb_u8(c.x), srgb_u8(c.y), srgb_u8(c.z), 255]
            });
            Image::new(w, h, data, true)
        });
        TextureImages {
            albedo,
            normal: normal_img,
            orm: orm_img,
            emissive,
        }
    }
}

/// RGBA8 bytes for `n` texels from `f(index)`, in parallel.
fn encode(n: usize, f: impl Fn(usize) -> [u8; 4] + Sync) -> Vec<u8> {
    let row = if n.is_multiple_of(1024) {
        1024
    } else {
        n.max(1)
    };
    let mut px = vec![[0u8; 4]; n];
    par_fill(&mut px, row, |x, y| f(y * row + x));
    px.concat()
}

/// 2×2 box downsample (sizes are powers of two or 1).
fn down<T>(src: &[T], w: usize, h: usize, nw: usize, nh: usize) -> Vec<T>
where
    T: Copy + Send + Sync + std::ops::Add<Output = T> + std::ops::Mul<f32, Output = T>,
{
    let sx = if w > 1 { 2 } else { 1 };
    let sy = if h > 1 { 2 } else { 1 };
    let k = 1.0 / (sx * sy) as f32;
    let mut out = vec![src[0]; nw * nh];
    let f = |x: usize, y: usize| {
        let mut acc = src[(y * sy) * w + x * sx] * k;
        for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
            if dx < sx && dy < sy {
                acc = acc + src[(y * sy + dy) * w + x * sx + dx] * k;
            }
        }
        acc
    };
    if nw * nh >= 4096 {
        par_fill(&mut out, nw, f);
    } else {
        for (i, v) in out.iter_mut().enumerate() {
            *v = f(i % nw, i / nw);
        }
    }
    out
}

/// Smooth 0..1 step.
#[inline(always)]
pub fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear interpolation of colors.
#[inline(always)]
pub fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// Linear interpolation.
#[inline(always)]
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Linear color from an sRGB hex like `0x8a7f70`.
pub fn hex(rgb: u32) -> Vec3 {
    let c = engine_render::material::srgb_hex_to_linear(rgb);
    Vec3::new(c[0], c[1], c[2])
}

/// Scale a linear color's brightness, keeping hue.
#[inline(always)]
pub fn shade(c: Vec3, k: f32) -> Vec3 {
    c * k
}

/// Shift saturation: 0 = gray, 1 = unchanged, >1 more saturated.
#[inline(always)]
pub fn saturate_color(c: Vec3, s: f32) -> Vec3 {
    let l = c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    (Vec3::splat(l) + (c - Vec3::splat(l)) * s).max(Vec3::ZERO)
}

/// Keep albedo physically plausible for dielectrics (no pure black or white).
#[inline(always)]
pub fn plausible(c: Vec3) -> Vec3 {
    c.clamp(Vec3::splat(0.02), Vec3::splat(0.92))
}
