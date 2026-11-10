//! Disk cache of block-compressed material images, so an image is compressed once per machine
//! instead of once per run (the texture part of what Unreal calls its derived data cache and
//! Unity its Library folder).
//!
//! Without it every run uploads every material image as RGBA8 and compresses it on the GPU a
//! little each frame (`crate::image_cache`), which takes minutes for a large world. Until then
//! the GPU holds four bytes a texel instead of one, and the allocator keeps the freed RGBA8
//! space reserved afterwards, so the driver only ever sees the peak. With the cache, an image
//! whose blocks are on disk is uploaded compressed from the start: its RGBA8 copy never reaches
//! the GPU.
//!
//! A file is keyed by a 128-bit hash of everything its blocks are made from: the pixels (and
//! stored mips), the size and level count, the color space the mips were filtered in, the
//! block format and [`CACHE_VERSION`]. It holds the blocks of every level exactly as the GPU
//! encoders wrote them, behind a header and a checksum; a file that fails to read is removed and
//! the image compressed again. Files are written by a thread of their own, to a temporary name
//! first, so nothing waits for the disk and readers never see half a file.
//!
//! Location: `ENGINE_TEXTURE_CACHE` (a folder, or `off`), else the user cache folder
//! (`%LOCALAPPDATA%`, `~/Library/Caches`, `$XDG_CACHE_HOME` or `~/.cache`) under
//! `engine/textures`, beside the cluster cache. When the folder grows past its budget the least
//! recently used files are removed at the next start. Under a software GPU (lavapipe) the cache
//! is used only when the variable names a folder, so tests and headless tools stay independent
//! of earlier runs.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Sender;

use crate::virtual_geometry::StableHasher;

/// Changes whenever the same image would give different blocks (another encoder profile, mip
/// filter or file layout), so old files are never used for new uploads.
pub(crate) const CACHE_VERSION: u32 = 2;

const MAGIC: [u8; 8] = *b"ENGTXC\0\x01";
const HEADER_BYTES: usize = 40;
const EXTENSION: &str = "txc";

/// Least recently used files go once the folder holds more than this (4 GiB: the showcase's
/// whole texture set is under 1 GiB compressed).
const MAX_BYTES: u64 = 4 << 30;

/// What a file's blocks are: checked against the texture they are loaded for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockShape {
    /// The block format (`wgpu::TextureFormat` without its sRGB suffix), as a small number.
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
}

/// Hash `bytes` in eight lanes (so the multiplies of neighbouring words overlap in the CPU):
/// the frame's 16 MB of new images take about a millisecond.
pub(crate) fn hash_pixels(h: &mut StableHasher, bytes: &[u8]) {
    // Eight lanes of one multiply and one turn a word: the lanes don't wait for each other,
    // so a 64 MB image takes a few milliseconds, where a single chain took thirty. The turn
    // carries high bits back down (a multiply alone only moves differences up), and every
    // lane goes through the hasher's full mix at the end.
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut lanes: [u64; 8] = [
        0x9ae1_6a3b_2f90_404f,
        0xc3a5_c85c_97cb_3127,
        0xb492_b66f_be98_f273,
        0x1656_67b1_9e37_79f9,
        0x2545_f491_4f6c_dd1d,
        0xd6e8_feb8_6659_fd93,
        0xa076_1d64_78bd_642f,
        0xe703_7ed1_a0b4_28db,
    ];
    let (blocks, rest) = bytes.as_chunks::<64>();
    for block in blocks {
        let (words, _) = block.as_chunks::<8>();
        for (lane, word) in lanes.iter_mut().zip(words) {
            *lane = (*lane ^ u64::from_le_bytes(*word))
                .wrapping_mul(K)
                .rotate_left(27);
        }
    }
    h.write_u64(bytes.len() as u64);
    for lane in lanes {
        h.write_u64(lane);
    }
    h.write_bytes(rest);
}

/// The default folder (see the module docs); `None` when the cache is switched off or no cache
/// folder is known. `explicit_only`: only a folder named by `ENGINE_TEXTURE_CACHE` counts.
pub(crate) fn default_dir(explicit_only: bool) -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("ENGINE_TEXTURE_CACHE") {
        let off = dir
            .to_str()
            .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "off" | "0" | "false"));
        return (!off).then_some(dir);
    }
    if explicit_only {
        return None;
    }
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("engine").join("textures"))
}

/// A file for the writer thread: its final path and its bytes.
type NewFile = (PathBuf, Vec<u8>);

