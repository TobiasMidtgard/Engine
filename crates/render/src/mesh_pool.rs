//! Shared vertex and index buffers for the instanced meshes (after Unreal's and Unity's global
//! vertex and index buffers, and Aaltonen and Haar, "GPU-Driven Rendering Pipelines",
//! SIGGRAPH 2015): each mesh and its levels of detail take a range of a page's two buffers, so
//! draws keep one vertex and one index buffer bound and differ only in their first index, base
//! vertex and instances, which lets runs of them go out as one `multi_draw_indexed_indirect`.
//!
//! Pages start small and double up to [`MAX_PAGE_BYTES`], so a small scene stays small and a
//! large one needs few pages; a page is never resized (no copies), a mesh too big for the next
//! page gets one of its own, and pages left empty are released. Ranges are best fit with
//! neighbours merged when freed.
//!
//! A page's index buffer is sized from what the pool's meshes have needed so far (index bytes
//! for each vertex byte, levels of detail included, plus [`INDEX_HEADROOM`]) instead of being
//! as large as its vertex buffer: at 48 bytes a vertex most meshes need a fifth to a half of
//! that, and the rest was video memory nobody used (888 MB of index buffers at the showcase's
//! peak). A mesh whose levels of detail are still being built holds room for them in its page,
//! so the tighter buffer doesn't fill up before they arrive.

use std::collections::BTreeMap;

use crate::mesh::Vertex;

/// Bytes of each buffer of the first page.
const FIRST_PAGE_BYTES: u64 = 8 << 20;
/// Bytes of each buffer of the largest pages (also capped by the device's buffer size limit).
const MAX_PAGE_BYTES: u64 = 128 << 20;
const VERTEX_BYTES: u64 = std::mem::size_of::<Vertex>() as u64;
/// Index bytes for each vertex byte of a page made before the pool has meshes to measure.
const FIRST_INDEX_SHARE: f64 = 0.5;
/// Smallest and largest index share of a page (the largest is the old equal-sized buffers).
const INDEX_SHARE_RANGE: (f64, f64) = (0.125, 1.0);
/// A new page's index buffer has this much more room than the measured share asks for.
const INDEX_HEADROOM: f64 = 1.5;
/// The measured share counts once this many vertex bytes are in use.
const MEASURE_FROM_BYTES: u64 = 1 << 20;

/// `ENGINE_MESH_INDEX_SHARE=<percent>`: a fixed index share for every page instead of the
/// measured one (100 = index buffers as large as the vertex buffers, as before), for
/// before/after memory tables.
fn fixed_index_share() -> Option<f64> {
    static SHARE: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *SHARE.get_or_init(|| {
        let percent: f64 = std::env::var("ENGINE_MESH_INDEX_SHARE")
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Some((percent / 100.0).clamp(INDEX_SHARE_RANGE.0, INDEX_SHARE_RANGE.1))
    })
}

/// Index bytes for each vertex byte of the next page: what the pool's pages have in use
/// (vertex bytes, then index bytes with the room held for levels of detail) plus headroom. A
/// run of index-heavy meshes that fills a page's indices first leaves vertex room in it for
/// later meshes, and raises the share of the pages after it.
fn index_share(vertex_bytes: u64, index_bytes: u64) -> f64 {
    if vertex_bytes < MEASURE_FROM_BYTES {
        return FIRST_INDEX_SHARE;
    }
    (index_bytes as f64 / vertex_bytes as f64 * INDEX_HEADROOM)
        .clamp(INDEX_SHARE_RANGE.0, INDEX_SHARE_RANGE.1)
}

/// Bytes of a new page's vertex and index buffers: `size` for the vertices (more for a mesh
/// that needs more) and `share` of that for the indices, at least the `index_wanted` bytes of
/// the mesh it is made for; `None` when `limit` (the largest buffer) can't hold the mesh's
/// vertices and its `index_needed` bytes.
fn page_bytes(
    size: u64,
    limit: u64,
    share: f64,
    vertex_bytes: u64,
    index_needed: u64,
    index_wanted: u64,
) -> Option<(u64, u64)> {
    let vertex_size = size.max(vertex_bytes).min(limit);
    let index_size = ((vertex_size as f64 * share) as u64)
        .max(index_wanted)
        .min(limit)
        & !3;
    (vertex_size >= vertex_bytes && index_size >= index_needed).then_some((vertex_size, index_size))
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
    /// Indices held for levels of detail still being built ([`PoolMesh::lod_room`]s).
    held: u32,
}

/// Where a mesh's vertices and full index list live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolMesh {
    pub page: u16,
    pub base_vertex: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub index_count: u32,
    /// Indices its page holds free for its levels of detail, until
    /// [`MeshPool::release_lod_room`].
    pub lod_room: u32,
}

#[derive(Default)]
pub(crate) struct MeshPool {
    pages: Vec<Option<Page>>,
    /// Bytes of the vertex buffer of the next page made.
    next_bytes: u64,
    /// Levels of detail that found no room in their mesh's page (the mesh is then drawn with
    /// the levels that fit).
    pub lods_refused: u32,
}

