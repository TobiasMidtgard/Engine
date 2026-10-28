//! Probe GI: dynamic diffuse global illumination (DDGI, Majercik et al. 2019) without
//! ray-tracing hardware. A grid of probes scrolls with the camera; each frame a share of them
//! shoots rays against a software hierarchy of the meshes around (`bvh.rs`): one per mesh, built
//! when it first shows up (large meshes on a worker thread), and one over the instances near
//! the grid. A hit
//! is lit by the sun and the point lights through shadow rays, by its own glow and by last
//! frame's probes (so light bounces more than once over frames); a miss sees the environment
//! map. `cs_update` blends the rays into each probe's octahedral irradiance and distance maps
//! (the atlas, frame binding 23), and `probe_gi` in lib_frame.wgsl lights every surface from
//! the eight probes around it, with DDGI's visibility weights so light doesn't leak through
//! walls. See /engine-design/dynamic-gi.md in the project files for the research behind it.
//!
//! Made cheap enough to be on by default (/engine-design/world-gi-on.md): the instances are
//! gathered again only when a mesh entity changes (by change ticks), a mesh or material asset
//! changes, or the camera leaves the snapped region; they stay relative to an anchor that moves
//! in 64 m steps, so a moving camera leaves them as they are. The hierarchies are packed in one
//! store that only appends what's new. Each frame traces the probes that fit
//! `RenderSettings::gi_budget_ms` at the cost per ray timestamps measured, and probes with
//! nothing near sleep (Majercik et al. 2021).
//!
//! The same scene gives world reflections (`encode_reflections`, `RenderSettings::
//! world_reflections`): what screen-space reflections can't see is traced at half resolution and
//! lit like a probe ray's hit (see /engine-design/lighting-round-reflections.md). Rays see a
//! textured surface in its texture's mean color once the texture was uploaded.
//!
//! Meshes only for now ([`crate::MeshRenderer`]); terrain, foliage and water are not in the
//! rays' scene yet, and rays that miss see the environment map's ground below the horizon.

mod bvh;

use std::collections::HashMap;

use bevy_ecs::change_detection::Tick;
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::{GpuShared, GrowBuffer};
use crate::frame::{FrameUniform, ViewData};
use crate::material::{AlphaMode, StandardMaterial};
use crate::mesh::Mesh;
use crate::settings::RenderSettings;
use engine_assets::Assets;

/// Texels along a probe's tile side (`PROBE_GI_TEXELS` in lib_frame.wgsl).
const TEXELS: u32 = 8;
/// Tiles per atlas row (`PROBE_GI_TILES_X`).
const TILES_X: u32 = 128;
/// Probes the atlas holds: 1024 × 1024 texels, two layers (16 MB).
pub(crate) const MAX_PROBES: u32 = TILES_X * TILES_X;
/// Rays per probe at most (`MAX_RAYS` in probe_gi.wgsl).
const MAX_RAYS: u32 = 256;
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Triangles whose hierarchies may be built per frame on the render thread (at least one mesh
/// is).
const BUILD_TRIANGLES_PER_FRAME: usize = 60_000;
/// Meshes with more triangles than this build on a worker thread instead.
const INLINE_BUILD_TRIANGLES: usize = 20_000;
/// Worker builds at once.
const MAX_BUILD_JOBS: usize = 2;
/// Meters between the anchors instances are kept relative to.
const ANCHOR_CELL: f64 = 64.0;
/// Meters the region instances are gathered from snaps to.
const REGION_SNAP: f64 = 8.0;
/// Frames between gathers of a scene that didn't change (texture colors arrive late).
const REGATHER_FRAMES: u64 = 240;
/// Probes a frame at least under a time budget.
const MIN_PROBES_PER_FRAME: u32 = 128;
/// Probes a frame under a time budget before timestamps measured their cost.
const UNMEASURED_PROBES_PER_FRAME: u32 = 512;
/// Frames a mesh's hierarchy stays after its last use.
const KEEP_UNUSED_FRAMES: u64 = 600;

/// `GiPass` in probe_gi.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GiPass {
    counts: [u32; 4],
    rotation: [[f32; 4]; 3],
    params: [f32; 4],
    world_origin: [i32; 4],
    anchor: [f32; 4],
}

/// `Instance` in probe_gi.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuInstance {
    to_local: [[f32; 4]; 3],
    mesh: [u32; 4],
    albedo: [f32; 4],
    emissive: [f32; 4],
}

/// A mesh's hierarchy: where it sits in the shared buffers ([`BlasStore`]).
struct MeshEntry {
    version: u64,
    root: u32,
    first_triangle: u32,
    /// Its nodes and triangle corners in the store.
    nodes: u32,
    corners: u32,
    last_used: u64,
    /// In the store (false: left out for the GPU's binding limit).
    uploaded: bool,
    /// No triangles: nothing to hit.
    empty: bool,
}

/// A hierarchy building on a worker thread.
struct BuildJob {
    version: u64,
    job: std::thread::JoinHandle<bvh::MeshBvh>,
}

