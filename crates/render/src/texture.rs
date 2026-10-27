//! Textures: the [`Image`] asset and the PNG/JPEG/HDR [`ImageLoader`].
//!
//! An [`Image`] is CPU-side pixel data (RGBA8, or RGBA16F for HDR) plus an optional mip chain.
//! Load one from disk (`server.load(&mut images, "textures/bark.png")`) or build one in code
//! ([`Image::from_fn`], [`Image::checker`]) and point a material at it:
//!
//! ```ignore
//! let checker = images.add(Image::checker(256, 8, [240, 240, 240, 255], [30, 30, 30, 255]));
//! let mat = StandardMaterial { base_color_texture: Some(checker), ..Default::default() };
//! ```
//!
//! Color space: base color and emissive textures are decoded as sRGB when [`Image::srgb`] is
//! set (the default for loaded PNG/JPEG files); normal, metallic/roughness and occlusion
//! textures are always sampled as linear data, whatever the flag says. HDR (`.hdr`) files are
//! linear floating point and stay that way (`Rgba16Float` on the GPU).
//!
//! Mipmaps are generated on the CPU (loaded files: on the loading thread; images made in code:
//! when first uploaded), averaging in linear light for sRGB images.
//!
//! Releasing pixels: with `RenderSettings::release_image_pixels` the renderer frees an image's
//! `data` and `mips` once it has uploaded the image and the image has stayed unchanged for
//! [`RELEASE_AFTER_FRAMES`](crate::image_cache::RELEASE_AFTER_FRAMES) frames; the GPU copy is
//! then the only one. Width, height, format and flags stay, [`Image::is_released`] turns true,
//! and assigning new `data` uploads the image again. Set [`Image::keep_pixels`] on images that
//! code reads on the CPU or uploads itself.

use engine_assets::{Asset, AssetLoader, LoadContext};
use serde::{Deserialize, Serialize};

/// Pixel layout of an [`Image`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PixelFormat {
    /// 8 bits per channel RGBA (4 bytes per pixel). sRGB or linear per [`Image::srgb`].
    #[default]
    Rgba8,
    /// Half-float RGBA (8 bytes per pixel, little-endian f16), always linear. Used for HDR.
    Rgba16Float,
}

impl PixelFormat {
    /// Bytes per pixel.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgba8 => 4,
            PixelFormat::Rgba16Float => 8,
        }
    }
}

/// A 2D texture on the CPU. An asset (`Assets<Image>`); the renderer uploads it when a
/// material uses it and re-uploads it when its version changes.
///
/// With `RenderSettings::release_image_pixels` the renderer may free `data` and `mips` after
/// uploading the image (see the module docs); [`Image::keep_pixels`] opts an image out.
#[derive(Clone, PartialEq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    pub format: PixelFormat,
    /// Level 0 pixels, rows top to bottom, tightly packed.
    pub data: Vec<u8>,
    /// The RGB channels are sRGB-encoded color (albedo, emissive). `false` for linear data
    /// (normal maps, roughness, masks) and for HDR images.
    pub srgb: bool,
    /// Use (and generate when missing) a mip chain. Turn off for UI-like images that are
    /// never minified.
    pub mipmaps: bool,
    /// Mip levels 1.. (each half the size of the previous, down to 1×1). Empty means "generate
    /// on upload" when `mipmaps` is set.
    pub mips: Vec<Vec<u8>>,
    /// Never free this image's pixels after uploading it, even with
    /// `RenderSettings::release_image_pixels` (default false). Set it on images that code reads
    /// back on the CPU (`data`, [`Image::pixel`]), writes in place less often than every couple
    /// of seconds, or uploads to the GPU itself from the pixels instead of through
    /// `GpuShared::image_texture`.
    pub keep_pixels: bool,
}

impl Asset for Image {}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("srgb", &self.srgb)
            .field("mipmaps", &self.mipmaps)
            .field("mip_levels", &(self.mips.len() + 1))
            .field("keep_pixels", &self.keep_pixels)
            .field("released", &self.is_released())
            .finish()
    }
}

impl Default for Image {
    /// A 1×1 opaque white sRGB image.
    fn default() -> Self {
        Self::solid([255; 4], true)
    }
}