/// A folder of compressed images.
pub(crate) struct BlockCache {
    dir: PathBuf,
    /// Keys with a file (so a miss costs no system call), and keys being written.
    known: Mutex<HashSet<(u64, u64)>>,
    /// To the writer thread.
    writer: Mutex<Option<Sender<NewFile>>>,
}

impl BlockCache {
    /// Open `dir` (made when the first file is stored), listing what it holds and trimming it
    /// to `max_bytes`.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self::open_with_budget(dir, MAX_BYTES)
    }

    pub fn open_with_budget(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        let dir = dir.into();
        let mut files = cache_files(&dir);
        let mut total: u64 = files.iter().map(|f| f.2).sum();
        if total > max_bytes {
            // Oldest first.
            files.sort_by_key(|f| f.3);
            files.retain(|(path, _, len, _)| {
                if total > max_bytes && std::fs::remove_file(path).is_ok() {
                    total -= len;
                    false
                } else {
                    true
                }
            });
        }
        Self {
            known: Mutex::new(files.iter().map(|f| f.1).collect()),
            dir,
            writer: Mutex::new(None),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Files known to be in the folder.
    pub fn len(&self) -> usize {
        self.known.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn path(&self, key: (u64, u64)) -> PathBuf {
        self.dir
            .join(format!("{:016x}{:016x}.{EXTENSION}", key.0, key.1))
    }

    /// The blocks stored under `key`, when there is a readable file of exactly `shape` and
    /// `bytes` of blocks.
    pub fn load(&self, key: (u64, u64), shape: BlockShape, bytes: u64) -> Option<Vec<u8>> {
        if !self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&key)
        {
            return None;
        }
        let path = self.path(key);
        let mut file = Vec::new();
        let read = std::fs::File::open(&path).and_then(|mut f| f.read_to_end(&mut file));
        let blocks = read.ok().and_then(|_| decode(file, shape, bytes));
        if blocks.is_some() {
            // Mark it used: the least recently used files are the ones trimmed.
            if let Ok(f) = std::fs::File::options().append(true).open(&path) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
        } else {
            // Half written by a run that was killed, from another version, or damaged: the
            // image is compressed again and stored anew.
            tracing::debug!("ignoring unreadable texture cache file {}", path.display());
            let _ = std::fs::remove_file(&path);
            self.known
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
        }
        blocks
    }

    /// Store `blocks` under `key`, on the writer thread.
    pub fn store(&self, key: (u64, u64), shape: BlockShape, blocks: &[u8]) {
        if !self
            .known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
        {
            return;
        }
        let file = encode(shape, blocks);
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if writer.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<NewFile>();
            let spawned = std::thread::Builder::new()
                .name("texture cache".into())
                .spawn(move || {
                    for (path, bytes) in rx {
                        if let Err(e) = write_file(&path, &bytes) {
                            tracing::debug!("texture cache: {}: {e}", path.display());
                        }
                    }
                });
            if spawned.is_ok() {
                *writer = Some(tx);
            }
        }
        if let Some(tx) = writer.as_ref() {
            let _ = tx.send((self.path(key), file));
        }
    }

    /// Wait until every stored file is on disk (tests; the writer thread ends with the cache).
    #[cfg(test)]
    fn flush(&self) {
        let sender = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(sender);
        // The thread ends once its queue is drained; files appear under their final names.
        for _ in 0..2000 {
            let waiting = std::fs::read_dir(&self.dir).map_or(0, |d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION))
                    .count()
            });
            if waiting >= self.len() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// Write to a temporary name, then rename: readers never see half a file.
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::File::create(&tmp)?.write_all(bytes)?;
    std::fs::rename(&tmp, path)
}

fn checksum(blocks: &[u8]) -> u64 {
    let mut h = StableHasher::new(0x7e87_c0de);
    hash_pixels(&mut h, blocks);
    h.finish()
}

fn encode(shape: BlockShape, blocks: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + blocks.len());
    out.extend_from_slice(&MAGIC);
    for word in [
        CACHE_VERSION,
        shape.format,
        shape.width,
        shape.height,
        shape.levels,
        0,
    ] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(&checksum(blocks).to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_BYTES);
    out.extend_from_slice(blocks);
    out
}