/// Every mesh's hierarchy, packed one after another as the GPU reads them (node and triangle
/// indices inside one are relative to its first, so hierarchies can move). New ones append, and
/// only what was appended is uploaded; forgotten ones leave gaps until a compaction.
#[derive(Default)]
struct BlasStore {
    nodes: Vec<bvh::Node>,
    /// Three corners per triangle.
    corners: Vec<[f32; 4]>,
    /// Nodes and corners of forgotten hierarchies.
    dead_nodes: usize,
    dead_corners: usize,
    /// How much of each the GPU holds: a prefix, the rest is new.
    gpu_nodes: usize,
    gpu_corners: usize,
    /// Bumped when hierarchies move (a compaction): gather the instances again.
    layout: u64,
}

impl BlasStore {
    /// Append a hierarchy: its first node and first triangle, or `None` past `limit` bytes.
    fn add(&mut self, bvh: &bvh::MeshBvh, limit: u64) -> Option<(u32, u32)> {
        let nodes = (self.nodes.len() + bvh.nodes.len()) * size_of::<bvh::Node>();
        let corners = (self.corners.len() + bvh.triangles.len()) * 16;
        if nodes as u64 > limit || corners as u64 > limit {
            return None;
        }
        let at = (self.nodes.len() as u32, (self.corners.len() / 3) as u32);
        self.nodes.extend_from_slice(&bvh.nodes);
        self.corners.extend_from_slice(&bvh.triangles);
        Some(at)
    }

    /// Close the gaps: copy every stored hierarchy down, in `meshes` order.
    fn compact(&mut self, meshes: &mut HashMap<u32, MeshEntry>) {
        let mut nodes = Vec::with_capacity(self.nodes.len() - self.dead_nodes);
        let mut corners = Vec::with_capacity(self.corners.len() - self.dead_corners);
        for e in meshes.values_mut().filter(|e| e.uploaded) {
            let n = e.root as usize;
            let c = e.first_triangle as usize * 3;
            e.root = nodes.len() as u32;
            e.first_triangle = (corners.len() / 3) as u32;
            nodes.extend_from_slice(&self.nodes[n..n + e.nodes as usize]);
            corners.extend_from_slice(&self.corners[c..c + e.corners as usize]);
        }
        self.nodes = nodes;
        self.corners = corners;
        self.dead_nodes = 0;
        self.dead_corners = 0;
        self.gpu_nodes = 0;
        self.gpu_corners = 0;
        self.layout += 1;
    }

    /// Upload what the GPU doesn't hold yet; true when a buffer was recreated.
    fn upload(&mut self, active: &mut Active, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let mut regroup = false;
        if self.gpu_nodes < self.nodes.len() {
            regroup |= append(&mut active.blas, device, queue, &self.nodes, self.gpu_nodes);
            self.gpu_nodes = self.nodes.len();
        }
        if self.gpu_corners < self.corners.len() {
            regroup |= append(
                &mut active.triangles,
                device,
                queue,
                &self.corners,
                self.gpu_corners,
            );
            self.gpu_corners = self.corners.len();
        }
        regroup
    }
}

/// Write `data[held..]` after the `held` items `buffer` holds, or all of it where it doesn't fit;
/// true when the buffer was recreated.
fn append<T: Pod>(
    buffer: &mut GrowBuffer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &[T],
    held: usize,
) -> bool {
    let bytes: &[u8] = bytemuck::cast_slice(data);
    if held == 0 || bytes.len() as u64 > buffer.capacity() {
        return buffer.write(device, queue, bytes);
    }
    let from = held * size_of::<T>();
    queue.write_buffer(&buffer.buffer, from as u64, &bytes[from..]);
    false
}

/// What the instances were last gathered for: gather again when any of it changes.
#[derive(Clone, Copy, PartialEq)]
struct GatherKey {
    /// The anchor's cell and the region (in `REGION_SNAP` cells).
    anchor: IVec3,
    region: (IVec3, IVec3),
    /// The mesh and material assets' latest versions and removals.
    meshes: (u64, u64),
    materials: (u64, u64),
    layout: u64,
}

/// The instances near the grid as last gathered (on the GPU with their hierarchy).
struct Gathered {
    key: GatherKey,
    frame: u64,
    instances: u32,
    /// Meshes were left for later (the frame's build budget, busy workers): gather again.
    incomplete: bool,
}

/// What the instance gather reads; only change ticks while nothing changed.
type GatherQuery = (
    Ref<'static, GlobalTransform>,
    Ref<'static, crate::MeshRenderer>,
    Ref<'static, crate::ResolvedMesh>,
);
/// Skinned meshes are left out: the rays would see their bind pose.
type GatherFilter = Without<crate::GpuSkin>;

/// The probes' placement: `dims` probes `spacing` apart from world probe `origin` (probe `i` of
/// the world lattice sits at `(i + 0.5) * spacing`).
#[derive(Clone, Copy, PartialEq)]
struct Grid {
    dims: UVec3,
    spacing: f64,
    origin: IVec3,
}

impl Grid {
    fn count(&self) -> u32 {
        self.dims.x * self.dims.y * self.dims.z
    }

    /// Storage slot of grid coordinate `g` (the storage wraps around as the grid scrolls).
    fn slot(&self, g: UVec3) -> u32 {
        let o = self.origin.rem_euclid(self.dims.as_ivec3()).as_uvec3();
        let c = (g + o) % self.dims;
        c.x + self.dims.x * (c.y + self.dims.y * c.z)
    }

