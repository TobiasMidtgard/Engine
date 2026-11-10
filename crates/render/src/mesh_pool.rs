//! Shared vertex and index buffers for the instanced meshes (after Unreal's and Unity's global
//! vertex and index buffers, and Aaltonen and Haar, "GPU-Driven Rendering Pipelines",
//! SIGGRAPH 2015): each mesh and its levels of detail take a range of a page's two buffers, so
//! draws keep one vertex and one index buffer bound and differ only in their first index, base
//! vertex and instances, which lets runs of them go out as one `multi_draw_indexed_indirect`.
//!
//! Pages start small and double up to [`MAX_PAGE_BYTES`], so a small scene stays small and a
//! large one needs few pages; a page's vertex buffer is never resized, a mesh too big for the
//! next page gets one of its own, and pages left empty are released. Ranges are best fit with
//! neighbours merged when freed.
//!
//! A page's index buffer starts at an eighth of its vertex buffer ([`FIRST_INDEX_SHARE`]) and
//! grows by a quarter whenever a mesh or a level of detail needs more, by a copy on the GPU
//! into a larger buffer. How many index bytes a vertex byte needs differs too much between
//! meshes to size it once: at 48 bytes a vertex a smooth closed mesh needs half as many index
//! bytes, a flat-shaded one a twelfth, and levels of detail arrive later. Buffers as large as
//! the vertex buffers were 888 MB at the showcase's peak for about 250 MB of indices, and ones
//! sized from the meshes seen so far were still 158 MB for 43 MB at its plaza.

use std::collections::BTreeMap;

use crate::mesh::Vertex;

/// Bytes of each buffer of the first page.
const FIRST_PAGE_BYTES: u64 = 8 << 20;
/// Bytes of each buffer of the largest pages (also capped by the device's buffer size limit).
const MAX_PAGE_BYTES: u64 = 128 << 20;
const VERTEX_BYTES: u64 = std::mem::size_of::<Vertex>() as u64;
/// Index bytes for each vertex byte of a new page.
const FIRST_INDEX_SHARE: f64 = 0.125;
/// An index buffer that is full grows by this much of its size (at least [`MIN_GROWTH_BYTES`]
/// and what the mesh needs).
const INDEX_GROWTH: f64 = 0.25;
const MIN_GROWTH_BYTES: u64 = 1 << 20;

/// 0: not read yet; otherwise the index share of new pages in thousandths plus one.
static INDEX_SHARE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Index bytes for each vertex byte of a new page: [`FIRST_INDEX_SHARE`], or
/// `ENGINE_MESH_INDEX_SHARE=<percent>` (100 = index buffers as large as the vertex buffers, as
/// they were before they could grow), for before/after memory tables.
fn index_share() -> f64 {
    use std::sync::atomic::Ordering;
    let stored = INDEX_SHARE.load(Ordering::Relaxed);
    if stored > 0 {
        return (stored - 1) as f64 / 1000.0;
    }
    let share = std::env::var("ENGINE_MESH_INDEX_SHARE")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map_or(FIRST_INDEX_SHARE, |percent| {
            (percent / 100.0).clamp(1.0 / 64.0, 1.0)
        });
    INDEX_SHARE.store((share * 1000.0) as u32 + 1, Ordering::Relaxed);
    share
}

