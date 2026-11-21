//! Triangle records: what a hit reads of its triangle besides its corners (the uv, the corners'
//! normals, the mean vertex colour), one record a triangle in one buffer, binding 4 of the
//! scene's group. Written on the GPU from the mesh pool's pages in the frame a geometry's
//! structure is built (ray_attr.wgsl says what a record holds), so the main thread does
//! nothing for them. Only geometries built from the pool's pages have records; a row without
//! them is shaded with its face's normal and its row's colour.
//!
//! A geometry's range is one header record (its uv box and the reach of its uv edges) and its
//! triangles' records in the order the structure has them: a hit's record is the row's
//! `tris` plus its `primitive_index`. The range belongs to the structure it was written for
//! and is freed with it (`blas.rs`).

use wgpu::util::DeviceExt;

use crate::mesh_pool::Ranges;

const SHADER: &str = include_str!("../shaders/ray_attr.wgsl");
/// Bytes of a record: 5 words.
const RECORD_BYTES: u64 = 20;
/// Records the buffer starts with (12 MB) and may double up to (48 MB).
const FIRST_RECORDS: u32 = 600_000;
const MOST_RECORDS: u32 = 2_400_000;
/// Triangles of a workgroup, and workgroups in a row of a dispatch (`ROW` in ray_attr.wgsl is
/// their product).
const GROUP: u32 = 64;
const ROW_GROUPS: u32 = 1024;

/// A geometry built this frame whose records are written.
pub(super) struct Job<'a> {
    pub vertex: &'a wgpu::Buffer,
    pub index: &'a wgpu::Buffer,
    pub first_vertex: u32,
    pub first_index: u32,
    pub triangles: u32,
    /// Its header's record: the first of its range.
    pub header: u32,
}

struct Pipelines {
    layout: wgpu::BindGroupLayout,
    boxes: wgpu::ComputePipeline,
    tris: wgpu::ComputePipeline,
}

/// The records' buffer and who holds which part of it.
pub(super) struct Records {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    /// Records the buffer holds, and those the frame's next `encode` makes it hold.
    capacity: u32,
    wanted: u32,
    most: u32,
    ranges: Ranges,
    /// Records in ranges, and ranges refused since the scene was made.
    used: u32,
    refused: u32,
    max_binding: u64,
    pipelines: Option<Pipelines>,
}