    fn contains(&self, world: IVec3) -> bool {
        let g = world - self.origin;
        g.cmpge(IVec3::ZERO).all() && g.cmplt(self.dims.as_ivec3()).all()
    }
}

/// What `encode` records this frame.
struct Work {
    probes: u32,
    rays: u32,
    /// Atlas tile rows written, as (first, count) ranges.
    rows: Vec<(u32, u32)>,
}

/// GPU resources, made when probe GI first turns on.
struct Active {
    trace: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    /// World reflections (`cs_reflect`) and their inputs and output (group 2).
    reflect: wgpu::ComputePipeline,
    reflect_layout: wgpu::BindGroupLayout,
    layout: wgpu::BindGroupLayout,
    /// What `cs_update` writes; copied into the atlas the frame samples.
    next: wgpu::Texture,
    next_view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    probe_state: wgpu::Buffer,
    tlas: GrowBuffer,
    instances: GrowBuffer,
    blas: GrowBuffer,
    triangles: GrowBuffer,
    rays: GrowBuffer,
    updates: GrowBuffer,
    group: Option<wgpu::BindGroup>,
}

/// Probe GI's state, owned by the renderer for the main view (extra views go without).
pub(crate) struct ProbeGi {
    atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    active: Option<Active>,
    meshes: HashMap<u32, MeshEntry>,
    store: BlasStore,
    /// Hierarchies of large meshes building on workers, by mesh index.
    jobs: HashMap<u32, BuildJob>,
    /// Each mesh's local bounds by version, to leave meshes away from the grid unbuilt.
    bounds: HashMap<u32, (u64, (Vec3, Vec3))>,
    /// Triangles left out for the binding limit (warned once).
    left_out_warned: bool,
    gathered: Option<Gathered>,
    query: Option<QueryState<GatherQuery, GatherFilter>>,
    /// The world's change tick at the last gather check, and the mesh entities it found.
    tick: Option<Tick>,
    entities: usize,
    /// Measured GPU milliseconds per thousand probe rays (0: not yet), and rays traced a frame
    /// lately (a moving average, as the measurements arrive a few frames late).
    ms_per_kray: f32,
    rays_lately: f32,
    grid: Option<Grid>,
    /// Bumped when every probe's history is void (a jump, a new grid): `probe_gi` ignores probes
    /// last written in another epoch.
    epoch: u32,
    /// The next slot the round robin updates.
    cursor: u32,
    frame: u64,
    work: Option<Work>,
    stats: Stats,
}

#[derive(Default, Clone, Copy)]
struct Stats {
    probes: u32,
    instances: u32,
    meshes: u32,
    built_triangles: u32,
    building: u32,
    gathered: bool,
    build_ms: f32,
    extract_ms: f32,
}

fn atlas_texture(device: &wgpu::Device, size: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe gi atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage,
        view_formats: &[],
    })
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// Diffuse albedo and glow (cd/m², not pre-exposed) of a material, as the probes' rays see it.
/// Albedo and emissive (cd/m^2) a ray sees on `m`; `texture_mean`: the mean color of its base
/// color texture, when known.
fn material_light(m: &StandardMaterial, texture_mean: Option<Vec3>) -> (Vec3, Vec3) {
    let tint = Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]);
    let albedo = match m.preset {
        Some(p) => tint * Vec3::from(p.mean_albedo()),
        // A texture's mean before it is uploaded: about half of the tint.
        None if m.base_color_texture.is_some() => {
            tint * texture_mean.unwrap_or(Vec3::splat(0.5)) * (1.0 - m.metallic.clamp(0.0, 1.0))
        }
        None => tint * (1.0 - m.metallic.clamp(0.0, 1.0)),
    };
    let emissive = Vec3::from(m.emissive) * m.emissive_strength;
    (albedo.min(Vec3::splat(0.95)), emissive)
}

/// The world box of a mesh's local box under `affine`, relative to `origin`.
fn world_box(local: (Vec3, Vec3), affine: &DAffine3, origin: DVec3) -> (Vec3, Vec3) {
    let (lo, hi) = (local.0.as_dvec3(), local.1.as_dvec3());
    let center = affine.transform_point3((lo + hi) * 0.5) - origin;
    let half = (hi - lo) * 0.5;
    let m = affine.matrix3;
    let extent = DVec3::new(
        m.row(0).abs().dot(half),
        m.row(1).abs().dot(half),
        m.row(2).abs().dot(half),
    );
    ((center - extent).as_vec3(), (center + extent).as_vec3())
}

/// This frame's ray rotation (DDGI turns the ray set randomly every frame so the probes see
/// every direction over time).
fn rotation(frame: u64) -> Mat3 {
    let mut h = frame.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        (h >> 40) as f32 / (1u64 << 24) as f32
    };
    // A uniform random rotation (Shoemake 1992).
    let (u1, u2, u3) = (next(), next(), next());
    let (a, b) = ((1.0 - u1).sqrt(), u1.sqrt());
    let q = Quat::from_xyzw(
        a * (std::f32::consts::TAU * u2).sin(),
        a * (std::f32::consts::TAU * u2).cos(),
        b * (std::f32::consts::TAU * u3).sin(),
        b * (std::f32::consts::TAU * u3).cos(),
    );
    Mat3::from_quat(q.normalize())
}