impl Image {
    /// An RGBA8 image from raw pixels (`width * height * 4` bytes, rows top to bottom).
    /// Missing bytes are filled with opaque black, extra bytes dropped.
    pub fn new(width: u32, height: u32, mut data: Vec<u8>, srgb: bool) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let len = width as usize * height as usize * 4;
        if data.len() != len {
            data.resize(len, 0);
        }
        Self {
            width,
            height,
            format: PixelFormat::Rgba8,
            data,
            srgb,
            mipmaps: true,
            mips: Vec::new(),
            keep_pixels: false,
        }
    }

    /// A linear half-float image from `width * height` RGBA f32 pixels (HDR data, e.g.
    /// luminance in cd/m²).
    pub fn from_rgba_f32(width: u32, height: u32, pixels: &[f32]) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let count = width as usize * height as usize * 4;
        let mut data = Vec::with_capacity(count * 2);
        for i in 0..count {
            let v = pixels
                .get(i)
                .copied()
                .unwrap_or(if i % 4 == 3 { 1.0 } else { 0.0 });
            data.extend_from_slice(&f32_to_f16(v).to_le_bytes());
        }
        Self {
            width,
            height,
            format: PixelFormat::Rgba16Float,
            data,
            srgb: false,
            mipmaps: true,
            mips: Vec::new(),
            keep_pixels: false,
        }
    }

    /// A 1×1 image of one color.
    pub fn solid(rgba: [u8; 4], srgb: bool) -> Self {
        let mut image = Self::new(1, 1, rgba.to_vec(), srgb);
        image.mipmaps = false;
        image
    }

    /// The image an [`crate::ExtraView`] renders into (`ViewTarget::Image`), `size` pixels: no
    /// CPU pixels, its texture is the view's picture.
    pub fn render_target(size: engine_core::prelude::UVec2) -> Self {
        Self {
            width: size.x.max(1),
            height: size.y.max(1),
            format: PixelFormat::Rgba8,
            data: Vec::new(),
            srgb: true,
            mipmaps: false,
            mips: Vec::new(),
            keep_pixels: false,
        }
    }

    /// An RGBA8 image computed per pixel: `f(x, y) -> [r, g, b, a]`.
    pub fn from_fn(
        width: u32,
        height: u32,
        srgb: bool,
        mut f: impl FnMut(u32, u32) -> [u8; 4],
    ) -> Self {
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&f(x, y));
            }
        }
        Self::new(width, height, data, srgb)
    }

    /// A `size`×`size` sRGB checkerboard with `cells`×`cells` squares of colors `a` and `b`.
    /// Handy for prototyping and for checking UVs.
    pub fn checker(size: u32, cells: u32, a: [u8; 4], b: [u8; 4]) -> Self {
        let cell = (size / cells.max(1)).max(1);
        Self::from_fn(size, size, true, |x, y| {
            if ((x / cell) + (y / cell)).is_multiple_of(2) {
                a
            } else {
                b
            }
        })
    }

    /// Decode PNG, JPEG or Radiance HDR bytes. `extension` (lowercase, no dot) picks the format;
    /// `None` guesses from the content. `srgb` applies to 8-bit images (HDR is always linear).
    pub fn decode(bytes: &[u8], extension: Option<&str>, srgb: bool) -> Result<Self, String> {
        let format = match extension {
            Some("png") => Some(image::ImageFormat::Png),
            Some("jpg") | Some("jpeg") => Some(image::ImageFormat::Jpeg),
            Some("hdr") => Some(image::ImageFormat::Hdr),
            _ => None,
        };
        let decoded = match format {
            Some(f) => image::load_from_memory_with_format(bytes, f),
            None => image::load_from_memory(bytes),
        }
        .map_err(|e| format!("could not decode the image: {e}"))?;
        let (w, h) = (decoded.width(), decoded.height());
        Ok(match decoded {
            image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_) => {
                Self::from_rgba_f32(w, h, decoded.to_rgba32f().as_raw())
            }
            other => Self::new(w, h, other.to_rgba8().into_raw(), srgb),
        })
    }

    /// Number of levels in a full mip chain for this size (1 for a 1×1 image).
    pub fn full_mip_count(&self) -> u32 {
        32 - self.width.max(self.height).max(1).leading_zeros()
    }

    /// Size of mip `level`.
    pub fn level_size(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// True once the renderer has freed the pixels (`data` and `mips`) after uploading them
    /// (`RenderSettings::release_image_pixels`): the GPU copy is the only one left. Assigning new
    /// `data` makes the image whole again (and uploads it again).
    pub fn is_released(&self) -> bool {
        self.data.is_empty()
    }

    /// Bytes of pixels held on the CPU: level 0 plus the stored mips (0 once released).
    pub fn pixel_bytes(&self) -> usize {
        self.data.len() + self.mips.iter().map(Vec::len).sum::<usize>()
    }

    /// Free `data` and `mips` (keeping the size, format and flags); returns the bytes freed.
    pub(crate) fn release_pixels(&mut self) -> usize {
        let bytes = self.pixel_bytes();
        self.data = Vec::new();
        self.mips = Vec::new();
        bytes
    }

    /// RGBA8 pixel at (x, y) of level 0 (half-float images are converted, clamped to 0..1).
    /// Transparent black once the pixels were released ([`Image::is_released`]).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y.min(self.height.max(1) - 1) as usize * self.width as usize
            + x.min(self.width.max(1) - 1) as usize)
            * self.format.bytes_per_pixel();
        if i + self.format.bytes_per_pixel() > self.data.len() {
            return [0; 4];
        }
        match self.format {
            PixelFormat::Rgba8 => [
                self.data[i],
                self.data[i + 1],
                self.data[i + 2],
                self.data[i + 3],
            ],
            PixelFormat::Rgba16Float => {
                let mut out = [0u8; 4];
                for (c, o) in out.iter_mut().enumerate() {
                    let h = u16::from_le_bytes([self.data[i + c * 2], self.data[i + c * 2 + 1]]);
                    *o = (f16_to_f32(h).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
                out
            }
        }
    }

    /// Fill [`Image::mips`] with a full chain (box filter; in linear light for sRGB images).
    pub fn generate_mips(&mut self) {
        self.mips = self.mip_chain(self.srgb);
    }

    /// Compute mip levels 1.. without storing them. `srgb` chooses whether RGB is averaged in
    /// linear light (sRGB-encoded data) or as-is (linear data). Empty when `data` is shorter
    /// than level 0 (a released image, see [`Image::is_released`]).
    pub fn mip_chain(&self, srgb: bool) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::new();
        let level0 = self.width as usize * self.height as usize * self.format.bytes_per_pixel();
        if self.data.len() < level0 {
            return out;
        }
        let (mut w, mut h) = (self.width, self.height);
        let mut prev: &[u8] = &self.data;
        let mut level = 1;
        while level < self.full_mip_count() {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let next = match self.format {
                PixelFormat::Rgba8 => downsample_rgba8(prev, w, h, nw, nh, srgb),
                PixelFormat::Rgba16Float => downsample_f16(prev, w, h, nw, nh),
            };
            out.push(next);
            prev = out.last().map(|v| v.as_slice()).unwrap_or(&[]);
            (w, h) = (nw, nh);
            level += 1;
        }
        out
    }

    /// True if `data` (and every stored mip) has the size the dimensions require and there are
    /// no more mips than a full chain has (the GPU rejects extra levels).
    pub fn is_valid(&self) -> bool {
        let bpp = self.format.bytes_per_pixel();
        if self.width == 0
            || self.height == 0
            || self.data.len() != self.width as usize * self.height as usize * bpp
            || self.mips.len() >= self.full_mip_count() as usize
        {
            return false;
        }
        self.mips.iter().enumerate().all(|(i, m)| {
            let (w, h) = self.level_size(i as u32 + 1);
            m.len() == w as usize * h as usize * bpp
        })
    }
}

