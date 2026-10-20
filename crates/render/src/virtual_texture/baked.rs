//! The baked tile file (`.vtex`): a large image cut into tiles once, so it can be streamed tile by
//! tile without ever sitting whole in memory or on the GPU again.
//!
//! Baking is deterministic: the same source bytes always give the same file. It decodes the
//! source (PNG or JPEG, any size up to 131072 texels per side whose levels halve evenly), cuts
//! every level down to the one that fits a single tile into [`SLOT`]² tiles (the 128² tile and an
//! 8-texel border of its neighbors, wrapped at the image's edges), and compresses each tile on
//! its own: as JPEG (quality 95, no chroma subsampling) when the source was a JPEG without
//! alpha, so a lossy source isn't stored losslessly at several times its size, and as PNG
//! (lossless) otherwise. Coarser levels are box-filtered from the finer one, in linear light for
//! sRGB color, like `Image::generate_mips`.
//!
//! Layout (little-endian):
//!
//! | Bytes | Field |
//! |---|---|
//! | 0..4 | magic `EVTX` |
//! | 4..8 | version (1) |
//! | 8..16 | width, height (level 0 texels) |
//! | 16..20 | root level (levels 0..=root are stored; the root holds the whole image in one tile) |
//! | 20..28 | tile size (128), border (8) |
//! | 28..32 | flags: 1 = sRGB color, 2 = has alpha |
//! | 32..40 | tile codec (0 PNG, 1 JPEG), JPEG quality |
//! | 40..48 | source file length |
//! | 48..80 | source SHA-256 (a cache made from other bytes is baked again) |
//! | 80..84 | tile count |
//! | 84..88 | mean color, RGBA8 as stored |
//! | 88..128 | reserved (0) |
//! | 128.. | index: per tile, finest level first, rows top to bottom: offset u64, length u32 |
//! | .. | tile payloads |
//!
//! The mip tail (levels coarser than the root) is not stored: the root tile's slot in the
//! cache has its own mip levels, made when it is loaded, and the root stays resident.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::provider::{TILE_BYTES, TileProvider, cut_tile};
use super::residency::{BORDER, Layout, SLOT, TILE};
use crate::texture::downsample_rgba8;

const MAGIC: &[u8; 4] = b"EVTX";
const VERSION: u32 = 1;
const HEADER_BYTES: u64 = 128;
const INDEX_ENTRY: u64 = 12;
const JPEG_QUALITY: u8 = 95;

/// How the tiles of a baked file are compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileCodec {
    /// Lossless.
    Png,
    /// Lossy, at this quality (no chroma subsampling, no alpha).
    Jpeg(u8),
}

/// What a bake made.
#[derive(Debug, Clone, PartialEq)]
pub struct BakeReport {
    pub width: u32,
    pub height: u32,
    /// Levels stored (the coarsest holds the whole image in one tile).
    pub levels: u32,
    pub tiles: u32,
    pub codec: TileCodec,
    /// Size of the baked file.
    pub bytes: u64,
    pub seconds: f32,
}

/// The fixed part of a baked file.
#[derive(Debug, Clone, PartialEq)]
struct Header {
    width: u32,
    height: u32,
    root: u32,
    srgb: bool,
    alpha: bool,
    codec: TileCodec,
    source_len: u64,
    source_sha: [u8; 32],
    tiles: u32,
    mean: [u8; 4],
}

impl Header {
    fn encode(&self) -> [u8; HEADER_BYTES as usize] {
        let mut b = [0u8; HEADER_BYTES as usize];
        let mut put = |at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
        put(0, MAGIC);
        put(4, &VERSION.to_le_bytes());
        put(8, &self.width.to_le_bytes());
        put(12, &self.height.to_le_bytes());
        put(16, &self.root.to_le_bytes());
        put(20, &TILE.to_le_bytes());
        put(24, &BORDER.to_le_bytes());
        put(
            28,
            &(self.srgb as u32 | (self.alpha as u32) << 1).to_le_bytes(),
        );
        let (codec, quality) = match self.codec {
            TileCodec::Png => (0u32, 0u32),
            TileCodec::Jpeg(q) => (1, q as u32),
        };
        put(32, &codec.to_le_bytes());
        put(36, &quality.to_le_bytes());
        put(40, &self.source_len.to_le_bytes());
        put(48, &self.source_sha);
        put(80, &self.tiles.to_le_bytes());
        put(84, &self.mean);
        b
    }

