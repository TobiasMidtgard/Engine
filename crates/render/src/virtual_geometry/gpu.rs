//! GPU side of virtual geometry ([`ClusterRenderer`]): the cluster data of every built mesh in
//! shared storage buffers, per-frame culling and level-of-detail selection in compute shaders,
//! and indirect draws of the visible clusters. Owned and driven by `MeshFeature`.
//!
//! ## Per frame
//!
//! 1. [`begin_frame`](ClusterRenderer::begin_frame) (prepare): finished background builds are
//!    uploaded, meshes nobody drew for a while are released, earlier frames' counters are read
//!    back (stats).
//! 2. [`push_instance`](ClusterRenderer::push_instance) for every entity drawn as clusters,
//!    then [`end_prepare`](ClusterRenderer::end_prepare): instances, the cull views (camera,
//!    shadow cascades, sky occlusion map) and reset indirect arguments are uploaded.
//! 3. [`encode_culling`](ClusterRenderer::encode_culling) (compute): `cluster_cull.wgsl`
//!    writes the visible clusters of every view into draw slots, each a visible list with its
//!    own indirect draw. The camera has one slot per bucket (sidedness and texture set), so
//!    each is drawn with its pipeline and textures; every shadow view has one slot.
//! 4. `draw_prepass`, `draw_scene`, `draw_shadow`: one `draw_indirect` per slot. The vertex
//!    stage fetches its triangle corner from the cluster buffers (`cluster_fetch.wgsl`).
//!
//! Mesh data lives in four pools (vertices, indices, clusters, levels) that grow by copying
//! on the GPU and hand out ranges to meshes, so adding a mesh never rebuilds the others. A
//! later streaming step can fill the same pools page by page.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread::JoinHandle;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use super::{CLUSTER_TRIANGLES, ClusterDag, Sphere, build};
use crate::features::{GpuShared, GrowBuffer};
use crate::frame::ViewData;
use crate::mesh::Mesh;
use crate::mesh_feature::MeshInstance;
use crate::renderer::layout_entry;
use crate::settings::RenderSettings;
use crate::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};

const CLUSTER_TYPES: &str = include_str!("../shaders/cluster_types.wgsl");
const CLUSTER_CULL: &str = include_str!("../shaders/cluster_cull.wgsl");
const CLUSTER_FETCH: &str = include_str!("../shaders/cluster_fetch.wgsl");
const CLUSTER_SCENE: &str = include_str!("../shaders/cluster.wgsl");
const CLUSTER_SHADOW: &str = include_str!("../shaders/cluster_shadow.wgsl");
const MESH: &str = include_str!("../shaders/mesh.wgsl");
const MESH_SHADOW: &str = include_str!("../shaders/mesh_shadow.wgsl");

/// Views culled every frame: the camera (0), the shadow cascades (1..=MAX_CASCADES), the sky
/// occlusion map (`1 + SKY_OCCLUSION_VIEW`) and the far shadow map
/// (`1 + FAR_SHADOW_CLUSTERS`). `VIEWS` in `cluster_cull.wgsl`.
const VIEWS: usize = 3 + MAX_CASCADES;
/// Shadow view index of the far shadow map (after the cascades and the sky occlusion map), for
/// [`ClusterRenderer::draw_shadow`].
pub const FAR_SHADOW_CLUSTERS: usize = SKY_OCCLUSION_VIEW + 1;
const _: () = assert!(VIEWS == 7 && FAR_SHADOW_CLUSTERS + 1 == VIEWS - 1);
/// Clusters per chunk (`CHUNK` in `cluster_cull.wgsl`).
const CHUNK: u32 = 64;
const WORKGROUP: u32 = 64;
/// Vertices drawn per visible cluster (`CLUSTER_VERTEX_COUNT`).
const CLUSTER_VERTICES: u32 = 3 * CLUSTER_TRIANGLES as u32;
/// `Draws` in `cluster_cull.wgsl`: a 128-byte header of counters, then one
/// `DrawIndirectArgs` per draw slot.
const HEADER_BYTES: u64 = 128;
const ARGS_BYTES: u64 = 16;
/// Header words read back for stats: chunks, overflow, two unused, then triangles and
/// clusters drawn per view.
const COUNTER_WORDS: usize = 4 + 2 * VIEWS;
const READBACKS: usize = 3;
/// Most visible clusters in one draw slot.
const MAX_SLOT_ENTRIES: u32 = 1 << 18;
/// Most chunks culled per frame (16 MB of chunk list).
const MAX_CHUNKS: u32 = 1 << 20;
/// Most instances culled per frame (one thread per view and instance, 65535 workgroups).
const MAX_INSTANCES: usize = 600_000;
/// Builds of large meshes running at once (each uses up to four threads). One more slot is
/// kept for meshes under `SMALL_BUILD_TRIANGLES`.
const MAX_LARGE_BUILDS: usize = 2;
/// Meshes under this many triangles build in well under a second (in release builds).
const SMALL_BUILD_TRIANGLES: usize = 200_000;
/// A mesh is built once its version has stayed the same for this many frames, so meshes
/// that change every frame don't keep restarting builds (they stay on the instanced path).
const SETTLE_FRAMES: u64 = 4;
/// Meshes nobody drew for this many frames give their memory back.
const EVICT_AFTER_FRAMES: u64 = 600;

/// The transform mirrors the mesh (`INSTANCE_MIRRORED` in `cluster_types.wgsl`).
const INSTANCE_MIRRORED: u32 = 1;
/// Normal cones are valid: single-sided, rigid, uniformly scaled.
const INSTANCE_CONE_CULL: u32 = 2;
const INSTANCE_CASTS_SHADOWS: u32 = 4;

