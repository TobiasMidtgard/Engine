//! Disk cache of built cluster hierarchies, so dense scenes don't rebuild on every start.
//!
//! A build is keyed by a 128-bit hash of everything it is made from: the mesh's vertices and
//! indices, the [`Displacement`](super::Displacement) settings and its height (the height map's
//! texels, or the procedural source's parameters), and [`BUILDER_VERSION`]. The file holds the
//! [`ClusterDag`] exactly as built (lossless: byte-shuffled and deflated), with a checksum, so a
//! cached load draws the very same triangles as a fresh build. Files that fail to read are
//! ignored and rebuilt.
//!
//! Location: `ENGINE_VG_CACHE`, else the user cache folder (`%LOCALAPPDATA%`,
//! `~/Library/Caches`, `$XDG_CACHE_HOME` or `~/.cache`) under `engine/virtual-geometry`, the way
//! `tools/fetch-assets` places its download cache. When the folder grows past its budget the
//! least recently used files are removed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use engine_core::math::Vec3;

use super::{Cluster, ClusterDag, ClusterLevel, Sphere, StableHasher};
use crate::mesh::{Mesh, Vertex};

/// Changes whenever the builder (clustering, simplification, tessellation) makes different
/// output for the same input, so old cache files are never used for new builds.
pub const BUILDER_VERSION: u32 = 1;

const MAGIC: [u8; 8] = *b"ENGVGC\0\x01";

/// Default cache folder (see the module docs). None when no home or cache folder is known.
pub fn default_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("ENGINE_VG_CACHE") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("engine").join("virtual-geometry"))
}

/// The cache key of a build: the mesh's data, `extra` (displacement settings and height
/// source; 0 for a plain mesh) and the builder version.
pub fn build_key(mesh: &Mesh, extra: u64) -> (u64, u64) {
    let mut h = StableHasher::new(0xc1a5_7e85 ^ BUILDER_VERSION as u64);
    h.write_u64(mesh.vertices.len() as u64);
    h.write_bytes(bytemuck::cast_slice(&mesh.vertices));
    h.write_u64(mesh.indices.len() as u64);
    h.write_bytes(bytemuck::cast_slice(&mesh.indices));
    h.write_u64(extra);
    h.finish128()
}

/// A folder of cached builds with a size budget.
#[derive(Debug, Clone)]
pub struct BuildCache {
    pub dir: PathBuf,
    /// Least recently used files are removed past this many bytes.
    pub max_bytes: u64,
}