impl Records {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self::with_room(device, FIRST_RECORDS, MOST_RECORDS)
    }

    /// (Small numbers for the unit test of the growth.)
    fn with_room(device: &wgpu::Device, first: u32, most: u32) -> Self {
        let limits = device.limits();
        let max_binding = limits.max_storage_buffer_binding_size;
        let most = (most as u64).min(max_binding.min(limits.max_buffer_size) / RECORD_BYTES) as u32;
        let first = first.min(most);
        Self {
            device: device.clone(),
            buffer: Self::buffer_of(device, first),
            capacity: first,
            wanted: first,
            most,
            ranges: Ranges::new(first),
            used: 0,
            refused: 0,
            max_binding,
            pipelines: None,
        }
    }

    fn buffer_of(device: &wgpu::Device, records: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray scene triangle records"),
            size: (records as u64).max(1) * RECORD_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub(super) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Records in use, the buffer (MiB), ranges refused so far, and what the records in use
    /// and `missing` more take (MiB): with the records the geometries in the scene were
    /// refused ([`super::blas::Store::missing_records`]), the need, which may pass the 48 MB
    /// the buffer grows to.
    pub(super) fn stats(&self, missing: u64) -> (u32, f32, u32, f32) {
        let mib = |records: u64| (records * RECORD_BYTES) as f32 / (1u64 << 20) as f32;
        (
            self.used,
            mib(self.capacity as u64),
            self.refused,
            mib(self.used as u64 + missing),
        )
    }

    /// Room for the records of a geometry of `triangles` triangles whose vertices are the
    /// pool's (`PoolVertex`) in `vertex` and whose indices are `u32` in `index`: its range
    /// (the header's record, records), or `None`: its rows keep bit 9 clear.
    pub(super) fn take(
        &mut self,
        triangles: u32,
        vertex: &wgpu::Buffer,
        index: &wgpu::Buffer,
    ) -> Option<(u32, u32)> {
        // (A page the shader can't bind whole would fail the frame's submit.)
        let max = self.max_binding;
        let bindable = |b: &wgpu::Buffer| {
            b.usage().contains(wgpu::BufferUsages::STORAGE)
                && b.size() <= max
                && b.size().is_multiple_of(4)
        };
        let len = triangles
            .checked_add(1)
            .filter(|&len| len <= self.most && bindable(vertex) && bindable(index));
        let Some(len) = len else {
            self.refused += 1;
            return None;
        };
        loop {
            if let Some(first) = self.ranges.alloc(len) {
                self.used += len;
                return Some((first, len));
            }
            if self.wanted >= self.most {
                self.refused += 1;
                return None;
            }
            // Twice the room from this frame's `encode` on. The ranges know it at once: a
            // range is written, and read through a row, no earlier than that.
            self.wanted = self.wanted.saturating_mul(2).min(self.most);
            self.ranges.grow(self.wanted);
        }
    }

    pub(super) fn release(&mut self, first: u32, len: u32) {
        self.ranges.release(first, len);
        self.used -= len;
    }

    /// Record, after the frame's builds: the larger buffer the frame's ranges asked for
    /// (what the old one holds is copied over) and the dispatches that write `jobs`'
    /// records. True when the buffer is another from now on.
    pub(super) fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        jobs: &mut [Job<'_>],
    ) -> bool {
        let grown = self.wanted > self.capacity;
        if grown {
            let larger = Self::buffer_of(&self.device, self.wanted);
            encoder.copy_buffer_to_buffer(&self.buffer, 0, &larger, 0, self.buffer.size());
            self.buffer = larger;
            self.capacity = self.wanted;
        }
        if jobs.is_empty() {
            return grown;
        }
        let device = &self.device;
        let pipelines = self.pipelines.get_or_insert_with(|| Pipelines::new(device));
        // One dispatch of each pass for every pair of pool pages.
        jobs.sort_unstable_by(|a, b| (a.vertex, a.index).cmp(&(b.vertex, b.index)));
        let mut runs: Vec<(wgpu::BindGroup, u32)> = Vec::new();
        for run in jobs.chunk_by(|a, b| a.vertex == b.vertex && a.index == b.index) {
            let mut table: Vec<u32> = Vec::with_capacity(run.len() * 5);
            let mut boxes: Vec<u32> = Vec::with_capacity(run.len() * 5);
            let mut triangles = 0u32;
            for job in run {
                table.extend([
                    triangles,
                    job.triangles,
                    job.first_index,
                    job.first_vertex,
                    job.header,
                ]);
                // (Lowest and highest uv as keys, the longest edge: see ray_attr.wgsl.)
                boxes.extend([u32::MAX, u32::MAX, 0, 0, 0]);
                triangles += job.triangles;
            }
            let storage = |label, words: &[u32]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(words),
                    usage: wgpu::BufferUsages::STORAGE,
                })
            };
            let (table, boxes) = (
                storage("ray records jobs", &table),
                storage("ray records uv boxes", &boxes),
            );
            // (A function, not a closure: its result borrows its argument.)
            fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
                wgpu::BindGroupEntry {
                    binding,
                    resource: buffer.as_entire_binding(),
                }
            }
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ray records"),
                layout: &pipelines.layout,
                entries: &[
                    entry(0, run[0].vertex),
                    entry(1, run[0].index),
                    entry(2, &table),
                    entry(3, &boxes),
                    entry(4, &self.buffer),
                ],
            });
            runs.push((group, triangles.div_ceil(GROUP)));
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ray records"),
            timestamp_writes: None,
        });
        // Every geometry's uv box first: its records are packed against it.
        for pipeline in [&pipelines.boxes, &pipelines.tris] {
            pass.set_pipeline(pipeline);
            for (group, groups) in &runs {
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups((*groups).min(ROW_GROUPS), groups.div_ceil(ROW_GROUPS), 1);
            }
        }
        grown
    }
}

