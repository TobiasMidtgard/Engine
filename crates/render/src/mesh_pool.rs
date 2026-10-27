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

use std::collections::BTreeMap;

use crate::mesh::Vertex;

/// Bytes of each buffer of the first page.
const FIRST_PAGE_BYTES: u64 = 8 << 20;
/// Bytes of each buffer of the largest pages (also capped by the device's buffer size limit).
const MAX_PAGE_BYTES: u64 = 128 << 20;
const VERTEX_BYTES: u64 = std::mem::size_of::<Vertex>() as u64;

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
        self.capacity - self.free.values().sum::<u32>()
    }
}

struct Page {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    vertices: Ranges,
    indices: Ranges,
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
    /// Bytes of each buffer of the next page made.
    next_bytes: u64,
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
        let fits = |p: &mut Page| {
            let base = p.vertices.alloc(vertex_count)?;
            match p.indices.alloc(index_count) {
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
                    // Room for its levels of detail too.
                    index_count as u64 * 4 * 2,
                )?;
                (n, fits(self.pages[n].as_mut()?)?)
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
    /// the page has no room left for them.
    pub fn add_indices(&mut self, queue: &wgpu::Queue, page: u16, indices: &[u32]) -> Option<u32> {
        if indices.is_empty() {
            return None;
        }
        let p = self.pages.get_mut(page as usize)?.as_mut()?;
        let first = p.indices.alloc(indices.len() as u32)?;
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

    pub fn describe(&self) -> serde_json::Value {
        let (pages, reserved, used) = self.usage();
        serde_json::json!({ "pages": pages, "reserved_bytes": reserved, "used_bytes": used })
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

    fn new_page(
        &mut self,
        device: &wgpu::Device,
        vertex_bytes: u64,
        index_bytes: u64,
    ) -> Option<usize> {
        let limit = device
            .limits()
            .max_buffer_size
            .min(MAX_PAGE_BYTES.max(vertex_bytes).max(index_bytes));
        let size = self.next_bytes.max(FIRST_PAGE_BYTES);
        let vertex_size = size.max(vertex_bytes).min(limit);
        let index_size = size.max(index_bytes).min(limit) & !3;
        if vertex_size < vertex_bytes || index_size < index_bytes / 2 {
            return None;
        }
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
    use super::Ranges;

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