impl ProbeGi {
    pub fn new(device: &wgpu::Device) -> Self {
        // A one-tile stand-in until probe GI turns on.
        let atlas = atlas_texture(
            device,
            TEXELS,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let atlas_view = array_view(&atlas);
        Self {
            atlas,
            atlas_view,
            active: None,
            meshes: HashMap::new(),
            store: BlasStore::default(),
            jobs: HashMap::new(),
            bounds: HashMap::new(),
            left_out_warned: false,
            gathered: None,
            query: None,
            tick: None,
            entities: 0,
            ms_per_kray: 0.0,
            rays_lately: 0.0,
            grid: None,
            epoch: 1,
            cursor: 0,
            frame: 0,
            work: None,
            stats: Stats::default(),
        }
    }

    /// The atlas frame binding 23 samples.
    pub fn atlas_view(&self) -> &wgpu::TextureView {
        &self.atlas_view
    }

    fn activate(&mut self, shared: &GpuShared) {
        let device = &shared.device;
        let size = TEXELS * TILES_X;
        self.atlas = atlas_texture(
            device,
            size,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        self.atlas_view = array_view(&self.atlas);
        let next = atlas_texture(
            device,
            size,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let next_view = array_view(&next);
        use crate::renderer::layout_entry;
        use wgpu::ShaderStages as S;
        let storage = |read_only: bool| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe gi"),
            entries: &[
                layout_entry(
                    0,
                    S::COMPUTE,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(1, S::COMPUTE, storage(true)),
                layout_entry(2, S::COMPUTE, storage(true)),
                layout_entry(3, S::COMPUTE, storage(true)),
                layout_entry(4, S::COMPUTE, storage(true)),
                layout_entry(5, S::COMPUTE, storage(false)),
                layout_entry(6, S::COMPUTE, storage(false)),
                layout_entry(
                    7,
                    S::COMPUTE,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: ATLAS_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                ),
                layout_entry(8, S::COMPUTE, storage(true)),
            ],
        });
        let reflect_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world reflection inputs"),
            entries: &[
                layout_entry(
                    0,
                    S::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    1,
                    S::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    2,
                    S::COMPUTE,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        });
        let module = shared.shader("probe gi", include_str!("../shaders/probe_gi.wgsl"), true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe gi"),
            bind_group_layouts: &[Some(&shared.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let reflect_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("world reflections"),
                bind_group_layouts: &[
                    Some(&shared.frame_layout),
                    Some(&layout),
                    Some(&reflect_layout),
                ],
                immediate_size: 0,
            });
        let pipeline = |entry: &str, layout: &wgpu::PipelineLayout| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let usage = wgpu::BufferUsages::STORAGE;
        self.active = Some(Active {
            trace: pipeline("cs_trace", &pipeline_layout),
            update: pipeline("cs_update", &pipeline_layout),
            reflect: pipeline("cs_reflect", &reflect_pipeline_layout),
            reflect_layout,
            layout,
            next,
            next_view,
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe gi pass"),
                size: std::mem::size_of::<GiPass>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            probe_state: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe gi state"),
                size: MAX_PROBES as u64 * 16,
                usage,
                mapped_at_creation: false,
            }),
            tlas: GrowBuffer::new(device, "probe gi tlas", usage, 4096),
            instances: GrowBuffer::new(device, "probe gi instances", usage, 4096),
            blas: GrowBuffer::new(device, "probe gi blas", usage, 1 << 16),
            triangles: GrowBuffer::new(device, "probe gi triangles", usage, 1 << 16),
            rays: GrowBuffer::new(device, "probe gi rays", usage, 1 << 20),
            updates: GrowBuffer::new(device, "probe gi updates", usage, 4096),
            group: None,
        });
        // The new buffers hold nothing yet.
        self.store.gpu_nodes = 0;
        self.store.gpu_corners = 0;
        self.gathered = None;
        self.grid = None;
    }

    /// A GPU time for the latest probe pass the timestamps measured (milliseconds).
    pub fn measured(&mut self, ms: f32) {
        if self.rays_lately < 1000.0 || !ms.is_finite() || ms <= 0.0 {
            return;
        }
        let sample = (ms / (self.rays_lately / 1000.0)).clamp(1e-5, 100.0);
        self.ms_per_kray = if self.ms_per_kray > 0.0 {
            self.ms_per_kray * 0.8 + sample * 0.2
        } else {
            sample
        };
    }

    /// Probes to trace this frame: what fits `gi_budget_ms` at the measured cost, up to
    /// `gi_probes_per_frame` (always that many without a budget, timestamps or a hardware GPU).
    fn probes_this_frame(&self, shared: &GpuShared, settings: &RenderSettings, count: u32) -> u32 {
        let most = settings.gi_probes_per_frame.clamp(1, count);
        let budgeted = settings.gi_budget_ms > 0.0
            && shared.adapter_info.device_type != wgpu::DeviceType::Cpu
            && shared
                .device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY);
        if !budgeted {
            return most;
        }
        let least = MIN_PROBES_PER_FRAME.min(most);
        if self.ms_per_kray <= 0.0 {
            return UNMEASURED_PROBES_PER_FRAME.clamp(least, most);
        }
        let rays = settings.gi_rays_per_probe.clamp(16, MAX_RAYS) as f32;
        let fit = settings.gi_budget_ms / self.ms_per_kray * 1000.0 / rays;
        (fit as u32).clamp(least, most)
    }