/// Index bytes for each vertex byte of pages made from now on (see [`index_share`]), for tests
/// that compare a pool whose index buffers grow with one whose buffers never need to.
#[doc(hidden)]
pub fn set_index_share(share: f64) {
    INDEX_SHARE.store(
        (share.clamp(1.0 / 64.0, 1.0) * 1000.0) as u32 + 1,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Bytes of a new page's vertex and index buffers: `size` for the vertices (more for a mesh
/// that needs more) and `share` of that for the indices, at least the `index_needed` bytes of
/// the mesh it is made for; `None` when `limit` (the largest buffer) can't hold the mesh.
fn page_bytes(
    size: u64,
    limit: u64,
    share: f64,
    vertex_bytes: u64,
    index_needed: u64,
) -> Option<(u64, u64)> {
    let vertex_size = size.max(vertex_bytes).min(limit);
    let index_size = ((vertex_size as f64 * share) as u64)
        .max(index_needed)
        .min(limit)
        .next_multiple_of(4);
    (vertex_size >= vertex_bytes && index_size >= index_needed).then_some((vertex_size, index_size))
}

/// Bytes of an index buffer of `size` bytes after it has grown to take `needed` more than it
/// has room for; `None` when that is more than `limit`.
fn grown_bytes(size: u64, needed: u64, limit: u64) -> Option<u64> {
    let least = size + needed;
    if least > limit {
        return None;
    }
    let step = ((size as f64 * INDEX_GROWTH) as u64).max(MIN_GROWTH_BYTES);
    Some((size + step).max(least).min(limit).next_multiple_of(4))
}

/// Free ranges of a buffer, in elements.
#[derive(Debug, Default)]
struct Ranges {
    /// Start -> length of each free range; neighbours are always merged.
    free: BTreeMap<u32, u32>,
    capacity: u32,
}

impl Ranges {
    fn new(capacity: u32) -> Self {
        let mut free = BTreeMap::new();
        if capacity > 0 {
            free.insert(0, capacity);
        }
        Self { free, capacity }
    }

    /// The smallest free range that fits `len`, lowest first among equals.
    fn alloc(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return Some(0);
        }
        let (&start, &free) = self
            .free
            .iter()
            .filter(|&(_, &l)| l >= len)
            .min_by_key(|&(&s, &l)| (l, s))?;
        self.free.remove(&start);
        if free > len {
            self.free.insert(start + len, free - len);
        }
        Some(start)
    }

    fn release(&mut self, start: u32, len: u32) {
        if len == 0 {
            return;
        }
        let (mut start, mut len) = (start, len);
        if let Some((&before, &l)) = self.free.range(..start).next_back()
            && before + l == start
        {
            self.free.remove(&before);
            start = before;
            len += l;
        }
        if let Some(l) = self.free.remove(&(start + len)) {
            len += l;
        }
        self.free.insert(start, len);
    }

    fn is_empty(&self) -> bool {
        self.free.get(&0) == Some(&self.capacity)
    }

    /// Elements free at the very end.
    fn free_tail(&self) -> u32 {
        match self.free.iter().next_back() {
            Some((&start, &len)) if start + len == self.capacity => len,
            _ => 0,
        }
    }

    /// More room at the end: `capacity` elements in all from now on.
    fn grow(&mut self, capacity: u32) {
        if capacity > self.capacity {
            let (start, len) = (self.capacity, capacity - self.capacity);
            self.capacity = capacity;
            self.release(start, len);
        }
    }

    fn used(&self) -> u32 {
        self.capacity - self.unused()
    }

    fn unused(&self) -> u32 {
        self.free.values().sum()
    }
}

struct Page {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    vertices: Ranges,
    indices: Ranges,
    /// The most bytes its index buffer may grow to.
    index_limit: u64,
}

impl Page {
    /// `count` indices in a row, from a larger index buffer when this one has no such room:
    /// what it holds is copied over on the GPU, after the writes queued so far and before
    /// those that follow. `None` when the buffer may not grow that far.
    fn alloc_indices(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: u32,
        grown: &mut u32,
    ) -> Option<u32> {
        if let Some(first) = self.indices.alloc(count) {
            return Some(first);
        }
        let needed = count.saturating_sub(self.indices.free_tail()) as u64 * 4;
        let size = grown_bytes(self.index.size(), needed, self.index_limit)?;
        let larger = index_buffer(device, size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mesh pool indices"),
        });
        encoder.copy_buffer_to_buffer(&self.index, 0, &larger, 0, self.index.size());
        queue.submit([encoder.finish()]);
        self.index = larger;
        self.indices.grow((size / 4) as u32);
        *grown += 1;
        self.indices.alloc(count)
    }
}

fn index_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh pool indices"),
        size,
        usage: wgpu::BufferUsages::INDEX
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Where a mesh's vertices and full index list live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolMesh {
    pub page: u16,
    pub base_vertex: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Default)]
pub(crate) struct MeshPool {
    pages: Vec<Option<Page>>,
    /// Bytes of the vertex buffer of the next page made.
    next_bytes: u64,
    /// Levels of detail that found no room in their mesh's page, whose index buffer is as
    /// large as it may be (the mesh is then drawn with the levels that fit).
    pub lods_refused: u32,
    /// Times an index buffer grew.
    pub index_growths: u32,
}

