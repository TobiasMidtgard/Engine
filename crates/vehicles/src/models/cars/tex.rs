//! Procedural texture helpers: parallel image filling, hash noise, a distance canvas that
//! line segments are stamped into (shut lines, stripes, lettering), normal maps baked from
//! height buffers, and a small stroke font for tire and livery lettering.

use engine_core::math::Vec2;
use engine_render::Image;

use super::mesher::par_fill;

/// An RGBA8 image computed per pixel on all cores.
pub fn par_image(w: u32, h: u32, srgb: bool, f: impl Fn(u32, u32) -> [u8; 4] + Sync) -> Image {
    let [img] = par_images(w, h, [srgb], |x, y| [f(x, y)]);
    img
}

/// Two RGBA8 images (e.g. color and normal map) computed per pixel in one pass.
pub fn par_image2(
    w: u32,
    h: u32,
    srgb: (bool, bool),
    f: impl Fn(u32, u32) -> ([u8; 4], [u8; 4]) + Sync,
) -> (Image, Image) {
    let [a, b] = par_images(w, h, [srgb.0, srgb.1], |x, y| {
        let (a, b) = f(x, y);
        [a, b]
    });
    (a, b)
}

/// `N` RGBA8 images (e.g. color, normal and metallic-roughness maps) computed per pixel in
/// one pass.
pub fn par_images<const N: usize>(
    w: u32,
    h: u32,
    srgb: [bool; N],
    f: impl Fn(u32, u32) -> [[u8; 4]; N] + Sync,
) -> [Image; N] {
    let row_len = w as usize * 4;
    let mut data: [Vec<u8>; N] = std::array::from_fn(|_| vec![0u8; row_len * h as usize]);
    {
        // Row y of every image, filled in place.
        let mut chunks: Vec<_> = data
            .iter_mut()
            .map(|d| d.chunks_exact_mut(row_len))
            .collect();
        let mut rows: Vec<[&mut [u8]; N]> = (0..h)
            .map(|_| std::array::from_fn(|k| chunks[k].next().expect("row")))
            .collect();
        par_fill(&mut rows, |y, out| {
            let mut px: [&mut [[u8; 4]]; N] = out.each_mut().map(|r| r.as_chunks_mut::<4>().0);
            for x in 0..w as usize {
                let p = f(x as u32, y as u32);
                for (row, v) in px.iter_mut().zip(p) {
                    row[x] = v;
                }
            }
        });
    }
    let mut k = 0;
    data.map(|d| {
        let img = Image::new(w, h, d, srgb[k]);
        k += 1;
        img
    })
}

/// A float buffer computed per pixel on all cores.
pub fn par_buffer(w: u32, h: u32, f: impl Fn(u32, u32) -> f32 + Sync) -> Vec<f32> {
    let mut out = vec![0.0f32; (w * h) as usize];
    par_fill(&mut out, |i, v| *v = f(i as u32 % w, i as u32 / w));
    out
}

/// Integer hash → 0..1.
#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// Smooth value noise (0..1), period `wrap` cells when nonzero.
pub fn value_noise(p: Vec2, seed: u32, wrap: i32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let w = |a: i32| if wrap > 0 { a.rem_euclid(wrap) } else { a };
    let (x0, y0) = (i.x as i32, i.y as i32);
    let a = hash(w(x0), w(y0), seed);
    let b = hash(w(x0 + 1), w(y0), seed);
    let c = hash(w(x0), w(y0 + 1), seed);
    let d = hash(w(x0 + 1), w(y0 + 1), seed);
    let ab = a + (b - a) * u.x;
    let cd = c + (d - c) * u.x;
    ab + (cd - ab) * u.y
}

/// Fractal value noise (0..1), tiling with period `wrap` cells at the base octave.
pub fn fbm(p: Vec2, octaves: u32, seed: u32, wrap: i32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    let mut q = p;
    let mut period = wrap;
    for o in 0..octaves {
        sum += amp * value_noise(q, seed.wrapping_add(o * 101), period);
        norm += amp;
        amp *= 0.5;
        q *= 2.0;
        period *= 2;
    }
    sum / norm
}

/// A per-pixel distance field that segments are stamped into: each pixel keeps the distance
/// (in pixels) to the nearest stamped segment and that segment's tag.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub dist: Vec<f32>,
    pub tag: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            dist: vec![f32::MAX; (w * h) as usize],
            tag: vec![0; (w * h) as usize],
        }
    }

    /// Stamp segment a–b (pixel coordinates), recording distances out to `reach` pixels.
    pub fn segment(&mut self, a: Vec2, b: Vec2, reach: f32, tag: u8) {
        let lo = a.min(b) - Vec2::splat(reach + 1.0);
        let hi = a.max(b) + Vec2::splat(reach + 1.0);
        let x0 = lo.x.max(0.0) as u32;
        let y0 = lo.y.max(0.0) as u32;
        let x1 = (hi.x.max(0.0) as u32).min(self.w - 1);
        let y1 = (hi.y.max(0.0) as u32).min(self.h - 1);
        let ab = b - a;
        let l2 = ab.length_squared().max(1e-12);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
                let d = (p - a - ab * t).length();
                let i = (y * self.w + x) as usize;
                if d < self.dist[i] {
                    self.dist[i] = d;
                    self.tag[i] = tag;
                }
            }
        }
    }

    /// Stamp a polyline.
    pub fn polyline(&mut self, pts: &[Vec2], reach: f32, tag: u8) {
        for w in pts.windows(2) {
            self.segment(w[0], w[1], reach, tag);
        }
    }

    /// Distance at a pixel (clamped to the canvas).
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.w as i32 - 1) as u32;
        let y = y.clamp(0, self.h as i32 - 1) as u32;
        self.dist[(y * self.w + x) as usize]
    }
}