    /// Store the hierarchies the workers finished; true when any did.
    fn collect_builds(&mut self, limit: u64) -> bool {
        let done: Vec<u32> = self
            .jobs
            .iter()
            .filter(|(_, j)| j.job.is_finished())
            .map(|(k, _)| *k)
            .collect();
        for key in &done {
            let Some(BuildJob { version, job }) = self.jobs.remove(key) else {
                continue;
            };
            match job.join() {
                Ok(bvh) => self.add_mesh(*key, version, bvh, limit),
                Err(_) => tracing::warn!("probe GI: building a mesh's hierarchy failed"),
            }
        }
        !done.is_empty()
    }

    /// Store mesh `key`'s hierarchy (replacing an older version's).
    fn add_mesh(&mut self, key: u32, version: u64, bvh: bvh::MeshBvh, limit: u64) {
        if let Some(old) = self.meshes.remove(&key) {
            self.forget(&old);
        }
        let root = bvh.nodes[0];
        let empty = root.count == 0 && root.min[0] > root.max[0];
        let at = self.store.add(&bvh, limit);
        if at.is_none() && !self.left_out_warned {
            self.left_out_warned = true;
            tracing::warn!(
                "probe GI: the meshes near the grid don't fit one GPU binding ({} MB); the rays \
                 miss some",
                limit >> 20
            );
        }
        let (root, first_triangle) = at.unwrap_or((0, 0));
        self.meshes.insert(
            key,
            MeshEntry {
                version,
                root,
                first_triangle,
                nodes: bvh.nodes.len() as u32,
                corners: bvh.triangles.len() as u32,
                last_used: self.frame,
                uploaded: at.is_some(),
                empty,
            },
        );
    }

    fn forget(&mut self, e: &MeshEntry) {
        if e.uploaded {
            self.store.dead_nodes += e.nodes as usize;
            self.store.dead_corners += e.corners as usize;
        }
    }

    /// Forget the hierarchies of meshes no gather used in a while, and close the gaps once
    /// they're most of the store.
    fn forget_unused(&mut self) {
        let frame = self.frame;
        let gone: Vec<u32> = self
            .meshes
            .iter()
            .filter(|(_, e)| frame - e.last_used >= KEEP_UNUSED_FRAMES)
            .map(|(k, _)| *k)
            .collect();
        for key in gone {
            if let Some(e) = self.meshes.remove(&key) {
                self.forget(&e);
            }
        }
        let store = &self.store;
        if store.dead_nodes > 4096 && store.dead_nodes * 2 > store.nodes.len() {
            self.store.compact(&mut self.meshes);
        }
        if self.bounds.len() > 4 * self.meshes.len() + 1024 {
            let meshes = &self.meshes;
            self.bounds.retain(|k, _| meshes.contains_key(k));
        }
    }