/// `struct ClusterInstance` in `cluster_types.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuInstance {
    m: [[f32; 4]; 3],
    n: [[f32; 4]; 3],
    prev: [[f32; 4]; 3],
    extra: [u32; 4],
    sphere: [f32; 4],
    mesh: [u32; 4],
    lod: [f32; 4],
}

/// `struct Cluster` in `cluster_types.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuCluster {
    bounds: [f32; 4],
    lod_bounds: [f32; 4],
    parent_bounds: [f32; 4],
    cone: [f32; 4],
    errors: [f32; 4],
    data: [u32; 4],
}

/// `struct CullView` in `cluster_cull.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct CullView {
    planes: [[f32; 4]; 6],
    eye: [f32; 4],
    forward: [f32; 4],
    lod: [f32; 4],
    slots: [u32; 4],
}

/// `struct CullParams` in `cluster_cull.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct CullParams {
    counts: [u32; 4],
    views: [CullView; VIEWS],
}

/// One entity drawn as clusters this frame.
#[derive(Debug, Clone, Copy)]
pub struct ClusterInstance {
    /// Mesh asset index (see [`ClusterRenderer::ready`]).
    pub mesh: u32,
    /// Camera-relative model matrix.
    pub model: Mat4,
    /// Last frame's model matrix, relative to last frame's camera (motion vectors).
    pub prev_model: Mat4,
    /// Slot in the mesh feature's material buffer.
    pub material: u32,
    /// The mesh feature's texture set.
    pub textures: u32,
    pub double_sided: bool,
    pub casts_shadows: bool,
}

/// What the cluster path drew. Clusters and triangles are counted on the GPU and read back,
/// so they describe a frame a few frames ago.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct ClusterStats {
    /// Instances drawn as clusters this frame.
    pub instances: u32,
    /// Clusters and triangles the camera drew.
    pub clusters: u32,
    pub triangles: u64,
    /// Clusters and triangles drawn into the shadow cascades and the sky occlusion map.
    pub shadow_clusters: u32,
    pub shadow_triangles: u64,
    /// Clusters or chunks dropped because a list was full (0 unless the scene is huge).
    pub overflow: u32,
    /// Indirect draws issued this frame.
    pub draw_calls: u32,
}

/// Free ranges of a pool (start, length), sorted by start and merged; first fit.
#[derive(Debug, Clone, PartialEq)]
struct Ranges(Vec<(u64, u64)>);

impl Ranges {
    fn alloc(&mut self, len: u64) -> Option<u64> {
        let i = self.0.iter().position(|&(_, l)| l >= len)?;
        let (start, l) = self.0[i];
        if l == len {
            self.0.remove(i);
        } else {
            self.0[i] = (start + len, l - len);
        }
        Some(start)
    }

    fn release(&mut self, start: u64, len: u64) {
        if len == 0 {
            return;
        }
        let free = &mut self.0;
        let i = free.partition_point(|&(s, _)| s < start);
        free.insert(i, (start, len));
        if i + 1 < free.len() && start + len == free[i + 1].0 {
            free[i].1 += free[i + 1].1;
            free.remove(i + 1);
        }
        if i > 0 && free[i - 1].0 + free[i - 1].1 == start {
            free[i - 1].1 += free[i].1;
            free.remove(i);
        }
    }

    /// Length of the free range that ends at `end`, if any.
    fn tail(&self, end: u64) -> u64 {
        self.0
            .last()
            .filter(|(s, l)| s + l == end)
            .map_or(0, |(_, l)| *l)
    }
}

/// A storage buffer shared by every clustered mesh, handed out in ranges.
struct Pool {
    label: &'static str,
    buffer: wgpu::Buffer,
    stride: u64,
    /// Elements the buffer holds.
    capacity: u64,
    free: Ranges,
}

impl Pool {
    fn new(device: &wgpu::Device, label: &'static str, stride: u64, capacity: u64) -> Self {
        Self {
            label,
            buffer: pool_buffer(device, label, capacity * stride),
            stride,
            capacity,
            free: Ranges(vec![(0, capacity)]),
        }
    }

    fn release(&mut self, start: u64, len: u64) {
        self.free.release(start, len);
    }

    /// Allocate `len` elements, growing the buffer (its contents copied on the GPU) when no
    /// free range is large enough. None when the device can't hold a buffer that large.
    fn alloc_or_grow(&mut self, gpu: &GpuShared, len: u64, max_bytes: u64) -> Option<u64> {
        if let Some(start) = self.free.alloc(len) {
            return Some(start);
        }
        let needed = self.capacity + len - self.free.tail(self.capacity);
        let capacity = needed.max(self.capacity * 2).min(max_bytes / self.stride);
        if capacity < needed {
            return None;
        }
        let buffer = pool_buffer(&gpu.device, self.label, capacity * self.stride);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grow cluster pool"),
            });
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &buffer, 0, self.capacity * self.stride);
        gpu.queue.submit([encoder.finish()]);
        self.buffer = buffer;
        let old = self.capacity;
        self.capacity = capacity;
        self.free.release(old, capacity - old);
        self.free.alloc(len)
    }

    fn write(&self, queue: &wgpu::Queue, start: u64, data: &[u8]) {
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, start * self.stride, data);
        }
    }
}

fn pool_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(256),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// A built mesh on the GPU: where its data sits in the pools (start, length in elements).
struct GpuDag {
    vertices: (u64, u64),
    indices: (u64, u64),
    clusters: (u64, u64),
    levels: (u64, u64),
    bounds: Sphere,
    /// Mesh-space bottom and height (the instanced path's wind inputs).
    min_y: f32,
    height: f32,
    /// Chunks of [`CHUNK`] clusters over all its levels (sizes the chunk list).
    chunks: u32,
}

enum State {
    /// Waiting for a free build slot.
    Waiting,
    Building,
    Ready(GpuDag),
    /// The build failed or doesn't fit on the GPU; drawn by the instanced path.
    Failed,
}

