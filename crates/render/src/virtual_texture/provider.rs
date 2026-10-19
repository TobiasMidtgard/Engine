//! Where tiles come from: an [`Image`] in memory (images over the size threshold), a baked
//! tile file on disk, or a procedural [`TileSource`]. Every provider fills one tile with its
//! border as RGBA8; the workers then add the tile's own mip levels for the cache.

use std::sync::{Arc, OnceLock};

use super::TileSource;
use super::residency::{BORDER, Layout, SLOT, TILE};
use crate::texture::{Image, PixelFormat, downsample_rgba8};

/// Bytes of one tile with its border, RGBA8.
pub const TILE_BYTES: usize = (SLOT * SLOT * 4) as usize;

/// Something that can fill tiles of one virtual texture (crate side of [`TileSource`]).
pub(crate) trait TileProvider: Send + Sync {
    /// Size of level 0.
    fn size(&self) -> (u32, u32);
    /// RGB is sRGB-encoded color: averaged in linear light when the cache's levels are made.
    fn srgb(&self) -> bool;
    /// Fill `out` ([`TILE_BYTES`], RGBA8 rows of [`SLOT`] texels) with tile (`x`, `y`) of
    /// `level` and its border: texel (i, j) of `out` is texel (x·TILE − BORDER + i,
    /// y·TILE − BORDER + j) of the level, wrapped around its edges.
    fn read_tile(&self, level: u32, x: u32, y: u32, out: &mut [u8]) -> Result<(), String>;
}

/// The cache levels of a tile: its slot at full size, then halved [`levels`] − 1 times (box
/// filter, in linear light for sRGB color), as RGBA8, concatenated.
pub fn tile_levels(tile: &[u8], srgb: bool, levels: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(TILE_BYTES * 4 / 3 + 64);
    out.extend_from_slice(tile);
    let mut size = SLOT;
    let mut start = 0;
    for _ in 1..levels {
        let next = downsample_rgba8(
            &out[start..start + (size * size * 4) as usize],
            size,
            size,
            size / 2,
            size / 2,
            srgb,
        );
        start = out.len();
        out.extend_from_slice(&next);
        size /= 2;
    }
    out
}

/// Copy the tile of a level held in memory (`data`: `w`×`h` RGBA8) into `out`, wrapping.
pub fn cut_tile(data: &[u8], w: u32, h: u32, x: u32, y: u32, out: &mut [u8]) {
    let x0 = (x * TILE) as i64 - BORDER as i64;
    let y0 = (y * TILE) as i64 - BORDER as i64;
    let row_bytes = (SLOT * 4) as usize;
    for j in 0..SLOT as i64 {
        let sy = (y0 + j).rem_euclid(h as i64) as usize;
        let dst = &mut out[j as usize * row_bytes..(j as usize + 1) * row_bytes];
        let src_row = &data[sy * w as usize * 4..(sy + 1) * w as usize * 4];
        if x0 >= 0 && x0 + SLOT as i64 <= w as i64 {
            dst.copy_from_slice(&src_row[x0 as usize * 4..x0 as usize * 4 + row_bytes]);
        } else {
            for i in 0..SLOT as i64 {
                let sx = (x0 + i).rem_euclid(w as i64) as usize;
                dst[i as usize * 4..i as usize * 4 + 4]
                    .copy_from_slice(&src_row[sx * 4..sx * 4 + 4]);
            }
        }
    }
}

/// An image over the size threshold, copied once when it went virtual; its missing mip levels
/// are made by the first tile load.
pub(crate) struct ImageProvider {
    image: Image,
    srgb: bool,
    levels: OnceLock<Vec<Vec<u8>>>,
    root: u32,
}

impl ImageProvider {
    /// `srgb`: averages levels in linear light (an sRGB image seen through an sRGB view).
    pub fn new(image: Image, srgb: bool) -> Result<Self, String> {
        if image.format != PixelFormat::Rgba8 {
            return Err("only RGBA8 images can be virtual".into());
        }
        if !image.is_valid() {
            return Err("the image's pixels don't match its size".into());
        }
        let layout = Layout::new(image.width, image.height)?;
        Ok(Self {
            image,
            srgb,
            levels: OnceLock::new(),
            root: layout.root,
        })
    }