impl BuildCache {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self {
            dir: dir.into(),
            max_bytes,
        }
    }

    fn path(&self, key: (u64, u64)) -> PathBuf {
        self.dir.join(format!("{:016x}{:016x}.vgc", key.0, key.1))
    }

    /// The cached build for `key`, if there is a readable one.
    pub fn load(&self, key: (u64, u64)) -> Option<ClusterDag> {
        let path = self.path(key);
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        match decode(&bytes) {
            Some(dag) => {
                // Mark it used (least recently used eviction goes by modification time).
                if let Ok(f) = std::fs::File::options().append(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                Some(dag)
            }
            None => {
                tracing::warn!("ignoring unreadable cluster cache file {}", path.display());
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Store a build under `key` (written to a temporary file, then renamed, so readers never
    /// see half a file). Returns the file's size.
    pub fn store(&self, key: (u64, u64), dag: &ClusterDag) -> std::io::Result<u64> {
        std::fs::create_dir_all(&self.dir)?;
        let bytes = encode(dag);
        let path = self.path(key);
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        self.evict(&path);
        Ok(bytes.len() as u64)
    }

    /// Total size of the cache files, bytes.
    pub fn size(&self) -> u64 {
        cache_files(&self.dir).iter().map(|f| f.1).sum()
    }

    /// Remove the least recently used files until the folder fits its budget (never `keep`).
    fn evict(&self, keep: &Path) {
        let mut files = cache_files(&self.dir);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total <= self.max_bytes {
            return;
        }
        files.sort_by_key(|f| f.2);
        for (path, len, _) in files {
            if total <= self.max_bytes {
                break;
            }
            if path != keep && std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }
}

/// (path, bytes, modified) of every cache file in `dir`.
fn cache_files(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "vgc"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((
                e.path(),
                m.len(),
                m.modified().unwrap_or(std::time::UNIX_EPOCH),
            ))
        })
        .collect()
}

/// Words per cluster in the file.
const CLUSTER_WORDS: usize = 22;

fn sphere_words(s: Sphere) -> [u32; 4] {
    s.to_array().map(f32::to_bits)
}

fn sphere_of(w: &[u32]) -> Sphere {
    Sphere {
        center: Vec3::new(
            f32::from_bits(w[0]),
            f32::from_bits(w[1]),
            f32::from_bits(w[2]),
        ),
        radius: f32::from_bits(w[3]),
    }
}

/// Byte planes: every `stride`-th byte together, which deflates far better for floats.
fn shuffle(data: &[u8], stride: usize) -> Vec<u8> {
    let n = data.len() / stride;
    let mut out = vec![0u8; data.len()];
    for (i, chunk) in data.chunks_exact(stride).enumerate() {
        for (b, &x) in chunk.iter().enumerate() {
            out[b * n + i] = x;
        }
    }
    out
}

fn unshuffle(data: &[u8], stride: usize) -> Vec<u8> {
    let n = data.len() / stride;
    let mut out = vec![0u8; data.len()];
    for i in 0..n {
        for b in 0..stride {
            out[i * stride + b] = data[b * n + i];
        }
    }
    out
}

fn encode(dag: &ClusterDag) -> Vec<u8> {
    let mut words: Vec<u32> = Vec::with_capacity(
        dag.vertices.len() * 12 + dag.indices.len() + dag.clusters.len() * CLUSTER_WORDS,
    );
    words.extend_from_slice(bytemuck::cast_slice(&dag.vertices));
    words.extend_from_slice(&dag.indices);
    for c in &dag.clusters {
        words.extend([c.index_offset, c.vertex_count, c.triangle_count, c.level]);
        words.extend(sphere_words(c.bounds));
        words.extend(
            [
                c.cone_axis.x,
                c.cone_axis.y,
                c.cone_axis.z,
                c.cone_cos,
                c.error,
            ]
            .map(f32::to_bits),
        );
        words.extend(sphere_words(c.lod_bounds));
        words.push(c.parent_error.to_bits());
        words.extend(sphere_words(c.parent_bounds));
    }
    for l in &dag.levels {
        words.extend([
            l.first,
            l.count,
            l.min_error.to_bits(),
            l.max_parent_error.to_bits(),
        ]);
    }
    words.extend(sphere_words(dag.bounds));
    words.push(dag.source_triangles);
    let payload =
        miniz_oxide::deflate::compress_to_vec(&shuffle(bytemuck::cast_slice(&words), 4), 1);

    let mut out = Vec::with_capacity(payload.len() + 64);
    out.extend_from_slice(&MAGIC);
    for n in [
        BUILDER_VERSION,
        dag.vertices.len() as u32,
        dag.indices.len() as u32,
        dag.clusters.len() as u32,
        dag.levels.len() as u32,
        words.len() as u32,
    ] {
        out.extend_from_slice(&n.to_le_bytes());
    }
    let mut h = StableHasher::new(0xc0ffee);
    h.write_bytes(bytemuck::cast_slice(&words));
    out.extend_from_slice(&h.finish().to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

fn decode(bytes: &[u8]) -> Option<ClusterDag> {
    if bytes.len() < 40 || bytes[..8] != MAGIC {
        return None;
    }
    let header: Vec<u32> = bytes[8..32]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let [version, nv, ni, nc, nl, nw] = [0, 1, 2, 3, 4, 5].map(|i| header[i] as usize);
    if version != BUILDER_VERSION as usize || nw != nv * 12 + ni + nc * CLUSTER_WORDS + nl * 4 + 5 {
        return None;
    }
    let checksum = u64::from_le_bytes(bytes[32..40].try_into().ok()?);
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes[40..], nw * 4).ok()?;
    if raw.len() != nw * 4 {
        return None;
    }
    let raw = unshuffle(&raw, 4);
    let words: Vec<u32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let mut h = StableHasher::new(0xc0ffee);
    h.write_bytes(bytemuck::cast_slice(&words));
    if h.finish() != checksum {
        return None;
    }
    let (vertex_words, rest) = words.split_at(nv * 12);
    let (indices, rest) = rest.split_at(ni);
    let (cluster_words, rest) = rest.split_at(nc * CLUSTER_WORDS);
    let (level_words, rest) = rest.split_at(nl * 4);
    let vertices: Vec<Vertex> = bytemuck::cast_slice::<u32, Vertex>(vertex_words).to_vec();
    let f = f32::from_bits;
    let clusters = cluster_words
        .as_chunks::<CLUSTER_WORDS>()
        .0
        .iter()
        .map(|w| Cluster {
            index_offset: w[0],
            vertex_count: w[1],
            triangle_count: w[2],
            level: w[3],
            bounds: sphere_of(&w[4..8]),
            cone_axis: Vec3::new(f(w[8]), f(w[9]), f(w[10])),
            cone_cos: f(w[11]),
            error: f(w[12]),
            lod_bounds: sphere_of(&w[13..17]),
            parent_error: f(w[17]),
            parent_bounds: sphere_of(&w[18..22]),
        })
        .collect();
    let levels = level_words
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| ClusterLevel {
            first: w[0],
            count: w[1],
            min_error: f(w[2]),
            max_parent_error: f(w[3]),
        })
        .collect();
    Some(ClusterDag {
        vertices,
        indices: indices.to_vec(),
        clusters,
        levels,
        bounds: sphere_of(&rest[..4]),
        source_triangles: rest[4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::icosphere;

    fn dag() -> (Mesh, ClusterDag) {
        let mut m = icosphere(1.0, 4);
        for v in &mut m.vertices {
            let p = Vec3::from(v.position);
            v.position = (p * (1.0 + 0.1 * (p.x * 9.0).sin())).to_array();
        }
        m.compute_normals();
        let dag = super::super::build(&m);
        (m, dag)
    }

    fn same(a: &ClusterDag, b: &ClusterDag) {
        assert_eq!(a.vertices, b.vertices);
        assert_eq!(a.indices, b.indices);
        assert_eq!(a.clusters, b.clusters);
        assert_eq!(a.levels, b.levels);
        assert_eq!(a.bounds, b.bounds);
        assert_eq!(a.source_triangles, b.source_triangles);
    }

    #[test]
    fn builds_round_trip_exactly() {
        let (mesh, built) = dag();
        let dir = std::env::temp_dir().join(format!("engine-vgc-test-{}", std::process::id()));
        let cache = BuildCache::new(&dir, u64::MAX);
        let key = build_key(&mesh, 0);
        assert!(cache.load(key).is_none());
        let bytes = cache.store(key, &built).unwrap();
        let raw = built.vertices.len() * 48 + built.indices.len() * 4 + built.clusters.len() * 88;
        assert!(
            (bytes as usize) < raw,
            "{bytes} bytes on disk for {raw} bytes of data"
        );
        same(&built, &cache.load(key).expect("cached"));
        // Another key misses; a damaged file is ignored (and removed).
        assert!(cache.load(build_key(&mesh, 1)).is_none());
        let path = cache.path(key);
        let mut data = std::fs::read(&path).unwrap();
        let n = data.len();
        data[n - 10] ^= 0x55;
        std::fs::write(&path, &data).unwrap();
        assert!(cache.load(key).is_none());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_follow_the_inputs() {
        let (mut mesh, _) = dag();
        let a = build_key(&mesh, 0);
        assert_eq!(a, build_key(&mesh, 0));
        assert_ne!(a, build_key(&mesh, 5));
        mesh.vertices[3].position[0] += 1e-6;
        assert_ne!(a, build_key(&mesh, 0));
    }

    #[test]
    fn the_folder_keeps_to_its_budget() {
        let (mesh, built) = dag();
        let dir =
            std::env::temp_dir().join(format!("engine-vgc-budget-test-{}", std::process::id()));
        let one = BuildCache::new(&dir, u64::MAX)
            .store(build_key(&mesh, 100), &built)
            .unwrap();
        let cache = BuildCache::new(&dir, one * 2 + one / 2);
        for k in 0..5 {
            cache.store(build_key(&mesh, k), &built).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        assert!(cache.size() <= cache.max_bytes, "{} bytes", cache.size());
        // The newest survive.
        assert!(cache.load(build_key(&mesh, 4)).is_some());
        assert!(cache.load(build_key(&mesh, 0)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
