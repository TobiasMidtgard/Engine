//! Probe GI: dynamic diffuse global illumination (DDGI, Majercik et al. 2019). A grid of probes
//! scrolls with the camera; each frame a share of them shoots rays against the meshes around. On
//! a GPU with ray queries ([`GpuShared::ray_queries`], `RenderSettings::gi_hardware_rays`) the
//! GPU builds a structure per mesh and one over the instances near the grid, and its
//! ray-tracing cores walk them (`rt.rs`). Elsewhere the rays walk a software hierarchy
//! (`bvh.rs`): one per mesh, built when it first shows up (large meshes on a worker thread), and
//! one over the instances near the grid. Both paths see the same triangles. A hit
//! is lit by the sun and the point lights through shadow rays, by its own glow and by last
//! frame's probes (so light bounces more than once over frames); a miss sees the environment
//! map. `cs_update` blends the rays into each probe's octahedral irradiance and distance maps
//! (the atlas, frame binding 23), and `probe_gi` in lib_frame.wgsl lights every surface from
//! the eight probes around it, with DDGI's visibility weights so light doesn't leak through
//! walls. See /engine-design/dynamic-gi.md in the project files for the research behind it.
//!
//! Made cheap enough to be on by default (/engine-design/world-gi-on.md): every mesh entity's
//! world box is cached and updated only when its change ticks moved (a share of the cache is
//! checked again each frame for meshes edited in place and entities gone), so a frame reads
//! the components of only what changed, never the whole world. The instances near the grid are
//! kept the same way, one per entity: a frame updates only the instances of entities that
//! changed, entered or left the snapped region, or whose mesh or material asset changed (Unreal
//! updates only dirty ray-tracing instances too), refits the instance hierarchy when only boxes
//! moved and rebuilds it when instances came or went, and writes only the instances that
//! changed. They stay relative to an anchor that moves in 64 m steps, so a moving camera
//! leaves them as they are; only the anchor's step or moved hierarchies read them all again.
//! A share of them is checked again each frame (textures whose mean color arrived). Hierarchies of
//! all but small meshes build on worker threads; the small ones a frame asks for build side by
//! side on the render prepare threads. A [`WorldShift`] (a floating-origin move, a jump that
//! re-anchors the world) moves the cached boxes with it instead of refreshing every mesh entity
//! its new transform marks changed. The hierarchies are packed in one
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
mod rt;
mod schedule;

use crate::gpu_counts::{GpuCounts, GpuReadback};
use std::collections::{HashMap, HashSet};

use bevy_ecs::change_detection::Tick;
use bevy_ecs::entity::{EntityHashMap, EntityHashSet};
use bevy_ecs::message::{MessageCursor, Messages};
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::{GpuShared, GrowBuffer};
use crate::frame::{FrameUniform, ViewData};
use crate::material::{AlphaMode, StandardMaterial};
use crate::mesh::Mesh;
use crate::ray_scene::look::{material_light, texture_mean};
use crate::settings::RenderSettings;
use engine_assets::{Assets, Handle};
use engine_world::WorldShift;

/// Texels along a probe's tile side (`PROBE_GI_TILE` in lib_frame.wgsl): its maps, 8 texels a
/// side, and a border of one texel around them, copies across the octahedron's folds.
const TILE: u32 = 10;
/// Tiles per atlas row (`PROBE_GI_TILES_X`).
const TILES_X: u32 = 128;
/// Probes the atlas holds: 1280 × 1280 texels, two layers (26 MB).
pub(crate) const MAX_PROBES: u32 = TILES_X * TILES_X;
/// Rays per probe at most (`MAX_RAYS` in probe_gi.wgsl).
const MAX_RAYS: u32 = 256;
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What the sun's rays found, a texel a pixel (`cs_sun`'s `sun_out`): a format a compute pass
/// may write on every GPU.
const SUN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// `trial_cut_more::RAY_PROBES` where the rays trace on ray-tracing hardware: the most probes
/// a frame and the GPU time they may take, times these (8,192 of the default 2,048 within
/// 2.4 ms of 1.5).
const RAISED_PROBES: u32 = 4;
const RAISED_BUDGET: f32 = 1.6;
/// `GiPass::world_origin`'s w, flags: a probe stuck inside something sleeps
/// (`GI_STUCK_SLEEP` in probe_gi.wgsl; `trial_cut_more::RAY_PROBES` on ray-tracing hardware).
const GI_STUCK_SLEEP: i32 = 1;
/// Triangles whose hierarchies may be built per frame where the gather runs (at least one mesh
/// is).
const BUILD_TRIANGLES_PER_FRAME: usize = 8_000;
/// Meshes with more triangles than this build on a worker thread instead (a small mesh's
/// hierarchy takes microseconds; a few thousand triangles, a fraction of a millisecond).
const INLINE_BUILD_TRIANGLES: usize = 2_000;
/// Worker builds at once.
const MAX_BUILD_JOBS: usize = 4;
/// Cached mesh entities checked again a frame (are they still there, has their mesh asset
/// changed): the whole cache in turn, without a frame that scans it all.
const REVALIDATE_PER_FRAME: usize = 1024;
/// Meters between the anchors instances are kept relative to.
const ANCHOR_CELL: f64 = 64.0;
/// Meters the region instances are gathered from snaps to.
const REGION_SNAP: f64 = 8.0;
/// Instances checked again a frame, in turn (a texture's mean color arriving, anything the
/// change ticks and asset versions don't show).
const RECHECK_PER_FRAME: usize = 256;
/// Refits of the instance hierarchy before it is built again (a refit tree slows the rays as
/// instances wander from where it was built).
const REFITS_BEFORE_REBUILD: u32 = 30;
/// Frames between sweeps of hierarchies no instance used in a while.
const FORGET_EVERY_FRAMES: u64 = 60;
/// Probes a frame at least under a time budget.
const MIN_PROBES_PER_FRAME: u32 = 128;
/// Probes a frame under a time budget before timestamps measured their cost.
const UNMEASURED_PROBES_PER_FRAME: u32 = 512;
/// Frames a mesh edited in place keeps its last hierarchy before it is built again: a mesh
/// edited every frame (a deforming or regenerated mesh) is built twice a second at 60 fps, not
/// every frame, and the rays see its shape from a moment ago (diffuse light barely shows it).
const REBUILD_AFTER_FRAMES: u64 = 30;
/// Frames a mesh's hierarchy stays after its last use.
const KEEP_UNUSED_FRAMES: u64 = 600;
/// Losses of the hardware rays' scene after which the rays trace in software
/// (`ProbeGi::hw_losses`).
const HW_LOSSES_MAX: u32 = 3;
/// The extract's parts, timed as `render.gi.ms_<part>`: placing the grid, collecting the
/// workers' builds, following shifts and reading change ticks; refreshing the changed entities'
/// boxes and checking a share again; the region, entity count, asset and recheck lists; reading
/// the dirty entities; building the small meshes they asked for and reading their entities
/// again; uploading the hierarchies; the instance hierarchy and the instances.
const PARTS: [&str; 7] = [
    "changes",
    "refresh",
    "lists",
    "read",
    "builds",
    "upload",
    "instances",
];
/// An extract slower than this (ms) logs its parts, so a hitch log names the slow one.
const SLOW_EXTRACT_MS: f32 = 30.0;
/// Bytes of new hierarchies uploaded a frame into each store buffer; meshes wait for theirs. A
/// zone's arrival appended 25 MB in one frame (181 ms on the cloud VM).
const UPLOAD_BYTES_PER_FRAME: usize = 4 << 20;

/// How close (meters, and per matrix element) a transform must be to its cached one (moved by a
/// [`WorldShift`]) to count as unmoved (the shift and the transform propagation round
/// differently).
const UNMOVED: f64 = 1e-6;

/// What the probes' rays, the world reflections' and the sun's trace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Rays {
    /// The probes' own scene, walked in software (GPUs without ray queries).
    Software,
    /// The probes' own scene in acceleration structures of its own (`rt.rs`).
    Hardware,
    /// The frame's ray scene, which every feature fills ([`crate::ray_scene`]): the probes
    /// gather nothing themselves.
    World,
}

/// probe_gi.wgsl after the scene its rays trace (see the head of probe_gi.wgsl).
fn shader_source(rays: Rays) -> String {
    let body = include_str!("../shaders/probe_gi.wgsl");
    let own = include_str!("../shaders/probe_gi_scene_old.wgsl");
    match rays {
        Rays::Software => format!(
            "{}\n{own}\n{body}",
            include_str!("../shaders/probe_gi_trace_sw.wgsl")
        ),
        Rays::Hardware => format!(
            "{}\n{own}\n{body}",
            include_str!("../shaders/probe_gi_trace_hw.wgsl")
        ),
        // (Every pixel's reflection is a dispatch of this program too, after the probes', and
        // so are the mirrors' pictures and Rendering's check of the probes; their hits are lit
        // by ray_hit_light.wgsl, which the water's reflection ray composes too.)
        Rays::World => format!(
            "{}\n{}\n{}\n{body}\n{}\n{}\n{}",
            crate::ray_scene::shader_lib(1),
            include_str!("../shaders/ray_hit_light.wgsl"),
            include_str!("../shaders/probe_gi_rays.wgsl"),
            include_str!("../shaders/ray_reflect.wgsl"),
            include_str!("../shaders/ray_mirror.wgsl"),
            include_str!("../shaders/ray_probe_check.wgsl")
        ),
    }
}

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

/// `Instance` in probe_gi_scene_old.wgsl.
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
    /// The frame it was stored.
    built: u64,
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

    /// Upload what the GPU doesn't hold yet, appends up to [`UPLOAD_BYTES_PER_FRAME`] of each
    /// buffer a frame (all of it into an empty one); true when a buffer was recreated.
    fn upload(&mut self, active: &mut Active, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let mut regroup = false;
        if self.gpu_nodes < self.nodes.len() {
            let end = upload_end(self.gpu_nodes, self.nodes.len(), size_of::<bvh::Node>());
            regroup |= append(
                &mut active.blas,
                device,
                queue,
                &self.nodes,
                self.gpu_nodes,
                end,
            );
            self.gpu_nodes = end;
        }
        if self.gpu_corners < self.corners.len() {
            let end = upload_end(self.gpu_corners, self.corners.len(), 16);
            regroup |= append(
                &mut active.triangles,
                device,
                queue,
                &self.corners,
                self.gpu_corners,
                end,
            );
            self.gpu_corners = end;
        }
        regroup
    }

    /// Whether the GPU holds all of `e`'s hierarchy by the end of this frame's upload (an empty
    /// buffer takes everything at once).
    fn holds(&self, e: &MeshEntry) -> bool {
        (self.gpu_nodes == 0 || (e.root + e.nodes) as usize <= self.gpu_nodes)
            && (self.gpu_corners == 0
                || e.first_triangle as usize * 3 + e.corners as usize <= self.gpu_corners)
    }
}

/// Where this frame's append of a store's `len` items ends, `held` of them on the GPU: all of
/// them into an empty buffer (the first upload, or after a compaction, when every instance
/// points at them), else at most [`UPLOAD_BYTES_PER_FRAME`].
fn upload_end(held: usize, len: usize, size: usize) -> usize {
    if held == 0 {
        len
    } else {
        len.min(held + (UPLOAD_BYTES_PER_FRAME / size).max(1))
    }
}

/// Write `data[held..end]` after the `held` items `buffer` holds (all of `data[..end]` into an
/// empty one), making room for all of `data`; true when the buffer was recreated.
fn append<T: Pod>(
    buffer: &mut GrowBuffer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &[T],
    held: usize,
    end: usize,
) -> bool {
    let bytes: &[u8] = bytemuck::cast_slice(&data[..end]);
    if held == 0 {
        return buffer.write(device, queue, bytes);
    }
    let all = std::mem::size_of_val(data) as u64;
    // Grown: the GPU copies what it held (a zone's arrival rewrote 50 MB in one frame).
    let from = held * size_of::<T>();
    let grown = buffer.grow_keeping(device, queue, all, from as u64);
    queue.write_buffer(&buffer.buffer, from as u64, &bytes[from..]);
    grown
}

/// What every instance was last gathered for: all of them are read again when it changes.
#[derive(Clone, Copy, PartialEq)]
struct GatherKey {
    /// The anchor's cell: instances are relative to it.
    anchor: IVec3,
    /// The hierarchies' places in the store: instances point into it.
    layout: u64,
}

/// What the rays see of one mesh entity near the grid.
#[derive(Clone, Copy)]
struct Record {
    instance: GpuInstance,
    /// Anchor-relative world box.
    bounds: (Vec3, Vec3),
    /// Mesh -> anchor-relative world, rows of a 3x4 (the hardware structure's instance; zeros
    /// on the software walk).
    to_world: [f32; 12],
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        bytemuck::bytes_of(&self.instance) == bytemuck::bytes_of(&other.instance)
            && self.bounds == other.bounds
            && bytemuck::bytes_of(&self.to_world) == bytemuck::bytes_of(&other.to_world)
    }
}

impl Record {
    /// Its mesh and placement in the hardware structure.
    fn placement(&self) -> (u32, [f32; 12]) {
        (self.instance.mesh[3], self.to_world)
    }
}

/// A mesh entity in the gather region as last read: the asset versions it was read at, and its
/// instance (`None`: the rays don't see it, hidden, see-through, unlit, not loaded or built yet).
struct Entry {
    mesh: Handle<Mesh>,
    mesh_version: u64,
    material: Handle<StandardMaterial>,
    material_version: u64,
    record: Option<Record>,
}

/// The instances on the GPU, in the instance hierarchy's order, and that hierarchy (the
/// software walk's; the hardware's takes them in entity order).
#[derive(Default)]
struct Placed {
    entities: Vec<Entity>,
    instances: Vec<GpuInstance>,
    boxes: Vec<(Vec3, Vec3)>,
    /// Each instance's mesh and placement, for the hardware structure.
    placements: Vec<(u32, [f32; 12])>,
    /// An instance's placement changed in place since the hardware structure last took them
    /// (one whose material alone changed leaves the structure as it is).
    placements_changed: bool,
    slot_of: EntityHashMap<u32>,
    tlas: Vec<bvh::Node>,
    /// Refits since it was built.
    refits: u32,
}