/// A tangent-space normal map (+Y toward decreasing v, i.e. image up) from a height buffer
/// in pixels of height per pixel of distance times `strength`. `wrap`: tile at the edges.
pub fn normal_map(w: u32, h: u32, height: &[f32], strength: f32, wrap: bool) -> Image {
    // Neighbors are at most one pixel outside the image.
    let (wi, hi) = (w as i32, h as i32);
    let at = |x: i32, y: i32| -> f32 {
        let (x, y) = if wrap {
            let x = if x < 0 {
                x + wi
            } else if x >= wi {
                x - wi
            } else {
                x
            };
            let y = if y < 0 {
                y + hi
            } else if y >= hi {
                y - hi
            } else {
                y
            };
            (x, y)
        } else {
            (x.clamp(0, wi - 1), y.clamp(0, hi - 1))
        };
        height[(y * wi + x) as usize]
    };
    par_image(w, h, false, |x, y| {
        let (x, y) = (x as i32, y as i32);
        let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5 * strength;
        let dy = (at(x, y + 1) - at(x, y - 1)) * 0.5 * strength;
        encode_normal(-dx, dy)
    })
}

/// Encode a tangent-space normal from its slopes.
#[inline]
pub fn encode_normal(nx: f32, ny: f32) -> [u8; 4] {
    let l = (nx * nx + ny * ny + 1.0).sqrt();
    let e = |v: f32| ((v / l * 0.5 + 0.5) * 255.0 + 0.5) as u8;
    [e(nx), e(ny), e(1.0), 255]
}

/// sRGB-encode a linear 0..1 value (table lookup, 1/8192 steps).
#[inline]
pub fn srgb(v: f32) -> u8 {
    static LUT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..=8192)
            .map(|i| {
                let v = i as f32 / 8192.0;
                let s = if v <= 0.003_130_8 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0 + 0.5) as u8
            })
            .collect()
    });
    lut[(v.clamp(0.0, 1.0) * 8192.0 + 0.5) as usize]
}

/// Stroke-font glyphs on a 4 × 6 grid (y up), one polyline per string, points as digit
/// pairs ("0004" = (0,0)→(0,4)).
fn glyph(c: char) -> &'static [&'static str] {
    match c {
        'A' => &["0004264440", "0343"],
        'B' => &["00063645443303", "3342413000"],
        'C' => &["4536160501103041"],
        'D' => &["00062644422000"],
        'E' => &["46060040", "0333"],
        'F' => &["460600", "0333"],
        'G' => &["45361605011030414323"],
        'H' => &["0006", "4046", "0343"],
        'I' => &["1636", "2620", "1030"],
        'J' => &["4641301001"],
        'K' => &["0006", "4602", "1340"],
        'L' => &["060040"],
        'M' => &["0006234640"],
        'N' => &["00064046"],
        'O' => &["100105163645413010"],
        'P' => &["00063645443303"],
        'Q' => &["100105163645413010", "2240"],
        'R' => &["00063645443303", "2340"],
        'S' => &["453616050413334241301001"],
        'T' => &["0646", "2620"],
        'U' => &["060110304146"],
        'V' => &["062046"],
        'W' => &["0610243046"],
        'X' => &["0046", "0640"],
        'Y' => &["062346", "2320"],
        'Z' => &["06460040"],
        '0' => &["100105163645413010", "1135"],
        '1' => &["152620", "1030"],
        '2' => &["05163645440040"],
        '3' => &["05163645443313", "334241301001"],
        '4' => &["30360242"],
        '5' => &["4606033342413000"],
        '6' => &["4536160501103041423303"],
        '7' => &["064610"],
        '8' => &["13040516364544331302011030414233"],
        '9' => &["0110304145361605041343"],
        '/' => &["0046"],
        '-' => &["1333"],
        '.' => &["2021"],
        _ => &[],
    }
}

fn glyph_points(s: &str) -> Vec<Vec2> {
    let d: Vec<f32> = s
        .chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c.to_digit(10).unwrap() as f32)
        .collect();
    d.as_chunks::<2>()
        .0
        .iter()
        .map(|p| Vec2::new(p[0], p[1]))
        .collect()
}

/// Line segments spelling `text` with its baseline-left corner at `origin`, glyphs `size`
/// tall (6 grid units), along direction `dir` (unit) with `up` the glyph up direction, in
/// the caller's 2D space. Letters advance 5.6 grid units.
pub fn text_segments(
    text: &str,
    origin: Vec2,
    size: f32,
    dir: Vec2,
    up: Vec2,
) -> Vec<(Vec2, Vec2)> {
    let k = size / 6.0;
    let mut out = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let base = origin + dir * (i as f32 * 5.6 * k);
        for stroke in glyph(c.to_ascii_uppercase()) {
            let pts = glyph_points(stroke);
            for w in pts.windows(2) {
                let m = |p: Vec2| base + dir * (p.x * k) + up * (p.y * k);
                out.push((m(w[0]), m(w[1])));
            }
            if pts.len() == 1 {
                let m = |p: Vec2| base + dir * (p.x * k) + up * (p.y * k);
                out.push((m(pts[0]), m(pts[0])));
            }
        }
    }
    out
}

/// Width of `text` at glyph height `size`.
pub fn text_width(text: &str, size: f32) -> f32 {
    let n = text.chars().count() as f32;
    (n * 5.6 - 1.6).max(0.0) * size / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_parse() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/-.".chars() {
            for s in glyph(c) {
                let pts = glyph_points(s);
                assert!(!pts.is_empty(), "{c}");
                for p in pts {
                    assert!(p.x <= 4.0 && p.y <= 6.0, "{c} {p}");
                }
            }
        }
    }
}