/// The blocks of `file` when it is a whole file of `shape` with `bytes` of blocks.
fn decode(mut file: Vec<u8>, shape: BlockShape, bytes: u64) -> Option<Vec<u8>> {
    if file.len() as u64 != HEADER_BYTES as u64 + bytes || file[..8] != MAGIC {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes(file[8 + i * 4..12 + i * 4].try_into().unwrap());
    let header = [word(0), word(1), word(2), word(3), word(4)];
    let expected = [
        CACHE_VERSION,
        shape.format,
        shape.width,
        shape.height,
        shape.levels,
    ];
    let sum = u64::from_le_bytes(file[32..40].try_into().unwrap());
    if header != expected || sum != checksum(&file[HEADER_BYTES..]) {
        return None;
    }
    file.drain(..HEADER_BYTES);
    Some(file)
}

/// (path, key, bytes, modified) of every cache file in `dir`.
fn cache_files(dir: &Path) -> Vec<(PathBuf, (u64, u64), u64, std::time::SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != EXTENSION {
                return None;
            }
            let stem = path.file_stem()?.to_str()?;
            if stem.len() != 32 {
                return None;
            }
            let key = (
                u64::from_str_radix(&stem[..16], 16).ok()?,
                u64::from_str_radix(&stem[16..], 16).ok()?,
            );
            let m = e.metadata().ok()?;
            Some((
                path,
                key,
                m.len(),
                m.modified().unwrap_or(std::time::UNIX_EPOCH),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "engine-block-cache-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    const SHAPE: BlockShape = BlockShape {
        format: 7,
        width: 64,
        height: 32,
        levels: 3,
    };

    #[test]
    fn stored_blocks_come_back_the_same_in_the_next_run() {
        let dir = temp_dir("roundtrip");
        let blocks: Vec<u8> = (0..4096u32).map(|i| (i * 31 % 251) as u8).collect();
        {
            let cache = BlockCache::open(&dir);
            assert_eq!(cache.load((1, 2), SHAPE, 4096), None);
            cache.store((1, 2), SHAPE, &blocks);
            cache.flush();
        }
        // A new run lists the folder.
        let cache = BlockCache::open(&dir);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.load((1, 2), SHAPE, 4096), Some(blocks));
        assert_eq!(cache.load((1, 3), SHAPE, 4096), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_of_another_shape_or_with_damage_is_dropped() {
        let dir = temp_dir("damage");
        let blocks = vec![9u8; 512];
        let cache = BlockCache::open(&dir);
        cache.store((5, 5), SHAPE, &blocks);
        cache.store((6, 6), SHAPE, &blocks);
        cache.flush();
        // Asked for as another size: not this image's blocks.
        let other = BlockShape {
            width: 128,
            ..SHAPE
        };
        assert_eq!(cache.load((5, 5), other, 512), None);
        assert!(!cache.path((5, 5)).exists(), "the bad file is removed");
        // A flipped byte fails the checksum.
        let path = cache.path((6, 6));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[HEADER_BYTES + 100] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(cache.load((6, 6), SHAPE, 512), None);
        assert_eq!(cache.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_oldest_files_go_when_the_folder_is_over_its_budget() {
        let dir = temp_dir("budget");
        let cache = BlockCache::open(&dir);
        for k in 0..4u64 {
            cache.store((k, 0), SHAPE, &vec![k as u8; 1000]);
        }
        cache.flush();
        // Make key 0 the oldest and key 3 the newest whatever the clock's resolution.
        for k in 0..4u64 {
            let f = std::fs::File::options()
                .append(true)
                .open(cache.path((k, 0)))
                .unwrap();
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + k);
            f.set_modified(t).unwrap();
        }
        let file = (HEADER_BYTES + 1000) as u64;
        let cache = BlockCache::open_with_budget(&dir, file * 2 + 10);
        assert_eq!(cache.len(), 2);
        assert!(cache.load((0, 0), SHAPE, 1000).is_none());
        assert!(cache.load((3, 0), SHAPE, 1000).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pixel_hash_tells_images_apart() {
        let key = |bytes: &[u8]| {
            let mut h = StableHasher::new(1);
            hash_pixels(&mut h, bytes);
            h.finish128()
        };
        let a: Vec<u8> = (0..10_007u32).map(|i| (i % 256) as u8).collect();
        let mut b = a.clone();
        assert_eq!(key(&a), key(&b));
        // One changed byte anywhere: in a lane, and in the tail past the last whole block.
        for at in [0, 5000, 10_006] {
            b[at] ^= 0x10;
            assert_ne!(key(&a), key(&b), "byte {at}");
            b[at] ^= 0x10;
        }
        // Two bytes swapped between lanes.
        b.swap(64, 72);
        assert_ne!(key(&a), key(&b));
        assert_ne!(key(&a), key(&a[..10_006]));
    }
}