fn srgb_to_linear_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let s = i as f32 / 255.0;
            *v = if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            };
        }
        t
    })
}

/// Linear (0..1) → sRGB byte through a 4096-entry table.
fn linear_to_srgb_u8(v: f32) -> u8 {
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

/// Source texels covering destination texel `d` along one axis (box filter; 1 or 2 texels,
/// 3 for the last texel of an odd-sized axis so nothing is skipped).
fn footprint(d: u32, src: u32, dst: u32) -> std::ops::Range<u32> {
    if src <= 1 {
        return 0..1;
    }
    let start = d * 2;
    let end = if d + 1 == dst {
        src
    } else {
        (start + 2).min(src)
    };
    start.min(src - 1)..end.max(start + 1)
}

pub(crate) fn downsample_rgba8(
    src: &[u8],
    w: u32,
    h: u32,
    nw: u32,
    nh: u32,
    srgb: bool,
) -> Vec<u8> {
    let lut = srgb_to_linear_lut();
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    for y in 0..nh {
        let ys = footprint(y, h, nh);
        for x in 0..nw {
            let xs = footprint(x, w, nw);
            let mut sum = [0.0f32; 4];
            let mut n = 0.0;
            for sy in ys.clone() {
                for sx in xs.clone() {
                    let i = (sy as usize * w as usize + sx as usize) * 4;
                    for c in 0..3 {
                        sum[c] += if srgb {
                            lut[src[i + c] as usize]
                        } else {
                            src[i + c] as f32 / 255.0
                        };
                    }
                    sum[3] += src[i + 3] as f32 / 255.0;
                    n += 1.0;
                }
            }
            for (c, s) in sum.iter().enumerate() {
                let v = s / n;
                out.push(if srgb && c < 3 {
                    linear_to_srgb_u8(v)
                } else {
                    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
                });
            }
        }
    }
    out
}

fn downsample_f16(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let read = |i: usize| f16_to_f32(u16::from_le_bytes([src[i], src[i + 1]]));
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 8);
    for y in 0..nh {
        let ys = footprint(y, h, nh);
        for x in 0..nw {
            let xs = footprint(x, w, nw);
            let mut sum = [0.0f32; 4];
            let mut n = 0.0;
            for sy in ys.clone() {
                for sx in xs.clone() {
                    let i = (sy as usize * w as usize + sx as usize) * 8;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += read(i + c * 2);
                    }
                    n += 1.0;
                }
            }
            for s in sum {
                out.extend_from_slice(&f32_to_f16(s / n).to_le_bytes());
            }
        }
    }
    out
}