/// What the rays have of an instance's mesh ([`ProbeGi::read`]).
enum Have {
    /// Something to hit: its first node and first triangle in the software store (zeros for a
    /// hardware structure), and whether to read the entity again next frame (an older version's
    /// serves while the mesh's waits).
    At([u32; 2], bool),
    /// Nothing to hit (yet, when `true`: read the entity again next frame).
    Nothing(bool),
    /// Still uploading: the instance read before stays; read the entity again next frame.
    KeepOld,
}

/// What reading an entity found: `None` when it's no longer a mesh entity the gather reads,
/// else its entry and whether it waits on its hierarchy (a worker, the frame's build budget),
/// to be read again next frame. (An option rather than an enum of two: the entry is large and
/// the other case empty.)
type Read = Option<(Entry, bool)>;

/// The small meshes this frame's reads asked to have built (`BUILD_TRIANGLES_PER_FRAME`).
#[derive(Default)]
struct Builds {
    /// Mesh index, version and handle of each.
    queued: Vec<(u32, u64, Handle<Mesh>)>,
    triangles: usize,
    /// Built: later reads this frame ask for nothing more.
    closed: bool,
}

/// Build the hierarchies of `queued` side by side on the render prepare threads (on Tobias's PC
/// a jump's frames each built up to 8000 triangles one mesh after another).
fn build_small(
    meshes: &Assets<Mesh>,
    queued: &[(u32, u64, Handle<Mesh>)],
) -> Vec<(u32, u64, bvh::MeshBvh)> {
    use rayon::prelude::*;
    let build = |&(key, version, handle): &(u32, u64, Handle<Mesh>)| {
        let mesh = meshes.get(handle)?;
        let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
        Some((key, version, bvh::build_mesh(&positions, &mesh.indices)))
    };
    if queued.len() == 1 {
        return queued.iter().filter_map(build).collect();
    }
    crate::prepare::pool().install(|| queued.par_iter().filter_map(build).collect())
}

/// What reading entities needs from the world.
struct Look<'a> {
    world: &'a World,
    meshes: &'a Assets<Mesh>,
    materials: Option<&'a Assets<StandardMaterial>>,
    images: Option<&'a Assets<crate::Image>>,
    shared: &'a GpuShared,
    default_material: &'a StandardMaterial,
    anchor: DVec3,
    /// Leaf crowns are in the canopy map (`RenderSettings::canopy`): the rays see them there.
    canopy: bool,
}

/// What the instance gather reads; only change ticks while nothing changed.
type GatherQuery = (
    Entity,
    Ref<'static, GlobalTransform>,
    Ref<'static, crate::MeshRenderer>,
    Ref<'static, crate::ResolvedMesh>,
);
/// Instances smaller than this (half their box's diagonal, meters) are left out of the probes'
/// scene: next to the default 0.75 m probe spacing they change little of what a probe sees, but
/// cost every ray that passes their box. Assassin's Creed Shadows leaves small objects out of its
/// GI rays the same way.
pub const MIN_INSTANCE_RADIUS_M: f64 = 0.3;

/// Skinned meshes are left out (the rays would see their bind pose), shadow-only stand-ins, and
/// `MeshInstances` regions (small plants, too small for the rays).
type GatherFilter = (
    Without<crate::GpuSkin>,
    Without<crate::ShadowOnly>,
    Without<crate::MeshInstances>,
);
/// [`GatherFilter`]'s entities whose transform, renderer or resolved mesh changed: the change
/// pass reads only these (a walk of every entity's change ticks took 0.6-2.4 ms a frame at the
/// lake on Tobias's PC, ~90k mesh entities).
type ChangeFilter = (
    Without<crate::GpuSkin>,
    Without<crate::ShadowOnly>,
    Without<crate::MeshInstances>,
    Or<(
        Changed<GlobalTransform>,
        Changed<crate::MeshRenderer>,
        Changed<crate::ResolvedMesh>,
    )>,
);

/// A mesh entity as the gather last saw it: its world box (absolute), its transform (to tell a
/// [`WorldShift`]'s move from its own) and its mesh's version.
#[derive(Clone, Copy)]
struct Cached {
    lo: DVec3,
    hi: DVec3,
    affine: DAffine3,
    mesh: u32,
    version: u64,
}

impl Cached {
    fn overlaps(&self, region: &(DVec3, DVec3)) -> bool {
        self.hi.cmpge(region.0).all() && self.lo.cmple(region.1).all()
    }
}

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

/// What gathering the probes' own scene found ([`ProbeGi::gather`]); all zero while the rays
/// trace the frame's ray scene.
#[derive(Default)]
struct Gathered {
    /// A buffer of the scene's group was made again.
    regroup: bool,
    instances: u32,
    /// The instances' anchor (absolute).
    anchor: DVec3,
    /// The instances on the GPU changed; every instance was read again; their hierarchy
    /// was built again.
    gathered: bool,
    full: bool,
    rebuilt: bool,
    /// Triangles of the hierarchies built.
    built: u32,
    /// Entities read, and mesh entities whose change ticks moved.
    read: u32,
    changed: u32,
    shifted: u32,
    unchanged: u32,
    build_ms: f32,
    parts: [f32; PARTS.len()],
}

/// What `encode` records this frame.
struct Work {
    probes: u32,
    /// Probes written empty after the traced ones (they scrolled in and wait for a trace).
    cleared: u32,
    /// Probes scrolled into the grid this frame: until this frame's rows are copied, the
    /// atlas holds in their slots the probes that left at the far side.
    scrolled: bool,
    rays: u32,
    /// Atlas tile rows written, as (first, count) ranges.
    rows: Vec<(u32, u32)>,
}

/// GPU resources, made when probe GI first turns on.
struct Active {
    /// What the rays trace; the pipelines and `layout` are made for it.
    rays_trace: Rays,
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
    /// The software walk's hierarchies (unused, and empty, on the hardware path).
    tlas: GrowBuffer,
    instances: GrowBuffer,
    blas: GrowBuffer,
    triangles: GrowBuffer,
    rays: GrowBuffer,
    updates: GrowBuffer,
    group: Option<wgpu::BindGroup>,
    /// The rays' scene in the GPU's acceleration structures, on the hardware path.
    hw: Option<rt::HwScene>,
    /// The sun's rays (`RenderSettings::ray_sun_shadows`), on ray-tracing hardware.
    sun: Option<SunRays>,
    /// Every pixel's reflection (`trial_cut_more::RAY_REFLECTIONS`), on the frame's ray scene.
    full: Option<FullReflections>,
    /// The mirrors' pictures (`trial_cut_more::RAY_MIRROR_PICTURES`), on the frame's ray scene.
    mirrors: Option<MirrorPrograms>,
    /// Rendering's check of the probes (`RenderSettings::ray_probe_check`), on the frame's
    /// ray scene.
    check: Option<ProbeCheck>,
    /// This frame's inputs of the world reflections and of the sun's rays (group 2), made by
    /// [`ProbeGi::begin_rays`] for the dispatches in the frame's ray pass.
    reflect_inputs: Option<wgpu::BindGroup>,
    sun_inputs: Option<wgpu::BindGroup>,
    /// This frame's sun rays are counted.
    sun_counted: bool,
    /// `cs_update`'s count of the probes it updated: planned, traced (the rest slept through
    /// their turn), awake after (`render.gi.probes_counted`, `probes_traced`, `probes_awake`).
    probe_counts: GpuCounts,
    /// The frame's ray pass updates the probes: copy their counts out after it.
    probes_counting: bool,
}

/// What the frame's ray pass holds of the probes' ([`ProbeGi::begin_rays`]).
#[derive(Default, Clone, Copy)]
pub(crate) struct RayWork {
    /// The probes' trace and update.
    pub probes: bool,
    /// World reflections into `targets.world_reflection`.
    pub reflections: bool,
    /// Every pixel's reflection ([`ProbeGi::full_reflections`]), in place of those.
    pub full_reflections: bool,
    /// The sun's rays.
    pub sun: bool,
    /// Rendering's check of the probes ([`ProbeGi::dispatch_check`]).
    pub check: bool,
    /// The probe grid scrolled this frame: the reflections must not read the atlas before
    /// this frame's rows are copied into it ([`ProbeGi::end_rays`]), or a hit at the grid's
    /// leading edge is lit by the probes that left at the far side.
    pub scrolled: bool,
}

impl RayWork {
    /// Whether the pass holds anything.
    pub fn any(&self) -> bool {
        self.probes || self.reflections || self.full_reflections || self.sun || self.check
    }

    /// Whether the pass holds reflections, of either kind.
    pub fn any_reflections(&self) -> bool {
        self.reflections || self.full_reflections
    }
}

/// The reflections the frame's ray pass traces ([`ProbeGi::begin_rays`]).
#[derive(Clone, Copy)]
pub(crate) enum ReflectionRays<'a> {
    /// World reflections at half size into `targets.world_reflection`, for screen-space
    /// reflections to blend under theirs; `alternate`: `trial_cut::REFLECTIONS_ALTERNATE`.
    Half { alternate: bool },
    /// Every pixel's reflection (`trial_cut_more::RAY_REFLECTIONS`, ray_reflect.wgsl):
    /// `picture` is last frame's finished picture (any texture before the view has one:
    /// `frame.reflections.y` 0 tells the shader), `count` counts them.
    Full {
        picture: &'a wgpu::TextureView,
        count: bool,
    },
}

/// Every pixel's reflection (`cs_reflect_full`, ray_reflect.wgsl): its program, its output and
/// its count.
struct FullReflections {
    pipeline: wgpu::ComputePipeline,
    /// Group 2: the prepass's normals and depth, the motion, last frame's picture, `out`,
    /// `counts`.
    layout: wgpu::BindGroupLayout,
    /// What each pixel's reflection found (radiance, the distance to it), made at first use
    /// and again when the view's size changes, dropped in a frame without it. 8 bytes a
    /// pixel, 29.5 MB at 2560x1440.
    out: Option<crate::targets::Tex>,
    /// The pixels under the roughness limit, those the screen answered, the rays traced and
    /// their hits taken from last frame's picture, counted by the pass while
    /// `shade_skip::COUNT_MESH_RUNS` is set.
    counts: GpuCounts,
    /// This frame's inputs ([`ProbeGi::begin_rays`]), until the pass takes them.
    inputs: Option<wgpu::BindGroup>,
    /// This frame's reflections are counted, and wait for their counts to be copied out.
    counting: bool,
    /// The last frame that asked traced every pixel's reflection.
    traced: bool,
}

/// The mirrors' pictures traced in the frame's ray pass (`cs_mirror_picture`, ray_mirror.wgsl;
/// their targets are the renderer's, `crate::ray_mirror`): the program, what each picture binds
/// this frame, and their count.
struct MirrorPrograms {
    pipeline: wgpu::ComputePipeline,
    /// Group 2: the inputs every pixel's reflection reads (the prepass's normals and depth,
    /// the motion, last frame's picture), the picture's trace, its colour and depth, `counts`.
    layout: wgpu::BindGroupLayout,
    /// The texels, those the screen answered, the rays traced and their hits taken from last
    /// frame's picture, counted by the pass while `shade_skip::COUNT_MESH_RUNS` is set.
    counts: GpuCounts,
    /// This frame's pictures ([`ProbeGi::begin_mirrors`]) with their sizes, until the pass
    /// takes them.
    inputs: Vec<(wgpu::BindGroup, UVec2)>,
    /// This frame's pictures are counted, and wait for their counts to be copied out.
    counting: bool,
}

/// Rendering's check of the probes (`RenderSettings::ray_probe_check`, ray_probe_check.wgsl
/// `cs_probe_check`): the irradiance the probe nearest the camera holds facing each of the six
/// axis directions, against a brute-force reference of 4,096 rays a direction a frame through
/// the frame's ray scene, each seen as a probe ray sees it, kept as a mean over the frames.
struct ProbeCheck {
    pipeline: wgpu::ComputePipeline,
    /// Group 2: `out`.
    group: wgpu::BindGroup,
    /// `check_out` (ray_probe_check.wgsl), read back.
    out: GpuReadback,
    /// The frame's ray pass checks: copy `out` out after it.
    ran: bool,
    /// What the readbacks told, while they are of one probe.
    seen: Option<CheckedProbe>,
}

/// The six axis directions the check reads, in its order, as the diagnostics name them.
const CHECK_AXES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];
/// `check_out`: the probe's six, its position, then the sums, `CHECK_GROUPS` a direction.
const CHECK_GROUPS: usize = 16;
const CHECK_BYTES: u64 = ((7 + 6 * CHECK_GROUPS) * 16) as u64;
/// The means are over the frames read since the probe was last another, up to this many (a
/// running mean, then a blend that forgets older frames as the light settles): enough of the
/// probe's updates that its own noise (a fifth from one to the next under a canopy) leaves its
/// mean a few percent off at most.
const CHECK_FRAMES: u32 = 128;

/// The probe the check read last, and the means of the frames read since it became that probe:
/// the probe's own irradiance and the reference's, so that the two differ by what the probes
/// get wrong on the whole, and the probe's spread from frame to frame, its flicker.
#[derive(Clone, Copy, Debug, Default)]
struct CheckedProbe {
    /// Camera-relative position and grid index, as the latest readback had them.
    position: [f32; 3],
    index: f32,
    /// The probe's own irradiance (lux, rgb) facing each direction, over `frames` frames.
    own: [[f64; 3]; 6],
    /// The mean of the square of its luminance over the same frames.
    own_square: [f64; 6],
    /// Its state (1: lit in this epoch, + 2: in use), as the latest readback read it.
    state: f32,
    /// The reference's irradiance (lux, rgb) facing each direction, over `frames` frames.
    reference: [[f64; 3]; 6],
    frames: u32,
}