impl Pipelines {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ray records"),
            source: wgpu::ShaderSource::Wgsl(crate::features::compose_shader(SHADER, false).into()),
        });
        let storage = |binding, read_only| {
            crate::renderer::layout_entry(
                binding,
                wgpu::ShaderStages::COMPUTE,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray records"),
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, true),
                storage(3, false),
                storage(4, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray records"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            boxes: pipeline("cs_ray_uv_box"),
            tris: pipeline("cs_ray_tris"),
            layout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Mesh, PoolVertex, Shape, Vertex};
    use engine_core::prelude::*;

    #[test]
    fn shader_validates() {
        let code = crate::features::compose_shader(SHADER, false);
        crate::features::validate_wgsl("ray records", &code).unwrap();
    }

    fn device() -> Option<crate::gpu::Gpu> {
        match crate::gpu::create(None, 1, 1, false) {
            Ok((gpu, _)) => Some(gpu),
            Err(e) => {
                eprintln!("skipping: no GPU ({e})");
                None
            }
        }
    }

    /// A pool page pair holding `meshes` one after another: each mesh's first vertex and
    /// first index there.
    fn pages(
        gpu: &crate::gpu::Gpu,
        meshes: &[&Mesh],
    ) -> (wgpu::Buffer, wgpu::Buffer, Vec<(u32, u32)>) {
        let mut vertices: Vec<PoolVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut at = Vec::new();
        for mesh in meshes {
            at.push((vertices.len() as u32, indices.len() as u32));
            vertices.extend(mesh.vertices.iter().map(PoolVertex::from));
            indices.extend(&mesh.indices);
        }
        let buffer = |label, contents: &[u8]| {
            gpu.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                })
        };
        (
            buffer("test vertex page", bytemuck::cast_slice(&vertices)),
            buffer("test index page", bytemuck::cast_slice(&indices)),
            at,
        )
    }

    /// The words of `records`' buffer, after what was submitted.
    fn read_words(gpu: &crate::gpu::Gpu, records: &Records) -> Vec<u32> {
        let size = records.buffer().size();
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test records readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(records.buffer(), 0, &staging, 0, size);
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        rx.recv().unwrap().expect("map");
        let words =
            bytemuck::cast_slice::<u8, u32>(&staging.slice(..).get_mapped_range().expect("mapped"))
                .to_vec();
        staging.unmap();
        words
    }

    fn submit(gpu: &crate::gpu::Gpu, records: &mut Records, jobs: &mut [Job<'_>]) -> bool {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let grown = records.encode(&mut encoder, jobs);
        gpu.queue.submit([encoder.finish()]);
        grown
    }

    fn unorm16(w: u32) -> Vec2 {
        Vec2::new((w & 0xffff) as f32, (w >> 16) as f32) / 65535.0
    }

    fn snorm16(w: u32) -> Vec2 {
        let s = |h: u32| (h as u16 as i16 as f32 / 32767.0).max(-1.0);
        Vec2::new(s(w & 0xffff), s(w >> 16))
    }

    fn snorm8(w: u32, byte: u32) -> f32 {
        ((w >> (8 * byte)) as u8 as i8 as f32 / 127.0).max(-1.0)
    }

    /// `oct_decode` of lib_types.wgsl.
    fn oct_decode(e: Vec2) -> Vec3 {
        let mut n = Vec3::new(e.x, e.y, 1.0 - e.x.abs() - e.y.abs());
        let t = (-n.z).clamp(0.0, 1.0);
        n.x += if n.x >= 0.0 { -t } else { t };
        n.y += if n.y >= 0.0 { -t } else { t };
        n.normalize()
    }

    /// A job's mesh, as the records must hold it: its uv box, the reach of its uv edges.
    fn uv_box(mesh: &Mesh) -> (Vec2, Vec2, f32) {
        let uv = |i: u32| Vec2::from(mesh.vertices[i as usize].uv);
        let (mut lo, mut hi, mut reach) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN), 0f32);
        for t in mesh.indices.as_chunks::<3>().0 {
            for &i in t {
                lo = lo.min(uv(i));
                hi = hi.max(uv(i));
            }
            let e = (uv(t[1]) - uv(t[0])).abs().max((uv(t[2]) - uv(t[0])).abs());
            reach = reach.max(e.max_element());
        }
        (lo, hi - lo, reach.max(32767.0 / 2048.0))
    }

    fn check_job(words: &[u32], mesh: &Mesh, header: u32) {
        let (lo, size, reach) = uv_box(mesh);
        let h = header as usize * 5;
        let f = |i: usize| f32::from_bits(words[h + i]);
        assert_eq!(Vec2::new(f(0), f(1)), lo, "the uv box's low corner");
        assert_eq!(
            Vec2::new(f(2), f(3)),
            size.max(Vec2::splat(1e-6)),
            "the uv box's size"
        );
        assert_eq!(f(4), reach, "the edges' reach");
        for (k, t) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            let r = (header as usize + 1 + k) * 5;
            let v = |c: usize| &mesh.vertices[t[c] as usize];
            let uv0 = lo + unorm16(words[r]) * size;
            assert!(
                (uv0 - Vec2::from(v(0).uv)).abs().max_element()
                    <= size.max_element() / 65535.0 + 1e-5,
                "triangle {k}: uv {uv0} for {:?}",
                v(0).uv
            );
            for (e, c) in [(1, 1), (2, 2)] {
                let edge = snorm16(words[r + e]) * reach;
                let want = Vec2::from(v(c).uv) - Vec2::from(v(0).uv);
                assert!(
                    (edge - want).abs().max_element() <= reach / 32767.0 + 1e-5,
                    "triangle {k}: edge {e} {edge} for {want}"
                );
            }
            let a = words[r + 3];
            let b = words[r + 4];
            let normals = [
                oct_decode(Vec2::new(snorm8(a, 0), snorm8(a, 1))),
                oct_decode(Vec2::new(snorm8(a, 2), snorm8(a, 3))),
                oct_decode(Vec2::new(snorm8(b, 0), snorm8(b, 1))),
            ];
            for (c, n) in normals.iter().enumerate() {
                let want = Vec3::from(v(c).normal).normalize();
                assert!(
                    n.dot(want) > 2f32.to_radians().cos(),
                    "triangle {k}: corner {c}'s normal {n} for {want}"
                );
            }
            let mean = (Vec3::from_slice(&v(0).color[..3])
                + Vec3::from_slice(&v(1).color[..3])
                + Vec3::from_slice(&v(2).color[..3]))
                / 3.0;
            let shade = mean.clamp(Vec3::ZERO, Vec3::ONE).map(f32::sqrt);
            let w = b >> 16;
            let got = Vec3::new(
                (w >> 11) as f32 / 31.0,
                ((w >> 5) & 63) as f32 / 63.0,
                (w & 31) as f32 / 31.0,
            );
            let step = Vec3::new(1.0 / 31.0, 1.0 / 63.0, 1.0 / 31.0);
            assert!(
                (got - shade).abs().cmple(step * 0.5 + 1e-4).all(),
                "triangle {k}: colour {got} for {shade}"
            );
        }
    }

    #[test]
    fn records_hold_what_the_vertices_hold() {
        let Some(gpu) = device() else { return };
        // Other meshes before the ones the jobs name, so the jobs' offsets count.
        let filler = Shape::Cuboid {
            size: [1.0, 2.0, 3.0],
        }
        .mesh();
        let sphere = Shape::Sphere {
            radius: 2.0,
            segments: 16,
        }
        .mesh();
        let mut quad = Mesh::new(
            vec![
                Vertex::new(Vec3::ZERO, Vec3::Y, Vec2::new(0.0, 0.0)),
                Vertex::new(Vec3::X, Vec3::Y, Vec2::new(300.0, 0.0)),
                Vertex::new(Vec3::new(1.0, 0.0, 1.0), Vec3::Y, Vec2::new(300.0, 300.0)),
                Vertex::new(Vec3::Z, Vec3::Y, Vec2::new(0.0, 300.0)),
            ],
            vec![0, 3, 2, 0, 2, 1],
        );
        let paints = [
            Vec4::new(0.9, 0.1, 0.1, 1.0),
            Vec4::new(0.1, 0.6, 0.1, 1.0),
            Vec4::new(0.02, 0.05, 0.8, 1.0),
            Vec4::new(1.0, 1.0, 1.0, 1.0),
        ];
        for (v, c) in quad.vertices.iter_mut().zip(paints) {
            v.color = c.to_array();
        }
        let other = Shape::Cuboid {
            size: [4.0, 0.5, 0.5],
        }
        .mesh()
        .with_vertex_color(Vec4::new(0.25, 0.5, 0.75, 1.0));
        let (vertex_a, index_a, at_a) = pages(&gpu, &[&filler, &sphere, &quad]);
        let (vertex_b, index_b, at_b) = pages(&gpu, &[&filler, &other]);
        let mut records = Records::new(&gpu.device);
        let mut take = |mesh: &Mesh, vertex, index| {
            records
                .take(mesh.indices.len() as u32 / 3, vertex, index)
                .expect("room")
                .0
        };
        let headers = [
            take(&sphere, &vertex_a, &index_a),
            take(&quad, &vertex_a, &index_a),
            take(&other, &vertex_b, &index_b),
        ];
        let job =
            |mesh: &Mesh, (first_vertex, first_index): (u32, u32), vertex, index, header| Job {
                vertex,
                index,
                first_vertex,
                first_index,
                triangles: mesh.indices.len() as u32 / 3,
                header,
            };
        let mut jobs = vec![
            job(&other, at_b[1], &vertex_b, &index_b, headers[2]),
            job(&sphere, at_a[1], &vertex_a, &index_a, headers[0]),
            job(&quad, at_a[2], &vertex_a, &index_a, headers[1]),
        ];
        assert!(!submit(&gpu, &mut records, &mut jobs), "no growth");
        let words = read_words(&gpu, &records);
        check_job(&words, &sphere, headers[0]);
        check_job(&words, &quad, headers[1]);
        check_job(&words, &other, headers[2]);
        let (used, mb, refused, wanted_mb) = records.stats(0);
        let triangles = (sphere.indices.len() + quad.indices.len() + other.indices.len()) / 3;
        assert_eq!(used as usize, triangles + 3);
        assert_eq!(refused, 0);
        // (MiB: the 12 MB the buffer starts with.)
        let mib = |bytes: f64| (bytes / (1u64 << 20) as f64) as f32;
        assert!((mb - mib(12e6)).abs() < 1e-3, "{mb} MiB");
        let need = mib((used as u64 * RECORD_BYTES) as f64);
        assert!((wanted_mb - need).abs() < 1e-6, "{wanted_mb} MiB");
    }

    #[test]
    fn records_keep_their_place_when_the_buffer_grows() {
        let Some(gpu) = device() else { return };
        let small = Shape::Cuboid {
            size: [1.0, 1.0, 1.0],
        }
        .mesh()
        .with_vertex_color(Vec4::new(0.5, 0.25, 1.0, 1.0));
        let (vertex, index, at) = pages(&gpu, &[&small]);
        let mut records = Records::with_room(&gpu.device, 64, 256);
        let triangles = small.indices.len() as u32 / 3;
        let job = |header| Job {
            vertex: &vertex,
            index: &index,
            first_vertex: at[0].0,
            first_index: at[0].1,
            triangles,
            header,
        };
        let (first, len) = records.take(triangles, &vertex, &index).expect("room");
        assert!(!submit(&gpu, &mut records, &mut [job(first)]));
        let before = read_words(&gpu, &records);
        let range = |words: &[u32]| words[first as usize * 5..(first + len) as usize * 5].to_vec();
        // More than the 64 records the buffer has: it doubles (13 records a cuboid, so the
        // fifth range is the one that asks).
        let mut headers = Vec::new();
        while records.wanted == records.capacity {
            headers.push(records.take(triangles, &vertex, &index).expect("room").0);
        }
        assert_eq!(
            (headers.len(), records.wanted),
            (4, 128),
            "a growth was asked for"
        );
        let mut jobs: Vec<Job> = headers.iter().map(|&h| job(h)).collect();
        assert!(
            submit(&gpu, &mut records, &mut jobs),
            "the buffer is another"
        );
        let after = read_words(&gpu, &records);
        assert_eq!(
            range(&after),
            range(&before),
            "the first range kept its words"
        );
        assert_eq!(after.len(), records.capacity as usize * 5);
        check_job(&after, &small, first);
        check_job(&after, &small, *headers.last().unwrap());
        // Past the most the buffer may hold: refused and counted.
        assert!(records.take(300, &vertex, &index).is_none());
        assert_eq!(records.stats(0).2, 1);
    }

    #[test]
    fn released_ranges_are_given_out_again() {
        let Some(gpu) = device() else { return };
        let (vertex, index, _) = pages(
            &gpu,
            &[&Shape::Cuboid {
                size: [1.0, 1.0, 1.0],
            }
            .mesh()],
        );
        let mut records = Records::with_room(&gpu.device, 64, 64);
        let a = records.take(9, &vertex, &index).expect("room");
        let b = records.take(19, &vertex, &index).expect("room");
        assert_eq!(records.stats(0).0, 30);
        records.release(a.0, a.1);
        assert_eq!(
            records.take(9, &vertex, &index),
            Some(a),
            "the same place again"
        );
        records.release(a.0, a.1);
        records.release(b.0, b.1);
        assert_eq!(records.stats(0).0, 0);
        // The whole buffer, then nothing: no growth past the most.
        assert_eq!(records.take(63, &vertex, &index), Some((0, 64)));
        assert!(records.take(0, &vertex, &index).is_none());
        // A page that can't be bound whole gets no records.
        let unbindable = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test page without storage"),
            size: 64,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        assert!(records.take(1, &unbindable, &index).is_none());
        assert_eq!(records.stats(0).2, 2);
        // The need counts what is held (64) and what the scene's geometries miss.
        let need = (64 + 3) * RECORD_BYTES;
        assert_eq!(records.stats(3).3, need as f32 / (1u64 << 20) as f32);
    }
}