struct Entry {
    version: u64,
    state: State,
    last_used: u64,
    /// Frame its version last changed (or it was first seen).
    changed: u64,
}

struct Build {
    mesh: u32,
    version: u64,
    /// Under `SMALL_BUILD_TRIANGLES`.
    small: bool,
    handle: JoinHandle<ClusterDag>,
}

const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// Composed sources of the cluster shaders, before `compose_shader`: (label, source, whether
/// they need the frame library). For shader validation.
pub fn shader_sources() -> [(&'static str, String, bool); 3] {
    [
        (
            "cluster_cull",
            [CLUSTER_TYPES, CLUSTER_CULL].join("\n"),
            false,
        ),
        (
            "cluster",
            [MESH, CLUSTER_TYPES, CLUSTER_FETCH, CLUSTER_SCENE].join("\n"),
            true,
        ),
        (
            "cluster_shadow",
            [MESH_SHADOW, CLUSTER_TYPES, CLUSTER_FETCH, CLUSTER_SHADOW].join("\n"),
            false,
        ),
    ]
}

/// Draws dense meshes as clusters: builds their cluster DAGs in the background, keeps them on
/// the GPU, culls and picks levels of detail per view on the GPU every frame, and draws the
/// result. See the module docs for the frame order.
pub struct ClusterRenderer {
    cull_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    cull_instances: wgpu::ComputePipeline,
    cull_dispatch: wgpu::ComputePipeline,
    cull_clusters: wgpu::ComputePipeline,
    cull_finish: wgpu::ComputePipeline,
    /// Scene and prepass pipelines, indexed by double-sided.
    scene: [wgpu::RenderPipeline; 2],
    prepass: [wgpu::RenderPipeline; 2],
    shadow: wgpu::RenderPipeline,

    vertices: Pool,
    indices: Pool,
    clusters: Pool,
    levels: Pool,
    /// Largest buffer the device can bind as storage.
    max_bytes: u64,
    instances: GrowBuffer,
    params: wgpu::Buffer,
    chunks: wgpu::Buffer,
    chunk_capacity: u32,
    draws: wgpu::Buffer,
    draw_slot_capacity: u32,
    visible: wgpu::Buffer,
    visible_capacity: u64,
    dispatch: wgpu::Buffer,
    dispatch_group: wgpu::BindGroup,
    /// Rebuilt when a buffer they bind is recreated.
    cull_group: Option<wgpu::BindGroup>,
    draw_group: Option<wgpu::BindGroup>,

    meshes: HashMap<u32, Entry>,
    running: Vec<Build>,
    frame: u64,

    pending: Vec<ClusterInstance>,
    gpu_instances: Vec<GpuInstance>,
    /// Camera buckets (double-sided, texture set); bucket i draws slot i.
    buckets: Vec<(bool, u32)>,
    /// Draw slot per shadow view (cascades, then the sky occlusion map), when it is on.
    shadow_slots: [Option<u32>; VIEWS - 1],
    /// Something to cull and draw this frame.
    active: bool,
    slots: u32,

    readbacks: Vec<Readback>,
    /// Readback this frame's commands write.
    written: Option<usize>,
    stats: ClusterStats,
}

impl ClusterRenderer {
    /// Create the pipelines. `material_layout` and `texture_layout` are the mesh feature's
    /// groups 1 and 2, shared with the instanced path.
    pub fn new(
        gpu: &GpuShared,
        material_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let device = &gpu.device;
        let storage = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let compute = wgpu::ShaderStages::COMPUTE;
        let cull_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cluster cull"),
            entries: &[
                layout_entry(
                    0,
                    compute,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(1, compute, storage(true)),
                layout_entry(2, compute, storage(true)),
                layout_entry(3, compute, storage(true)),
                layout_entry(4, compute, storage(false)),
                layout_entry(5, compute, storage(false)),
                layout_entry(6, compute, storage(false)),
            ],
        });
        let dispatch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cluster dispatch"),
            entries: &[layout_entry(0, compute, storage(false))],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cluster draw"),
            entries: &(0..5)
                .map(|b| layout_entry(b, wgpu::ShaderStages::VERTEX, storage(true)))
                .collect::<Vec<_>>(),
        });

        let [cull_source, scene_source, shadow_source] = shader_sources();
        let cull_shader = gpu.shader("cluster cull", &cull_source.1, false);
        let with_dispatch = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cluster cull"),
            bind_group_layouts: &[Some(&cull_layout), Some(&dispatch_layout)],
            immediate_size: 0,
        });
        // The cluster pass dispatches indirectly from the dispatch buffer, so it must not bind
        // it as storage.
        let without_dispatch = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cluster cull"),
            bind_group_layouts: &[Some(&cull_layout)],
            immediate_size: 0,
        });
        let compute_pipeline = |layout: &wgpu::PipelineLayout, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                module: &cull_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let cull_instances = compute_pipeline(&with_dispatch, "cs_instances");
        let cull_dispatch = compute_pipeline(&with_dispatch, "cs_dispatch");
        let cull_clusters = compute_pipeline(&without_dispatch, "cs_clusters");
        let cull_finish = compute_pipeline(&without_dispatch, "cs_finish");

        let scene_shader = gpu.shader("cluster", &scene_source.1, true);
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cluster"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(material_layout),
                Some(texture_layout),
                Some(&draw_layout),
            ],
            immediate_size: 0,
        });
        let prepass_targets = gpu.prepass_targets();
        let scene_targets = [Some(wgpu::ColorTargetState {
            format: gpu.formats.hdr,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        // Mirrored instances swap two corners of every triangle (cluster_fetch.wgsl), so front
        // faces are always counter-clockwise.
        let render_pipeline = |double: bool, prepass: bool| {
            let (vs, fs, targets): (_, _, &[Option<wgpu::ColorTargetState>]) = if prepass {
                ("vs_cluster_prepass", "fs_prepass", &prepass_targets)
            } else {
                ("vs_cluster", "fs_main", &scene_targets)
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if prepass {
                    "cluster prepass"
                } else {
                    "cluster"
                }),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState {
                    module: &scene_shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: if double { None } else { Some(wgpu::Face::Back) },
                    ..Default::default()
                },
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &scene_shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let scene = [render_pipeline(false, false), render_pipeline(true, false)];
        let prepass = [render_pipeline(false, true), render_pipeline(true, true)];

        let shadow_shader = gpu.shader("cluster shadow", &shadow_source.1, false);
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cluster shadow"),
            bind_group_layouts: &[
                Some(&gpu.shadow_view_layout),
                Some(material_layout),
                None,
                Some(&draw_layout),
            ],
            immediate_size: 0,
        });
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cluster shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_cluster_shadow"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
                ..Default::default()
            },
            depth_stencil: Some(gpu.shadow_depth_state()),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let limits = device.limits();
        let max_bytes = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let dispatch = buffer("cluster dispatch", 16, U::STORAGE | U::INDIRECT);
        let dispatch_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cluster dispatch"),
            layout: &dispatch_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: dispatch.as_entire_binding(),
            }],
        });
        let readbacks = (0..READBACKS)
            .map(|_| Readback {
                buffer: buffer(
                    "cluster counters readback",
                    (COUNTER_WORDS * 4) as u64,
                    U::MAP_READ | U::COPY_DST,
                ),
                state: Arc::new(AtomicU8::new(FREE)),
            })
            .collect();
        Self {
            cull_layout,
            draw_layout,
            cull_instances,
            cull_dispatch,
            cull_clusters,
            cull_finish,
            scene,
            prepass,
            shadow,
            vertices: Pool::new(device, "cluster vertices", 48, 4096),
            indices: Pool::new(device, "cluster indices", 4, 16384),
            clusters: Pool::new(device, "clusters", 96, 256),
            levels: Pool::new(device, "cluster levels", 16, 64),
            max_bytes,
            instances: GrowBuffer::new(
                device,
                "cluster instances",
                U::STORAGE,
                64 * std::mem::size_of::<GpuInstance>() as u64,
            ),
            params: buffer(
                "cluster cull params",
                std::mem::size_of::<CullParams>() as u64,
                U::UNIFORM | U::COPY_DST,
            ),
            chunks: buffer("cluster chunks", 16 * 1024, U::STORAGE),
            chunk_capacity: 1024,
            draws: buffer(
                "cluster draws",
                HEADER_BYTES + ARGS_BYTES * 16,
                U::STORAGE | U::INDIRECT | U::COPY_DST | U::COPY_SRC,
            ),
            draw_slot_capacity: 16,
            visible: buffer("visible clusters", 8 * 4096, U::STORAGE),
            visible_capacity: 4096,
            dispatch,
            dispatch_group,
            cull_group: None,
            draw_group: None,
            meshes: HashMap::new(),
            running: Vec::new(),
            frame: 0,
            pending: Vec::new(),
            gpu_instances: Vec::new(),
            buckets: Vec::new(),
            shadow_slots: [None; VIEWS - 1],
            active: false,
            slots: 0,
            readbacks,
            written: None,
            stats: ClusterStats::default(),
        }
    }

    /// Start of a frame's prepare: upload finished builds, release unused meshes, collect
    /// counters from earlier frames.
    pub fn begin_frame(&mut self, gpu: &GpuShared) {
        self.frame += 1;
        self.pending.clear();
        self.active = false;

        // Finished builds.
        let mut i = 0;
        while i < self.running.len() {
            if !self.running[i].handle.is_finished() {
                i += 1;
                continue;
            }
            let b = self.running.swap_remove(i);
            let result = b.handle.join();
            let Some(entry) = self.meshes.get_mut(&b.mesh) else {
                continue;
            };
            if entry.version != b.version || !matches!(entry.state, State::Building) {
                continue; // the mesh changed while it was being built
            }
            entry.state = match result {
                Ok(dag) => match upload(
                    gpu,
                    [
                        &mut self.vertices,
                        &mut self.indices,
                        &mut self.clusters,
                        &mut self.levels,
                    ],
                    self.max_bytes,
                    &dag,
                ) {
                    Some(gpu_dag) => {
                        // Pools may have been recreated.
                        self.cull_group = None;
                        self.draw_group = None;
                        State::Ready(gpu_dag)
                    }
                    None => {
                        tracing::warn!(
                            "mesh {} ({} clusters) does not fit in GPU memory; drawing it \
                             without virtual geometry",
                            b.mesh,
                            dag.clusters.len()
                        );
                        State::Failed
                    }
                },
                Err(_) => {
                    tracing::error!(
                        "building clusters of mesh {} failed; drawing it without virtual geometry",
                        b.mesh
                    );
                    State::Failed
                }
            };
        }

        // Meshes nobody drew for a while.
        if self.frame.is_multiple_of(120) {
            let frame = self.frame;
            let stale: Vec<u32> = self
                .meshes
                .iter()
                .filter(|(_, e)| e.last_used + EVICT_AFTER_FRAMES < frame)
                .map(|(m, _)| *m)
                .collect();
            for m in stale {
                if let Some(e) = self.meshes.remove(&m) {
                    self.release(e.state);
                }
            }
        }

        // Counters: map last frame's readback (its commands are submitted by now) and read
        // any that finished.
        if let Some(slot) = self.written.take() {
            let rb = &self.readbacks[slot];
            rb.state.store(MAPPING, Ordering::Release);
            let state = rb.state.clone();
            rb.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
                });
        }
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        for rb in &self.readbacks {
            match rb.state.load(Ordering::Acquire) {
                MAPPED => {
                    if let Ok(view) = rb.buffer.slice(..).get_mapped_range() {
                        let words: &[u32] = bytemuck::cast_slice(&view);
                        self.stats.overflow = words[1];
                        let triangles = &words[4..4 + VIEWS];
                        let clusters = &words[4 + VIEWS..4 + 2 * VIEWS];
                        self.stats.triangles = triangles[0] as u64;
                        self.stats.clusters = clusters[0];
                        self.stats.shadow_triangles =
                            triangles[1..].iter().map(|&t| t as u64).sum();
                        self.stats.shadow_clusters = clusters[1..].iter().sum();
                    }
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                FAILED => {
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
    }

    /// Whether mesh `mesh` (asset index) at `version` can be drawn as clusters. The first call
    /// for a mesh (or a new version of it) starts building its clusters in the background;
    /// until they are on the GPU this returns false and the mesh should be drawn as usual.
    pub fn ready(&mut self, mesh: u32, version: u64, data: &Mesh) -> bool {
        let frame = self.frame;
        let entry = self.meshes.entry(mesh).or_insert(Entry {
            version,
            state: State::Waiting,
            last_used: frame,
            changed: frame,
        });
        entry.last_used = frame;
        if entry.version != version {
            // Only this mesh's clusters are dropped and rebuilt; nothing else moves.
            let old = std::mem::replace(&mut entry.state, State::Waiting);
            entry.version = version;
            entry.changed = frame;
            release_dag(
                [
                    &mut self.vertices,
                    &mut self.indices,
                    &mut self.clusters,
                    &mut self.levels,
                ],
                old,
            );
        }
        // Small meshes (a damaged wall being rebuilt) have a build slot of their own, so they
        // never wait behind large builds.
        let small = data.indices.len() / 3 < SMALL_BUILD_TRIANGLES;
        let large_running = self.running.iter().filter(|b| !b.small).count();
        let slot_free = self.running.len() < MAX_LARGE_BUILDS + 1
            && (small || large_running < MAX_LARGE_BUILDS);
        // A mesh that changes every frame (deforming, being edited) is never built.
        let settled = frame >= entry.changed + SETTLE_FRAMES;
        match entry.state {
            State::Ready(_) => true,
            State::Waiting if slot_free && settled => {
                let copy = data.clone();
                match std::thread::Builder::new()
                    .name("cluster build".into())
                    .spawn(move || build(&copy))
                {
                    Ok(handle) => {
                        entry.state = State::Building;
                        self.running.push(Build {
                            mesh,
                            version,
                            small,
                            handle,
                        });
                    }
                    Err(e) => {
                        tracing::error!("could not start a cluster build thread: {e}");
                        entry.state = State::Failed;
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// Mesh-space bounding sphere of a mesh [`ready`](Self::ready) to be drawn as clusters.
    pub fn bounds(&self, mesh: u32) -> Option<Sphere> {
        match self.meshes.get(&mesh) {
            Some(Entry {
                state: State::Ready(dag),
                ..
            }) => Some(dag.bounds),
            _ => None,
        }
    }

    /// Draw an instance of a mesh [`ready`](Self::ready) said yes to this frame.
    pub fn push_instance(&mut self, instance: ClusterInstance) {
        self.pending.push(instance);
    }

    fn release(&mut self, state: State) {
        release_dag(
            [
                &mut self.vertices,
                &mut self.indices,
                &mut self.clusters,
                &mut self.levels,
            ],
            state,
        );
    }

    /// Upload this frame's instances and cull views (end of the mesh feature's prepare).
    pub fn end_prepare(&mut self, gpu: &GpuShared, view: &ViewData, settings: &RenderSettings) {
        self.active = false;
        self.gpu_instances.clear();
        self.buckets.clear();
        self.shadow_slots = [None; VIEWS - 1];
        self.stats.instances = 0;
        self.stats.draw_calls = 0;
        if self.pending.len() > MAX_INSTANCES {
            tracing::warn!(
                "{} clustered instances; drawing the first {MAX_INSTANCES}",
                self.pending.len()
            );
            self.pending.truncate(MAX_INSTANCES);
        }

        // Shadow views that are on.
        let mut shadow_views = [false; VIEWS - 1];
        if view.sun_shadows {
            for v in shadow_views
                .iter_mut()
                .take((view.cascades.count as usize).min(MAX_CASCADES))
            {
                *v = true;
            }
        }
        shadow_views[SKY_OCCLUSION_VIEW] = view.sky_occlusion.enabled;
        shadow_views[FAR_SHADOW_CLUSTERS] = view.sun_shadows && view.far_shadow.enabled;
        let shadow_count = shadow_views.iter().filter(|v| **v).count() as u64;

        let mut bucket_of: HashMap<(bool, u32), u32> = HashMap::new();
        let mut entries = 0u64;
        let mut chunks = 0u64;
        for inst in &self.pending {
            let Some(Entry {
                state: State::Ready(dag),
                ..
            }) = self.meshes.get(&inst.mesh)
            else {
                continue;
            };
            let key = (inst.double_sided, inst.textures);
            let bucket = *bucket_of.entry(key).or_insert_with(|| {
                self.buckets.push(key);
                self.buckets.len() as u32 - 1
            });
            let model = inst.model;
            let axes = [model.x_axis, model.y_axis, model.z_axis].map(|a| a.truncate());
            let lengths = axes.map(Vec3::length);
            let scale = lengths[0].max(lengths[1]).max(lengths[2]);
            // Normal cones survive rotation and uniform scale (not shear or stretch).
            let conformal = lengths[0].min(lengths[1]).min(lengths[2]) >= scale * 0.99
                && [(0, 1), (1, 2), (0, 2)]
                    .iter()
                    .all(|&(a, b)| axes[a].dot(axes[b]).abs() <= 0.01 * scale * scale);
            let mut flags = 0;
            if Mat3::from_mat4(model).determinant() < 0.0 {
                flags |= INSTANCE_MIRRORED;
            }
            if !inst.double_sided && conformal {
                flags |= INSTANCE_CONE_CULL;
            }
            let views = 1 + if inst.casts_shadows {
                flags |= INSTANCE_CASTS_SHADOWS;
                shadow_count
            } else {
                0
            };
            let base = MeshInstance::new(model, inst.material, dag.min_y, dag.height)
                .with_previous(inst.prev_model);
            let center = model.transform_point3(dag.bounds.center);
            self.gpu_instances.push(GpuInstance {
                m: base.m,
                n: base.n,
                prev: base.prev,
                extra: [inst.material, flags, base.extra[2], base.extra[3]],
                sphere: [center.x, center.y, center.z, dag.bounds.radius * scale],
                mesh: [dag.levels.0 as u32, dag.levels.1 as u32, bucket, 0],
                lod: [scale, 0.0, 0.0, 0.0],
            });
            entries += dag.clusters.1;
            chunks += dag.chunks as u64 * views;
        }
        self.stats.instances = self.gpu_instances.len() as u32;
        if self.gpu_instances.is_empty() {
            self.stats.clusters = 0;
            self.stats.triangles = 0;
            self.stats.shadow_clusters = 0;
            self.stats.shadow_triangles = 0;
            self.stats.overflow = 0;
            return;
        }

        // Draw slots: the camera's buckets, then one per shadow view that is on.
        let mut slots = self.buckets.len() as u32;
        for (v, on) in shadow_views.iter().enumerate() {
            if *on {
                self.shadow_slots[v] = Some(slots);
                slots += 1;
            }
        }
        // Every slot's list holds as many clusters as the instances have, within the limits
        // of a draw's vertex index (slot * entries * vertices < 2^32) and of the device.
        let per_slot = (entries.next_power_of_two().min(MAX_SLOT_ENTRIES as u64) as u32)
            .min(u32::MAX / CLUSTER_VERTICES / slots)
            .min((self.max_bytes / 8 / slots as u64) as u32)
            .max(1);
        let device = &gpu.device;
        use wgpu::BufferUsages as U;
        let visible = slots as u64 * per_slot as u64;
        if visible > self.visible_capacity {
            self.visible_capacity = visible.next_power_of_two().min(self.max_bytes / 8);
            self.visible = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("visible clusters"),
                size: self.visible_capacity * 8,
                usage: U::STORAGE,
                mapped_at_creation: false,
            });
            self.cull_group = None;
            self.draw_group = None;
        }
        let chunk_capacity = (chunks.min(MAX_CHUNKS as u64) as u32).max(1);
        if chunk_capacity > self.chunk_capacity {
            self.chunk_capacity = chunk_capacity.next_power_of_two().min(MAX_CHUNKS);
            self.chunks = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cluster chunks"),
                size: self.chunk_capacity as u64 * 16,
                usage: U::STORAGE,
                mapped_at_creation: false,
            });
            self.cull_group = None;
        }
        if slots > self.draw_slot_capacity {
            self.draw_slot_capacity = slots.next_power_of_two();
            self.draws = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cluster draws"),
                size: HEADER_BYTES + ARGS_BYTES * self.draw_slot_capacity as u64,
                usage: U::STORAGE | U::INDIRECT | U::COPY_DST | U::COPY_SRC,
                mapped_at_creation: false,
            });
            self.cull_group = None;
        }
        if self.instances.write(
            device,
            &gpu.queue,
            bytemuck::cast_slice(&self.gpu_instances),
        ) {
            self.cull_group = None;
            self.draw_group = None;
        }

        // Cull views.
        let mut views = [CullView::zeroed(); VIEWS];
        let camera = &mut views[0];
        for (k, p) in view.frustum.planes.iter().enumerate() {
            // Degenerate planes (an infinite far plane) keep everything.
            camera.planes[k] = if p.normal.length_squared() < 0.5 {
                [0.0, 0.0, 0.0, 1.0]
            } else {
                [p.normal.x, p.normal.y, p.normal.z, p.d]
            };
        }
        let forward = view.forward();
        camera.eye = [0.0, 0.0, 0.0, if view.orthographic { 0.0 } else { 1.0 }];
        camera.forward = [forward.x, forward.y, forward.z, 1.0];
        // Pixels per meter at 1 m (perspective) or anywhere (orthographic).
        camera.lod = [
            view.unjittered_proj.y_axis.y * view.height as f32 * 0.5,
            settings.cluster_error_px.max(1e-3),
            view.near.max(1e-3),
            0.0,
        ];
        camera.slots = [0, 0, per_slot, 1];
        let shadow_threshold = settings.cluster_shadow_error_texels.max(1e-3);
        for (v, slot) in self.shadow_slots.iter().enumerate() {
            let Some(slot) = *slot else { continue };
            let cull = &mut views[1 + v];
            // The far map keeps only casters big enough to show at its resolution.
            let (view_proj, texels_per_meter, near, min_radius) = if v == SKY_OCCLUSION_VIEW {
                let s = &view.sky_occlusion;
                (
                    s.view_proj,
                    s.map_size as f32 / s.extent.max(1e-3),
                    true,
                    0.0,
                )
            } else if v == FAR_SHADOW_CLUSTERS {
                let f = &view.far_shadow;
                (
                    f.view_proj,
                    1.0 / f.texel().max(1e-3),
                    false,
                    f.min_caster_radius,
                )
            } else {
                let c = &view.cascades;
                let half = c.half_extent[v].max(1e-3);
                (c.view_proj[v], c.map_size as f32 / (2.0 * half), false, 0.0)
            };
            cull.planes = ortho_planes(&view_proj, near);
            cull.lod = [texels_per_meter, shadow_threshold, 1.0, min_radius];
            cull.slots = [slot, 1, per_slot, 1];
        }
        let params = CullParams {
            counts: [
                self.gpu_instances.len() as u32,
                self.chunk_capacity,
                slots,
                self.buckets.len() as u32,
            ],
            views,
        };
        gpu.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));

        // Reset the counters and every slot's draw: CLUSTER_VERTICES vertices per visible
        // cluster, starting at the slot's list.
        let mut draws = vec![0u32; (HEADER_BYTES / 4) as usize];
        for slot in 0..slots {
            draws.extend([CLUSTER_VERTICES, 0, slot * per_slot * CLUSTER_VERTICES, 0]);
        }
        gpu.queue
            .write_buffer(&self.draws, 0, bytemuck::cast_slice(&draws));

        if self.cull_group.is_none() {
            let entries: [&wgpu::Buffer; 7] = [
                &self.params,
                &self.instances.buffer,
                &self.clusters.buffer,
                &self.levels.buffer,
                &self.chunks,
                &self.draws,
                &self.visible,
            ];
            self.cull_group = Some(bind_buffers(
                device,
                "cluster cull",
                &self.cull_layout,
                &entries,
            ));
        }
        if self.draw_group.is_none() {
            let entries: [&wgpu::Buffer; 5] = [
                &self.vertices.buffer,
                &self.indices.buffer,
                &self.clusters.buffer,
                &self.instances.buffer,
                &self.visible,
            ];
            self.draw_group = Some(bind_buffers(
                device,
                "cluster draw",
                &self.draw_layout,
                &entries,
            ));
        }
        self.slots = slots;
        self.active = true;
        // Prepass and scene per bucket, one per shadow view.
        self.stats.draw_calls = 2 * self.buckets.len() as u32 + shadow_count as u32;
    }

    /// Cull every view and fill the draw slots (the mesh feature's compute).
    pub fn encode_culling(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.active {
            return;
        }
        let Some(cull) = &self.cull_group else {
            return;
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cluster culling: instances"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.cull_instances);
            pass.set_bind_group(0, cull, &[]);
            pass.set_bind_group(1, &self.dispatch_group, &[]);
            let threads = VIEWS as u32 * self.gpu_instances.len() as u32;
            pass.dispatch_workgroups(threads.div_ceil(WORKGROUP), 1, 1);
            pass.set_pipeline(&self.cull_dispatch);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cluster culling: clusters"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.cull_clusters);
            pass.set_bind_group(0, cull, &[]);
            pass.dispatch_workgroups_indirect(&self.dispatch, 0);
            pass.set_pipeline(&self.cull_finish);
            pass.dispatch_workgroups(self.slots.div_ceil(WORKGROUP), 1, 1);
        }
        if let Some(slot) = self
            .readbacks
            .iter()
            .position(|r| r.state.load(Ordering::Acquire) == FREE)
        {
            let rb = &self.readbacks[slot];
            encoder.copy_buffer_to_buffer(
                &self.draws,
                0,
                &rb.buffer,
                0,
                (COUNTER_WORDS * 4) as u64,
            );
            rb.state.store(IN_FLIGHT, Ordering::Release);
            self.written = Some(slot);
        }
    }

    fn draw_camera<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        pipelines: &'a [wgpu::RenderPipeline; 2],
        textures: impl Fn(u32) -> Option<&'a wgpu::BindGroup>,
    ) {
        let Some(group) = self.draw_group.as_ref().filter(|_| self.active) else {
            return;
        };
        for (slot, &(double, set)) in self.buckets.iter().enumerate() {
            pass.set_pipeline(&pipelines[double as usize]);
            if let Some(t) = textures(set) {
                pass.set_bind_group(2, t, &[]);
            }
            pass.set_bind_group(3, group, &[]);
            pass.draw_indirect(&self.draws, HEADER_BYTES + ARGS_BYTES * slot as u64);
        }
    }

    /// Draw the camera's clusters into the depth prepass (group 1, the materials, bound).
    /// `textures` gives the bind group of a texture set.
    pub fn draw_prepass<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        textures: impl Fn(u32) -> Option<&'a wgpu::BindGroup>,
    ) {
        self.draw_camera(pass, &self.prepass, textures);
    }

    /// Draw the camera's clusters into the scene (opaque phase; group 1 bound).
    pub fn draw_scene<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        textures: impl Fn(u32) -> Option<&'a wgpu::BindGroup>,
    ) {
        self.draw_camera(pass, &self.scene, textures);
    }

    /// Draw the clusters of shadow cascade `cascade`, of the sky occlusion map when it is
    /// `SKY_OCCLUSION_VIEW`, or of the far shadow map when it is [`FAR_SHADOW_CLUSTERS`] (group 1
    /// bound).
    pub fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize) {
        let Some(group) = self.draw_group.as_ref().filter(|_| self.active) else {
            return;
        };
        let Some(Some(slot)) = self.shadow_slots.get(cascade) else {
            return;
        };
        pass.set_pipeline(&self.shadow);
        pass.set_bind_group(3, group, &[]);
        pass.draw_indirect(&self.draws, HEADER_BYTES + ARGS_BYTES * *slot as u64);
    }

    /// Whether anything is drawn as clusters this frame.
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn stats(&self) -> ClusterStats {
        self.stats
    }

    /// State for `render.stats`.
    pub fn describe(&self) -> serde_json::Value {
        let count = |f: fn(&State) -> bool| self.meshes.values().filter(|e| f(&e.state)).count();
        let pool_bytes: u64 = [&self.vertices, &self.indices, &self.clusters, &self.levels]
            .iter()
            .map(|p| p.capacity * p.stride)
            .sum();
        serde_json::json!({
            "meshes_ready": count(|s| matches!(s, State::Ready(_))),
            "meshes_building": count(|s| matches!(s, State::Building | State::Waiting)),
            "meshes_failed": count(|s| matches!(s, State::Failed)),
            "stats": self.stats,
            "pool_bytes": pool_bytes,
            "buckets": self.buckets.len(),
        })
    }
}