    /// The main view's half on the main thread: place the grid, gather the instances near it,
    /// build what hierarchies are missing, upload, and fill the frame uniform's probe fields.
    /// Returns true when the atlas was recreated (rebuild the frame groups).
    pub fn extract(
        &mut self,
        world: &mut World,
        shared: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        uniform: &mut FrameUniform,
    ) -> bool {
        self.work = None;
        self.frame += 1;
        if !settings.probe_gi {
            if self.active.is_some() {
                // Off: free the GPU memory, keep a one-tile stand-in.
                *self = Self::new(&shared.device);
                return true;
            }
            return false;
        }
        let started = std::time::Instant::now();
        let mut recreated = false;
        if self.active.is_none() {
            self.activate(shared);
            recreated = true;
        }

        // The grid, centered on the camera and snapped to whole probes.
        let spacing = settings.gi_probe_spacing.max(0.1) as f64;
        let mut dims = UVec3::from(settings.gi_probe_grid).max(UVec3::splat(2));
        while dims.x * dims.y * dims.z > MAX_PROBES {
            dims = (dims / 2).max(UVec3::splat(2));
        }
        let center = (view.origin / spacing).floor();
        let origin = center.as_ivec3() - (dims / 2).as_ivec3();
        let grid = Grid {
            dims,
            spacing,
            origin,
        };
        let count = grid.count();
        let per_frame = self.probes_this_frame(shared, settings, count);
        let rays = settings.gi_rays_per_probe.clamp(16, MAX_RAYS);

        // Probes that scrolled in go first, then the rest in turn.
        let mut updates: Vec<u32> = Vec::with_capacity(per_frame as usize);
        match self.grid {
            Some(old) if old.dims == dims && old.spacing == spacing => {
                if old.origin != origin {
                    for z in 0..dims.z {
                        for y in 0..dims.y {
                            for x in 0..dims.x {
                                let g = UVec3::new(x, y, z);
                                if !old.contains(origin + g.as_ivec3()) {
                                    updates.push(grid.slot(g));
                                }
                            }
                        }
                    }
                    if updates.len() > per_frame as usize {
                        // A jump: every probe starts over.
                        updates.clear();
                        self.epoch += 1;
                    }
                }
            }
            _ => self.epoch += 1,
        }
        if self.epoch >= 2048 {
            self.epoch = 1;
        }
        self.grid = Some(grid);
        while updates.len() < per_frame as usize {
            updates.push(self.cursor % count);
            self.cursor = (self.cursor + 1) % count;
        }

        // Hierarchies the workers finished.
        let limit = shared.device.limits().max_storage_buffer_binding_size;
        let mut scene_changed = self.collect_builds(limit);

        // Instances are kept relative to an anchor that moves in whole cells, and gathered from a
        // region snapped outward, so a moving camera changes neither most frames.
        let anchor_cell = (view.origin / ANCHOR_CELL).floor();
        let anchor = anchor_cell * ANCHOR_CELL;
        // Probes sit between whole multiples of the spacing: level design snaps walls and floors
        // to whole meters, and a probe on a surface sees through it along its plane.
        let first_probe = (origin.as_dvec3() + 0.5) * spacing;
        let rel_origin = first_probe - view.origin;
        let margin = spacing * 16.0;
        let lo = ((first_probe - margin) / REGION_SNAP).floor();
        let hi = ((first_probe + (dims - 1).as_dvec3() * spacing + margin) / REGION_SNAP).ceil();
        let region = (
            (lo * REGION_SNAP - anchor).as_vec3(),
            (hi * REGION_SNAP - anchor).as_vec3(),
        );
        // Which mesh entities changed, from their change ticks alone.
        let mut query = self.query.take().unwrap_or_else(|| world.query_filtered());
        let (touched, entities) = match self.tick {
            Some(tick) => world.last_change_tick_scope(tick, |w| {
                let mut touched = false;
                let mut n = 0usize;
                for (transform, renderer, resolved) in query.iter(w) {
                    n += 1;
                    touched |=
                        transform.is_changed() || renderer.is_changed() || resolved.is_changed();
                }
                (touched, n)
            }),
            None => (true, 0),
        };
        // Changes from here on get a newer tick, so the next frame finds them.
        self.tick = Some(world.increment_change_tick());
        scene_changed |= touched || entities != self.entities;
        self.entities = entities;
        let world: &World = world;
        let meshes = world.get_resource::<Assets<Mesh>>();
        let materials = world.get_resource::<Assets<StandardMaterial>>();
        let mut key = GatherKey {
            anchor: anchor_cell.as_ivec3(),
            region: (lo.as_ivec3(), hi.as_ivec3()),
            meshes: meshes.map_or((0, 0), |m| (m.latest_version(), m.removals())),
            materials: materials.map_or((0, 0), |m| (m.latest_version(), m.removals())),
            layout: self.store.layout,
        };

        let regather = scene_changed
            || self.gathered.as_ref().is_none_or(|g| {
                g.key != key || g.incomplete || self.frame >= g.frame + REGATHER_FRAMES
            });

        let build_started = std::time::Instant::now();
        if regather {
            // Before gathering: a compaction moves hierarchies, so it can't come after.
            self.forget_unused();
            key.layout = self.store.layout;
        }
        let mut built = 0usize;
        let mut found: Vec<(GpuInstance, (Vec3, Vec3))> = Vec::new();
        let mut incomplete = false;
        if regather && let Some(meshes) = meshes {
            let images = world.get_resource::<Assets<crate::Image>>();
            let default_material = StandardMaterial::default();
            for (transform, renderer, resolved) in query.iter(world) {
                if !renderer.visible {
                    continue;
                }
                let material = materials
                    .and_then(|m| m.get(resolved.material))
                    .unwrap_or(&default_material);
                if material.unlit || matches!(material.alpha_mode, AlphaMode::Blend) {
                    continue;
                }
                let handle = meshes.canonical(resolved.mesh);
                let key = handle.index();
                let version = meshes.version(handle);
                // Only meshes near the grid are built and uploaded.
                let local = match self.bounds.get(&key) {
                    Some(&(v, b)) if v == version => b,
                    _ => {
                        let Some(mesh) = meshes.get(handle) else {
                            continue;
                        };
                        let aabb = mesh.aabb();
                        let b = (aabb.min, aabb.max);
                        self.bounds.insert(key, (version, b));
                        b
                    }
                };
                if local.0.cmpgt(local.1).any() {
                    continue;
                }
                let bounds = world_box(local, &transform.0, anchor);
                if bounds.1.cmplt(region.0).any() || bounds.0.cmpgt(region.1).any() {
                    continue;
                }
                let current = self.meshes.get(&key).is_some_and(|e| e.version == version);
                if !current {
                    if self.jobs.get(&key).is_some_and(|j| j.version == version) {
                        // Still building.
                        continue;
                    }
                    let Some(mesh) = meshes.get(handle) else {
                        continue;
                    };
                    let triangles = mesh.indices.len() / 3;
                    let positions: Vec<[f32; 3]> =
                        mesh.vertices.iter().map(|v| v.position).collect();
                    if triangles > INLINE_BUILD_TRIANGLES {
                        // Large meshes build on a worker; the rays miss them until it's done.
                        if self.jobs.len() < MAX_BUILD_JOBS {
                            let indices = mesh.indices.clone();
                            match std::thread::Builder::new()
                                .name("probe gi bvh".into())
                                .spawn(move || bvh::build_mesh(&positions, &indices))
                            {
                                Ok(job) => {
                                    self.jobs.insert(key, BuildJob { version, job });
                                }
                                Err(e) => tracing::warn!("probe GI: no worker for a mesh: {e}"),
                            }
                        } else {
                            incomplete = true;
                        }
                        continue;
                    }
                    if built > 0 && built + triangles > BUILD_TRIANGLES_PER_FRAME {
                        incomplete = true;
                        continue;
                    }
                    built += triangles;
                    let bvh = bvh::build_mesh(&positions, &mesh.indices);
                    self.add_mesh(key, version, bvh, limit);
                }
                let Some(entry) = self.meshes.get_mut(&key) else {
                    continue;
                };
                entry.last_used = self.frame;
                if !entry.uploaded || entry.empty {
                    continue;
                }
                // Anchor-relative -> mesh.
                let mut rel = transform.0;
                rel.translation -= anchor;
                let inv = rel.inverse();
                let m = inv.matrix3.as_mat3();
                let t = inv.translation.as_vec3();
                let row = |i: usize| {
                    let r = m.row(i);
                    [r.x, r.y, r.z, t[i]]
                };
                let texture_mean = match (&material.base_color_texture, images) {
                    (Some(handle), Some(images)) => {
                        shared.images.mean_color(images, *handle).map(Vec3::from)
                    }
                    _ => None,
                };
                let (albedo, emissive) = material_light(material, texture_mean);
                found.push((
                    GpuInstance {
                        to_local: [row(0), row(1), row(2)],
                        mesh: [
                            entry.root,
                            entry.first_triangle,
                            material.double_sided as u32,
                            key,
                        ],
                        albedo: albedo.extend(0.0).to_array(),
                        emissive: emissive.extend(0.0).to_array(),
                    },
                    bounds,
                ));
            }
        }
        self.query = Some(query);
        let build_ms = build_started.elapsed().as_secs_f32() * 1000.0;

        let active = self.active.as_mut().expect("activated above");
        let device = &shared.device;
        let queue = &shared.queue;
        let mut regroup = recreated || active.group.is_none();
        regroup |= self.store.upload(active, device, queue);
        if regather {
            let boxes: Vec<(Vec3, Vec3)> = found.iter().map(|(_, b)| *b).collect();
            let (tlas, order) = bvh::build(&boxes);
            let mut instances: Vec<GpuInstance> =
                order.iter().map(|&i| found[i as usize].0).collect();
            let count = instances.len();
            if instances.is_empty() {
                instances.push(GpuInstance::zeroed());
            }
            regroup |= active
                .tlas
                .write(device, queue, bytemuck::cast_slice(&tlas));
            regroup |= active
                .instances
                .write(device, queue, bytemuck::cast_slice(&instances));
            self.gathered = Some(Gathered {
                key,
                frame: self.frame,
                instances: count as u32,
                incomplete,
            });
        }
        let instance_count = self.gathered.as_ref().map_or(0, |g| g.instances);
        regroup |= active
            .updates
            .write(device, queue, bytemuck::cast_slice(&updates));
        regroup |= active
            .rays
            .reserve(device, updates.len() as u64 * rays as u64 * 16);
        let r = rotation(self.frame);
        let pass = GiPass {
            counts: [updates.len() as u32, rays, 0, count],
            rotation: [0, 1, 2].map(|i| r.row(i).extend(0.0).to_array()),
            params: [
                settings.gi_hysteresis.clamp(0.0, 0.995),
                50.0,
                instance_count as f32,
                settings.gi_shadowed_lights as f32,
            ],
            world_origin: origin.extend(0).to_array(),
            anchor: (anchor - view.origin).as_vec3().extend(0.0).to_array(),
        };
        queue.write_buffer(&active.uniform, 0, bytemuck::bytes_of(&pass));
        if regroup {
            fn buffer(b: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
                b.as_entire_binding()
            }
            active.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe gi"),
                layout: &active.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer(&active.uniform),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffer(&active.tlas.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer(&active.instances.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffer(&active.blas.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: buffer(&active.triangles.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: buffer(&active.rays.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: buffer(&active.probe_state),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&active.next_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: buffer(&active.updates.buffer),
                    },
                ],
            }));
        }

        // The atlas rows the update writes, merged into ranges.
        let mut rows: Vec<u32> = updates.iter().map(|s| s / TILES_X).collect();
        rows.sort_unstable();
        rows.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for row in rows {
            match ranges.last_mut() {
                Some((first, n)) if *first + *n == row => *n += 1,
                _ => ranges.push((row, 1)),
            }
        }
        self.work = Some(Work {
            probes: updates.len() as u32,
            rays,
            rows: ranges,
        });

        let offset = origin.rem_euclid(dims.as_ivec3()).as_vec3();
        uniform.gi_volume = [
            rel_origin.x as f32,
            rel_origin.y as f32,
            rel_origin.z as f32,
            spacing as f32,
        ];
        uniform.gi_size = [dims.x as f32, dims.y as f32, dims.z as f32, 1.0];
        uniform.gi_grid = [offset.x, offset.y, offset.z, self.epoch as f32];
        let rays_now = (updates.len() as u32 * rays) as f32;
        self.rays_lately = if self.rays_lately > 0.0 {
            self.rays_lately * 0.7 + rays_now * 0.3
        } else {
            rays_now
        };
        self.stats = Stats {
            probes: updates.len() as u32,
            instances: instance_count,
            meshes: self.meshes.len() as u32,
            built_triangles: built as u32,
            building: self.jobs.len() as u32,
            gathered: regather,
            build_ms,
            extract_ms: started.elapsed().as_secs_f32() * 1000.0,
        };
        recreated
    }