/// f32 → IEEE half (round to nearest, clamped to the largest finite half, NaN preserved).
pub fn f32_to_f16(v: f32) -> u16 {
    if v.is_nan() {
        return 0x7e00;
    }
    let v = v.clamp(-65504.0, 65504.0);
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    let e = exp - 127 + 15;
    if e <= 0 {
        // Subnormal half (or zero).
        let shift = 14 - e;
        if shift > 24 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let rounded = (m + (1 << (shift - 1))) >> shift;
        return sign | rounded as u16;
    }
    let half = ((e as u32) << 10) | (mant >> 13);
    // Round to nearest; a carry into the exponent is still the correct value.
    let half = half + ((mant >> 12) & 1);
    sign | half.min(0x7bff) as u16
}

/// IEEE half → f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = match exp {
        0 if mant == 0 => sign,
        0 => {
            // Subnormal: normalize.
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (mant << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

/// Loads `.png`, `.jpg`/`.jpeg` (sRGB color, RGBA8) and `.hdr` (linear RGBA16F) files as
/// [`Image`]s with a generated mip chain. Registered by `RenderPlugin`.
pub struct ImageLoader;

impl AssetLoader for ImageLoader {
    fn extensions(&self) -> &[&str] {
        &["png", "jpg", "jpeg", "hdr"]
    }

    fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String> {
        let ext = ctx.extension();
        let mut image = Image::decode(bytes, Some(&ext), true)?;
        image.generate_mips();
        ctx.add_primary(image);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip() {
        for v in [
            0.0f32, 1.0, -2.5, 0.333, 1000.0, 65504.0, 6.1e-5, 1e-6, 3.0e-8,
        ] {
            let back = f16_to_f32(f32_to_f16(v));
            let tolerance = (v.abs() * 1e-3).max(6e-8);
            assert!((back - v).abs() <= tolerance, "{v} -> {back}");
        }
        assert_eq!(f16_to_f32(f32_to_f16(1e9)), 65504.0);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
        assert_eq!(f32_to_f16(1.0), 0x3c00);
    }

    #[test]
    fn mip_chain_has_the_right_sizes() {
        let img = Image::checker(64, 4, [255, 0, 0, 255], [0, 0, 255, 255]);
        assert_eq!(img.full_mip_count(), 7);
        let mut img2 = Image::new(5, 3, vec![128; 5 * 3 * 4], false);
        img2.generate_mips();
        assert_eq!(img2.mips.len(), 2);
        assert!(img2.is_valid());
        assert_eq!(img2.level_size(1), (2, 1));
        assert_eq!(img2.level_size(2), (1, 1));
        let chain = img.mip_chain(true);
        assert_eq!(chain.len(), 6);
        assert_eq!(chain.last().unwrap().len(), 4);
        // An extra 1x1 level past the end of the chain would fail GPU validation (and with it
        // the whole frame), so it makes the image invalid.
        img2.mips.push(vec![0; 4]);
        assert!(!img2.is_valid());
        let mut empty = Image::new(1, 1, vec![0; 4], false);
        empty.width = 0;
        assert!(!empty.is_valid());
    }

    #[test]
    fn released_images_keep_their_shape_and_cope_with_reads() {
        let mut img = Image::checker(8, 2, [255, 0, 0, 255], [0, 0, 255, 255]);
        img.generate_mips();
        let bytes = (64 + 16 + 4 + 1) * 4;
        assert_eq!(img.pixel_bytes(), bytes);
        assert!(!img.is_released());
        assert_eq!(img.release_pixels(), bytes);
        assert!(img.is_released());
        assert_eq!(
            (img.width, img.height, img.format),
            (8, 8, PixelFormat::Rgba8)
        );
        assert!(img.srgb && img.mipmaps && !img.keep_pixels);
        assert_eq!(img.pixel_bytes(), 0);
        assert_eq!(img.pixel(1, 1), [0; 4]);
        assert!(img.mip_chain(true).is_empty());
        img.generate_mips();
        assert!(img.mips.is_empty());
        assert!(!img.is_valid());
        assert!(format!("{img:?}").contains("released: true"));
        // New pixels make it whole again.
        img.data = vec![9; 8 * 8 * 4];
        assert!(!img.is_released() && img.is_valid());
    }

    #[test]
    fn srgb_mips_average_in_linear_light() {
        // Black/white checker: the 1x1 mip should be 50% linear grey = sRGB ~188, not 128.
        let img = Image::checker(2, 2, [0, 0, 0, 255], [255, 255, 255, 255]);
        let srgb = img.mip_chain(true);
        let linear = img.mip_chain(false);
        assert!((srgb[0][0] as i32 - 188).abs() <= 1, "{:?}", srgb[0]);
        assert!((linear[0][0] as i32 - 128).abs() <= 1, "{:?}", linear[0]);
        assert_eq!(srgb[0][3], 255);
    }

    #[test]
    fn decodes_png_and_hdr() {
        let mut png = Vec::new();
        let src = image::RgbaImage::from_fn(4, 2, |x, _| image::Rgba([x as u8 * 60, 10, 20, 255]));
        src.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img = Image::decode(&png, Some("png"), true).unwrap();
        assert_eq!(
            (img.width, img.height, img.format),
            (4, 2, PixelFormat::Rgba8)
        );
        assert_eq!(img.pixel(3, 1), [180, 10, 20, 255]);
        assert!(img.srgb);

        let mut hdr = Vec::new();
        let pixels: Vec<image::Rgb<f32>> = vec![image::Rgb([2.0, 0.5, 0.25]); 4];
        image::codecs::hdr::HdrEncoder::new(&mut hdr)
            .encode(&pixels, 2, 2)
            .unwrap();
        let img = Image::decode(&hdr, Some("hdr"), true).unwrap();
        assert_eq!(img.format, PixelFormat::Rgba16Float);
        assert!(!img.srgb);
        let r = f16_to_f32(u16::from_le_bytes([img.data[0], img.data[1]]));
        assert!((r - 2.0).abs() < 0.05, "{r}");
        assert!(Image::decode(b"not an image", Some("png"), true).is_err());
    }
}