impl MeshPool {
    /// Upload a mesh into a page with room for it (a new one when none has), holding
    /// `lod_room` more indices free there for its levels of detail (0 when it gets none).
    /// `None` when it is larger than the device allows one buffer to be.
    pub fn add(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[Vertex],
        indices: &[u32],
        lod_room: u32,
    ) -> Option<PoolMesh> {
        let (vertex_count, index_count) = (vertices.len() as u32, indices.len() as u32);
        let fits = |p: &mut Page, lod_room: u32| {
            // Room held for other meshes' levels stays theirs.
            let wanted = index_count as u64 + lod_room as u64 + p.held as u64;
            if (p.indices.unused() as u64) < wanted {
                return None;
            }
            let base = p.vertices.alloc(vertex_count)?;
            match p.indices.alloc(index_count) {
                Some(first) => {
                    p.held += lod_room;
                    Some((base, first))
                }
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
            .find_map(|(n, p)| fits(p.as_mut()?, lod_room).map(|r| (n, r, lod_room)));
        let (page, (base_vertex, first_index), lod_room) = match found {
            Some(found) => found,
            None => {
                let n = self.new_page(
                    device,
                    vertex_count as u64 * VERTEX_BYTES,
                    index_count as u64 * 4,
                    (index_count as u64 + lod_room as u64) * 4,
                )?;
                // A mesh near the largest buffer holds what is left of its own page.
                let p = self.pages[n].as_mut()?;
                let lod_room = lod_room.min(p.indices.capacity.saturating_sub(index_count));
                (n, fits(p, lod_room)?, lod_room)
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
            lod_room,
        })
    }

    /// The mesh's levels of detail are ready to add (or will never come): the room held for
    /// them is free again, for them or anything else.
    pub fn release_lod_room(&mut self, mesh: &mut PoolMesh) {
        if let Some(p) = self
            .pages
            .get_mut(mesh.page as usize)
            .and_then(Option::as_mut)
        {
            p.held = p.held.saturating_sub(mesh.lod_room);
        }
        mesh.lod_room = 0;
    }

    /// Add more indices over a mesh's vertices (a level of detail) in its page; `None` when
    /// the page has no room left for them.
    pub fn add_indices(&mut self, queue: &wgpu::Queue, page: u16, indices: &[u32]) -> Option<u32> {
        if indices.is_empty() {
            return None;
        }
        let p = self.pages.get_mut(page as usize)?.as_mut()?;
        let Some(first) = p.indices.alloc(indices.len() as u32) else {
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
            p.held = p.held.saturating_sub(mesh.lod_room);
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

    /// A page for a mesh of `vertex_bytes` and `index_needed` bytes, with `index_wanted`
    /// bytes of indices when the device allows (its levels of detail too).
    fn new_page(
        &mut self,
        device: &wgpu::Device,
        vertex_bytes: u64,
        index_needed: u64,
        index_wanted: u64,
    ) -> Option<usize> {
        let limit = device
            .limits()
            .max_buffer_size
            .min(MAX_PAGE_BYTES.max(vertex_bytes).max(index_wanted));
        let size = self.next_bytes.max(FIRST_PAGE_BYTES);
        let share = fixed_index_share().unwrap_or_else(|| {
            let (vertices, indices) = self.pages.iter().flatten().fold((0, 0), |(v, i), p| {
                (
                    v + p.vertices.used() as u64 * VERTEX_BYTES,
                    i + (p.indices.used() as u64 + p.held as u64) * 4,
                )
            });
            index_share(vertices, indices)
        });
        let (vertex_size, index_size) =
            page_bytes(size, limit, share, vertex_bytes, index_needed, index_wanted)?;
        self.next_bytes = (size * 2).min(MAX_PAGE_BYTES);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let page = Page {
            vertex: buffer(
                "mesh pool vertices",
                vertex_size,
                // Skinned on the GPU in place (`skinning`), from a copy of the bind pose.
                wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC,
            ),
            index: buffer("mesh pool indices", index_size, wgpu::BufferUsages::INDEX),
            vertices: Ranges::new((vertex_size / VERTEX_BYTES) as u32),
            indices: Ranges::new((index_size / 4) as u32),
            held: 0,
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
    use super::{INDEX_HEADROOM, Ranges, index_share, page_bytes};

    const MB: u64 = 1 << 20;

    #[test]
    fn a_page_has_index_room_for_what_the_meshes_so_far_needed() {
        // Nothing to measure yet: half the vertex bytes.
        assert_eq!(index_share(0, 0), 0.5);
        // Meshes that used 20 index bytes per 100 vertex bytes: that and the headroom.
        let share = index_share(100 * MB, 20 * MB);
        assert!((share - 0.2 * INDEX_HEADROOM).abs() < 1e-9, "{share}");
        // Never under an eighth, never more than the vertex buffer.
        assert_eq!(index_share(100 * MB, MB), 0.125);
        assert_eq!(index_share(100 * MB, 300 * MB), 1.0);

        let limit = 128 * MB;
        assert_eq!(
            page_bytes(64 * MB, limit, share, MB, MB, 2 * MB),
            Some((64 * MB, (64.0 * 0.3 * MB as f64) as u64 & !3))
        );
        // A mesh with more indices than the share gives gets them, its levels too if they fit.
        assert_eq!(
            page_bytes(8 * MB, limit, 0.125, 4 * MB, 6 * MB, 12 * MB),
            Some((8 * MB, 12 * MB))
        );
        // At the largest buffer the levels' room goes first, then the mesh doesn't fit.
        assert_eq!(
            page_bytes(8 * MB, limit, 0.5, 4 * MB, 100 * MB, 200 * MB),
            Some((8 * MB, limit))
        );
        assert_eq!(
            page_bytes(8 * MB, limit, 0.5, 4 * MB, 129 * MB, 200 * MB),
            None
        );
        assert_eq!(page_bytes(8 * MB, limit, 0.5, 129 * MB, MB, MB), None);
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