impl CheckedProbe {
    /// Take in one readback of `check_out` (little-endian `vec4<f32>`s).
    fn read(seen: Option<CheckedProbe>, bytes: &[u8]) -> Option<CheckedProbe> {
        let f = |i: usize| -> f32 {
            let at = i * 4;
            f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if bytes.len() < CHECK_BYTES as usize {
            return seen;
        }
        let position = [f(24), f(25), f(26)];
        let index = f(27);
        // Another probe (the camera moved off this one, or the grid scrolled under it): start
        // the reference's mean again.
        let same = seen.is_some_and(|s| {
            s.index == index && (0..3).all(|i| (s.position[i] - position[i]).abs() < 0.01)
        });
        let mut now = if same {
            seen.unwrap()
        } else {
            CheckedProbe::default()
        };
        now.position = position;
        now.index = index;
        now.state = f(3);
        now.frames = (now.frames + 1).min(CHECK_FRAMES);
        let keep = 1.0 - 1.0 / now.frames as f64;
        for k in 0..6 {
            let e = [f(k * 4) as f64, f(k * 4 + 1) as f64, f(k * 4 + 2) as f64];
            if e.iter().any(|v| !v.is_finite()) {
                continue;
            }
            for (value, e) in now.own[k].iter_mut().zip(e) {
                *value = *value * keep + e * (1.0 - keep);
            }
            now.own_square[k] = now.own_square[k] * keep + luma(e).powi(2) * (1.0 - keep);
        }
        for (k, reference) in now.reference.iter_mut().enumerate() {
            let (mut sum, mut rays) = ([0.0f64; 3], 0.0f64);
            for g in 0..CHECK_GROUPS {
                let at = (7 + k * CHECK_GROUPS + g) * 4;
                for (c, sum) in sum.iter_mut().enumerate() {
                    *sum += f(at + c) as f64;
                }
                rays += f(at + 3) as f64;
            }
            if rays <= 0.0 || sum.iter().any(|v| !v.is_finite()) {
                continue;
            }
            for (c, value) in reference.iter_mut().enumerate() {
                // Mean radiance over cosine-spread directions, times pi: irradiance.
                let e = std::f64::consts::PI * sum[c] / rays;
                *value = *value * keep + e * (1.0 - keep);
            }
        }
        Some(now)
    }
}

/// Luminance of a linear rgb.
fn luma(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// One ray toward the sun from every pixel (`cs_sun`, a measurement for now): its program, its
/// output and the count of its rays.
struct SunRays {
    pipeline: wgpu::ComputePipeline,
    /// Group 2: the prepass's normals and depth, `out`, `counts`.
    layout: wgpu::BindGroupLayout,
    /// What each pixel's ray found (r: 1 the sun, 0 something in the way), made at first use
    /// and again when the view's size changes. 4 bytes a pixel, 14.7 MB at 2560x1440.
    out: Option<crate::targets::Tex>,
    /// The rays shot and those stopped, counted by the pass while
    /// `shade_skip::COUNT_MESH_RUNS` is set.
    counts: GpuCounts,
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
    /// Each base color image's mean color by version (`None`: not RGBA8), read from its pixels
    /// here: the image cache's copy waits on its lock, which an upload beside the gather holds
    /// (on a zone's arrival 226 reads waited 1.08 s in one frame on the cloud VM).
    texture_means: HashMap<u32, (u64, Option<Vec3>)>,
    /// Triangles left out for the binding limit (warned once).
    left_out_warned: bool,
    /// What the instances were read for (`None`: read them all next frame).
    gathered: Option<GatherKey>,
    /// Every entity in the region as last read, and the instances on the GPU.
    entries: EntityHashMap<Entry>,
    placed: Placed,
    /// Entities waiting on their mesh's hierarchy.
    pending: Vec<Entity>,
    /// The entries in the order `RECHECK_PER_FRAME` walks them.
    recheck: Vec<Entity>,
    recheck_at: usize,
    /// The mesh and material assets' latest versions and removals at the last check.
    asset_marks: ((u64, u64), (u64, u64)),
    query: Option<QueryState<GatherQuery, GatherFilter>>,
    changes: Option<QueryState<GatherQuery, ChangeFilter>>,
    /// The world's change tick at the last gather check, and the mesh entities it found.
    tick: Option<Tick>,
    entities: usize,
    /// Every visible mesh entity's world box, updated only for those whose change ticks moved
    /// (Unreal caches static ray-tracing instances the same way): a frame reads the change
    /// ticks of all and the components of only what changed.
    cache: EntityHashMap<Cached>,
    /// The cached entities whose box overlaps the gather region, and that region (absolute).
    in_region: EntityHashSet,
    region: Option<(DVec3, DVec3)>,
    /// The cache's entities in the order `REVALIDATE_PER_FRAME` walks them.
    revalidate: Vec<Entity>,
    revalidate_at: usize,
    /// Where the [`WorldShift`] messages were read up to.
    shifts: MessageCursor<WorldShift>,
    /// Measured GPU milliseconds per thousand probe rays (0: not yet), and rays traced a frame
    /// lately (a moving average, as the measurements arrive a few frames late).
    ms_per_kray: f32,
    rays_lately: f32,
    grid: Option<Grid>,
    /// Which probes trace each frame, and the epoch (schedule.rs).
    schedule: schedule::Schedule,
    /// The main light last frame (direction, luminance of its color): when it jumps, every
    /// probe starts over (`light_jumped`).
    light: Option<(Vec3, f32)>,
    frame: u64,
    /// Times the hardware rays' scene was lost (a submission holding its builds failed) and
    /// started over; after `HW_LOSSES_MAX` the rays trace in software for the rest of the run.
    hw_losses: u32,
    work: Option<Work>,
    stats: Stats,
    /// Last frame's world reflections, for the texels this frame doesn't trace
    /// (`trial_cut::REFLECTIONS_ALTERNATE`); made the first frame the switch is on.
    reflect_last: Option<crate::targets::Tex>,
}

#[derive(Default, Clone, Copy)]
struct Stats {
    probes: u32,
    /// Probes the time budget held back this frame (0: none).
    budget_cut: u32,
    /// `trial_cut_more::RAY_PROBES` acts: the rays trace on ray-tracing hardware.
    raised: bool,
    /// Probes that scrolled in this frame, and those cleared to wait for their first trace.
    scrolled_in: u32,
    cleared: u32,
    instances: u32,
    meshes: u32,
    built_triangles: u32,
    building: u32,
    /// The instances on the GPU changed.
    gathered: bool,
    /// Every instance was read again.
    full: bool,
    /// Entities read, and mesh entities whose change ticks moved.
    read: u32,
    changed: u32,
    /// Mesh entities a [`WorldShift`] alone moved this frame (their entries moved with it).
    shifted: u32,
    /// Mesh entities whose change ticks moved with nothing the gather reads changed.
    unchanged: u32,
    rebuilt: bool,
    build_ms: f32,
    extract_ms: f32,
    /// Milliseconds in each of [`PARTS`].
    parts: [f32; PARTS.len()],
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

/// Copy the tile rows `rows` (first, count) the update wrote from `next` into the atlas the
/// frame samples.
fn copy_atlas_rows(
    encoder: &mut wgpu::CommandEncoder,
    next: &wgpu::Texture,
    atlas: &wgpu::Texture,
    rows: &[(u32, u32)],
) {
    let size = TILE * TILES_X;
    for &(first, n) in rows {
        let origin = wgpu::Origin3d {
            x: 0,
            y: first * TILE,
            z: 0,
        };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: next,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: atlas,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size,
                height: n * TILE,
                depth_or_array_layers: 2,
            },
        );
    }
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// Whether the main light changed so much in one frame (direction, luminance of its color in
/// lux) that the probes' light is void: more than four times brighter or darker, or turned more
/// than 10° while it is brighter than 10 lux (the sun handing over to the moon at dusk turns it
/// in a frame, but both are dim then). A day passing at any speed the clock runs changes it far
/// less a frame.
fn light_jumped(last: (Vec3, f32), now: (Vec3, f32)) -> bool {
    const RATIO: f32 = 4.0;
    const TURN_COS: f32 = 0.984_807_8; // cos 10°
    const LIT_LUX: f32 = 10.0;
    let (a, b) = (last.1.max(1e-3), now.1.max(1e-3));
    if a.max(b) / a.min(b) > RATIO && a.max(b) > 1e-2 {
        return true;
    }
    a.max(b) > LIT_LUX && last.0.normalize_or_zero().dot(now.0.normalize_or_zero()) < TURN_COS
}

/// The world box of a mesh's local box under `affine`, relative to `origin`.
/// The rows of `a` as a 3x4 in f32 (an instance's transform on the GPU).
fn affine_rows(a: &DAffine3) -> [[f32; 4]; 3] {
    let m = a.matrix3.as_mat3();
    let t = a.translation.as_vec3();
    [0, 1, 2].map(|i| {
        let r = m.row(i);
        [r.x, r.y, r.z, t[i]]
    })
}

fn world_box(local: (Vec3, Vec3), affine: &DAffine3, origin: DVec3) -> (Vec3, Vec3) {
    let (lo, hi) = world_box_abs(local, affine);
    ((lo - origin).as_vec3(), (hi - origin).as_vec3())
}

/// The world box of a mesh's local box under `affine` (absolute).
fn world_box_abs(local: (Vec3, Vec3), affine: &DAffine3) -> (DVec3, DVec3) {
    box_under(local.0.as_dvec3(), local.1.as_dvec3(), affine)
}

/// The box around box `lo`..`hi` under `affine`.
fn box_under(lo: DVec3, hi: DVec3, affine: &DAffine3) -> (DVec3, DVec3) {
    let center = affine.transform_point3((lo + hi) * 0.5);
    let half = (hi - lo) * 0.5;
    let m = affine.matrix3;
    let extent = DVec3::new(
        m.row(0).abs().dot(half),
        m.row(1).abs().dot(half),
        m.row(2).abs().dot(half),
    );
    (center - extent, center + extent)
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
            TILE,
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
            texture_means: HashMap::new(),
            left_out_warned: false,
            gathered: None,
            entries: EntityHashMap::default(),
            placed: Placed::default(),
            pending: Vec::new(),
            recheck: Vec::new(),
            recheck_at: 0,
            asset_marks: ((0, 0), (0, 0)),
            query: None,
            changes: None,
            tick: None,
            entities: 0,
            cache: EntityHashMap::default(),
            in_region: EntityHashSet::default(),
            shifts: MessageCursor::default(),
            region: None,
            revalidate: Vec::new(),
            revalidate_at: 0,
            ms_per_kray: 0.0,
            rays_lately: 0.0,
            grid: None,
            schedule: schedule::Schedule::new(),
            reflect_last: None,
            light: None,
            frame: 0,
            hw_losses: 0,
            work: None,
            stats: Stats::default(),
        }
    }

    /// The atlas frame binding 23 samples.
    pub fn atlas_view(&self) -> &wgpu::TextureView {
        &self.atlas_view
    }