    fn decode(b: &[u8]) -> Result<Self, String> {
        if b.len() < HEADER_BYTES as usize || &b[0..4] != MAGIC {
            return Err("not a baked virtual texture".into());
        }
        let u = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]));
        if u(4) != VERSION {
            return Err(format!(
                "baked with format version {}, expected {VERSION}",
                u(4)
            ));
        }
        if u(20) != TILE || u(24) != BORDER {
            return Err("baked with another tile size".into());
        }
        let codec = match u(32) {
            0 => TileCodec::Png,
            1 => TileCodec::Jpeg(u(36) as u8),
            c => return Err(format!("unknown tile codec {c}")),
        };
        let mut source_sha = [0u8; 32];
        source_sha.copy_from_slice(&b[48..80]);
        let mut mean = [0u8; 4];
        mean.copy_from_slice(&b[84..88]);
        Ok(Self {
            width: u(8),
            height: u(12),
            root: u(16),
            srgb: u(28) & 1 != 0,
            alpha: u(28) & 2 != 0,
            codec,
            source_len: u64::from_le_bytes(b[40..48].try_into().unwrap_or([0; 8])),
            source_sha,
            tiles: u(80),
            mean,
        })
    }
}

/// SHA-256 of a file's bytes.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// Where the baked copy of `source` is kept: next to it (`<file>.vtex`).
pub fn baked_path(source: &Path) -> PathBuf {
    let mut name = source.file_name().unwrap_or_default().to_os_string();
    name.push(".vtex");
    source.with_file_name(name)
}