/// Planes of an orthographic light view-projection (standard Z), camera-relative: left, right,
/// bottom, top, far, and near only when `near` (shadow cascades keep casters toward the sun:
/// they are pancaked onto the near plane).
fn ortho_planes(m: &Mat4, near: bool) -> [[f32; 4]; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    let normalize = |v: Vec4| {
        let len = v.truncate().length();
        if len < 1e-12 {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            (v / len).to_array()
        }
    };
    [
        normalize(r3 + r0),
        normalize(r3 - r0),
        normalize(r3 + r1),
        normalize(r3 - r1),
        normalize(r3 - r2),
        if near {
            normalize(r2)
        } else {
            [0.0, 0.0, 0.0, 1.0]
        },
    ]
}

fn bind_buffers(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    buffers: &[&wgpu::Buffer],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

/// Copy a built DAG into the pools (vertices, indices, clusters, levels). None (with nothing
/// allocated) when it doesn't fit.
fn upload(
    gpu: &GpuShared,
    pools: [&mut Pool; 4],
    max_bytes: u64,
    dag: &ClusterDag,
) -> Option<GpuDag> {
    if dag.clusters.is_empty() {
        return None;
    }
    let lens = [
        dag.vertices.len() as u64,
        dag.indices.len() as u64,
        dag.clusters.len() as u64,
        dag.levels.len() as u64,
    ];
    let mut starts = [0u64; 4];
    for i in 0..4 {
        match pools[i].alloc_or_grow(gpu, lens[i], max_bytes) {
            // Pool positions are 32-bit in the shaders.
            Some(s) if s + lens[i] <= u32::MAX as u64 => starts[i] = s,
            other => {
                if let Some(s) = other {
                    pools[i].release(s, lens[i]);
                }
                for j in 0..i {
                    pools[j].release(starts[j], lens[j]);
                }
                return None;
            }
        }
    }
    let [vertex_base, index_base, cluster_base, _] = starts.map(|s| s as u32);
    let gpu_clusters: Vec<GpuCluster> = dag
        .clusters
        .iter()
        .map(|c| GpuCluster {
            bounds: c.bounds.to_array(),
            lod_bounds: c.lod_bounds.to_array(),
            parent_bounds: c.parent_bounds.to_array(),
            cone: [c.cone_axis.x, c.cone_axis.y, c.cone_axis.z, c.cone_cos],
            errors: [c.error, c.parent_error, 0.0, 0.0],
            data: [
                index_base + c.index_offset,
                vertex_base,
                c.vertex_count | c.triangle_count << 16,
                c.level,
            ],
        })
        .collect();
    let gpu_levels: Vec<[u32; 4]> = dag
        .levels
        .iter()
        .map(|l| {
            [
                cluster_base + l.first,
                l.count,
                l.min_error.to_bits(),
                l.max_parent_error.to_bits(),
            ]
        })
        .collect();
    let queue = &gpu.queue;
    pools[0].write(queue, starts[0], bytemuck::cast_slice(&dag.vertices));
    pools[1].write(queue, starts[1], bytemuck::cast_slice(&dag.indices));
    pools[2].write(queue, starts[2], bytemuck::cast_slice(&gpu_clusters));
    pools[3].write(queue, starts[3], bytemuck::cast_slice(&gpu_levels));
    let (min_y, max_y) = dag
        .vertices
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v.position[1]), hi.max(v.position[1]))
        });
    Some(GpuDag {
        vertices: (starts[0], lens[0]),
        indices: (starts[1], lens[1]),
        clusters: (starts[2], lens[2]),
        levels: (starts[3], lens[3]),
        bounds: dag.bounds,
        min_y,
        height: (max_y - min_y).max(1e-3),
        chunks: dag.levels.iter().map(|l| l.count.div_ceil(CHUNK)).sum(),
    })
}