impl MeshPool {
    /// Upload a mesh into a page with room for it (a new one when none has). `None` when it is
    /// larger than the device allows one buffer to be.
    pub fn add(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> Option<PoolMesh> {
        let (vertex_count, index_count) = (vertices.len() as u32, indices.len() as u32);
        let grown = &mut self.index_growths;
        let mut fits = |p: &mut Page| {
            let base = p.vertices.alloc(vertex_count)?;
            match p.alloc_indices(device, queue, index_count, grown) {
                Some(first) => Some((base, first)),
                None => {
                    p.vertices.release(base, vertex_count);
                    None
                }
            }
        };
        let found = self
            .pages
            .iter_mut()
            .enumerate()
            .find_map(|(n, p)| fits(p.as_mut()?).map(|r| (n, r)));
        let (page, (base_vertex, first_index)) = match found {
            Some(found) => found,
            None => {
                let n = self.new_page(
                    device,
                    vertex_count as u64 * VERTEX_BYTES,
                    index_count as u64 * 4,
                )?;
                let p = self.pages[n].as_mut()?;
                let base = p.vertices.alloc(vertex_count)?;
                (n, (base, p.indices.alloc(index_count)?))
            }
        };
        let p = self.pages[page].as_ref()?;
        queue.write_buffer(
            &p.vertex,
            base_vertex as u64 * VERTEX_BYTES,
            bytemuck::cast_slice(vertices),
        );
        queue.write_buffer(
            &p.index,
            first_index as u64 * 4,
            bytemuck::cast_slice(indices),
        );
        Some(PoolMesh {
            page: page as u16,
            base_vertex,
            vertex_count,
            first_index,
            index_count,
        })
    }

    /// Add more indices over a mesh's vertices (a level of detail) in its page; `None` when
    /// its index buffer is full and as large as it may be.
    pub fn add_indices(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        page: u16,
        indices: &[u32],
    ) -> Option<u32> {
        if indices.is_empty() {
            return None;
        }
        let p = self.pages.get_mut(page as usize)?.as_mut()?;
        let count = indices.len() as u32;
        let Some(first) = p.alloc_indices(device, queue, count, &mut self.index_growths) else {
            self.lods_refused += 1;
            return None;
        };
        queue.write_buffer(&p.index, first as u64 * 4, bytemuck::cast_slice(indices));
        Some(first)
    }

    pub fn free_indices(&mut self, page: u16, first: u32, count: u32) {
        if let Some(p) = self.pages.get_mut(page as usize).and_then(Option::as_mut) {
            p.indices.release(first, count);
        }
        self.release_if_empty(page);
    }

    /// Free a mesh's vertices and full index list (its levels of detail go with
    /// [`MeshPool::free_indices`]).
    pub fn free(&mut self, mesh: &PoolMesh) {
        if let Some(p) = self
            .pages
            .get_mut(mesh.page as usize)
            .and_then(Option::as_mut)
        {
            p.vertices.release(mesh.base_vertex, mesh.vertex_count);
            p.indices.release(mesh.first_index, mesh.index_count);
        }
        self.release_if_empty(mesh.page);
    }

    /// A page's vertex and index buffers.
    pub fn buffers(&self, page: u16) -> Option<(&wgpu::Buffer, &wgpu::Buffer)> {
        let p = self.pages.get(page as usize)?.as_ref()?;
        Some((&p.vertex, &p.index))
    }

    /// Pages, bytes reserved and bytes in use (both buffers).
    pub fn usage(&self) -> (usize, u64, u64) {
        self.pages
            .iter()
            .flatten()
            .fold((0, 0, 0), |(n, reserved, used), p| {
                (
                    n + 1,
                    reserved + p.vertex.size() + p.index.size(),
                    used + p.vertices.used() as u64 * VERTEX_BYTES + p.indices.used() as u64 * 4,
                )
            })
    }

    /// Bytes reserved and in use by the vertex buffers, then by the index buffers.
    pub fn split(&self) -> [(u64, u64); 2] {
        self.pages.iter().flatten().fold([(0, 0); 2], |[v, i], p| {
            [
                (
                    v.0 + p.vertex.size(),
                    v.1 + p.vertices.used() as u64 * VERTEX_BYTES,
                ),
                (i.0 + p.index.size(), i.1 + p.indices.used() as u64 * 4),
            ]
        })
    }

    pub fn describe(&self) -> serde_json::Value {
        let (pages, reserved, used) = self.usage();
        let [vertices, indices] = self.split();
        serde_json::json!({
            "pages": pages,
            "reserved_bytes": reserved,
            "used_bytes": used,
            "vertex_bytes": [vertices.0, vertices.1],
            "index_bytes": [indices.0, indices.1],
            "lods_refused": self.lods_refused,
            "index_growths": self.index_growths,
        })
    }

    fn release_if_empty(&mut self, page: u16) {
        // The first page stays: most scenes come back to it.
        if page > 0
            && let Some(slot) = self.pages.get_mut(page as usize)
            && slot
                .as_ref()
                .is_some_and(|p| p.vertices.is_empty() && p.indices.is_empty())
        {
            *slot = None;
        }
    }

    /// A page for a mesh of `vertex_bytes` and `index_needed` bytes.
    fn new_page(
        &mut self,
        device: &wgpu::Device,
        vertex_bytes: u64,
        index_needed: u64,
    ) -> Option<usize> {
        let limit = device
            .limits()
            .max_buffer_size
            .min(MAX_PAGE_BYTES.max(vertex_bytes).max(index_needed));
        let size = self.next_bytes.max(FIRST_PAGE_BYTES);
        let (vertex_size, index_size) =
            page_bytes(size, limit, index_share(), vertex_bytes, index_needed)?;
        self.next_bytes = (size * 2).min(MAX_PAGE_BYTES);
        let page = Page {
            vertex: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh pool vertices"),
                size: vertex_size,
                // Skinned on the GPU in place (`skinning`), from a copy of the bind pose.
                usage: wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            index: index_buffer(device, index_size),
            vertices: Ranges::new((vertex_size / VERTEX_BYTES) as u32),
            indices: Ranges::new((index_size / 4) as u32),
            index_limit: limit,
        };
        match self.pages.iter().position(Option::is_none) {
            Some(n) => {
                self.pages[n] = Some(page);
                Some(n)
            }
            None => {
                self.pages.push(Some(page));
                Some(self.pages.len() - 1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_GROWTH_BYTES, Ranges, grown_bytes, page_bytes};

    const MB: u64 = 1 << 20;

    #[test]
    fn a_page_starts_with_a_small_index_buffer_that_grows_as_needed() {
        let limit = 128 * MB;
        assert_eq!(
            page_bytes(64 * MB, limit, 0.125, MB, MB),
            Some((64 * MB, 8 * MB))
        );
        // A mesh with more indices than the share gives gets them.
        assert_eq!(
            page_bytes(8 * MB, limit, 0.125, 4 * MB, 6 * MB),
            Some((8 * MB, 6 * MB))
        );
        // Nothing fits past the largest buffer.
        assert_eq!(page_bytes(8 * MB, limit, 0.5, 4 * MB, 129 * MB), None);
        assert_eq!(page_bytes(8 * MB, limit, 0.5, 129 * MB, MB), None);

        // A quarter more, at least a megabyte, at least what is needed, never past the limit.
        assert_eq!(grown_bytes(40 * MB, 4, limit), Some(50 * MB));
        assert_eq!(grown_bytes(MB, 4, limit), Some(MB + MIN_GROWTH_BYTES));
        assert_eq!(grown_bytes(40 * MB, 30 * MB, limit), Some(70 * MB));
        assert_eq!(grown_bytes(120 * MB, 4, limit), Some(limit));
        assert_eq!(grown_bytes(120 * MB, 9 * MB, limit), None);
    }

    #[test]
    fn ranges_grow_at_the_end() {
        let mut r = Ranges::new(100);
        assert_eq!(r.alloc(90), Some(0));
        assert_eq!(r.free_tail(), 10);
        assert_eq!(r.alloc(30), None);
        r.grow(150);
        assert_eq!(r.free_tail(), 60);
        // The old free tail and the new room are one range.
        assert_eq!(r.alloc(60), Some(90));
        assert_eq!(r.free_tail(), 0);
        r.release(0, 90);
        assert_eq!(r.free_tail(), 0);
        r.grow(160);
        assert_eq!((r.used(), r.unused()), (60, 100));
    }

    #[test]
    fn ranges_fit_best_and_merge_when_freed() {
        let mut r = Ranges::new(100);
        let a = r.alloc(30).unwrap();
        let b = r.alloc(20).unwrap();
        let c = r.alloc(10).unwrap();
        assert_eq!((a, b, c), (0, 30, 50));
        assert_eq!(r.used(), 60);
        r.release(b, 20);
        // Best fit: the 20-long hole, not the 40 at the end.
        assert_eq!(r.alloc(15), Some(30));
        r.release(30, 15);
        r.release(a, 30);
        r.release(c, 10);
        assert!(r.is_empty(), "{:?}", r.free);
        assert_eq!(r.alloc(101), None);
        assert_eq!(r.alloc(100), Some(0));
    }
}