/// Bake `source` (PNG or JPEG) into the tile file `out`. `srgb`: the image is sRGB color
/// (levels averaged in linear light). Writes a temporary file and renames it, so a reader never
/// sees half a file.
pub fn bake(source: &Path, out: &Path, srgb: bool) -> Result<BakeReport, String> {
    let started = Instant::now();
    let bytes =
        std::fs::read(source).map_err(|e| format!("could not read {}: {e}", source.display()))?;
    let source_sha = sha256(&bytes);
    let source_len = bytes.len() as u64;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let jpeg_source = reader.format() == Some(image::ImageFormat::Jpeg);
    reader.no_limits();
    let decoded = reader
        .decode()
        .map_err(|e| format!("could not decode {}: {e}", source.display()))?;
    drop(bytes);
    let (width, height) = (decoded.width(), decoded.height());
    let layout = Layout::new(width, height)?;
    let mut level = decoded.into_rgba8().into_raw();
    let alpha = level.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
    let codec = if jpeg_source && !alpha {
        TileCodec::Jpeg(JPEG_QUALITY)
    } else {
        TileCodec::Png
    };
    let header_for = |mean: [u8; 4]| Header {
        width,
        height,
        root: layout.root,
        srgb,
        alpha,
        codec,
        source_len,
        source_sha,
        tiles: layout.entries,
        mean,
    };
    let tmp = out.with_extension("vtex.part");
    let file = File::create(&tmp).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    let mut w = BufWriter::with_capacity(1 << 20, file);
    let index_bytes = layout.entries as u64 * INDEX_ENTRY;
    w.write_all(&header_for([0; 4]).encode())
        .map_err(|e| e.to_string())?;
    w.write_all(&vec![0u8; index_bytes as usize])
        .map_err(|e| e.to_string())?;
    let mut offset = HEADER_BYTES + index_bytes;
    let mut index: Vec<(u64, u32)> = Vec::with_capacity(layout.entries as usize);
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 8);
    let mut mean = [0u8; 4];
    for m in 0..=layout.root {
        let (lw, lh) = layout.level_size(m);
        let (px, py) = layout.pages(m);
        // Encode a band of tile rows at a time on every thread, then write them in order.
        let band = (threads as u32).max(1);
        let mut row = 0;
        while row < py {
            let rows = band.min(py - row);
            let encoded: Vec<Result<Vec<Vec<u8>>, String>> = std::thread::scope(|s| {
                let handles: Vec<_> = (row..row + rows)
                    .map(|y| {
                        let level = &level;
                        s.spawn(move || {
                            let mut tile = vec![0u8; TILE_BYTES];
                            (0..px)
                                .map(|x| {
                                    cut_tile(level, lw, lh, x, y, &mut tile);
                                    encode_tile(&tile, codec)
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .unwrap_or_else(|_| Err("tile encoder panicked".into()))
                    })
                    .collect()
            });
            for tiles in encoded {
                for t in tiles? {
                    w.write_all(&t).map_err(|e| e.to_string())?;
                    index.push((offset, t.len() as u32));
                    offset += t.len() as u64;
                }
            }
            row += rows;
        }
        if m == layout.root {
            mean = mean_color(&level, srgb);
        } else {
            level = downsample_rgba8(&level, lw, lh, lw / 2, lh / 2, srgb);
        }
    }
    drop(level);
    let mut file = w.into_inner().map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    file.write_all(&header_for(mean).encode())
        .map_err(|e| e.to_string())?;
    let mut idx = Vec::with_capacity(index_bytes as usize);
    for (o, l) in &index {
        idx.extend_from_slice(&o.to_le_bytes());
        idx.extend_from_slice(&l.to_le_bytes());
    }
    file.write_all(&idx).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&tmp, out).map_err(|e| format!("could not write {}: {e}", out.display()))?;
    Ok(BakeReport {
        width,
        height,
        levels: layout.root + 1,
        tiles: layout.entries,
        codec,
        bytes: offset,
        seconds: started.elapsed().as_secs_f32(),
    })
}

/// Mean of an RGBA8 level (in linear light for sRGB color, stored back as sRGB).
fn mean_color(data: &[u8], srgb: bool) -> [u8; 4] {
    let mut sum = [0.0f64; 4];
    let n = (data.len() / 4).max(1) as f64;
    for p in data.as_chunks::<4>().0 {
        for c in 0..4 {
            let v = p[c] as f64 / 255.0;
            sum[c] += if srgb && c < 3 { srgb_to_linear(v) } else { v };
        }
    }
    let mut out = [0u8; 4];
    for c in 0..4 {
        let v = sum[c] / n;
        let v = if srgb && c < 3 { linear_to_srgb(v) } else { v };
        out[c] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    }
    out
}

fn srgb_to_linear(s: f64) -> f64 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f64) -> f64 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

fn encode_tile(tile: &[u8], codec: TileCodec) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match codec {
        TileCodec::Png => {
            use image::ImageEncoder;
            use image::codecs::png::{CompressionType, FilterType, PngEncoder};
            PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
                .write_image(tile, SLOT, SLOT, image::ExtendedColorType::Rgba8)
                .map_err(|e| e.to_string())?;
        }
        TileCodec::Jpeg(quality) => {
            let rgb: Vec<u8> = tile
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .encode(&rgb, SLOT, SLOT, image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

fn decode_tile(bytes: &[u8], codec: TileCodec, out: &mut [u8]) -> Result<(), String> {
    let format = match codec {
        TileCodec::Png => image::ImageFormat::Png,
        TileCodec::Jpeg(_) => image::ImageFormat::Jpeg,
    };
    let img =
        image::load_from_memory_with_format(bytes, format).map_err(|e| format!("bad tile: {e}"))?;
    if img.width() != SLOT || img.height() != SLOT {
        return Err("bad tile size".into());
    }
    match img {
        image::DynamicImage::ImageRgba8(rgba) => out.copy_from_slice(rgba.as_raw()),
        image::DynamicImage::ImageRgb8(rgb) => {
            for (o, p) in out
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(rgb.as_raw().as_chunks::<3>().0)
            {
                *o = [p[0], p[1], p[2], 255];
            }
        }
        other => out.copy_from_slice(other.to_rgba8().as_raw()),
    }
    Ok(())
}

/// A baked file opened for streaming: its header and index are in memory, tiles are read and
/// decoded on demand (from any thread).
pub(crate) struct BakedProvider {
    file: File,
    header: Header,
    index: Vec<(u64, u32)>,
    layout: Layout,
}

impl BakedProvider {
    /// Open `path`. With `source`, the file must have been baked from exactly that source
    /// (length and SHA-256), or this fails (bake it again).
    pub fn open(path: &Path, source: Option<(u64, [u8; 32])>) -> Result<Self, String> {
        let mut file =
            File::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
        let mut head = [0u8; HEADER_BYTES as usize];
        file.read_exact(&mut head).map_err(|e| e.to_string())?;
        let header = Header::decode(&head)?;
        if let Some((len, sha)) = source
            && (header.source_len != len || header.source_sha != sha)
        {
            return Err("baked from a different source".into());
        }
        let layout = Layout::new(header.width, header.height)?;
        if layout.root != header.root || layout.entries != header.tiles {
            return Err("tile index doesn't match the size".into());
        }
        let mut idx = vec![0u8; (header.tiles as u64 * INDEX_ENTRY) as usize];
        file.read_exact(&mut idx).map_err(|e| e.to_string())?;
        let index = idx
            .as_chunks::<{ INDEX_ENTRY as usize }>()
            .0
            .iter()
            .map(|c| {
                (
                    u64::from_le_bytes(c[0..8].try_into().unwrap_or([0; 8])),
                    u32::from_le_bytes(c[8..12].try_into().unwrap_or([0; 4])),
                )
            })
            .collect();
        Ok(Self {
            file,
            header,
            index,
            layout,
        })
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(buf, offset)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut done = 0;
            while done < buf.len() {
                let n = self
                    .file
                    .seek_read(&mut buf[done..], offset + done as u64)?;
                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                done += n;
            }
            Ok(())
        }
    }
}

impl TileProvider for BakedProvider {
    fn size(&self) -> (u32, u32) {
        (self.header.width, self.header.height)
    }

    fn srgb(&self) -> bool {
        self.header.srgb
    }

    fn read_tile(&self, level: u32, x: u32, y: u32, out: &mut [u8]) -> Result<(), String> {
        if !self.layout.contains(level, x, y) {
            return Err("no such tile".into());
        }
        let (offset, len) = self.index[self.layout.index(level, x, y)];
        let mut buf = vec![0u8; len as usize];
        self.read_at(offset, &mut buf).map_err(|e| e.to_string())?;
        decode_tile(&buf, self.header.codec, out)
    }
}

/// Open the baked copy of `source`, baking it first when it is missing or was made from other
/// bytes. Returns the provider and, when it baked, the report.
pub(crate) fn open_or_bake(
    source: &Path,
    srgb: bool,
) -> Result<(BakedProvider, Option<BakeReport>), String> {
    let path = baked_path(source);
    // Fast path: a bake written after its source was last changed, of the source's length, is
    // trusted without hashing the source (hundreds of MB for a 16k scan, half a minute from a
    // cold disk). Otherwise the content hash decides.
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified());
    if let (Ok(source_time), Ok(bake_time)) = (modified(source), modified(&path))
        && bake_time > source_time
        && let Ok(p) = BakedProvider::open(&path, None)
        && p.header.srgb == srgb
        && std::fs::metadata(source).is_ok_and(|m| m.len() == p.header.source_len)
    {
        return Ok((p, None));
    }
    let bytes =
        std::fs::read(source).map_err(|e| format!("could not read {}: {e}", source.display()))?;
    let id = (bytes.len() as u64, sha256(&bytes));
    drop(bytes);
    if let Ok(p) = BakedProvider::open(&path, Some(id))
        && p.header.srgb == srgb
    {
        // Same bytes, newer file time (a fresh checkout or copy): mark the bake current so
        // the next open takes the fast path.
        let _ = File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Ok((p, None));
    }
    // One bake at a time: a 16k bake holds the decoded image (1 GiB) and uses every core.
    static BAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = BAKING.lock().unwrap_or_else(|e| e.into_inner());
    let report = bake(source, &path, srgb)?;
    let provider = BakedProvider::open(&path, Some(id))?;
    Ok((provider, Some(report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 7 + y) as u8, (y * 3) as u8, (x ^ y) as u8, 255])
        })
    }

    #[test]
    fn baking_is_deterministic_and_tiles_round_trip() {
        let dir = std::env::temp_dir().join(format!("engine-vt-bake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("pattern.png");
        pattern(512, 256).save(&src).unwrap();
        let a = dir.join("a.vtex");
        let b = dir.join("b.vtex");
        let report = bake(&src, &a, false).unwrap();
        bake(&src, &b, false).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert_eq!(report.codec, TileCodec::Png);
        assert_eq!(report.levels, 3);
        assert_eq!(report.tiles, 4 * 2 + 2 + 1);

        let p = BakedProvider::open(&a, None).unwrap();
        assert_eq!(p.size(), (512, 256));
        let mut tile = vec![0u8; TILE_BYTES];
        p.read_tile(0, 1, 1, &mut tile).unwrap();
        let img = pattern(512, 256);
        let mut expect = vec![0u8; TILE_BYTES];
        cut_tile(img.as_raw(), 512, 256, 1, 1, &mut expect);
        assert_eq!(tile, expect, "lossless tiles");
        assert!(p.read_tile(3, 0, 0, &mut tile).is_err());

        // A changed source makes the cache stale.
        let (fresh, baked) = open_or_bake(&src, false).unwrap();
        assert!(baked.is_some());
        let (_, again) = open_or_bake(&src, false).unwrap();
        assert!(again.is_none(), "the second open reuses the bake");
        // A source touched but not changed (a fresh checkout) is hashed, not baked again, and
        // the bake is marked current.
        let mtime = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
        File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
        assert!(mtime(&baked_path(&src)) <= mtime(&src));
        let (_, touched) = open_or_bake(&src, false).unwrap();
        assert!(touched.is_none(), "same bytes: no new bake");
        assert!(mtime(&baked_path(&src)) > mtime(&src));
        drop(fresh);
        pattern(256, 256).save(&src).unwrap();
        let (p, rebaked) = open_or_bake(&src, false).unwrap();
        assert!(rebaked.is_some());
        assert_eq!(p.size(), (256, 256));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