    /// Trace and update this frame's probes (after the environment map, before anything shades
    /// with them). `frame_group`: the view's scene group (it binds the atlas being read).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, frame_group: &wgpu::BindGroup) {
        let (Some(work), Some(active)) = (self.work.take(), self.active.as_ref()) else {
            return;
        };
        let Some(group) = &active.group else {
            return;
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("probe gi"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.set_pipeline(&active.trace);
            pass.dispatch_workgroups((work.probes * work.rays).div_ceil(64), 1, 1);
            pass.set_pipeline(&active.update);
            pass.dispatch_workgroups(work.probes, 1, 1);
        }
        let size = TEXELS * TILES_X;
        for (first, n) in work.rows {
            let origin = wgpu::Origin3d {
                x: 0,
                y: first * TEXELS,
                z: 0,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &active.next,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: size,
                    height: n * TEXELS,
                    depth_or_array_layers: 2,
                },
            );
        }
    }

    /// Trace world reflections into `targets.world_reflection` from the prepass (after it, before
    /// screen-space reflections). False, writing nothing, while probe GI is off or has no scene
    /// yet: the target then holds no reflections only if the caller cleared it.
    pub fn encode_reflections(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        targets: &crate::targets::Targets,
    ) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        let Some(group) = &active.group else {
            return false;
        };
        let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("world reflection inputs"),
            layout: &active.reflect_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&targets.prepass_normal.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&targets.prepass_depth.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&targets.world_reflection.view),
                },
            ],
        });
        let size = targets.world_reflection.texture.size();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("world reflections"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&active.reflect);
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(1, group, &[]);
        pass.set_bind_group(2, &inputs, &[]);
        pass.dispatch_workgroups(size.width.div_ceil(8), size.height.div_ceil(8), 1);
        true
    }

    /// `render.gi.*` diagnostics.
    pub fn publish(&self, d: &mut Diagnostics) {
        let on = self.active.is_some();
        d.set("render.gi.on", on as u8 as f64);
        if !on {
            return;
        }
        let s = self.stats;
        d.set("render.gi.probes_updated", s.probes as f64);
        d.set(
            "render.gi.probes",
            self.grid.map_or(0, |g| g.count()) as f64,
        );
        d.set("render.gi.instances", s.instances as f64);
        d.set("render.gi.meshes", s.meshes as f64);
        d.set("render.gi.built_triangles", s.built_triangles as f64);
        d.set("render.gi.building", s.building as f64);
        d.set("render.gi.gathered", s.gathered as u8 as f64);
        d.set("render.gi.gpu_ms_per_kray", self.ms_per_kray as f64);
        d.set("render.gi.build_ms", s.build_ms as f64);
        d.set("render.gi.extract_ms", s.extract_ms as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_slots_wrap_and_stay_with_their_probe_as_it_scrolls() {
        let a = Grid {
            dims: UVec3::new(4, 3, 5),
            spacing: 1.0,
            origin: IVec3::new(-7, 2, 11),
        };
        let b = Grid {
            origin: a.origin + IVec3::new(1, -1, 2),
            ..a
        };
        let mut seen = std::collections::HashSet::new();
        for z in 0..5 {
            for y in 0..3 {
                for x in 0..4 {
                    let g = UVec3::new(x, y, z);
                    assert!(seen.insert(a.slot(g)), "slots are unique");
                    // A probe both grids hold keeps its slot.
                    let world = a.origin + g.as_ivec3();
                    if b.contains(world) {
                        let gb = (world - b.origin).as_uvec3();
                        assert_eq!(a.slot(g), b.slot(gb));
                    }
                }
            }
        }
        assert_eq!(seen.len() as u32, a.count());
    }

    #[test]
    fn shader_validates() {
        let code = crate::features::compose_shader(include_str!("../shaders/probe_gi.wgsl"), true);
        crate::features::validate_wgsl("probe gi", &code).unwrap();
    }

    #[test]
    fn ray_rotations_are_rotations() {
        for frame in 0..20 {
            let r = rotation(frame);
            assert!((r.determinant() - 1.0).abs() < 1e-4);
            assert!((r * r.transpose()).abs_diff_eq(Mat3::IDENTITY, 1e-4));
        }
        assert!(!rotation(1).abs_diff_eq(rotation(2), 1e-3));
    }
}