    fn levels(&self) -> &[Vec<u8>] {
        self.levels.get_or_init(|| {
            let stored_match = !self.image.mips.is_empty() && self.image.srgb == self.srgb;
            let mut mips = if stored_match {
                self.image.mips.clone()
            } else {
                self.image.mip_chain(self.srgb)
            };
            mips.truncate(self.root as usize);
            mips
        })
    }
}

impl TileProvider for ImageProvider {
    fn size(&self) -> (u32, u32) {
        (self.image.width, self.image.height)
    }

    fn srgb(&self) -> bool {
        self.srgb
    }

    fn read_tile(&self, level: u32, x: u32, y: u32, out: &mut [u8]) -> Result<(), String> {
        let (w, h) = self.image.level_size(level);
        let data: &[u8] = if level == 0 {
            &self.image.data
        } else {
            self.levels()
                .get(level as usize - 1)
                .ok_or("level out of range")?
        };
        if data.len() < (w * h * 4) as usize {
            return Err("level data is short".into());
        }
        cut_tile(data, w, h, x, y, out);
        Ok(())
    }
}

/// A procedural [`TileSource`] registered with `VirtualTextures::add_procedural`.
pub(crate) struct ProceduralProvider(pub Arc<dyn TileSource>);

impl TileProvider for ProceduralProvider {
    fn size(&self) -> (u32, u32) {
        self.0.size()
    }

    fn srgb(&self) -> bool {
        self.0.srgb()
    }

    fn read_tile(&self, level: u32, x: u32, y: u32, out: &mut [u8]) -> Result<(), String> {
        self.0.paint_tile(level, x, y, out);
        Ok(())
    }
}

/// The image of a provider's root level (the whole texture in at most 128×128 texels): the
/// preview stand-in other features see, and the source of its mean color.
pub(crate) fn root_image(provider: &dyn TileProvider) -> Result<Image, String> {
    let (w, h) = provider.size();
    let layout = Layout::new(w, h)?;
    let mut tile = vec![0u8; TILE_BYTES];
    provider.read_tile(layout.root, 0, 0, &mut tile)?;
    let (rw, rh) = layout.level_size(layout.root);
    let mut data = Vec::with_capacity((rw * rh * 4) as usize);
    for y in 0..rh {
        let row = ((y + BORDER) * SLOT + BORDER) as usize * 4;
        data.extend_from_slice(&tile[row..row + rw as usize * 4]);
    }
    let mut image = Image::new(rw, rh, data, provider.srgb());
    image.keep_pixels = true;
    image.generate_mips();
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_wrap_their_borders_around_the_image() {
        // 256x256 image whose texel encodes its own coordinates.
        let img = Image::from_fn(256, 256, false, |x, y| [x as u8, y as u8, 0, 255]);
        let p = ImageProvider::new(img, false).unwrap();
        let mut out = vec![0u8; TILE_BYTES];
        p.read_tile(0, 0, 0, &mut out).unwrap();
        let px = |out: &[u8], i: u32, j: u32| {
            let o = ((j * SLOT + i) * 4) as usize;
            [out[o], out[o + 1]]
        };
        assert_eq!(px(&out, BORDER, BORDER), [0, 0]);
        assert_eq!(px(&out, 0, 0), [248, 248], "wrapped from the far corner");
        assert_eq!(px(&out, SLOT - 1, BORDER), [135, 0]);
        p.read_tile(0, 1, 1, &mut out).unwrap();
        assert_eq!(px(&out, SLOT - 1, SLOT - 1), [7, 7], "wrapped past the end");
        // Level 1 comes from the generated mips: 128x128, so one tile, the root.
        p.read_tile(1, 0, 0, &mut out).unwrap();
        // Box filtered: (0 + 1) / 2 and (2 + 3) / 2, rounded.
        assert_eq!(px(&out, BORDER, BORDER)[0], 1);
        assert_eq!(px(&out, BORDER + 1, BORDER)[0], 3);
    }

    #[test]
    fn tile_levels_halve_down() {
        let tile = vec![200u8; TILE_BYTES];
        let levels = tile_levels(&tile, true, 4);
        let s = SLOT as usize;
        assert_eq!(
            levels.len(),
            (s * s + s * s / 4 + s * s / 16 + s * s / 64) * 4
        );
        assert!(levels.iter().all(|&b| b == 200));
    }
}