    /// Make the GPU resources for rays that trace `rays`.
    fn activate(&mut self, shared: &GpuShared, rays: Rays) {
        let hardware = rays == Rays::Hardware;
        let world = rays == Rays::World;
        let device = &shared.device;
        let size = TILE * TILES_X;
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
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let atlas_out = wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: ATLAS_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        };
        // The pass, the instances (the probes' own scene only), the rays, the probes' state,
        // the atlas written, the slots updated and the count of what the update did: group 1
        // with the probes' own scene (probe_gi_scene_old.wgsl), group 2 behind the ray scene
        // (probe_gi_rays.wgsl).
        let mut entries = if world {
            vec![
                layout_entry(10, S::COMPUTE, uniform),
                layout_entry(15, S::COMPUTE, storage(false)),
                layout_entry(16, S::COMPUTE, storage(false)),
                layout_entry(17, S::COMPUTE, atlas_out),
                layout_entry(18, S::COMPUTE, storage(true)),
                layout_entry(19, S::COMPUTE, storage(false)),
            ]
        } else {
            vec![
                layout_entry(0, S::COMPUTE, uniform),
                layout_entry(2, S::COMPUTE, storage(true)),
                layout_entry(5, S::COMPUTE, storage(false)),
                layout_entry(6, S::COMPUTE, storage(false)),
                layout_entry(7, S::COMPUTE, atlas_out),
                layout_entry(8, S::COMPUTE, storage(true)),
                layout_entry(9, S::COMPUTE, storage(false)),
            ]
        };
        if world {
            // (The scene is the ray scene's group.)
        } else if hardware {
            // The instances' acceleration structure (probe_gi_trace_hw.wgsl `scene`).
            entries.push(layout_entry(
                1,
                S::COMPUTE,
                wgpu::BindingType::AccelerationStructure {
                    vertex_return: true,
                },
            ));
        } else {
            // The instance hierarchy, the meshes' and their triangles
            // (probe_gi_trace_sw.wgsl).
            for binding in [1, 3, 4] {
                entries.push(layout_entry(binding, S::COMPUTE, storage(true)));
            }
        }
        entries.sort_by_key(|e| e.binding);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe gi"),
            entries: &entries,
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
                // Last frame's result and the screen motion (`REFLECTIONS_ALTERNATE`).
                layout_entry(
                    3,
                    S::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    4,
                    S::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
            ],
        });
        let module = match rays {
            Rays::World => shared.shader("probe gi rays", &shader_source(rays), true),
            Rays::Hardware => shared.shader("probe gi rt", &shader_source(rays), true),
            Rays::Software => shared.shader("probe gi", &shader_source(rays), true),
        };
        // Group 1 is what the rays trace: the ray scene, or the probes' own scene with their
        // buffers. Every pipeline of the frame's ray pass carries the ray scene's layout
        // there, also `cs_update`, which traces nothing: a pipeline with another layout at
        // group 1 would have wgpu walk the scene's instances again for the next one.
        let ray_layout = world.then(|| crate::ray_scene::RayScene::layout(shared));
        let scene_layout = ray_layout.as_ref().unwrap_or(&layout);
        let own_groups = [
            Some(&shared.frame_layout),
            Some(scene_layout),
            Some(&layout),
        ];
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe gi"),
            bind_group_layouts: if world {
                &own_groups[..]
            } else {
                &own_groups[..2]
            },
            immediate_size: 0,
        });
        let reflect_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("world reflections"),
                bind_group_layouts: &[
                    Some(&shared.frame_layout),
                    Some(scene_layout),
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
        // The sun's rays: the prepass's normals and depth as the reflections read them, what
        // the rays found, and their count.
        let sun = (rays != Rays::Software).then(|| {
            let inputs = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ray shadow inputs"),
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
                        5,
                        S::COMPUTE,
                        wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: SUN_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                    ),
                    layout_entry(6, S::COMPUTE, storage(false)),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ray shadows"),
                bind_group_layouts: &[
                    Some(&shared.frame_layout),
                    Some(scene_layout),
                    Some(&inputs),
                ],
                immediate_size: 0,
            });
            SunRays {
                pipeline: pipeline("cs_sun", &pipeline_layout),
                layout: inputs,
                out: None,
                counts: GpuCounts::new(device, "ray shadow counts"),
            }
        });
        // Every pixel's reflection: the prepass's normals and depth, the motion, last frame's
        // picture (sampled between its texels), what the reflections found, and their count.
        let full = world.then(|| {
            let texture = |filterable: bool| wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            };
            let inputs = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ray reflection inputs"),
                entries: &[
                    layout_entry(20, S::COMPUTE, texture(false)),
                    layout_entry(
                        21,
                        S::COMPUTE,
                        wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    ),
                    layout_entry(22, S::COMPUTE, texture(false)),
                    layout_entry(23, S::COMPUTE, texture(true)),
                    layout_entry(
                        24,
                        S::COMPUTE,
                        wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba16Float,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                    ),
                    layout_entry(25, S::COMPUTE, storage(false)),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ray reflections"),
                bind_group_layouts: &[
                    Some(&shared.frame_layout),
                    Some(scene_layout),
                    Some(&inputs),
                ],
                immediate_size: 0,
            });
            FullReflections {
                pipeline: pipeline("cs_reflect_full", &pipeline_layout),
                layout: inputs,
                out: None,
                counts: GpuCounts::new(device, "ray reflection counts"),
                inputs: None,
                counting: false,
                traced: false,
            }
        });
        // The mirrors' pictures: what every pixel's reflection reads, the picture's trace, what
        // it found (the colour's level 0 and the depth), and their count.
        let mirrors = world.then(|| {
            let texture = |filterable: bool| wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            };
            let written = |format: wgpu::TextureFormat| wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            };
            let inputs = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ray mirror inputs"),
                entries: &[
                    layout_entry(20, S::COMPUTE, texture(false)),
                    layout_entry(
                        21,
                        S::COMPUTE,
                        wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    ),
                    layout_entry(22, S::COMPUTE, texture(false)),
                    layout_entry(23, S::COMPUTE, texture(true)),
                    layout_entry(26, S::COMPUTE, uniform),
                    layout_entry(27, S::COMPUTE, written(wgpu::TextureFormat::Rgba16Float)),
                    layout_entry(28, S::COMPUTE, written(wgpu::TextureFormat::R32Float)),
                    layout_entry(29, S::COMPUTE, storage(false)),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ray mirror pictures"),
                bind_group_layouts: &[
                    Some(&shared.frame_layout),
                    Some(scene_layout),
                    Some(&inputs),
                ],
                immediate_size: 0,
            });
            MirrorPrograms {
                pipeline: pipeline("cs_mirror_picture", &pipeline_layout),
                layout: inputs,
                counts: GpuCounts::new(device, "ray mirror counts"),
                inputs: Vec::new(),
                counting: false,
            }
        });
        // Rendering's check of the probes: where its sums go.
        let check = world.then(|| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("probe check"),
                entries: &[layout_entry(30, S::COMPUTE, storage(false))],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("probe check"),
                bind_group_layouts: &[
                    Some(&shared.frame_layout),
                    Some(scene_layout),
                    Some(&layout),
                ],
                immediate_size: 0,
            });
            let out = GpuReadback::new(device, "probe check", CHECK_BYTES);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe check"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 30,
                    resource: out.buffer.as_entire_binding(),
                }],
            });
            ProbeCheck {
                pipeline: pipeline("cs_probe_check", &pipeline_layout),
                group,
                out,
                ran: false,
                seen: None,
            }
        });
        let usage = wgpu::BufferUsages::STORAGE;
        let grow_usage = usage | wgpu::BufferUsages::COPY_SRC;
        self.active = Some(Active {
            rays_trace: rays,
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
            // Copied into a grown buffer on the GPU (`append`).
            blas: GrowBuffer::new(device, "probe gi blas", grow_usage, 1 << 16),
            triangles: GrowBuffer::new(device, "probe gi triangles", grow_usage, 1 << 16),
            rays: GrowBuffer::new(device, "probe gi rays", usage, 1 << 20),
            updates: GrowBuffer::new(device, "probe gi updates", usage, 4096),
            group: None,
            hw: hardware.then(|| rt::HwScene::new(device)),
            sun,
            full,
            mirrors,
            check,
            reflect_inputs: None,
            sun_inputs: None,
            sun_counted: false,
            probe_counts: GpuCounts::new(device, "probe gi counts"),
            probes_counting: false,
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

    /// Probes to trace this frame, and those the time budget held back: what fits
    /// `gi_budget_ms` at the measured cost, up to `gi_probes_per_frame` (always that many
    /// without a budget, timestamps or a GPU that is not a CPU). `raised`
    /// (`trial_cut_more::RAY_PROBES` on ray-tracing hardware): [`RAISED_PROBES`] times as many
    /// within [`RAISED_BUDGET`] times the time. `share` is the frame's
    /// [`crate::FramePacing::share`]: the settings come paced already, and the floor and the
    /// count before a measurement are paced the same way, so a second holds as many probes
    /// whatever the frame rate.
    fn probes_this_frame(
        &self,
        shared: &GpuShared,
        settings: &RenderSettings,
        count: u32,
        share: f32,
        raised: bool,
    ) -> (u32, u32) {
        let (most, budget_ms) = if raised {
            let most = settings.gi_probes_per_frame.saturating_mul(RAISED_PROBES);
            let most = crate::settings::gi_probes_cap().map_or(most, |cap| most.min(cap));
            (most, settings.gi_budget_ms * RAISED_BUDGET)
        } else {
            (settings.gi_probes_per_frame, settings.gi_budget_ms)
        };
        let most = most.clamp(1, count);
        let budgeted = budget_ms > 0.0
            && shared.adapter_info.device_type != wgpu::DeviceType::Cpu
            && shared
                .device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY);
        if !budgeted {
            return (most, 0);
        }
        let paced = |n: u32| ((n as f32 * share.clamp(0.25, 1.0)).round() as u32).max(1);
        let least = paced(MIN_PROBES_PER_FRAME).min(most);
        let fits = if self.ms_per_kray <= 0.0 {
            paced(UNMEASURED_PROBES_PER_FRAME).clamp(least, most)
        } else {
            let rays = settings.gi_rays_per_probe.clamp(16, MAX_RAYS) as f32;
            let fit = budget_ms / self.ms_per_kray * 1000.0 / rays;
            (fit as u32).clamp(least, most)
        };
        (fits, most - fits)
    }

    /// A mesh's local bounds at its current version (`None` while it isn't loaded or has no
    /// triangles).
    fn local_bounds(
        &mut self,
        meshes: &Assets<Mesh>,
        handle: Handle<Mesh>,
    ) -> Option<(Vec3, Vec3)> {
        let key = handle.index();
        let version = meshes.version(handle);
        let local = match self.bounds.get(&key) {
            Some(&(v, b)) if v == version => b,
            _ => {
                let aabb = meshes.get(handle)?.aabb();
                let b = (aabb.min, aabb.max);
                self.bounds.insert(key, (version, b));
                b
            }
        };
        (!local.0.cmpgt(local.1).any()).then_some(local)
    }

    /// Bring `entity`'s cache entry up to date (or drop it, gone or hidden). True when its box
    /// was or now is in `region`: its instance must be read again.
    fn refresh(
        &mut self,
        entity: Entity,
        query: &mut QueryState<GatherQuery, GatherFilter>,
        world: &World,
        meshes: &Assets<Mesh>,
        region: &(DVec3, DVec3),
    ) -> bool {
        let old = self.cache.remove(&entity);
        let was_in = old.is_some_and(|c| c.overlaps(region));
        let new = match query.get(world, entity) {
            Ok((_, transform, renderer, resolved)) if renderer.visible => {
                let handle = meshes.canonical(resolved.mesh);
                self.local_bounds(meshes, handle).and_then(|local| {
                    let (lo, hi) = world_box_abs(local, &transform.0);
                    // Too small to change what a probe sees (pebbles, cones, small props): left
                    // out of the rays' scene; screen-space AO darkens around them.
                    ((hi - lo).length() >= 2.0 * MIN_INSTANCE_RADIUS_M).then_some(Cached {
                        lo,
                        hi,
                        affine: transform.0,
                        mesh: handle.index(),
                        version: meshes.version(handle),
                    })
                })
            }
            _ => None,
        };
        let is_in = new.is_some_and(|c| c.overlaps(region));
        if let Some(c) = new {
            self.cache.insert(entity, c);
        }
        if was_in && !is_in {
            self.in_region.remove(&entity);
        }
        if is_in && !was_in {
            self.in_region.insert(entity);
        }
        was_in || is_in
    }

    /// This frame's [`WorldShift`]s combined (none without the world module).
    fn read_shifts(&mut self, world: &World) -> Option<WorldShift> {
        let messages = world.get_resource::<Messages<WorldShift>>()?;
        // Shifts missed (frames the renderer skipped) only cost refreshes: an entity is taken
        // as shifted only where its transform is its cached one moved by the shifts read.
        self.shifts
            .read(messages)
            .copied()
            .reduce(|acc, next| acc.then(&next))
    }

    /// The world moved to a new frame (a floating-origin move, or a jump that re-anchors it):
    /// move every cached box and transform with it. A shift moves every root transform, so the
    /// change ticks name every mesh entity in the world (big_space: recentering "triggers change
    /// detection even for objects that did not move"); refreshing them all took 17 ms on
    /// Tobias's PC when a jump re-anchored the showcase at the lake. Unreal's scene instead
    /// "shifts primitive position and all relevant data" by the offset (`ApplyWorldOffset`);
    /// here an entity whose transform is its cached one moved by the shift keeps its entry.
    /// The instances are read again (the anchor moved under them).
    fn follow_shift(&mut self, s: &WorldShift) {
        let affine = DAffine3::from_rotation_translation(s.rotation, s.translation);
        let translation_only = s.is_translation_only();
        let bounds = &self.bounds;
        for c in self.cache.values_mut() {
            if translation_only {
                c.affine.translation += s.translation;
                c.lo += s.translation;
                c.hi += s.translation;
            } else {
                c.affine = affine * c.affine;
                (c.lo, c.hi) = match bounds.get(&c.mesh) {
                    Some(&(v, local)) if v == c.version => world_box_abs(local, &c.affine),
                    _ => box_under(c.lo, c.hi, &affine),
                };
            }
        }
        // Which boxes are in the region, and every instance, again.
        self.region = None;
        self.gathered = None;
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
                built: self.frame,
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

    /// Forget the hierarchies (or structures) of meshes no instance used in a while, and close
    /// the store's gaps once they're most of it.
    fn forget_unused(&mut self) {
        let frame = self.frame;
        let used: HashSet<u32> = self.placed.instances.iter().map(|i| i.mesh[3]).collect();
        let gone: Vec<u32> = self
            .meshes
            .iter()
            .filter(|(k, e)| frame - e.last_used >= KEEP_UNUSED_FRAMES && !used.contains(k))
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
        let hw = self.active.as_mut().and_then(|a| a.hw.as_mut());
        if let Some(hw) = hw {
            hw.forget_unused(frame, &used);
        }
        let hw = self.active.as_ref().and_then(|a| a.hw.as_ref());
        let known = hw.map_or(self.meshes.len(), |hw| hw.len());
        if self.bounds.len() > 4 * known + 1024 {
            let meshes = &self.meshes;
            self.bounds
                .retain(|k, _| hw.map_or_else(|| meshes.contains_key(k), |hw| hw.has(*k)));
        }
        if self.texture_means.len() > 8192 {
            self.texture_means.clear();
        }
    }

    /// Read `entity` again and update its entry and instance; true when instances came or went
    /// (the instance hierarchy is built again). Instances that changed in place go in `moved`.
    fn gather_one(
        &mut self,
        look: &Look,
        query: &mut QueryState<GatherQuery, GatherFilter>,
        entity: Entity,
        builds: &mut Builds,
        moved: &mut Vec<u32>,
    ) -> bool {
        let read = if self.in_region.contains(&entity) {
            self.read(look, query, entity, builds)
        } else {
            None
        };
        let (old, new) = match read {
            None => {
                // Gone from the world (one that only left the region stays cached).
                if self.in_region.remove(&entity) {
                    self.cache.remove(&entity);
                }
                (self.entries.remove(&entity), None)
            }
            Some((entry, waiting)) => {
                if waiting {
                    self.pending.push(entity);
                }
                let record = entry.record;
                (self.entries.insert(entity, entry), record)
            }
        };
        match (old.and_then(|e| e.record), new) {
            (None, None) => false,
            (Some(a), Some(b)) if a == b => false,
            (Some(a), Some(b)) => match self.placed.slot_of.get(&entity) {
                Some(&slot) => {
                    self.placed.instances[slot as usize] = b.instance;
                    self.placed.boxes[slot as usize] = b.bounds;
                    let ((mesh_a, at_a), (mesh_b, at_b)) = (a.placement(), b.placement());
                    if mesh_a != mesh_b || bytemuck::bytes_of(&at_a) != bytemuck::bytes_of(&at_b) {
                        self.placed.placements[slot as usize] = (mesh_b, at_b);
                        self.placed.placements_changed = true;
                    }
                    moved.push(slot);
                    false
                }
                None => true,
            },
            _ => true,
        }
    }

    /// Read `entity`'s instance: its mesh's hierarchy (built here when small and the frame's
    /// budget allows, on a worker when large), its transform and its material's light.
    fn read(
        &mut self,
        look: &Look,
        query: &mut QueryState<GatherQuery, GatherFilter>,
        entity: Entity,
        builds: &mut Builds,
    ) -> Read {
        let Ok((_, transform, renderer, resolved)) = query.get(look.world, entity) else {
            return None;
        };
        let meshes = look.meshes;
        let handle = meshes.canonical(resolved.mesh);
        let mut entry = Entry {
            mesh: handle,
            mesh_version: meshes.version(handle),
            material: resolved.material,
            material_version: look.materials.map_or(0, |m| m.version(resolved.material)),
            record: None,
        };
        if !renderer.visible {
            return Some((entry, false));
        }
        let material = look
            .materials
            .and_then(|m| m.get(resolved.material))
            .unwrap_or(look.default_material);
        if material.unlit || matches!(material.alpha_mode, AlphaMode::Blend) {
            return Some((entry, false));
        }
        // Swaying leaf cards: as solid triangles they made a roof of the crowns (and cost every
        // ray through them); the rays see the canopy map's leaves instead.
        if look.canopy
            && matches!(material.alpha_mode, AlphaMode::Mask(_))
            && material.wind_sway > 0.0
        {
            return Some((entry, false));
        }
        let key = handle.index();
        let version = entry.mesh_version;
        let Some(local) = self.local_bounds(meshes, handle) else {
            // Not loaded: its version changes when it is.
            return Some((entry, false));
        };
        let hardware = self.active.as_ref().is_some_and(|a| a.hw.is_some());
        let have = match self.active.as_mut().and_then(|a| a.hw.as_mut()) {
            // Built now if the frame's budget allows (before the instances' structure, in the
            // same build).
            Some(hw) => match hw.want(
                &look.shared.device,
                key,
                version,
                meshes.get(handle),
                self.frame,
            ) {
                rt::Want::Ready => Have::At([0, 0], false),
                rt::Want::Older => Have::At([0, 0], true),
                rt::Want::Waiting => Have::Nothing(true),
                rt::Want::Empty => Have::Nothing(false),
            },
            None => self.software_mesh(meshes, handle, key, version, builds),
        };
        let ([root, first_triangle], waiting) = match have {
            Have::At(at, waiting) => (at, waiting),
            Have::Nothing(waiting) => return Some((entry, waiting)),
            Have::KeepOld => {
                // Its hierarchy is still uploading: the instance read before stays (an older
                // version's, intact until a compaction re-reads everything).
                entry.record = self.entries.get(&entity).and_then(|e| e.record);
                return Some((entry, true));
            }
        };
        // Anchor-relative -> mesh, and back.
        let mut rel = transform.0;
        rel.translation -= look.anchor;
        let texture_mean = match (&material.base_color_texture, look.images) {
            (Some(handle), Some(images)) => {
                texture_mean(&mut self.texture_means, look.shared, images, *handle)
            }
            _ => None,
        };
        let (albedo, emissive) = material_light(material, texture_mean);
        entry.record = Some(Record {
            instance: GpuInstance {
                to_local: affine_rows(&rel.inverse()),
                mesh: [root, first_triangle, material.double_sided as u32, key],
                albedo: albedo.extend(0.0).to_array(),
                emissive: emissive.extend(0.0).to_array(),
            },
            bounds: world_box(local, &transform.0, look.anchor),
            // Only the hardware's structure takes it (so the software walk's records compare as
            // they did without it).
            to_world: if hardware {
                bytemuck::cast(affine_rows(&rel))
            } else {
                [0.0; 12]
            },
        });
        Some((entry, waiting))
    }

    /// Mesh `key`'s hierarchy in the software store for an instance of it: built here when
    /// small and the frame's budget allows (after the reads, side by side), on a worker when
    /// large.
    fn software_mesh(
        &mut self,
        meshes: &Assets<Mesh>,
        handle: Handle<Mesh>,
        key: u32,
        version: u64,
        builds: &mut Builds,
    ) -> Have {
        // Read again next frame: its hierarchy is building, waits for the frame's budget, or
        // is an older version's (the rays see that one meanwhile).
        let mut waiting = false;
        let stored = self.meshes.get(&key).map(|e| (e.version, e.built));
        if stored.is_none_or(|(v, _)| v != version) {
            waiting = true;
            let recent = stored.is_some_and(|(_, at)| self.frame < at + REBUILD_AFTER_FRAMES);
            if !recent && !self.jobs.contains_key(&key) {
                let Some(mesh) = meshes.get(handle) else {
                    return Have::Nothing(false);
                };
                let triangles = mesh.indices.len() / 3;
                let positions =
                    || -> Vec<[f32; 3]> { mesh.vertices.iter().map(|v| v.position).collect() };
                if triangles > INLINE_BUILD_TRIANGLES {
                    // Larger meshes build on a worker; until it's done the rays see the older
                    // version, or miss a new mesh.
                    if self.jobs.len() < MAX_BUILD_JOBS {
                        let positions = positions();
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
                    }
                } else if builds.queued.iter().any(|q| q.0 == key) {
                    // Already asked for this frame.
                } else if !builds.closed
                    && (builds.triangles == 0
                        || builds.triangles + triangles <= BUILD_TRIANGLES_PER_FRAME)
                {
                    // Built after the reads, side by side with the frame's others.
                    builds.triangles += triangles;
                    builds.queued.push((key, version, handle));
                }
            }
        }
        let Some(mesh_entry) = self.meshes.get_mut(&key) else {
            return Have::Nothing(waiting);
        };
        mesh_entry.last_used = self.frame;
        if !mesh_entry.uploaded || mesh_entry.empty {
            return Have::Nothing(waiting);
        }
        if !self.store.holds(mesh_entry) {
            return Have::KeepOld;
        }
        Have::At([mesh_entry.root, mesh_entry.first_triangle], waiting)
    }

    /// The main view's half on the main thread: place the grid, gather the instances near it,
    /// build what hierarchies are missing, upload, and fill the frame uniform's probe fields.
    /// Returns true when the atlas was recreated (rebuild the frame groups).
    /// Make the query [`extract`](Self::extract) reads with (registering its components):
    /// before it, with the world borrowed mutably.
    pub fn ensure_query(&mut self, world: &mut World) {
        if self.query.is_none() {
            self.query = Some(world.query_filtered());
        }
        if self.changes.is_none() {
            self.changes = Some(world.query_filtered());
        }
    }

    /// Gather the probes' own scene (the software walk's, or the hardware structures of
    /// `rt.rs`): read the mesh entities that changed around `grid`, build what their meshes
    /// need and place their instances. Not while the rays trace the frame's ray scene.
    #[allow(clippy::too_many_arguments)]
    fn gather(
        &mut self,
        world: &World,
        shared: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        grid: &Grid,
        started: std::time::Instant,
        recreated: bool,
    ) -> Gathered {
        let (spacing, dims, origin) = (grid.spacing, grid.dims, grid.origin);
        // Hierarchies the workers finished: the entities waiting on them are pending.
        let limit = shared.device.limits().max_storage_buffer_binding_size;
        self.collect_builds(limit);

        // Instances are kept relative to an anchor that moves in whole cells, and gathered from a
        // region snapped outward, so a moving camera changes neither most frames.
        let anchor_cell = (view.origin / ANCHOR_CELL).floor();
        let anchor = anchor_cell * ANCHOR_CELL;
        let first_probe = (origin.as_dvec3() + 0.5) * spacing;
        let margin = spacing * 16.0;
        let lo = ((first_probe - margin) / REGION_SNAP).floor();
        let hi = ((first_probe + (dims - 1).as_dvec3() * spacing + margin) / REGION_SNAP).ceil();
        let region_abs = (lo * REGION_SNAP, hi * REGION_SNAP);
        // Which mesh entities changed, from their change ticks alone.
        let mut query = self
            .query
            .take()
            .or_else(|| world.try_query_filtered())
            .expect("ProbeGi::ensure_query runs before extract");
        let shift = self.read_shifts(world);
        if let Some(s) = &shift {
            self.follow_shift(s);
        }
        let mut changed: Vec<Entity> = Vec::new();
        let mut shifted = 0u32;
        let mut unchanged = 0u32;
        let entities = match self.tick {
            Some(tick) => {
                let mut changes = self
                    .changes
                    .take()
                    .or_else(|| world.try_query_filtered())
                    .expect("ProbeGi::ensure_query runs before extract");
                for (entity, transform, renderer, resolved) in
                    crate::features::iter_since(&mut changes, world, tick)
                {
                    let renderer_changed = renderer.is_changed();
                    let resolved_changed = resolved.is_changed();
                    if !transform.is_changed() && !renderer_changed && !resolved_changed {
                        continue;
                    }
                    let cached = self.cache.get(&entity);
                    // Hidden and not cached: a refresh would find nothing to do.
                    if !renderer.visible && cached.is_none() {
                        continue;
                    }
                    // Its transform is its cached one (moved by the shift, if any), it is still
                    // visible and names the same mesh and material: nothing the gather reads
                    // changed. Systems that write a transform or renderer every frame without
                    // changing it mark thousands like this at the lake on Tobias's PC.
                    if !resolved_changed
                        && renderer.visible
                        && cached.is_some_and(|c| c.affine.abs_diff_eq(transform.0, UNMOVED))
                    {
                        if shift.is_some() {
                            shifted += 1;
                        } else {
                            unchanged += 1;
                        }
                        continue;
                    }
                    changed.push(entity);
                }
                self.changes = Some(changes);
                // The filter is by archetype alone, so every entity of a matched archetype is one.
                query.update_archetypes(world);
                query
                    .matched_archetypes()
                    .filter_map(|id| world.archetypes().get(id))
                    .map(|a| a.len() as usize)
                    .sum()
            }
            None => {
                changed.extend(query.iter(world).map(|q| q.0));
                changed.len()
            }
        };
        // Changes from here on get a newer tick, so the next frame finds them.
        self.tick = Some(crate::features::take_change_tick(world));
        let meshes = world.get_resource::<Assets<Mesh>>();
        let materials = world.get_resource::<Assets<StandardMaterial>>();

        // The entities whose instance must be read again.
        let build_started = std::time::Instant::now();
        let mut parts = [0.0f32; PARTS.len()];
        parts[0] = (build_started - started).as_secs_f32() * 1000.0;
        let mut lap = build_started;
        let mut part = |i: usize| {
            let now = std::time::Instant::now();
            parts[i] = (now - lap).as_secs_f32() * 1000.0;
            lap = now;
        };
        let mut dirty: Vec<Entity> = Vec::new();
        if let Some(meshes) = meshes {
            // The cache: the changed entities, then a share of the rest checked again.
            for &entity in &changed {
                if self.refresh(entity, &mut query, world, meshes, &region_abs) {
                    dirty.push(entity);
                }
            }
            if self.revalidate_at >= self.revalidate.len() {
                self.revalidate.clear();
                self.revalidate.extend(self.cache.keys().copied());
                self.revalidate_at = 0;
            }
            let end = (self.revalidate_at + REVALIDATE_PER_FRAME).min(self.revalidate.len());
            for i in self.revalidate_at..end {
                let entity = self.revalidate[i];
                let current = match (self.cache.get(&entity), query.get(world, entity)) {
                    (Some(c), Ok((_, _, renderer, resolved))) => {
                        renderer.visible
                            && meshes.canonical(resolved.mesh).index() == c.mesh
                            && meshes.version(meshes.canonical(resolved.mesh)) == c.version
                    }
                    (None, _) => true,
                    (Some(_), Err(_)) => false,
                };
                if !current && self.refresh(entity, &mut query, world, meshes, &region_abs) {
                    dirty.push(entity);
                }
            }
            self.revalidate_at = end;
        }
        part(1);
        if self.region != Some(region_abs) {
            // The camera crossed a snap cell: which cached boxes are in the new region; those
            // that entered or left are read again.
            self.region = Some(region_abs);
            self.in_region.clear();
            self.in_region.extend(
                self.cache
                    .iter()
                    .filter(|(_, c)| c.overlaps(&region_abs))
                    .map(|(e, _)| *e),
            );
            let entries = &self.entries;
            dirty.extend(self.in_region.iter().filter(|e| !entries.contains_key(*e)));
            let in_region = &self.in_region;
            dirty.extend(entries.keys().filter(|e| !in_region.contains(*e)));
        }
        if entities != self.entities {
            // Some went (or came, and those changed): the entries whose entity is gone.
            let query = &query;
            dirty.extend(
                self.entries
                    .keys()
                    .filter(|&&e| query.get_manual(world, e).is_err()),
            );
        }
        self.entities = entities;
        // An asset changed somewhere: the entries whose mesh or material it was.
        let marks = (
            meshes.map_or((0, 0), |m| (m.latest_version(), m.removals())),
            materials.map_or((0, 0), |m| (m.latest_version(), m.removals())),
        );
        if marks != self.asset_marks {
            self.asset_marks = marks;
            for (&entity, entry) in &self.entries {
                let mesh = meshes.map_or(0, |m| m.version(entry.mesh));
                let material = materials.map_or(0, |m| m.version(entry.material));
                if mesh != entry.mesh_version || material != entry.material_version {
                    dirty.push(entity);
                }
            }
        }
        dirty.append(&mut self.pending);
        // And a share of the rest, in turn.
        if self.recheck_at >= self.recheck.len() {
            self.recheck.clear();
            self.recheck.extend(self.entries.keys().copied());
            self.recheck_at = 0;
        }
        let end = (self.recheck_at + RECHECK_PER_FRAME).min(self.recheck.len());
        dirty.extend_from_slice(&self.recheck[self.recheck_at..end]);
        self.recheck_at = end;

        // Hierarchies no instance used in a while go; a compaction moves the rest, and then
        // every instance is read again.
        if self.frame.is_multiple_of(FORGET_EVERY_FRAMES) {
            self.forget_unused();
        }
        let key = GatherKey {
            anchor: anchor_cell.as_ivec3(),
            layout: self.store.layout,
        };
        let full = self.gathered != Some(key);
        if full {
            self.entries.clear();
            dirty.clear();
            dirty.extend(self.in_region.iter().copied());
            self.gathered = Some(key);
        }
        part(2);
        dirty.sort_unstable();
        dirty.dedup();

        let mut builds = Builds::default();
        // Instances came or went (the hierarchy is built again), or changed in place.
        let mut membership = full;
        let mut moved: Vec<u32> = Vec::new();
        if let Some(meshes) = meshes {
            let default_material = StandardMaterial::default();
            let look = Look {
                world,
                meshes,
                materials,
                images: world.get_resource::<Assets<crate::Image>>(),
                shared,
                default_material: &default_material,
                anchor,
                canopy: settings.canopy,
            };
            for &entity in &dirty {
                membership |= self.gather_one(&look, &mut query, entity, &mut builds, &mut moved);
            }
            // The small meshes the reads asked for, built side by side; then the entities
            // waiting on them read again, so they're in this frame's scene.
            part(3);
            if !builds.queued.is_empty() {
                let done = build_small(meshes, &builds.queued);
                for (key, version, bvh) in done {
                    self.add_mesh(key, version, bvh, limit);
                }
                let keys: HashSet<u32> = builds.queued.iter().map(|q| q.0).collect();
                let entries = &self.entries;
                let (again, rest): (Vec<Entity>, Vec<Entity>) = std::mem::take(&mut self.pending)
                    .into_iter()
                    .partition(|e| {
                        entries
                            .get(e)
                            .is_some_and(|entry| keys.contains(&entry.mesh.index()))
                    });
                self.pending = rest;
                builds.closed = true;
                for entity in again {
                    membership |=
                        self.gather_one(&look, &mut query, entity, &mut builds, &mut moved);
                }
            }
        }
        part(4);
        let built = builds.triangles;
        self.query = Some(query);
        let build_ms = build_started.elapsed().as_secs_f32() * 1000.0;

        let active = self.active.as_mut().expect("activated before the gather");
        let device = &shared.device;
        let queue = &shared.queue;
        let hardware = active.hw.is_some();
        let mut regroup = false;
        if !hardware {
            regroup |= self.store.upload(active, device, queue);
        }
        part(5);
        let placed = &mut self.placed;
        // (A refit hierarchy slows the software walk; the hardware's is built new each time.)
        let rebuild = membership
            || (!hardware && !moved.is_empty() && placed.refits >= REFITS_BEFORE_REBUILD);
        if rebuild {
            // Sorted, so the same scene builds the same hierarchy.
            let mut list: Vec<(Entity, Record)> = self
                .entries
                .iter()
                .filter_map(|(e, entry)| entry.record.map(|r| (*e, r)))
                .collect();
            list.sort_unstable_by_key(|(e, _)| *e);
            let boxes: Vec<(Vec3, Vec3)> = list.iter().map(|(_, r)| r.bounds).collect();
            // The software walk takes the instances in its hierarchy's order; the GPU builds
            // its own over them as they come.
            let (tlas, order) = if hardware {
                (Vec::new(), (0..list.len() as u32).collect())
            } else {
                bvh::build(&boxes)
            };
            placed.entities = order.iter().map(|&i| list[i as usize].0).collect();
            placed.instances = order.iter().map(|&i| list[i as usize].1.instance).collect();
            placed.boxes = order.iter().map(|&i| boxes[i as usize]).collect();
            placed.placements = order
                .iter()
                .map(|&i| list[i as usize].1.placement())
                .collect();
            placed.slot_of = placed
                .entities
                .iter()
                .enumerate()
                .map(|(slot, e)| (*e, slot as u32))
                .collect();
            placed.tlas = tlas;
            placed.refits = 0;
            if !hardware {
                regroup |= active
                    .tlas
                    .write(device, queue, bytemuck::cast_slice(&placed.tlas));
            }
            let none = [GpuInstance::zeroed()];
            let instances = if placed.instances.is_empty() {
                &none[..]
            } else {
                &placed.instances[..]
            };
            regroup |= active
                .instances
                .write(device, queue, bytemuck::cast_slice(instances));
        } else if !moved.is_empty() {
            // Same instances, some moved or changed: fit the hierarchy around them again and
            // write only what changed.
            if !hardware {
                bvh::refit(&mut placed.tlas, &placed.boxes);
                placed.refits += 1;
                regroup |= active
                    .tlas
                    .write(device, queue, bytemuck::cast_slice(&placed.tlas));
            }
            moved.sort_unstable();
            moved.dedup();
            if moved.len() * 4 > placed.instances.len() {
                regroup |=
                    active
                        .instances
                        .write(device, queue, bytemuck::cast_slice(&placed.instances));
            } else {
                let size = size_of::<GpuInstance>() as u64;
                for &slot in &moved {
                    queue.write_buffer(
                        &active.instances.buffer,
                        slot as u64 * size,
                        bytemuck::bytes_of(&placed.instances[slot as usize]),
                    );
                }
            }
        }
        let gathered = rebuild || !moved.is_empty();
        let placements_changed = std::mem::take(&mut placed.placements_changed) || rebuild;
        if let Some(hw) = active.hw.as_mut() {
            // The instances' structure, when they came, went or moved, or a mesh's structure
            // they point at changed (built again, compacted); the GPU builds it in
            // `encode_scene`.
            // (And every frame while a measurement adds its copies, and once more after it.)
            let stress = settings.ray_scene_stress;
            if placements_changed || hw.tlas_dirty() || stress != 0 || hw.stress() != 0 {
                regroup |= hw.place(device, &placed.placements, stress);
            }
        }
        part(6);
        // (A recreated scene's group is made by the caller.)
        let _ = recreated;
        Gathered {
            regroup,
            instances: placed.instances.len() as u32,
            anchor,
            gathered,
            full,
            rebuilt: rebuild,
            built: built as u32,
            read: dirty.len() as u32,
            changed: changed.len() as u32,
            shifted,
            unchanged,
            build_ms,
            parts,
        }
    }

    /// Reads the world shared, so it runs beside the features' shared prepares; call
    /// [`ensure_query`](Self::ensure_query) first. `ray_world`: the frame's ray scene is on
    /// (the renderer made it), so the rays trace that and the probes gather nothing
    /// themselves.
    pub fn extract(
        &mut self,
        world: &World,
        shared: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        uniform: &mut FrameUniform,
        ray_world: bool,
    ) -> bool {
        self.work = None;
        self.frame += 1;
        let lost = self
            .active
            .as_ref()
            .and_then(|a| a.hw.as_ref())
            .is_some_and(|hw| hw.lost());
        let losses = self.hw_losses + lost as u32;
        if lost {
            tracing::warn!(
                "probe GI: a submission holding the ray-tracing structures' builds failed; \
                 starting the scene over{}",
                if losses >= HW_LOSSES_MAX {
                    " in software for the rest of the run"
                } else {
                    ""
                }
            );
        }
        if !settings.probe_gi {
            if self.active.is_some() {
                // Off: free the GPU memory, keep a one-tile stand-in (and the count of losses,
                // which holds for the rest of the run).
                *self = Self::new(&shared.device);
                self.hw_losses = losses;
                return true;
            }
            return false;
        }
        let started = std::time::Instant::now();
        let mut recreated = false;
        let rays_trace = if ray_world {
            Rays::World
        } else if settings.gi_hardware_rays && shared.ray_queries() && losses < HW_LOSSES_MAX {
            Rays::Hardware
        } else {
            Rays::Software
        };
        if lost
            || self
                .active
                .as_ref()
                .is_some_and(|a| a.rays_trace != rays_trace)
        {
            // The rays switched paths, or the GPU's copy of the scene is unknown: start over.
            let frame = self.frame;
            *self = Self::new(&shared.device);
            self.frame = frame;
        }
        self.hw_losses = losses;
        if self.active.is_none() {
            self.activate(shared, rays_trace);
            recreated = true;
        }
        if let Some(hw) = self.active.as_mut().and_then(|a| a.hw.as_mut()) {
            hw.begin_frame();
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
        let share = world
            .get_resource::<crate::FramePacing>()
            .map_or(1.0, |p| p.share);
        // `trial_cut_more::RAY_PROBES`: four times the probes, and stuck ones asleep, where the
        // rays trace on ray-tracing hardware.
        let raised = settings.trial_cuts_more & crate::settings::trial_cut_more::RAY_PROBES != 0
            && rays_trace != Rays::Software;
        let (per_frame, budget_cut) =
            self.probes_this_frame(shared, settings, count, share, raised);
        let rays = settings.gi_rays_per_probe.clamp(16, MAX_RAYS);

        // Probes that scrolled in go first, then the rest in turn; those that scrolled in and
        // wait are cleared after them.
        static QUEUE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let queue = *QUEUE.get_or_init(|| {
            !matches!(
                std::env::var("ENGINE_GI_SCROLL_QUEUE").as_deref(),
                Ok("off" | "0")
            )
        });
        // A jump in the light (the clock set from day to night, a scene's sun switched) voids
        // every probe's history: blended a tenth a trace, and the asleep ones tracing one turn in
        // eight, the old light lingered for hundreds of frames (the market at 22:00 lit at
        // 400 lux by the sky the probes saw when it was still day, 10 Nov).
        let light = (
            view.sun_dir,
            view.sun_color.dot(Vec3::new(0.2126, 0.7152, 0.0722)),
        );
        if self.light.is_some_and(|last| light_jumped(last, light)) {
            self.schedule.relight();
        }
        self.light = Some(light);
        let plan = self.schedule.plan(self.grid, grid, per_frame, queue);
        self.grid = Some(grid);
        let traced = plan.traced.len() as u32;
        let cleared = plan.cleared.len() as u32;
        let mut updates = plan.traced;
        updates.extend_from_slice(&plan.cleared);

        // Probes sit between whole multiples of the spacing: level design snaps walls and floors
        // to whole meters, and a probe on a surface sees through it along its plane.
        let first_probe = (origin.as_dvec3() + 0.5) * spacing;
        let rel_origin = first_probe - view.origin;
        // The probes' own scene, unless the rays trace the frame's ray scene.
        let own = if rays_trace == Rays::World {
            Gathered::default()
        } else {
            self.gather(world, shared, view, settings, &grid, started, recreated)
        };
        let active = self.active.as_mut().expect("activated above");
        let device = &shared.device;
        let queue = &shared.queue;
        let mut regroup = own.regroup || recreated || active.group.is_none();
        let instance_count = own.instances;
        regroup |= active
            .updates
            .write(device, queue, bytemuck::cast_slice(&updates));
        regroup |= active
            .rays
            .reserve(device, traced.max(1) as u64 * rays as u64 * 16);
        let r = rotation(self.frame);
        let pass = GiPass {
            counts: [traced, rays, cleared, count],
            rotation: [0, 1, 2].map(|i| r.row(i).extend(0.0).to_array()),
            params: [
                settings.gi_hysteresis.clamp(0.0, 0.995),
                50.0,
                instance_count as f32,
                settings.gi_shadowed_lights as f32,
            ],
            world_origin: origin
                .extend(if raised { GI_STUCK_SLEEP } else { 0 })
                .to_array(),
            anchor: (own.anchor - view.origin).as_vec3().extend(0.0).to_array(),
        };
        queue.write_buffer(&active.uniform, 0, bytemuck::bytes_of(&pass));
        if regroup {
            fn buffer(b: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
                b.as_entire_binding()
            }
            fn entry(
                binding: u32,
                resource: wgpu::BindingResource<'_>,
            ) -> wgpu::BindGroupEntry<'_> {
                wgpu::BindGroupEntry { binding, resource }
            }
            let mut entries = if active.rays_trace == Rays::World {
                vec![
                    entry(10, buffer(&active.uniform)),
                    entry(15, buffer(&active.rays.buffer)),
                    entry(16, buffer(&active.probe_state)),
                    entry(17, wgpu::BindingResource::TextureView(&active.next_view)),
                    entry(18, buffer(&active.updates.buffer)),
                    entry(19, buffer(&active.probe_counts.counts)),
                ]
            } else {
                vec![
                    entry(0, buffer(&active.uniform)),
                    entry(2, buffer(&active.instances.buffer)),
                    entry(5, buffer(&active.rays.buffer)),
                    entry(6, buffer(&active.probe_state)),
                    entry(7, wgpu::BindingResource::TextureView(&active.next_view)),
                    entry(8, buffer(&active.updates.buffer)),
                    entry(9, buffer(&active.probe_counts.counts)),
                ]
            };
            match (&active.hw, active.rays_trace) {
                (Some(hw), _) => entries.push(entry(1, hw.binding())),
                (None, Rays::World) => {}
                (None, _) => entries.extend([
                    entry(1, buffer(&active.tlas.buffer)),
                    entry(3, buffer(&active.blas.buffer)),
                    entry(4, buffer(&active.triangles.buffer)),
                ]),
            }
            entries.sort_by_key(|e| e.binding);
            active.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe gi"),
                layout: &active.layout,
                entries: &entries,
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
            probes: traced,
            cleared,
            scrolled: plan.scrolled_in > 0,
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
        uniform.gi_grid = [offset.x, offset.y, offset.z, self.schedule.epoch as f32];
        let rays_now = (traced * rays) as f32;
        self.rays_lately = if self.rays_lately > 0.0 {
            self.rays_lately * 0.7 + rays_now * 0.3
        } else {
            rays_now
        };
        let hw = self.active.as_ref().and_then(|a| a.hw.as_ref());
        self.stats = Stats {
            probes: traced,
            budget_cut,
            raised,
            scrolled_in: plan.scrolled_in,
            cleared,
            instances: instance_count,
            meshes: hw.map_or(self.meshes.len(), |hw| hw.len()) as u32,
            built_triangles: hw.map_or(own.built, |hw| hw.stats().built_triangles),
            building: self.jobs.len() as u32,
            gathered: own.gathered,
            full: own.full,
            read: own.read,
            changed: own.changed,
            shifted: own.shifted,
            unchanged: own.unchanged,
            rebuilt: own.rebuilt,
            build_ms: own.build_ms,
            parts: own.parts,
            extract_ms: started.elapsed().as_secs_f32() * 1000.0,
        };
        if self.stats.extract_ms > SLOW_EXTRACT_MS {
            let s = &self.stats;
            let parts: Vec<String> = PARTS
                .iter()
                .zip(s.parts)
                .map(|(n, ms)| format!("{n} {ms:.1}"))
                .collect();
            tracing::info!(
                "probe GI: extract {:.1} ms ({}); {} changed, {} read, full {}, rebuilt {}, {} \
                 instances, {} built triangles, {} meshes, store {} nodes {} corners",
                s.extract_ms,
                parts.join(", "),
                s.changed,
                s.read,
                s.full,
                s.rebuilt,
                s.instances,
                s.built_triangles,
                s.meshes,
                self.store.nodes.len(),
                self.store.corners.len(),
            );
        }
        recreated
    }

    /// Build this frame's acceleration structures on the hardware path (before
    /// [`encode`](Self::encode) and [`encode_reflections`](Self::encode_reflections) trace
    /// through them): the meshes' new ones, and the instances' when they changed. Nothing on
    /// the software path.
    pub fn encode_scene(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(hw) = self.active.as_mut().and_then(|a| a.hw.as_mut()) {
            hw.encode(encoder);
        }
    }

    /// On the hardware path, swap the structures whose compaction is ready for their compacted
    /// copies. Before [`extract`](Self::extract), while no other thread submits: wgpu's
    /// compaction and a submit on another thread can deadlock (`rt::HwScene::swap_compacted`),
    /// and the prepare threads that run the extract submit buffer copies.
    pub fn swap_compacted(&mut self, queue: &wgpu::Queue) {
        if let Some(hw) = self.active.as_mut().and_then(|a| a.hw.as_mut()) {
            hw.swap_compacted(queue);
        }
    }

    /// After a submit (`failed`: wgpu rejected it): the hardware path counts it (structures it
    /// retired are freed once the GPU is past it), asks for its new structures' compaction (only
    /// possible once their builds were submitted), and starts its scene over next frame if the
    /// submission held its builds and failed.
    pub fn after_submit(&mut self, queue: &wgpu::Queue, failed: bool) {
        if let Some(hw) = self.active.as_mut().and_then(|a| a.hw.as_mut()) {
            hw.after_submit(queue, failed);
        }
    }

    /// Whether the rays trace on the GPU's ray-tracing hardware.
    pub fn hardware_rays(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.rays_trace != Rays::Software)
    }

    /// Whether the rays trace the frame's ray scene (as the last extract was told): they are
    /// then recorded by [`begin_rays`](Self::begin_rays), the three `dispatch_` calls and
    /// [`end_rays`](Self::end_rays) in the frame's one ray pass, and [`encode`](Self::encode),
    /// [`encode_reflections`](Self::encode_reflections) and [`encode_sun`](Self::encode_sun)
    /// record nothing.
    pub fn traces_ray_world(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.rays_trace == Rays::World)
    }

    /// Trace and update this frame's probes (after the environment map, before anything shades
    /// with them). `frame_group`: the view's scene group (it binds the atlas being read).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, frame_group: &wgpu::BindGroup) {
        if self.traces_ray_world() {
            return;
        }
        let work = self.work.take();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        // The counts of what an update did, as they arrive.
        active.probe_counts.take();
        let (Some(work), Some(group)) = (work, &active.group) else {
            return;
        };
        active.probe_counts.clear(encoder);
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
            pass.dispatch_workgroups(work.probes + work.cleared, 1, 1);
        }
        active.probe_counts.copy(encoder, true);
        copy_atlas_rows(encoder, &active.next, &self.atlas, &work.rows);
    }

    /// Trace world reflections into `targets.world_reflection` from the prepass (after it, before
    /// screen-space reflections). False, writing nothing, while probe GI is off or has no scene
    /// yet: the target then holds no reflections only if the caller cleared it. `alternate`
    /// (`trial_cut::REFLECTIONS_ALTERNATE`): every other texel is traced, in a checkerboard
    /// that swaps each frame, and the rest take last frame's reflection of their surface.
    pub fn encode_reflections(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        targets: &crate::targets::Targets,
        alternate: bool,
    ) -> bool {
        if self.traces_ray_world() {
            return false;
        }
        let Some(inputs) = self.reflection_inputs(device, encoder, targets, alternate) else {
            return false;
        };
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        let Some(group) = &active.group else {
            return false;
        };
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

    /// The world reflections' inputs and output for this frame (group 2 of `cs_reflect`), and
    /// last frame's reflections copied aside when `alternate` asks for them. `None` before
    /// the probes have a scene.
    fn reflection_inputs(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &crate::targets::Targets,
        alternate: bool,
    ) -> Option<wgpu::BindGroup> {
        let active = self.active.as_ref()?;
        active.group.as_ref()?;
        let now = &targets.world_reflection;
        if !alternate {
            self.reflect_last = None;
        } else {
            let last = match &mut self.reflect_last {
                Some(t) if (t.width, t.height) == (now.width, now.height) => t,
                slot => slot.insert(crate::targets::Tex::new(
                    device,
                    "world reflections, last frame",
                    now.width,
                    now.height,
                    wgpu::TextureFormat::Rgba16Float,
                    wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                )),
            };
            encoder.copy_texture_to_texture(
                now.texture.as_image_copy(),
                last.texture.as_image_copy(),
                now.texture.size(),
            );
        }
        // (Without the switch nothing reads it: any texture the pass doesn't write.)
        let last = self
            .reflect_last
            .as_ref()
            .map_or(&targets.motion.view, |t| &t.view);
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(last),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&targets.motion.view),
                },
            ],
        });
        Some(inputs)
    }

    /// The sun's measuring rays are not shot this frame (neither [`begin_rays`](Self::begin_rays)
    /// nor [`encode_sun`](Self::encode_sun) took them): `render.gi.sun_rays` and `sun_hits`
    /// read -1, not the last frame that shot them, and a copy still on its way is let go.
    pub fn forget_sun_counts(&mut self) {
        if let Some(sun) = self.active.as_mut().and_then(|a| a.sun.as_mut()) {
            sun.counts.take();
            sun.counts.latest = None;
        }
    }

    /// Shoot one ray toward the sun from every pixel the prepass drew
    /// (`RenderSettings::ray_sun_shadows`, a measurement: nothing reads what they find), after
    /// the prepass's depth was copied. `count` (`shade_skip::COUNT_MESH_RUNS`): the pass counts
    /// its rays and those stopped, for `render.gi.sun_rays` and `sun_hits` a few frames on.
    /// False, recording nothing, off the hardware path or before the scene's first frame.
    pub fn encode_sun(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        targets: &crate::targets::Targets,
        count: bool,
    ) -> bool {
        if self.traces_ray_world() {
            return false;
        }
        let Some(inputs) = self.sun_inputs(device, encoder, targets, count) else {
            return false;
        };
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        let (Some(group), Some(sun)) = (&active.group, active.sun.as_mut()) else {
            return false;
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ray shadows"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&sun.pipeline);
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.set_bind_group(2, &inputs, &[]);
            pass.dispatch_workgroups(targets.width.div_ceil(8), targets.height.div_ceil(8), 1);
        }
        sun.counts.copy(encoder, count);
        true
    }

    /// The sun's rays' inputs and output for this frame (group 2 of `cs_sun`), with the
    /// counts that arrived taken in and this frame's cleared when `count` asks for them.
    /// `None` off ray-tracing hardware or before the probes have a scene.
    fn sun_inputs(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &crate::targets::Targets,
        count: bool,
    ) -> Option<wgpu::BindGroup> {
        let active = self.active.as_mut()?;
        active.group.as_ref()?;
        let sun = active.sun.as_mut()?;
        sun.counts.take();
        if !count {
            sun.counts.latest = None;
        }
        let size = (targets.width, targets.height);
        let out = match &mut sun.out {
            Some(t) if (t.width, t.height) == size => t,
            slot => slot.insert(crate::targets::Tex::new(
                device,
                "ray shadows",
                size.0,
                size.1,
                SUN_FORMAT,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            )),
        };
        let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray shadow inputs"),
            layout: &sun.layout,
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
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&out.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: sun.counts.counts.as_entire_binding(),
                },
            ],
        });
        if count {
            sun.counts.clear(encoder);
        }
        Some(inputs)
    }

    /// Every pixel's reflection's inputs and output for this frame (group 2 of
    /// `cs_reflect_full`), its count cleared when `count` asks for it. `None` without the
    /// frame's ray scene or before the probes have a scene.
    fn full_inputs(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &crate::targets::Targets,
        picture: &wgpu::TextureView,
        count: bool,
    ) -> Option<wgpu::BindGroup> {
        let active = self.active.as_mut()?;
        active.group.as_ref()?;
        let full = active.full.as_mut()?;
        full.counts.take();
        if !count {
            full.counts.latest = None;
        }
        let size = (targets.width, targets.height);
        let out = match &mut full.out {
            Some(t) if (t.width, t.height) == size => t,
            slot => slot.insert(crate::targets::Tex::new(
                device,
                "ray reflections",
                size.0,
                size.1,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            )),
        };
        fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            }
        }
        let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray reflection inputs"),
            layout: &full.layout,
            entries: &[
                view(20, &targets.prepass_normal.view),
                view(21, &targets.prepass_depth.view),
                view(22, &targets.motion.view),
                view(23, picture),
                view(24, &out.view),
                wgpu::BindGroupEntry {
                    binding: 25,
                    resource: full.counts.counts.as_entire_binding(),
                },
            ],
        });
        if count {
            full.counts.clear(encoder);
        }
        full.counting = count;
        Some(inputs)
    }

    /// Every pixel's reflection as the last frame that traced it left it (ray_reflect.wgsl
    /// `full_out`): what ssr.wgsl `fs_reflect_resolve` blurs over each pixel's lobe. `None`
    /// unless that frame was the last to ask for reflections.
    pub fn full_reflections(&self) -> Option<&crate::targets::Tex> {
        let full = self.active.as_ref()?.full.as_ref()?;
        full.out.as_ref().filter(|_| full.traced)
    }

    /// Before the frame's ray pass, while the rays trace the frame's ray scene: make what
    /// its dispatches bind. `reflections`: trace world reflections at half size into
    /// `targets.world_reflection`, or every pixel's reflection ([`ReflectionRays`]); `sun`:
    /// shoot the sun's rays (its value: count them), see [`encode_sun`](Self::encode_sun);
    /// `check`: Rendering's check of the probes ([`dispatch_check`](Self::dispatch_check)).
    /// Says which the pass holds.
    pub fn begin_rays(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &crate::targets::Targets,
        reflections: Option<ReflectionRays<'_>>,
        sun: Option<bool>,
        check: bool,
    ) -> RayWork {
        let mut work = RayWork::default();
        if !self.traces_ray_world() || self.active.as_ref().is_none_or(|a| a.group.is_none()) {
            return work;
        }
        work.probes = self.work.is_some();
        work.scrolled = self.work.as_ref().is_some_and(|w| w.scrolled);
        let (mut reflect_inputs, mut full_inputs) = (None, None);
        match reflections {
            Some(ReflectionRays::Half { alternate }) => {
                reflect_inputs = self.reflection_inputs(device, encoder, targets, alternate);
            }
            Some(ReflectionRays::Full { picture, count }) => {
                full_inputs = self.full_inputs(device, encoder, targets, picture, count);
            }
            None => {}
        }
        let sun_inputs = sun.and_then(|count| self.sun_inputs(device, encoder, targets, count));
        work.reflections = reflect_inputs.is_some();
        work.full_reflections = full_inputs.is_some();
        work.sun = sun_inputs.is_some();
        if let Some(active) = self.active.as_mut() {
            if let Some(full) = active.full.as_mut() {
                full.traced = work.full_reflections;
                if !work.full_reflections {
                    // (A frame without them frees their 29.5 MB at 2560x1440.)
                    full.out = None;
                    full.counting = false;
                    full.counts.latest = None;
                }
                full.inputs = full_inputs;
            }
            if let Some(probe_check) = active.check.as_mut() {
                // What arrived of an earlier frame's check (a frame without one forgets it).
                if let Some(bytes) = probe_check.out.take() {
                    probe_check.seen = CheckedProbe::read(probe_check.seen, &bytes);
                }
                if !check {
                    probe_check.seen = None;
                }
                probe_check.ran = check;
                work.check = check;
            }
            active.reflect_inputs = reflect_inputs;
            active.sun_inputs = sun_inputs;
            active.sun_counted = sun == Some(true);
            // The probes' update counts what it did (`end_rays` copies the counts out).
            active.probe_counts.take();
            active.probes_counting = work.probes;
            if work.probes {
                active.probe_counts.clear(encoder);
            }
        }
        work
    }

    /// In the frame's ray pass (the frame's group and the ray scene's are set): trace and
    /// update this frame's probes.
    pub fn dispatch_probes(&self, pass: &mut wgpu::ComputePass<'_>) {
        let (Some(work), Some(active)) = (&self.work, &self.active) else {
            return;
        };
        let Some(group) = &active.group else {
            return;
        };
        pass.set_bind_group(2, group, &[]);
        pass.set_pipeline(&active.trace);
        pass.dispatch_workgroups((work.probes * work.rays).div_ceil(64), 1, 1);
        pass.set_pipeline(&active.update);
        pass.dispatch_workgroups(work.probes + work.cleared, 1, 1);
    }

    /// In the frame's ray pass: Rendering's check of the probes ([`begin_rays`](Self::begin_rays)
    /// said so; `RenderSettings::ray_probe_check`), 6 x 16 workgroups of 64 threads.
    pub fn dispatch_check(&self, pass: &mut wgpu::ComputePass<'_>) {
        let Some(probe_check) = self.active.as_ref().and_then(|a| a.check.as_ref()) else {
            return;
        };
        if !probe_check.ran {
            return;
        }
        pass.set_bind_group(2, &probe_check.group, &[]);
        pass.set_pipeline(&probe_check.pipeline);
        pass.dispatch_workgroups(6 * CHECK_GROUPS as u32, 1, 1);
    }

    /// In the frame's ray pass: the sun's rays ([`begin_rays`](Self::begin_rays) said so).
    pub fn dispatch_sun(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        targets: &crate::targets::Targets,
    ) {
        let Some(active) = &self.active else {
            return;
        };
        let (Some(sun), Some(inputs)) = (&active.sun, &active.sun_inputs) else {
            return;
        };
        pass.set_bind_group(2, inputs, &[]);
        pass.set_pipeline(&sun.pipeline);
        pass.dispatch_workgroups(targets.width.div_ceil(8), targets.height.div_ceil(8), 1);
    }

    /// World reflections ([`begin_rays`](Self::begin_rays) said so), once a frame: in the
    /// frame's ray pass, or, in a frame whose probe grid scrolled, in a pass of their own
    /// after [`end_rays`](Self::end_rays) (the frame's group and the ray scene's set there
    /// too). Takes what `begin_rays` made for them.
    pub fn dispatch_reflections(
        &mut self,
        pass: &mut wgpu::ComputePass<'_>,
        targets: &crate::targets::Targets,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if let Some(full) = active.full.as_mut()
            && let Some(inputs) = full.inputs.take()
        {
            pass.set_bind_group(2, &inputs, &[]);
            pass.set_pipeline(&full.pipeline);
            pass.dispatch_workgroups(targets.width.div_ceil(8), targets.height.div_ceil(8), 1);
            return;
        }
        let Some(inputs) = active.reflect_inputs.take() else {
            return;
        };
        let size = targets.world_reflection.texture.size();
        pass.set_bind_group(2, &inputs, &[]);
        pass.set_pipeline(&active.reflect);
        pass.dispatch_workgroups(size.width.div_ceil(8), size.height.div_ceil(8), 1);
    }

    /// After the pass that held the reflections ([`dispatch_reflections`](Self::dispatch_reflections)):
    /// copy every pixel's reflections' counts out, when they were counted.
    pub fn end_reflections(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(full) = self.active.as_mut().and_then(|a| a.full.as_mut()) {
            let counted = std::mem::take(&mut full.counting);
            full.counts.copy(encoder, counted);
        }
    }

    /// Whether the frame's ray pass can trace the mirrors' pictures (`crate::ray_mirror`):
    /// while the rays trace the frame's ray scene.
    /// Once after the frame's ray scene went: the rows [`publish`](Self::publish) keeps while
    /// it traces the scene (every pixel's reflection and the mirrors' pictures) read "not
    /// traced" (0) and "not counted" (-1), not their last values.
    pub(crate) fn publish_rays_none(d: &mut Diagnostics) {
        d.set("render.ray.reflect_full", 0.0);
        for row in [
            "reflect_pixels",
            "reflect_screen",
            "reflect_rays",
            "reflect_reused",
            "mirror_screen",
            "mirror_rays",
            "mirror_reused",
        ] {
            d.set(&format!("render.ray.{row}"), -1.0);
        }
    }

    pub fn traces_mirrors(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.mirrors.is_some())
    }

    /// Before the frame's ray pass: what the trace of each mirror's picture due this frame
    /// binds (group 2 of `cs_mirror_picture`): the prepass's maps and last frame's `picture` as
    /// every pixel's reflection reads them, and the picture's own; their count cleared when
    /// `count` asks for it. Says whether any is due.
    pub fn begin_mirrors<'a>(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &crate::targets::Targets,
        picture: &wgpu::TextureView,
        due: impl Iterator<Item = crate::ray_mirror::MirrorTarget<'a>>,
        count: bool,
    ) -> bool {
        let Some(mirrors) = self.active.as_mut().and_then(|a| a.mirrors.as_mut()) else {
            return false;
        };
        mirrors.counts.take();
        mirrors.inputs.clear();
        fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            }
        }
        for target in due {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ray mirror inputs"),
                layout: &mirrors.layout,
                entries: &[
                    view(20, &targets.prepass_normal.view),
                    view(21, &targets.prepass_depth.view),
                    view(22, &targets.motion.view),
                    view(23, picture),
                    wgpu::BindGroupEntry {
                        binding: 26,
                        resource: target.uniform.as_entire_binding(),
                    },
                    view(27, target.color),
                    view(28, target.depth),
                    wgpu::BindGroupEntry {
                        binding: 29,
                        resource: mirrors.counts.counts.as_entire_binding(),
                    },
                ],
            });
            mirrors.inputs.push((group, target.size));
        }
        let any = !mirrors.inputs.is_empty();
        mirrors.counting = any && count;
        if mirrors.counting {
            mirrors.counts.clear(encoder);
        } else {
            mirrors.counts.latest = None;
        }
        any
    }

    /// In the frame's ray pass (the frame's group and the ray scene's set): trace the mirrors'
    /// pictures [`begin_mirrors`](Self::begin_mirrors) made ready, in a frame whose probe grid
    /// scrolled in the pass after the atlas copies (they light their hits with the probes, as
    /// the reflections do).
    pub fn dispatch_mirror_pictures(&mut self, pass: &mut wgpu::ComputePass<'_>) {
        let Some(mirrors) = self.active.as_mut().and_then(|a| a.mirrors.as_mut()) else {
            return;
        };
        if mirrors.inputs.is_empty() {
            return;
        }
        pass.set_pipeline(&mirrors.pipeline);
        for (group, size) in mirrors.inputs.drain(..) {
            pass.set_bind_group(2, &group, &[]);
            pass.dispatch_workgroups(size.x.div_ceil(8), size.y.div_ceil(8), 1);
        }
    }

    /// After the pass that traced the mirrors' pictures: copy their counts out, when they
    /// were counted.
    pub fn end_mirrors(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(mirrors) = self.active.as_mut().and_then(|a| a.mirrors.as_mut()) {
            let counted = std::mem::take(&mut mirrors.counting);
            mirrors.counts.copy(encoder, counted);
        }
    }

    /// After the frame's ray pass: copy the atlas rows the probes' update wrote into the
    /// atlas the frame samples, and the probes' and the sun's counts out. (The reflections'
    /// inputs stay for a pass that follows these copies:
    /// [`dispatch_reflections`](Self::dispatch_reflections) takes them.)
    pub fn end_rays(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let work = self.work.take();
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if let Some(work) = work
            && active.group.is_some()
        {
            copy_atlas_rows(encoder, &active.next, &self.atlas, &work.rows);
        }
        let counted = std::mem::take(&mut active.probes_counting);
        active.probe_counts.copy(encoder, counted);
        if let Some(probe_check) = active.check.as_mut()
            && std::mem::take(&mut probe_check.ran)
        {
            probe_check.out.copy(encoder);
        }
        if active.sun_inputs.take().is_some()
            && let Some(sun) = active.sun.as_mut()
        {
            sun.counts.copy(encoder, active.sun_counted);
        }
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
        d.set("render.gi.probes_budget_cut", s.budget_cut as f64);
        d.set("render.gi.probes_raised", s.raised as u8 as f64);
        // What the update did in the latest frame read back (-1: none read yet): the probes it
        // was given, those that traced (the rest slept through their turn), those awake after.
        let counted = self.active.as_ref().and_then(|a| a.probe_counts.latest);
        let [given, traced, awake] =
            counted.map_or([-1.0; 3], |n| [n[0], n[1], n[2]].map(f64::from));
        d.set("render.gi.probes_counted", given);
        d.set("render.gi.probes_traced", traced);
        d.set("render.gi.probes_awake", awake);
        d.set("render.gi.scrolled_in", s.scrolled_in as f64);
        d.set("render.gi.cleared", s.cleared as f64);
        d.set("render.gi.pending", self.schedule.pending() as f64);
        d.set("render.gi.resets", self.schedule.resets as f64);
        d.set(
            "render.gi.probes",
            self.grid.map_or(0, |g| g.count()) as f64,
        );
        d.set("render.gi.instances", s.instances as f64);
        d.set("render.gi.meshes", s.meshes as f64);
        d.set("render.gi.built_triangles", s.built_triangles as f64);
        d.set("render.gi.building", s.building as f64);
        d.set("render.gi.gathered", s.gathered as u8 as f64);
        d.set("render.gi.full_gather", s.full as u8 as f64);
        d.set("render.gi.read", s.read as f64);
        d.set("render.gi.changed", s.changed as f64);
        d.set("render.gi.shifted", s.shifted as f64);
        d.set("render.gi.unchanged", s.unchanged as f64);
        d.set("render.gi.rebuilt", s.rebuilt as u8 as f64);
        d.set("render.gi.gpu_ms_per_kray", self.ms_per_kray as f64);
        d.set("render.gi.build_ms", s.build_ms as f64);
        for (name, ms) in PARTS.iter().zip(s.parts) {
            d.set(&format!("render.gi.ms_{name}"), ms as f64);
        }
        d.set("render.gi.extract_ms", s.extract_ms as f64);
        let hw = self.active.as_ref().and_then(|a| a.hw.as_ref());
        d.set("render.gi.hardware_rays", self.hardware_rays() as u8 as f64);
        d.set("render.gi.ray_world", self.traces_ray_world() as u8 as f64);
        if let Some(hw) = hw {
            let h = hw.stats();
            d.set("render.gi.rt_triangles", h.triangles as f64);
            d.set("render.gi.rt_built", h.built as f64);
            d.set("render.gi.rt_instances", h.instances as f64);
            d.set("render.gi.rt_tlas_built", h.tlas_built as u8 as f64);
            d.set("render.gi.rt_compacted", hw.compacted() as f64);
            d.set("render.gi.rt_retired", hw.retired() as f64);
            d.set("render.gi.rt_fences", hw.fences() as f64);
        } else {
            // No structures of the probes' own (the rays trace the frame's ray scene, or
            // walk in software): the rows of an earlier scene would stand otherwise.
            for row in [
                "triangles",
                "built",
                "instances",
                "tlas_built",
                "compacted",
                "retired",
                "fences",
            ] {
                d.set(&format!("render.gi.rt_{row}"), 0.0);
            }
        }
        d.set("render.gi.rt_losses", self.hw_losses as f64);
        // The sun's rays counted in the latest frame read back (-1: not counted now).
        if let Some(sun) = self.active.as_ref().and_then(|a| a.sun.as_ref()) {
            let [rays, hits] = sun
                .counts
                .latest
                .map_or([-1.0; 2], |n| [n[0], n[1]].map(f64::from));
            d.set("render.gi.sun_rays", rays);
            d.set("render.gi.sun_hits", hits);
        }
        // Every pixel's reflection (while the frame's ray scene is traced): whether the last
        // frame traced them, and their counts in the latest frame read back (-1: not counted
        // now): the pixels under the roughness limit, those the screen answered, the rays
        // traced, and the rays' hits taken from last frame's picture.
        if let Some(full) = self.active.as_ref().and_then(|a| a.full.as_ref()) {
            d.set("render.ray.reflect_full", full.traced as u8 as f64);
            let [pixels, screen, rays, reused] =
                full.counts.latest.map_or([-1.0; 4], |n| n.map(f64::from));
            d.set("render.ray.reflect_pixels", pixels);
            d.set("render.ray.reflect_screen", screen);
            d.set("render.ray.reflect_rays", rays);
            d.set("render.ray.reflect_reused", reused);
        }
        // The mirrors' pictures traced in the frame's ray pass (while the frame's ray scene is
        // traced): their counts in the latest frame read back (-1: not counted now): those the
        // screen answered, the rays traced, and the rays' hits taken from last frame's picture
        // (the texels are `render.ray.mirror_texels`).
        if let Some(mirrors) = self.active.as_ref().and_then(|a| a.mirrors.as_ref()) {
            let [_, screen, rays, reused] = mirrors
                .counts
                .latest
                .map_or([-1.0; 4], |n| n.map(f64::from));
            d.set("render.ray.mirror_screen", screen);
            d.set("render.ray.mirror_rays", rays);
            d.set("render.ray.mirror_reused", reused);
        }
        // Rendering's check of the probes, once a readback arrived (none while it is off): the
        // probe nearest the camera's irradiance (lux, its luminance) facing each axis
        // direction, the reference's, both means over the same frames, their ratio (-1 where
        // the reference is dark), the probe's spread from frame to frame (relative to its
        // mean), the worst ratio's distance from 1 and the worst spread over the directions
        // whose reference holds at least a tenth of the brightest's, the frames in the means,
        // the probe's state (1: lit in this epoch, + 2: in use) and where it is from the camera.
        if let Some(seen) = self
            .active
            .as_ref()
            .and_then(|a| a.check.as_ref())
            .and_then(|c| c.seen)
        {
            let brightest = seen
                .reference
                .iter()
                .map(|c| luma(*c))
                .fold(0.0f64, f64::max);
            let (mut worst, mut flicker) = (0.0f64, 0.0f64);
            for (k, axis) in CHECK_AXES.iter().enumerate() {
                let (own, reference) = (luma(seen.own[k]), luma(seen.reference[k]));
                let ratio = if reference > 1e-6 {
                    own / reference
                } else {
                    -1.0
                };
                // The probe's spread from frame to frame, relative to its mean.
                let spread = if own > 1e-6 {
                    (seen.own_square[k] - own * own).max(0.0).sqrt() / own
                } else {
                    0.0
                };
                if reference >= 0.1 * brightest && reference > 1e-6 {
                    worst = worst.max((ratio - 1.0).abs());
                    flicker = flicker.max(spread);
                }
                d.set(&format!("render.gi.check_probe_{axis}"), own);
                d.set(&format!("render.gi.check_ref_{axis}"), reference);
                d.set(&format!("render.gi.check_ratio_{axis}"), ratio);
                d.set(&format!("render.gi.check_spread_{axis}"), spread);
            }
            d.set("render.gi.check_worst", worst);
            d.set("render.gi.check_flicker", flicker);
            d.set("render.gi.check_frames", seen.frames as f64);
            d.set("render.gi.check_state", seen.state as f64);
            // Where the probe is from the camera (meters).
            for (axis, at) in ["dx", "dy", "dz"].iter().zip(seen.position) {
                d.set(&format!("render.gi.check_{axis}"), at as f64);
            }
        }
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
        for rays in [Rays::Software, Rays::Hardware, Rays::World] {
            let code = crate::features::compose_shader(&shader_source(rays), true);
            crate::features::validate_wgsl(&format!("probe gi ({rays:?})"), &code).unwrap();
        }
    }

    #[test]
    fn the_light_jumps_when_the_clock_does_not_as_it_runs() {
        let sun = |deg: f32, lux: f32| {
            let r = deg.to_radians();
            (Vec3::new(r.cos(), r.sin(), 0.0), lux)
        };
        // Day to night at once (the tour's clock set to 22:00): the sun hands over to the moon.
        assert!(light_jumped(
            sun(40.0, 115_700.0),
            (Vec3::new(-0.5, 0.7, -0.5), 0.25)
        ));
        // A storm deck rolling over in a frame, or a scene's sun switched off.
        assert!(light_jumped(sun(40.0, 100_000.0), sun(40.0, 10_000.0)));
        assert!(light_jumped(sun(40.0, 100_000.0), sun(40.0, 0.0)));
        // A day passing, even at a minute a frame (0.25° of sun), and its light with it.
        assert!(!light_jumped(sun(40.0, 100_000.0), sun(40.25, 99_000.0)));
        assert!(!light_jumped(sun(2.0, 3_000.0), sun(1.75, 2_400.0)));
        // Dusk: the sun fading out below 0.01 lux, and the moon taking over, both dim.
        assert!(!light_jumped(sun(-12.0, 0.004), sun(-12.0, 0.0)));
        assert!(!light_jumped(
            sun(-14.0, 0.29),
            (Vec3::new(-0.5, 0.7, -0.5), 0.3)
        ));
    }

    /// The hardware structure's instance (mesh -> anchor-relative world) and the row the
    /// shading reads (back to the mesh) undo each other, turned, stretched unevenly or
    /// mirrored, as the software walk's row alone did.
    #[test]
    fn an_instances_two_transforms_undo_each_other() {
        let transforms = [
            DAffine3::IDENTITY,
            DAffine3::from_scale_rotation_translation(
                DVec3::new(2.0, 0.5, 1.25),
                DQuat::from_euler(EulerRot::YXZ, 0.7, -0.4, 1.9),
                DVec3::new(12.5, -3.0, 40.0),
            ),
            // Mirrored (an odd number of negative scales).
            DAffine3::from_scale_rotation_translation(
                DVec3::new(-1.0, 3.0, 0.3),
                DQuat::from_euler(EulerRot::XYZ, -2.2, 0.1, 0.6),
                DVec3::new(-30.0, 7.0, 2.0),
            ),
        ];
        for a in transforms {
            let to_world = affine_rows(&a);
            let to_local = affine_rows(&a.inverse());
            let mat = |r: [[f32; 4]; 3]| {
                Mat4::from_cols_array_2d(&[r[0], r[1], r[2], [0.0, 0.0, 0.0, 1.0]]).transpose()
            };
            let both = mat(to_world) * mat(to_local);
            assert!(both.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{a:?}: {both:?}");
            let flat: [f32; 12] = bytemuck::cast(to_world);
            assert_eq!(
                flat[3], a.translation.x as f32,
                "row-major 3x4, translation last"
            );
            assert_eq!(flat[4], a.matrix3.row(1).x as f32);
        }
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