fn release_dag(pools: [&mut Pool; 4], state: State) {
    if let State::Ready(dag) = state {
        let [vertices, indices, clusters, levels] = pools;
        vertices.release(dag.vertices.0, dag.vertices.1);
        indices.release(dag.indices.0, dag.indices.1);
        clusters.release(dag.clusters.0, dag.clusters.1);
        levels.release(dag.levels.0, dag.levels.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_structs_match_the_shaders() {
        assert_eq!(std::mem::size_of::<GpuInstance>(), 208);
        assert_eq!(std::mem::size_of::<GpuCluster>(), 96);
        assert_eq!(std::mem::size_of::<CullView>(), 160);
        assert_eq!(std::mem::size_of::<CullParams>(), 16 + 160 * VIEWS);
        assert_eq!(CLUSTER_VERTICES, 384);
    }

    #[test]
    fn pool_ranges_are_reused_and_merged() {
        let mut free = Ranges(vec![(0, 100)]);
        assert_eq!(free.alloc(30), Some(0));
        assert_eq!(free.alloc(30), Some(30));
        assert_eq!(free.alloc(30), Some(60));
        assert_eq!(free.alloc(30), None);
        free.release(30, 30);
        assert_eq!(free.alloc(40), None, "{free:?}");
        assert_eq!(free.tail(100), 10);
        free.release(0, 30);
        free.release(60, 30);
        // Everything merged back into one range.
        assert_eq!(free, Ranges(vec![(0, 100)]));
        assert_eq!(free.alloc(100), Some(0));
        assert_eq!(free.tail(100), 0);
    }
}
