//! Drawing terrains: quadtree selection, parallel patch building, a pool of GPU vertex
//! slots, shadows, and the ground maps (footprints, wetness, puddles, snow) the shader reads.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};

use bytemuck::{Pod, Zeroable};
use engine_core::diagnostics::Diagnostics;
use engine_core::math::{DQuat, DVec3, IVec2, Mat3, UVec2, Vec3};
use engine_core::prelude::*;
use engine_render::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase,
};
use engine_render::frame::ViewData;
use engine_render::settings::RenderSettings;
use engine_world::{Grid2, SurfaceCell, SurfaceLayers};

use crate::chunk::{
    ChunkSource, QUADS, TerrainVertex, VERTS_PER_CHUNK, build_chunk, chunk_indices,
};
use crate::cube::NodeKey;
use crate::deform::{DeformationMap, SLOTS};
use crate::far_look::{FarLook, FarLookPages};
use crate::lights::{CityLights, NO_PAGE, NightPages};
use crate::lod::{Lod, SelectInput};
use crate::materials::{self, BakedMaterials, MATERIAL_COUNT};
use crate::paint_map::{PAINT_MAP_HALF_M, PAINT_MAP_TEXEL_M, PaintMap};
use crate::wear::{PathWear, TILE, WearSettings};
use crate::{TerrainRuntime, TerrainSettings, Terrains};

const SHADER: &str = include_str!("terrain.wgsl");
const SHADOW_SHADER: &str = include_str!("terrain_shadow.wgsl");

/// Patches per GPU vertex buffer page.
const SLOTS_PER_PAGE: u32 = 256;
/// Frames a patch may go unused before its slot is reused.
const STALE_FRAMES: u64 = 300;
/// Detail of the patches drawn into the far shadow map, relative to the view's.
const FAR_SHADOW_DETAIL: f64 = 0.5;
/// Most [`TerrainSettings::patch_budget`] may raise the detail.
const MAX_BOOST: f64 = 4.0;
/// Most finished patches uploaded to the GPU in one frame (the rest wait for the next).
const MAX_UPLOADS_PER_FRAME: usize = 64;
/// And no more once uploading has taken this long in a frame (ms), after the first few.
const UPLOAD_BUDGET_MS: f64 = 1.5;
/// Patches always uploaded in a frame, whatever the time.
const MIN_UPLOADS_PER_FRAME: usize = 8;
/// Frames over which the weather's surface layers are sent to the GPU again, a band of rows
/// at a time.
const SURFACE_UPLOAD_FRAMES: u32 = 6;
/// Patches a planet keeps queued for the workers at most (the most urgent).
const MAX_QUEUED: usize = 256;

/// The `TERRAIN_OFF` parts extra views leave out (a lake's mirror is drawn at half resolution
/// and rippled; Unreal's reflection captures and HDRP's planar probes strip the same way):
/// 1 material textures (each material's mean color and roughness instead: about a ninth of
/// the ground's pixel cost at the lake shore), 2 rock's larger scales, 4 parallax relief.
const EXTRA_VIEW_OFF: u32 = 1 | 2 | 4;
/// A drawn patch's children replace it at `split_factor` patch sizes; it takes over from
/// them again only past this many times that, so the detail doesn't flicker back and forth
/// at the boundary (or a patch budget easing up and down).
const MERGE_HYSTERESIS: f64 = 1.15;
/// A moving camera's patches are built ahead along its path, so a fast flight doesn't outrun
/// them (a coarse parent stands in until its children are built): twice the measured time
/// from asking for a patch to drawing it, within these seconds.
const LOOK_AHEAD_S: f64 = 3.0;
const LOOK_AHEAD_MIN_S: f64 = 0.75;
/// But no farther ahead than this, meters.
const LOOK_AHEAD_MAX_M: f64 = 3_000.0;
/// Faster than this the camera jumped (a teleport, a tour stop), meters per second.
const JUMP_SPEED: f64 = 5_000.0;

/// Per-draw data (instance vertex buffer).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ChunkInstance {
    /// Patch origin relative to the camera, meters.
    origin: [f32; 3],
    /// Index into the planet array.
    planet: u32,
    /// Vertex spacing (m), quadtree level, and the camera distances where the geomorph
    /// toward the parent patch starts and ends (0 = off).
    lod: [f32; 4],
    /// The night glow page the patch samples (see [`NightPages::lookup`]).
    night: [f32; 4],
}

impl ChunkInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
            5 => Float32x3, 6 => Uint32, 7 => Float32x4, 12 => Float32x4
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ChunkInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

/// Ground material info (uniform). Matches `Materials` in terrain.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MaterialsGpu {
    /// Per material: (mean albedo rgb, tile meters), (mean roughness, mean occlusion,
    /// height range m, 0).
    info: [[f32; 4]; 2 * MATERIAL_COUNT],
    /// Textures ready (0/1).
    flags: [f32; 4],
}

impl MaterialsGpu {
    fn from_baked(baked: Option<&BakedMaterials>) -> Self {
        let mut out = Self {
            info: [[0.0; 4]; 2 * MATERIAL_COUNT],
            flags: [0.0; 4],
        };
        for (i, m) in materials::GroundMaterial::ALL.iter().enumerate() {
            let (albedo, rough, occl, height, tile) = baked
                .and_then(|b| b.stats.get(i))
                .map(|s| {
                    (
                        s.mean_albedo,
                        s.mean_roughness,
                        s.mean_occlusion,
                        s.height_m,
                        s.tile_m,
                    )
                })
                .unwrap_or(([0.5; 3], 0.8, 1.0, 0.0, m.tile_m()));
            out.info[2 * i] = [albedo[0], albedo[1], albedo[2], tile];
            out.info[2 * i + 1] = [rough, occl, height, 0.0];
        }
        out.flags[0] = if baked.is_some() { 1.0 } else { 0.0 };
        out
    }
}

/// Per-terrain shading data (storage buffer). Matches `PlanetGpu` in terrain.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PlanetGpu {
    /// Columns of the terrain-local → world rotation.
    rot: [[f32; 4]; 3],
    /// Terrain origin relative to the camera (xyz), base radius (w).
    center: [f32; 4],
    /// lush, dry, soil, rock, sand, snow, dark, ocean, mud, concrete (rgb + unused).
    colors: [[f32; 4]; 10],
    /// snow below °C, rock slope, vegetation (0/1), ocean surface (0/1).
    params: [f32; 4],
    /// sphere (0/1), crater density, largest crater radius, crater seed.
    params2: [f32; 4],
    /// How far the terrain moved in the world since last frame (xyz, for motion vectors).
    motion: [f32; 4],
    /// Regolith scattering (0..1), rayed craters in `rays`, young areas in `young`, smallest
    /// crater radius (m).
    params3: [f32; 4],
    /// Rayed craters: planet-local unit direction of the center (xyz), radius on the ground
    /// (w, meters).
    rays: [[f32; 4]; MAX_RAYED],
    /// Young ground with few craters: planet-local direction of the middle times its youth
    /// (xyz), angular radius (w, radians).
    young: [[f32; 4]; MAX_YOUNG],
    /// Night lights of towns and cities (0..1), the distance from which the world's own
    /// lamps glow from their pages (m), lava glow (cd/m²), melt latitude (rad).
    params4: [f32; 4],
    /// The far look ([`FarLook`](crate::FarLook)): layer in the map array (−1: none), how
    /// strongly it tints the ground up close; the climate goes by the angle from the star
    /// (`Climate::substellar`, 0/1); shore mats (`TerrainPalette::shore_mats`).
    far: [f32; 4],
    /// The far look's mean linear albedo (rgb, unused), farmland (w, `TerrainPalette::farmland`).
    far_mean: [f32; 4],
}

/// Rayed craters per planet the shader draws.
const MAX_RAYED: usize = 8;
/// Young areas per planet the shader thins craters in.
const MAX_YOUNG: usize = 12;

/// Ground maps around the focus (uniform). Matches `Maps` in terrain.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
struct MapsGpu {
    /// Deformation grid: origin x/z relative to the camera, cell size, cells per side.
    deform: [f32; 4],
    /// Surface layers grid: origin x/z relative to the camera, cell size, cells per side.
    surface: [f32; 4],
    /// deformation on, surface layers on, LOD debug colors, unused.
    flags: [f32; 4],
    /// Wetness, puddle depth, snow depth and scorch beyond the surface layers' grid.
    far: [f32; 4],
    /// Worn paths window: its corner relative to the camera, cell size.
    wear_origin: [f32; 4],
    /// The window's column axis (world), cells per side.
    wear_u: [f32; 4],
    /// The window's row axis (world), 1 when paths are drawn.
    wear_v: [f32; 4],
    /// The coarse window of paths: its corner relative to the camera, cell size.
    wear_far: [f32; 4],
    /// The hand-made ground's paint map ([`PaintMap`]): texel (0, 0)'s center relative to
    /// the camera, texel size.
    paint_origin: [f32; 4],
    /// Its column axis (world), texels per side.
    paint_u: [f32; 4],
    /// Its row axis (world), 1 when there is a map.
    paint_v: [f32; 4],
    /// The home spot's up (world), unused.
    paint_up: [f32; 4],
    /// Where the deformation grid's window starts in its texture (it wraps around): as a share
    /// of the texture, and in cells.
    deform_base: [f32; 4],
    /// Where the surface layers' grid starts in its texture (it wraps around,
    /// [`SurfaceUpload`]): as a share of the texture, unused.
    surface_base: [f32; 4],
}

/// A print the shader draws sharp. Matches `Print` in terrain.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
struct PrintGpu {
    /// Center relative to the deformation grid's corner (x/z), forward (x/z).
    at: [f32; 4],
    /// Half width, half length, depth (m), rim share.
    size: [f32; 4],
    /// Grid depth at the center when pressed, contact kind, cell kind, unused.
    info: [f32; 4],
}

/// A contact kind as `print_distance` in terrain.wgsl numbers it.
fn contact_code(k: engine_world::ContactKind) -> f32 {
    use engine_world::ContactKind as C;
    match k {
        C::Foot => 0.0,
        C::Paw => 1.0,
        C::Hoof => 2.0,
        C::Wheel => 3.0,
        C::Track => 4.0,
        C::Body => 5.0,
    }
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    /// The scene pass for extra views (a lake's mirror, a portal): without the parts a
    /// half-resolution, rippled picture can't show ([`EXTRA_VIEW_OFF`]).
    extra_view_pipeline: wgpu::RenderPipeline,
    /// Depth, normals and motion for ambient occlusion, contact shadows and anti-aliasing.
    prepass_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    /// Rivers' and lakes' surfaces over their beds (blended, after the opaque scene).
    water_pipeline: wgpu::RenderPipeline,
    /// Its group 2 (the scene's color and depth copies), by the copies it binds.
    water_layout: wgpu::BindGroupLayout,
    water_groups: Vec<([wgpu::TextureView; 2], wgpu::BindGroup)>,
    water_group_now: usize,
    layout: wgpu::BindGroupLayout,
    index: wgpu::Buffer,
    index_count: u32,
    instances: GrowBuffer,
    /// Indirect arguments of every draw list, in [`DrawList`] order (multi-draw only).
    draw_args: Option<GrowBuffer>,
    planets: GrowBuffer,
    maps: wgpu::Buffer,
    sampler: wgpu::Sampler,
    deform_tex: Option<(wgpu::Texture, wgpu::TextureView, u32)>,
    /// The two newest prints reaching each deformation cell (Rg16Uint, slot + 1).
    print_ids: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Print shapes ([`SLOTS`] of them).
    prints: wgpu::Buffer,
    surface_tex: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    /// Worn paths around the camera (R8, cells per side).
    wear_tex: Option<(wgpu::Texture, wgpu::TextureView, u32)>,
    /// Ground material arrays: albedo + height, normal + roughness + occlusion.
    mat_albedo: wgpu::TextureView,
    mat_normal: wgpu::TextureView,
    mat_info: wgpu::Buffer,
    mat_sampler: wgpu::Sampler,
    /// Resolution of the uploaded material textures (0 = placeholders).
    mat_size: u32,
    /// Resolution and scan folder of the uploaded materials.
    mat_key: Option<(u32, Option<PathBuf>)>,
    /// Materials built from scans.
    mat_scanned: usize,
    /// City lights: the night glow of the world's own lamps.
    night: NightPages,
    /// Planets' looks from afar (see [`FarLook`](crate::FarLook)).
    far_looks: FarLookPages,
    /// The hand-made ground's paint ([`PaintMap`]): paint and tint layers (RGBA8), and the
    /// map's texel (0, 0) center in the overlay's frame, texel size and texels per side.
    paint_tex: (wgpu::Texture, wgpu::TextureView),
    paint_map: Option<([f64; 2], f64, u32)>,
    /// The overlay paint baked or being baked (its function's address), and the bake.
    paint_key: Option<usize>,
    paint_bake: Option<std::thread::JoinHandle<PaintMap>>,
    /// A baked paint map going up a band of rows a frame (32 MB at once was a hitch), and the
    /// next row; the shader uses it once all of it is up.
    paint_upload: Option<(PaintMap, u32)>,
    group: Option<wgpu::BindGroup>,
}

/// A 2-layer RGBA8 array for a paint map's paint and tint, `size` texels a side (empty until
/// [`write_paint_rows`] fills it; 1×1 without a map).
fn paint_texture(gpu: &GpuShared, size: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain paint map"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    (tex, view)
}

/// Two `MATERIAL_COUNT`-layer texture arrays (albedo + height in sRGB, normal + roughness +
/// occlusion linear) holding `baked`, or flat 1×1 placeholders.
fn material_arrays(
    gpu: &GpuShared,
    baked: Option<&BakedMaterials>,
) -> (wgpu::TextureView, wgpu::TextureView) {
    let (size, mips) = baked.map(|b| (b.size, b.mip_count)).unwrap_or((1, 1));
    let make = |label: &str, format: wgpu::TextureFormat, data: Option<&Vec<Vec<Vec<u8>>>>| {
        let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: MATERIAL_COUNT as u32,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for layer in 0..MATERIAL_COUNT {
            for mip in 0..mips {
                let w = (size >> mip).max(1);
                let flat = [128u8, 128, 204, 255];
                let bytes: &[u8] = match data {
                    Some(d) => &d[layer][mip as usize],
                    None => &flat,
                };
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: mip,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(w),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: w,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    };
    (
        make(
            "terrain material albedo",
            wgpu::TextureFormat::Rgba8UnormSrgb,
            baked.map(|b| &b.albedo_height),
        ),
        make(
            "terrain material normals",
            wgpu::TextureFormat::Rgba8Unorm,
            baked.map(|b| &b.normal_rough_ao),
        ),
    )
}

struct ChunkGpu {
    slot: u32,
    origin: DVec3,
    center: DVec3,
    radius: f64,
    spacing: f64,
    /// A river's or lake's surface shows over the patch.
    water: bool,
}

struct PlanetState {
    lod: Lod,
    version: u64,
    edits_seen: usize,
    chunks: HashMap<NodeKey, ChunkGpu>,
    /// Built patches to rebuild in place (edited), nearest first when drained.
    rebuild: Vec<NodeKey>,
    /// Patches being built on the workers or queued for them (see [`PatchWorkers`]).
    building: HashSet<NodeKey>,
    /// When each of them was first asked for.
    asked: HashMap<NodeKey, std::time::Instant>,
    /// Smoothed time from asking for a patch to uploading it (s).
    latency_s: f64,
    /// World position last frame (motion vectors for moving planets).
    last_position: Option<DVec3>,
    /// The shape's rayed craters, packed for the shader.
    rays: [[f32; 4]; MAX_RAYED],
    rayed: usize,
    /// The shape's young areas, packed for the shader.
    young: [[f32; 4]; MAX_YOUNG],
    young_count: usize,
    /// Detail multiplier from [`TerrainSettings::patch_budget`] (1 = none).
    boost: f64,
    /// Terrain-local camera position last frame and when, and its smoothed velocity.
    last_camera: Option<(DVec3, std::time::Instant)>,
    camera_velocity: DVec3,
}

impl PlanetState {
    fn new(t: &TerrainRuntime, src: &ChunkSource) -> Self {
        let mut rays = [[0.0; 4]; MAX_RAYED];
        let mut rayed = 0;
        if src.map.is_sphere() {
            for ((dir, r), slot) in t.config.shape.rayed_craters().into_iter().zip(&mut rays) {
                *slot = [dir.x as f32, dir.y as f32, dir.z as f32, r as f32];
                rayed += 1;
            }
        }
        let mut young = [[0.0; 4]; MAX_YOUNG];
        let mut young_count = 0;
        if src.map.is_sphere() {
            let shape = &t.config.shape;
            for (a, slot) in shape.landforms.young_areas(shape.radius_m).zip(&mut young) {
                let c = a.center * a.youth;
                *slot = [c.x as f32, c.y as f32, c.z as f32, a.angle as f32];
                young_count += 1;
            }
        }
        Self {
            lod: Lod::new(src.map, t.config.height_range(), t.config.finest_spacing_m),
            version: t.version,
            edits_seen: 0,
            chunks: HashMap::new(),
            rebuild: Vec::new(),
            building: HashSet::new(),
            asked: HashMap::new(),
            latency_s: 0.5,
            last_position: None,
            rays,
            rayed,
            young,
            young_count,
            boost: 1.0,
            last_camera: None,
            camera_velocity: DVec3::ZERO,
        }
    }
}

#[derive(Clone, Copy)]
struct Draw {
    slot: u32,
    instance: u32,
}

/// `draw_indexed_indirect` arguments (wgpu's `DrawIndexedIndirectArgs` layout).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DrawArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// The draw lists, in the order their indirect arguments are written.
#[derive(Clone, Copy)]
enum DrawList {
    Scene,
    Water,
    Shadow(usize),
    FarShadow,
}

impl DrawList {
    fn index(self) -> usize {
        match self {
            DrawList::Scene => 0,
            DrawList::Water => 1,
            DrawList::Shadow(i) => 2 + i,
            DrawList::FarShadow => 6,
        }
    }
}

/// Fixed-size vertex slots in a few large buffers.
#[derive(Default)]
struct SlotPool {
    pages: Vec<wgpu::Buffer>,
    free: Vec<u32>,
    capacity: u32,
}

impl SlotPool {
    fn alloc(&mut self, device: &wgpu::Device, max: u32) -> Option<u32> {
        if self.free.is_empty() {
            self.grow(device, max);
        }
        self.free.pop()
    }

    /// Add a page of free slots (none past `max`).
    fn grow(&mut self, device: &wgpu::Device, max: u32) {
        if self.capacity + SLOTS_PER_PAGE > max.max(SLOTS_PER_PAGE) {
            return;
        }
        let page = self.pages.len() as u32;
        self.pages
            .push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("terrain patches"),
                size: (SLOTS_PER_PAGE as usize
                    * VERTS_PER_CHUNK
                    * std::mem::size_of::<TerrainVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        self.capacity += SLOTS_PER_PAGE;
        // Hand out slots low to high.
        self.free
            .extend((page * SLOTS_PER_PAGE..(page + 1) * SLOTS_PER_PAGE).rev());
    }

    fn release(&mut self, slot: u32) {
        self.free.push(slot);
    }

    fn write(&self, queue: &wgpu::Queue, slot: u32, vertices: &[TerrainVertex]) {
        let page = (slot / SLOTS_PER_PAGE) as usize;
        let offset = (slot % SLOTS_PER_PAGE) as usize
            * VERTS_PER_CHUNK
            * std::mem::size_of::<TerrainVertex>();
        queue.write_buffer(
            &self.pages[page],
            offset as u64,
            bytemuck::cast_slice(vertices),
        );
    }

    fn used(&self) -> u32 {
        self.capacity - self.free.len() as u32
    }
}

/// The terrain render feature (added by [`TerrainPlugin`](crate::TerrainPlugin)).
#[derive(Default)]
pub struct TerrainFeature {
    gpu: Option<Gpu>,
    planets: HashMap<Entity, PlanetState>,
    pool: SlotPool,
    draws: Vec<Draw>,
    /// Patches with a river's or lake's surface on them (the water pass).
    water_draws: Vec<Draw>,
    shadow_draws: [Vec<Draw>; 4],
    /// Patches drawn into the far shadow map (big shadows beyond the cascades).
    far_shadow_draws: Vec<Draw>,
    /// Draw each list's runs of patches on one vertex page with one
    /// `multi_draw_indexed_indirect` (`RenderSettings::multi_draw_indirect`, where the GPU
    /// takes a first instance in indirect draws): a few commands a pass instead of one per
    /// patch.
    multi_draw: bool,
    /// Where each [`DrawList`]'s arguments start in the indirect buffer.
    list_args: [u32; 7],
    args: Vec<DrawArgs>,
    deform_generation: u64,
    /// The footprint grid the texture holds ([`DeformationMap::epoch`]).
    deform_epoch: u64,
    /// What the weather layers' texture holds ([`SurfaceUpload`]).
    surface: SurfaceUpload,
    /// The paths drawn: terrain, region, window corner (cells), generation.
    wear_window: Option<(Entity, usize, IVec2, IVec2, u64)>,
    wear_frame: u64,
    frame: u64,
    stats: FeatureStats,
    built_last_frame: usize,
    /// Patch builds run on these threads, never in the frame.
    workers: Option<PatchWorkers>,
    /// Patches built and waiting for an upload slot in a later frame.
    finished: Vec<FinishedPatch>,
}

/// How wet and snowy the ground is beyond the weather's grid around the player: like the open
/// ground along the grid's rim. The upper quartile of its last few cells, so shelters, fires
/// and the strip a move has just uncovered (empty until the weather fills it) don't count.
/// The weather's surface layers on the GPU (wetness, puddles, snow, scorch: a grid that
/// follows the focus, `engine_world::SurfaceLayers`). The texture wraps around, as a clipmap's
/// does (Asirvatham and Hoppe, "Terrain Rendering Using GPU-Based Geometry Clipmaps", GPU Gems
/// 2, ch. 2): when the grid moves, its cells keep their texels and only the strips that came
/// into it are sent. The rest goes a band of rows a frame ([`SURFACE_UPLOAD_FRAMES`]), and only
/// where a value changed, so neither the weather's slow changes nor flying cost a spike.
#[derive(Default)]
struct SurfaceUpload {
    /// The grid's origin when last sent.
    origin: Option<DVec2>,
    /// The texel of the grid's cell (0, 0).
    base: UVec2,
    /// The half floats in the texture, texel by texel (4 a texel).
    sent: Vec<u16>,
    /// The next band's first texel row.
    row: u32,
    /// Wetness and snow depth beyond the grid (see [`beyond_grid`]).
    far: [f32; 2],
}

impl SurfaceUpload {
    fn update(
        &mut self,
        g: &Grid2<SurfaceCell>,
        fresh: bool,
        tex: &wgpu::Texture,
        queue: &wgpu::Queue,
    ) {
        let w = g.width;
        for [x, y, cw, ch] in self.refresh(g, fresh) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&self.sent),
                wgpu::TexelCopyBufferLayout {
                    offset: ((y * w + x) * 8) as u64,
                    bytes_per_row: Some(w * 8),
                    rows_per_image: Some(ch),
                },
                wgpu::Extent3d {
                    width: cw,
                    height: ch,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Bring `sent` up to date for this frame; the texel rectangles to send (x, y, w, h).
    fn refresh(&mut self, g: &Grid2<SurfaceCell>, fresh: bool) -> Vec<[u32; 4]> {
        let (w, h) = (g.width, g.height);
        if w == 0 || h == 0 {
            return Vec::new();
        }
        // The move in whole cells, if it keeps some of the grid.
        let shift = self.origin.and_then(|o| {
            let s = (g.origin - o) / g.cell as f64;
            let r = s.round();
            ((s - r).abs().max_element() < 1e-3 && r.x.abs() < w as f64 && r.y.abs() < h as f64)
                .then(|| IVec2::new(r.x as i32, r.y as i32))
        });
        let n = (w * h) as usize * 4;
        let mut rects: Vec<[u32; 4]> = Vec::new();
        match shift {
            Some(d) if !fresh && self.sent.len() == n => {
                if d != IVec2::ZERO {
                    self.base = UVec2::new(
                        (self.base.x as i32 + d.x).rem_euclid(w as i32) as u32,
                        (self.base.y as i32 + d.y).rem_euclid(h as i32) as u32,
                    );
                    self.origin = Some(g.origin);
                    // The columns and rows that came in (grid cells), as texel spans.
                    let span = |d: i32, size: u32, base: u32| -> Option<(u32, u32)> {
                        let (from, len) = if d > 0 {
                            (size - d as u32, d as u32)
                        } else if d < 0 {
                            (0, (-d) as u32)
                        } else {
                            return None;
                        };
                        Some(((from + base) % size, len))
                    };
                    if let Some((x0, len)) = span(d.x, w, self.base.x) {
                        self.convert(g, [x0, 0, len, h]);
                        split_wrap(x0, len, w, |x, cw| rects.push([x, 0, cw, h]));
                    }
                    if let Some((y0, len)) = span(d.y, h, self.base.y) {
                        self.convert(g, [0, y0, w, len]);
                        split_wrap(y0, len, h, |y, ch| rects.push([0, y, w, ch]));
                    }
                }
                let band = h.div_ceil(SURFACE_UPLOAD_FRAMES);
                let (y0, y1) = (self.row, (self.row + band).min(h));
                self.row = if y1 >= h { 0 } else { y1 };
                if self.convert(g, [0, y0, w, y1 - y0]) {
                    rects.push([0, y0, w, y1 - y0]);
                }
                if self.row == 0 {
                    self.far = beyond_grid(g);
                }
            }
            _ => {
                self.origin = Some(g.origin);
                self.base = UVec2::ZERO;
                self.row = 0;
                self.sent.clear();
                self.sent.resize(n, 0);
                self.convert(g, [0, 0, w, h]);
                rects.push([0, 0, w, h]);
                self.far = beyond_grid(g);
            }
        }
        rects
    }

    /// Convert the grid's cells in a rectangle of texels (it may wrap around) into `sent`.
    /// True when any changed.
    fn convert(&mut self, g: &Grid2<SurfaceCell>, [x0, y0, cw, ch]: [u32; 4]) -> bool {
        let (w, h) = (g.width, g.height);
        let mut changed = false;
        for ty in (y0..y0 + ch).map(|y| y % h) {
            let gy = (ty + h - self.base.y) % h;
            let row = &g.data[(gy * w) as usize..((gy + 1) * w) as usize];
            for tx in (x0..x0 + cw).map(|x| x % w) {
                let c = &row[((tx + w - self.base.x) % w) as usize];
                let half = [
                    f16_bits(c.wetness),
                    f16_bits(c.puddle_m),
                    f16_bits(c.snow_m),
                    f16_bits(c.scorch),
                ];
                let at = ((ty * w + tx) * 4) as usize;
                if self.sent[at..at + 4] != half {
                    self.sent[at..at + 4].copy_from_slice(&half);
                    changed = true;
                }
            }
        }
        changed
    }
}

/// `f(start, len)` for the one or two runs a span of `len` from `start` takes in `0..size`
/// (it wraps around).
fn split_wrap(start: u32, len: u32, size: u32, mut f: impl FnMut(u32, u32)) {
    let first = len.min(size - start);
    f(start, first);
    if len > first {
        f(0, len - first);
    }
}

fn beyond_grid(g: &Grid2<SurfaceCell>) -> [f32; 2] {
    let ring = 4.min(g.width / 2).min(g.height / 2);
    let (mut wet, mut snow) = (Vec::new(), Vec::new());
    for y in 0..g.height {
        let row = &g.data[(y * g.width) as usize..((y + 1) * g.width) as usize];
        let edge_row = y < ring || y >= g.height - ring;
        let (left, rest) = row.split_at(if edge_row { row.len() } else { ring as usize });
        let right = &rest[rest.len() - if edge_row { 0 } else { ring as usize }..];
        for c in left.iter().chain(right) {
            wet.push(c.wetness);
            snow.push(c.snow_m);
        }
    }
    let upper_quartile = |v: &mut Vec<f32>| {
        if v.is_empty() {
            return 0.0;
        }
        let i = v.len() * 3 / 4;
        *v.select_nth_unstable_by(i, f32::total_cmp).1
    };
    [upper_quartile(&mut wet), upper_quartile(&mut snow)]
}

/// IEEE half-precision bits for an `f32` (round to nearest, flush tiny values to zero).
pub fn f16_bits(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x7f_ffff;
    if exp <= 0 {
        return sign;
    }
    if exp >= 31 {
        return sign | 0x7c00;
    }
    let m = (mant + 0x1000) >> 13;
    if m & 0x400 != 0 {
        // Mantissa rounding overflowed into the exponent.
        let e = exp + 1;
        if e >= 31 {
            return sign | 0x7c00;
        }
        return sign | ((e as u16) << 10);
    }
    sign | ((exp as u16) << 10) | (m as u16)
}

impl TerrainFeature {
    fn rebuild_group(&mut self, gpu_shared: &GpuShared) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let (Some(deform), Some(ids), Some(surface), Some(wear)) = (
            &gpu.deform_tex,
            &gpu.print_ids,
            &gpu.surface_tex,
            &gpu.wear_tex,
        ) else {
            return;
        };
        gpu.group = Some(
            gpu_shared
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain"),
                    layout: &gpu.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: gpu.planets.buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: gpu.maps.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&deform.1),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&surface.1),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: gpu.mat_info.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(&gpu.mat_albedo),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(&gpu.mat_normal),
                        },
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: wgpu::BindingResource::Sampler(&gpu.mat_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: wgpu::BindingResource::TextureView(&wear.1),
                        },
                        wgpu::BindGroupEntry {
                            binding: 10,
                            resource: wgpu::BindingResource::TextureView(&gpu.night.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: wgpu::BindingResource::TextureView(&ids.1),
                        },
                        wgpu::BindGroupEntry {
                            binding: 12,
                            resource: gpu.prints.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 13,
                            resource: wgpu::BindingResource::TextureView(&gpu.far_looks.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 14,
                            resource: wgpu::BindingResource::TextureView(&gpu.paint_tex.1),
                        },
                    ],
                }),
        );
    }

    /// Swap in the baked ground materials once the background bake is done.
    fn upload_materials(
        &mut self,
        gpu_shared: &GpuShared,
        resolution: u32,
        scans: Option<PathBuf>,
    ) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let key = (resolution.next_power_of_two().max(16), scans);
        if resolution == 0 || gpu.mat_key.as_ref() == Some(&key) {
            return;
        }
        let Some(baked) = materials::shared(resolution, key.1.as_deref()) else {
            return;
        };
        let (albedo, normal) = material_arrays(gpu_shared, Some(&baked));
        gpu.mat_albedo = albedo;
        gpu.mat_normal = normal;
        gpu.mat_size = baked.size;
        gpu.mat_scanned = baked.scanned();
        gpu.mat_key = Some(key);
        gpu_shared.queue.write_buffer(
            &gpu.mat_info,
            0,
            bytemuck::bytes_of(&MaterialsGpu::from_baked(Some(&baked))),
        );
        self.rebuild_group(gpu_shared);
    }

    /// Upload the worn paths around the camera: the window of the nearest region of paths,
    /// moved in whole tiles as the camera goes, and the tiles in it that changed. Two layers:
    /// the cells themselves near the camera, and four times coarser cells (how much of each is
    /// bare path) over a window four times wider, so paths still show from hills and the air.
    /// Returns whether the texture was recreated.
    fn upload_wear(
        &mut self,
        world: &mut World,
        gpu_shared: &GpuShared,
        view: &ViewData,
        maps: &mut MapsGpu,
    ) -> bool {
        let size = world
            .get_resource::<WearSettings>()
            .filter(|s| s.enabled)
            .map(|s| s.window.clamp(64, 4096).next_multiple_of(TILE as u32));
        let (Some(size), Some(terrains)) = (size, world.get_resource::<Terrains>()) else {
            return false;
        };
        let Some((entity, t)) = terrains.nearest(view.origin) else {
            return false;
        };
        let (position, rotation) = (t.position, t.rotation);
        let local = t.to_local(view.origin);
        let Some(mut wear) = world.get_resource_mut::<PathWear>() else {
            return false;
        };
        let cell = wear.cell_m;
        let Some((index, region)) = wear.region_near(entity, local) else {
            wear.dirty.clear();
            return false;
        };
        let (cu, cv) = region.cell_of(local, cell);
        let (anchor, axis_u, axis_v) = (region.anchor, region.axis_u, region.axis_v);
        // Window corners in fine cells, on whole tiles: the near one, and the far one (whose
        // cells are FAR fine cells across, so a tile fills TILE / FAR of them).
        const FAR: i32 = 4;
        let half = size as i32 / 2;
        let snap = |c: f64, span: i32| (c as i32 - span).div_euclid(TILE) * TILE;
        let corner = IVec2::new(snap(cu, half), snap(cv, half));
        let far_corner = IVec2::new(snap(cu, half * FAR), snap(cv, half * FAR));
        let gpu = self.gpu.as_mut().expect("gpu");
        let fresh = gpu.wear_tex.as_ref().is_none_or(|t| t.2 != size);
        if fresh {
            let tex = gpu_shared.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain worn paths"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 2,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let v = tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            gpu.wear_tex = Some((tex, v, size));
        }
        let key = (entity, index, corner, far_corner, wear.generation);
        let tex = &gpu.wear_tex.as_ref().expect("texture").0;
        let write = |layer: u32, x0: u32, y0: u32, w: u32, h: u32, data: &[u8]| {
            gpu_shared.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: x0,
                        y: y0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        };
        let near_bytes = |tile: Option<&[f32]>| -> Vec<u8> {
            match tile {
                Some(t) => t
                    .iter()
                    .map(|w| (w.clamp(0.0, 1.0) * 255.0).round() as u8)
                    .collect(),
                None => vec![0; (TILE * TILE) as usize],
            }
        };
        // A tile's share of bare path in each coarse cell (the shader's bare range of wear).
        let far_bytes = |tile: Option<&[f32]>| -> Vec<u8> {
            let n = TILE / FAR;
            let mut out = vec![0u8; (n * n) as usize];
            if let Some(t) = tile {
                for y in 0..n {
                    for x in 0..n {
                        let mut sum = 0.0;
                        for j in 0..FAR {
                            for i in 0..FAR {
                                let w = t[((y * FAR + j) * TILE + x * FAR + i) as usize];
                                let k = ((w - 0.45) / 0.4).clamp(0.0, 1.0);
                                sum += k * k * (3.0 - 2.0 * k);
                            }
                        }
                        out[(y * n + x) as usize] =
                            (sum / (FAR * FAR) as f32 * 255.0).round() as u8;
                    }
                }
            }
            out
        };
        // Where a tile lands in each layer, in texels, if it is inside the window.
        let n = TILE / FAR;
        let place = |k: IVec2| {
            let o = k * TILE - corner;
            let fo = (k * TILE - far_corner) / FAR;
            let inside = |o: IVec2, w: i32| {
                o.x >= 0 && o.y >= 0 && o.x + w <= size as i32 && o.y + w <= size as i32
            };
            (inside(o, TILE).then_some(o), inside(fo, n).then_some(fo))
        };
        if fresh || self.wear_window != Some(key) {
            // The whole window.
            let mut near = vec![0u8; (size * size) as usize];
            let mut far = vec![0u8; (size * size) as usize];
            let blit = |dst: &mut [u8], o: IVec2, w: i32, src: &[u8]| {
                for row in 0..w {
                    let at = ((o.y + row) * size as i32 + o.x) as usize;
                    dst[at..at + w as usize]
                        .copy_from_slice(&src[(row * w) as usize..((row + 1) * w) as usize]);
                }
            };
            for k in region.tile_keys() {
                let (o, fo) = place(k);
                if let Some(o) = o {
                    blit(&mut near, o, TILE, &near_bytes(region.tile(k)));
                }
                if let Some(fo) = fo {
                    blit(&mut far, fo, n, &far_bytes(region.tile(k)));
                }
            }
            write(0, 0, 0, size, size, &near);
            write(1, 0, 0, size, size, &far);
            self.wear_window = Some(key);
            self.wear_frame = self.frame;
            wear.dirty.clear();
        } else if self.frame >= self.wear_frame + 4 && !wear.dirty.is_empty() {
            // Tiles that changed (and empty ones that healed away).
            self.wear_frame = self.frame;
            let dirty: Vec<_> = wear.dirty.drain().collect();
            let Some((_, region)) = wear.region_near(entity, local) else {
                return fresh;
            };
            for (e, i, k) in dirty {
                if e != entity || i != index {
                    continue;
                }
                let (o, fo) = place(k);
                if let Some(o) = o {
                    let t = TILE as u32;
                    write(0, o.x as u32, o.y as u32, t, t, &near_bytes(region.tile(k)));
                }
                if let Some(fo) = fo {
                    let t = n as u32;
                    write(
                        1,
                        fo.x as u32,
                        fo.y as u32,
                        t,
                        t,
                        &far_bytes(region.tile(k)),
                    );
                }
            }
        }
        let rel_of = |c: IVec2| {
            let l =
                anchor + axis_u * (c.x as f64 * cell as f64) + axis_v * (c.y as f64 * cell as f64);
            position + rotation * l - view.origin
        };
        let (rel, far_rel) = (rel_of(corner), rel_of(far_corner));
        let (u, v) = (rotation * axis_u, rotation * axis_v);
        maps.wear_origin = [rel.x as f32, rel.y as f32, rel.z as f32, cell];
        maps.wear_u = [u.x as f32, u.y as f32, u.z as f32, size as f32];
        maps.wear_v = [v.x as f32, v.y as f32, v.z as f32, 1.0];
        maps.wear_far = [
            far_rel.x as f32,
            far_rel.y as f32,
            far_rel.z as f32,
            cell * FAR as f32,
        ];
        fresh
    }

    /// Bake the hand-made ground's paint into a map when a terrain's overlay is painted (on
    /// worker threads), upload it when done, and place its window. True when the texture is
    /// new (the bind group must be made again).
    fn upload_paint_map(
        &mut self,
        world: &World,
        gpu_shared: &GpuShared,
        view: &ViewData,
        maps: &mut MapsGpu,
    ) -> bool {
        let gpu = self.gpu.as_mut().expect("gpu");
        let terrains = world.get_resource::<Terrains>();
        // The first terrain with a painted overlay on a sphere.
        let painted = terrains.and_then(|ts| {
            ts.entries.values().find_map(|t| {
                let o = t.config.shape.overlay.as_ref().filter(|o| !o.flat)?;
                Some((t, o, o.paint.clone()?))
            })
        });
        let Some((t, overlay, paint)) = painted else {
            return false;
        };
        let key = Arc::as_ptr(&paint) as *const () as usize;
        let mut fresh = false;
        if gpu.paint_key != Some(key) {
            match gpu.paint_bake.take() {
                Some(job) if job.is_finished() => {
                    if let Ok(map) = job.join() {
                        engine_core::log::info!(
                            "terrain: paint map of the hand-made ground, {}² texels of {} m",
                            map.size,
                            map.texel_m
                        );
                        gpu.paint_tex = paint_texture(gpu_shared, map.size);
                        gpu.paint_map = None;
                        gpu.paint_key = Some(key);
                        gpu.paint_upload = Some((map, 0));
                        fresh = true;
                    }
                }
                Some(job) => gpu.paint_bake = Some(job),
                None => {
                    let (radius, blend) = (overlay.radius_m, overlay.blend_m);
                    gpu.paint_bake = Some(
                        std::thread::Builder::new()
                            .name("terrain-paint-map".into())
                            .spawn(move || {
                                PaintMap::bake_paint(
                                    &paint,
                                    radius,
                                    blend,
                                    PAINT_MAP_HALF_M,
                                    PAINT_MAP_TEXEL_M,
                                )
                            })
                            .expect("spawn the paint map bake"),
                    );
                }
            }
        }
        if let Some((map, row)) = gpu.paint_upload.take() {
            let next = (row + PAINT_ROWS_PER_FRAME).min(map.size);
            write_paint_rows(gpu_shared, &gpu.paint_tex.0, &map, row..next);
            if next < map.size {
                gpu.paint_upload = Some((map, next));
            } else {
                gpu.paint_map = Some((map.min, map.texel_m, map.size));
            }
        }
        let Some((min, texel, size)) = gpu.paint_map.filter(|_| gpu.paint_key == Some(key)) else {
            return fresh;
        };
        // The overlay's frame: planet-local = R⁻¹ (home − T); world = position + rotation ·
        // planet-local.
        let to_local = overlay.rotation.inverse();
        let world_of =
            |home: DVec3| t.position + t.rotation * (to_local * (home - overlay.translation));
        let axis = |d: DVec3| t.rotation * (to_local * d);
        let corner = world_of(DVec3::new(min[0], 0.0, min[1])) - view.origin;
        let (u, v, up) = (axis(DVec3::X), axis(DVec3::Z), axis(DVec3::Y));
        maps.paint_origin = [
            corner.x as f32,
            corner.y as f32,
            corner.z as f32,
            texel as f32,
        ];
        maps.paint_u = [u.x as f32, u.y as f32, u.z as f32, size as f32];
        maps.paint_v = [v.x as f32, v.y as f32, v.z as f32, 1.0];
        maps.paint_up = [up.x as f32, up.y as f32, up.z as f32, 0.0];
        fresh
    }

    /// Upload changed parts of the footprint grid and the weather surface layers.
    fn upload_maps(
        &mut self,
        world: &mut World,
        gpu_shared: &GpuShared,
        view: &ViewData,
        steps: &mut Steps,
    ) -> MapsGpu {
        let mut maps = MapsGpu::default();
        let mut recreate = false;
        let gpu = self.gpu.as_mut().expect("gpu");
        // Footprints.
        if let Some(mut def) = world.get_resource_mut::<DeformationMap>() {
            let size = def.size;
            let fresh = gpu.deform_tex.as_ref().is_none_or(|t| t.2 != size)
                || self.deform_epoch != def.epoch;
            if fresh {
                self.deform_epoch = def.epoch;
                let tex = gpu_shared.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("terrain footprints"),
                    size: wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rg16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = tex.create_view(&Default::default());
                gpu.deform_tex = Some((tex, view, size));
                let ids = gpu_shared.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("terrain print ids"),
                    size: wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rg16Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = ids.create_view(&Default::default());
                gpu.print_ids = Some((ids, view));
                recreate = true;
                // A new texture starts at zero: only the tiles holding something are sent.
                def.mark_all_dirty();
            }
            // Print shapes are kept relative to the grid's corner: all of them again when it
            // moved, else the new ones.
            let slots = if fresh || def.generation != self.deform_generation {
                self.deform_generation = def.generation;
                def.slots_dirty = None;
                Some((0, def.slots.len()))
            } else {
                def.slots_dirty.take()
            };
            if let Some((a, b)) = slots.filter(|(a, b)| a < b) {
                let o = def.origin;
                let data: Vec<PrintGpu> = def.slots[a..b]
                    .iter()
                    .map(|p| PrintGpu {
                        at: [
                            (p.center.x - o.x) as f32,
                            (p.center.y - o.y) as f32,
                            p.forward.x,
                            p.forward.y,
                        ],
                        size: [p.half.x, p.half.y, p.depth, p.rim],
                        info: [
                            p.center_depth,
                            contact_code(p.contact),
                            p.kind as u8 as f32,
                            0.0,
                        ],
                    })
                    .collect();
                gpu_shared.queue.write_buffer(
                    &gpu.prints,
                    (a * std::mem::size_of::<PrintGpu>()) as u64,
                    bytemuck::cast_slice(&data),
                );
            }
            // Changed tiles, in storage cells (the grid wraps around; see `DeformationMap`).
            for [x0, y0, x1, y1] in def.take_dirty() {
                let (w, h) = (x1 - x0, y1 - y0);
                let mut data = Vec::with_capacity((w * h * 2) as usize);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * size + x) as usize;
                        data.push(f16_bits(def.depth[i]));
                        data.push(f16_bits(def.flatten[i]));
                    }
                }
                let ids: Vec<[u16; 2]> = (y0..y1)
                    .flat_map(|y| {
                        let row = (y * size) as usize;
                        def.prints[row + x0 as usize..row + x1 as usize]
                            .iter()
                            .copied()
                    })
                    .collect();
                gpu_shared.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &gpu.print_ids.as_ref().expect("texture").0,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&ids),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                let tex = &gpu.deform_tex.as_ref().expect("texture").0;
                gpu_shared.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&data),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
            maps.deform = [
                (def.origin.x - view.origin.x) as f32,
                (def.origin.y - view.origin.z) as f32,
                def.cell,
                size as f32,
            ];
            maps.deform_base = [
                def.base[0] as f32 / size as f32,
                def.base[1] as f32 / size as f32,
                def.base[0] as f32,
                def.base[1] as f32,
            ];
            maps.flags[0] = 1.0;
        }
        steps.mark("maps: footprints");
        // Wetness, puddles, snow and scorch from the weather (every few frames).
        if let Some(layers) = world.get_resource::<SurfaceLayers>() {
            let g = &layers.grid;
            let fresh = gpu
                .surface_tex
                .as_ref()
                .is_none_or(|t| t.2 != g.width || t.3 != g.height);
            if fresh {
                let tex = gpu_shared.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("terrain surface layers"),
                    size: wgpu::Extent3d {
                        width: g.width,
                        height: g.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = tex.create_view(&Default::default());
                gpu.surface_tex = Some((tex, view, g.width, g.height));
                recreate = true;
            }
            let tex = &gpu.surface_tex.as_ref().expect("texture").0;
            self.surface.update(g, fresh, tex, &gpu_shared.queue);
            maps.surface = [
                (g.origin.x - view.origin.x) as f32,
                (g.origin.y - view.origin.z) as f32,
                g.cell,
                g.width as f32,
            ];
            maps.flags[1] = 1.0;
            maps.far = [self.surface.far[0], 0.0, self.surface.far[1], 0.0];
            maps.surface_base = [
                self.surface.base.x as f32 / g.width as f32,
                self.surface.base.y as f32 / g.height as f32,
                0.0,
                0.0,
            ];
        }
        steps.mark("maps: weather");
        recreate |= self.upload_wear(world, gpu_shared, view, &mut maps);
        steps.mark("maps: worn paths");
        recreate |= self.upload_paint_map(world, gpu_shared, view, &mut maps);
        steps.mark("maps: paint");
        let gpu = self.gpu.as_mut().expect("gpu");
        // Placeholders so the bind group is always complete.
        for (label, slot) in [("deform", 0usize), ("surface", 1usize), ("wear", 2usize)] {
            let missing = match slot {
                0 => gpu.deform_tex.is_none(),
                1 => gpu.surface_tex.is_none(),
                _ => gpu.wear_tex.is_none(),
            };
            if missing {
                let tex = gpu_shared.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: match slot {
                        0 => wgpu::TextureFormat::Rg16Float,
                        1 => wgpu::TextureFormat::Rgba16Float,
                        _ => wgpu::TextureFormat::R8Unorm,
                    },
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let v = tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(if slot == 2 {
                        wgpu::TextureViewDimension::D2Array
                    } else {
                        wgpu::TextureViewDimension::D2
                    }),
                    ..Default::default()
                });
                if slot == 0 {
                    let ids = gpu_shared.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("print ids"),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rg16Uint,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    let view = ids.create_view(&Default::default());
                    gpu.print_ids = Some((ids, view));
                }
                match slot {
                    0 => gpu.deform_tex = Some((tex, v, 0)),
                    1 => gpu.surface_tex = Some((tex, v, 0, 0)),
                    _ => gpu.wear_tex = Some((tex, v, 0)),
                }
                recreate = true;
            }
        }
        if recreate {
            self.rebuild_group(gpu_shared);
        }
        maps
    }
}

impl RenderFeature for TerrainFeature {
    fn name(&self) -> &str {
        "terrain"
    }

    fn order(&self) -> i32 {
        -10
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 13,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 14,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let buffers = [Some(TerrainVertex::layout()), Some(ChunkInstance::layout())];
        let shader = gpu.shader("terrain", SHADER, true);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        // The scene pass shades only what the prepass drew: an equal depth test without depth
        // writes or `discard`, so every GPU tests depth before shading (`discard` with depth
        // writes forces the test after the shader on many) and nothing hidden is shaded.
        //
        // Measuring the ground's pixel cost: `TERRAIN_SHADER_OFF=<bits>` switches parts of
        // `fs_main` off (the `TERRAIN_OFF` bits in terrain.wgsl), `TERRAIN_DEPTH=greater` draws
        // the scene pass the old way (`GreaterEqual` with depth writes and the mirror's clip).
        let off: f64 = std::env::var("TERRAIN_SHADER_OFF")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0) as f64;
        let greater = std::env::var("TERRAIN_DEPTH").is_ok_and(|v| v == "greater");
        let main_depth = if greater {
            gpu.depth_state(true)
        } else {
            wgpu::DepthStencilState {
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Equal),
                ..gpu.depth_state(false)
            }
        };
        let scene_pipeline = |label: &str, off: f64| {
            let constants = [
                ("TERRAIN_OFF", off),
                ("MIRROR_CLIP", if greater { 1.0 } else { 0.0 }),
            ];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(main_depth.clone()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = scene_pipeline("terrain", off);
        let extra_view_pipeline =
            scene_pipeline("terrain extra view", (off as u32 | EXTRA_VIEW_OFF) as f64);
        // Same vertices as the scene pass (the clip position is invariant), so its depth test
        // passes exactly where the prepass drew.
        let prepass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain prepass"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_prepass"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(gpu.depth_state(true)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_prepass"),
                compilation_options: Default::default(),
                targets: &gpu.prepass_targets(),
            }),
            multiview_mask: None,
            cache: None,
        });
        // The water's surface over the patches that hold a river or a lake: the same vertices
        // raised to the water, drawn over the scene. Its color adds the light the water
        // reflects and scatters back; the scene behind is dimmed by what the water lets
        // through (the alpha), so no copy of the scene is needed.
        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain water"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let wpl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain water"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout), Some(&water_layout)],
            immediate_size: 0,
        });
        let water_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain water"),
            layout: Some(&wpl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_water"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(gpu.depth_state(false)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_water"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.hdr,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::SrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let shadow_shader = gpu.shader("terrain shadow", SHADOW_SHADER, false);
        let spl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain shadow"),
            layout: Some(&spl),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &buffers,
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
        let indices = chunk_indices();
        let index = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain indices"),
            size: (indices.len() * 2).next_multiple_of(4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bytes: Vec<u8> = bytemuck::cast_slice(&indices).to_vec();
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        gpu.queue.write_buffer(&index, 0, &bytes);
        let maps = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain maps"),
            size: std::mem::size_of::<MapsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain maps"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Ground materials: placeholders until the background bake finishes.
        let (mat_albedo, mat_normal) = material_arrays(gpu, None);
        let mat_info = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain materials"),
            size: std::mem::size_of::<MaterialsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(
            &mat_info,
            0,
            bytemuck::bytes_of(&MaterialsGpu::from_baked(None)),
        );
        let mat_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain materials"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        self.gpu = Some(Gpu {
            pipeline,
            extra_view_pipeline,
            prepass_pipeline,
            shadow_pipeline,
            water_pipeline,
            water_layout,
            water_groups: Vec::new(),
            water_group_now: 0,
            layout,
            index,
            index_count: indices.len() as u32,
            instances: GrowBuffer::new(
                device,
                "terrain instances",
                wgpu::BufferUsages::VERTEX,
                4096 * std::mem::size_of::<ChunkInstance>() as u64,
            ),
            draw_args: None,
            planets: GrowBuffer::new(
                device,
                "terrain planets",
                wgpu::BufferUsages::STORAGE,
                8 * std::mem::size_of::<PlanetGpu>() as u64,
            ),
            maps,
            sampler,
            deform_tex: None,
            print_ids: None,
            prints: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("terrain prints"),
                size: (SLOTS * std::mem::size_of::<PrintGpu>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            surface_tex: None,
            wear_tex: None,
            mat_albedo,
            mat_normal,
            mat_info,
            mat_sampler,
            mat_size: 0,
            mat_key: None,
            mat_scanned: 0,
            night: NightPages::new(device),
            far_looks: FarLookPages::new(device),
            paint_tex: paint_texture(gpu, 1),
            paint_map: None,
            paint_key: None,
            paint_bake: None,
            paint_upload: None,
            group: None,
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu_shared: &GpuShared,
        view: &ViewData,
        render_settings: &RenderSettings,
    ) {
        // An extra view (a lake mirror, a portal, a picture-in-picture) draws what is built from
        // its own camera but leaves the quadtree, the builds and the frame count to the main
        // view: a camera a few meters under the lake would split and merge patches against it
        // every frame, and one elsewhere would replace the main view's wanted patches with its
        // own each frame.
        let mirror = view.extra_view || engine_render::rendering_reflection(world).is_some();
        if !mirror {
            self.frame += 1;
        }
        self.draws.clear();
        self.water_draws.clear();
        let lamp_handoff =
            crate::lights::handoff_distance(world, view, render_settings.max_point_lights);
        for s in &mut self.shadow_draws {
            s.clear();
        }
        self.far_shadow_draws.clear();
        if self.gpu.is_none() {
            return;
        }
        let mut settings = world
            .get_resource::<TerrainSettings>()
            .cloned()
            .unwrap_or_default();
        // Low and Medium quality draw fewer, larger patches: the patch count goes with the
        // square of the split factor, so both scale it by `view_distance` (High and Ultra
        // keep the look the showcase is tuned for).
        let reach = world
            .get_resource::<engine_world::Scalability>()
            .map_or(1.0, |s| s.view_distance.clamp(0.25, 1.0) as f64);
        settings.split_factor *= reach.sqrt();
        settings.patch_budget = (settings.patch_budget as f64 * reach).round() as usize;
        let mut steps = Steps::start();
        let maps = self.upload_maps(world, gpu_shared, view, &mut steps);
        steps.mark("maps: bind group");
        let Some(terrains) = world.get_resource::<Terrains>() else {
            return;
        };
        if !terrains.entries.is_empty() {
            let scans = settings.scan_folder(world);
            self.upload_materials(gpu_shared, settings.material_resolution, scans);
        }
        steps.mark("materials");
        let (materials_ready, scanned) = self
            .gpu
            .as_ref()
            .map_or((false, 0), |g| (g.mat_size > 0, g.mat_scanned));
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("terrain.materials_ready", materials_ready as u32 as f64);
            d.set("terrain.scanned_materials", scanned as f64);
        }
        let Some(terrains) = world.get_resource::<Terrains>() else {
            return;
        };

        // Forget terrains that are gone.
        let gone: Vec<Entity> = self
            .planets
            .keys()
            .filter(|e| !terrains.entries.contains_key(e))
            .copied()
            .collect();
        for e in gone {
            if let Some(p) = self.planets.remove(&e) {
                for c in p.chunks.values() {
                    self.pool.release(c.slot);
                }
            }
        }

        let mut entities: Vec<Entity> = terrains.entries.keys().copied().collect();
        entities.sort();
        let atmosphere = world.get_resource::<engine_render::Atmosphere>().cloned();
        let fog = world.get_resource::<engine_render::FogSettings>().copied();
        let (atmosphere, fog) = (atmosphere.as_ref(), fog.as_ref());
        let city = world.get_resource::<CityLights>();

        // Looks from afar, uploaded to a layer of the map array as their planets are drawn.
        let far_looks: HashMap<Entity, FarLook> = entities
            .iter()
            .filter_map(|e| world.get::<FarLook>(*e).map(|l| (*e, l.clone())))
            .collect();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.far_looks.begin_frame();
            gpu.night.begin_frame();
            gpu.night
                .retain_planets(|e| terrains.entries.contains_key(e));
        }
        steps.mark("far looks");
        let mut planet_gpu = Vec::with_capacity(entities.len());
        let mut instances: Vec<ChunkInstance> = Vec::new();
        let mut built_total = 0usize;
        // Patches the view lacks, and those on their way.
        let mut missing_total = 0usize;
        let mut building_total = 0usize;
        let mut latency_ms = 0.0f64;
        let mut max_boost = 1.0f64;
        let max_slots = settings.max_patches as u32;
        // Put the patches the workers finished into vertex slots, a few a frame.
        let workers = self.workers.get_or_insert_with(PatchWorkers::new);
        self.finished.extend(workers.take_done());
        if !self.finished.is_empty() && self.pool.used() + SLOTS_PER_PAGE > max_slots {
            for state in self.planets.values_mut() {
                free_stale(&mut self.pool, state);
            }
        }
        steps.mark("uploads: free stale");
        // A new vertex page before the uploads, if they may need one (timed on its own).
        if self.pool.free.len() < self.finished.len().min(MAX_UPLOADS_PER_FRAME) {
            self.pool.grow(&gpu_shared.device, max_slots);
        }
        steps.mark("uploads: new page");
        let mut uploads = 0;
        let mut waiting = Vec::new();
        let mut taken = Vec::new();
        let upload_started = std::time::Instant::now();
        for f in std::mem::take(&mut self.finished) {
            let Some(state) = self.planets.get_mut(&f.planet) else {
                taken.push((f.planet, f.mesh.key));
                continue;
            };
            if state.version != f.version {
                taken.push((f.planet, f.mesh.key));
                continue;
            }
            if uploads >= MAX_UPLOADS_PER_FRAME
                || (uploads >= MIN_UPLOADS_PER_FRAME
                    && upload_started.elapsed().as_secs_f64() * 1000.0 > UPLOAD_BUDGET_MS)
            {
                waiting.push(f);
                continue;
            }
            let key = f.mesh.key;
            taken.push((f.planet, key));
            upload(&mut self.pool, state, f.mesh, gpu_shared, max_slots);
            state.building.remove(&key);
            // How long patches take from being asked for to being drawn, for the look-ahead.
            if let Some(at) = state.asked.remove(&key) {
                let s = at.elapsed().as_secs_f64();
                state.latency_s += (s - state.latency_s) * 0.05;
            }
            uploads += 1;
        }
        self.finished = waiting;
        workers.taken(&taken);
        built_total += uploads;
        steps.mark("uploads");

        for (pi, e) in entities.iter().enumerate() {
            let t = &terrains.entries[e];
            let src = t.source.clone();
            let state = self
                .planets
                .entry(*e)
                .or_insert_with(|| PlanetState::new(t, &src));
            if state.version != t.version {
                for c in state.chunks.values() {
                    self.pool.release(c.slot);
                }
                *state = PlanetState::new(t, &src);
            }
            // Edited areas: rebuild the patches that cover them, keeping the old ones drawn.
            if state.edits_seen < t.edit_log.len() {
                for &(c, r) in &t.edit_log[state.edits_seen..] {
                    for k in state.lod.touching(c, r) {
                        if !state.rebuild.contains(&k) {
                            state.rebuild.push(k);
                        }
                    }
                }
                state.edits_seen = t.edit_log.len();
            }

            let rot: DQuat = t.rotation;
            let pos = t.position;
            let motion = state
                .last_position
                .map_or(Vec3::ZERO, |last| (pos - last).as_vec3());
            if !mirror {
                state.last_position = Some(pos);
            }
            let cam_local = rot.inverse() * (view.origin - pos);
            let frustum = view.frustum;
            let origin = view.origin;
            let visible = move |c: DVec3, r: f64| {
                let rel = (pos + rot * c - origin).as_vec3();
                frustum.intersects_sphere(rel, r as f32)
            };
            let split = settings.split_factor * t.config.detail.max(0.05) as f64 * state.boost;
            // Land deep in the air's haze splits later (`TerrainSettings::haze_detail`).
            let haze = (settings.haze_detail > 0.0 && src.map.is_sphere())
                .then(|| Haze::new(atmosphere, fog, pos, src.map.radius))
                .flatten();
            let radius = src.map.radius;
            let cam_alt = cam_local.length() - radius;
            let haze_fn = haze.map(|h| {
                move |dist: f64, center: DVec3| {
                    h.detail(
                        cam_alt,
                        center.length() - radius,
                        dist,
                        settings.haze_detail,
                    )
                }
            });
            let haze_detail = haze_fn.as_ref().map(|f| f as &dyn Fn(f64, DVec3) -> f64);

            // Select, and ask the workers for what the view lacks, nearest first. Until the
            // patches are built their parents are drawn. After a jump (many patches missing),
            // everything missing is queued at once.
            let mut sel = if mirror {
                // `walk` without a merge factor leaves the split state alone.
                state.lod.walk(&SelectInput {
                    camera: cam_local,
                    split_factor: split,
                    merge_factor: None,
                    visible: &visible,
                    min_size: 0.0,
                    detail: haze_detail,
                })
            } else {
                state.lod.select(&SelectInput {
                    camera: cam_local,
                    split_factor: split,
                    merge_factor: Some(split * MERGE_HYSTERESIS),
                    visible: &visible,
                    min_size: 0.0,
                    detail: haze_detail,
                })
            };
            steps.mark("patches: select");
            if mirror {
                sel.missing.clear();
            }
            // What the workers should build, most urgent first: the view's missing patches by
            // distance in patch sizes (coarse before fine at the same distance, the way the
            // screen-space error ranks them), then those the camera will need where it is
            // heading, then the far shadow map's (below). Re-asked every frame, so what the
            // view no longer needs is dropped before it is built.
            let mut wants: Vec<Want> = Vec::new();
            let urgency = |dist: f64, k: NodeKey| dist / src.map.node_size(k.level);
            for &(dist, k) in &sel.missing {
                wants.push((urgency(dist, k), k, false));
            }
            missing_total += sel.missing.len();
            // Where the camera will be by the time a patch asked for now is built and drawn
            // (the measured wait, with room to spare), up to a few kilometers ahead.
            if !mirror {
                let now = std::time::Instant::now();
                if let Some((last, at)) = state.last_camera {
                    let dt = now.duration_since(at).as_secs_f64();
                    if dt > 1e-4 {
                        let v = (cam_local - last) / dt;
                        state.camera_velocity = if v.length() > JUMP_SPEED {
                            DVec3::ZERO
                        } else {
                            state.camera_velocity.lerp(v, (dt * 4.0).min(1.0))
                        };
                    }
                }
                state.last_camera = Some((cam_local, now));
                let lead_s = (state.latency_s * 2.0).clamp(LOOK_AHEAD_MIN_S, LOOK_AHEAD_S);
                let ahead = (state.camera_velocity * lead_s).clamp_length_max(LOOK_AHEAD_MAX_M);
                if ahead.length() > 10.0 {
                    let soon = state.lod.walk(&SelectInput {
                        camera: cam_local + ahead,
                        split_factor: split,
                        merge_factor: None,
                        visible: &visible,
                        min_size: 0.0,
                        detail: haze_detail,
                    });
                    for &(dist, k) in &soon.missing {
                        wants.push((urgency(dist, k) + 1.0, k, false));
                    }
                }
            }
            steps.mark("patches: look-ahead");
            // Edited patches, kept drawn meanwhile, queued until built (one being built waits
            // for that build before the next: the edit may have come after it started).
            let mut i = 0;
            while !mirror && i < state.rebuild.len() {
                let k = state.rebuild[i];
                if state.building.contains(&k) {
                    i += 1;
                    continue;
                }
                state.rebuild.remove(i);
                let dist =
                    (cam_local - state.chunks.get(&k).map_or(cam_local, |c| c.center)).length();
                wants.push((urgency(dist, k), k, true));
            }
            // Few patches for the planet's share of the view (seen from orbit): split finer,
            // easing toward the detail that fills the budget. The patch count grows with the
            // square of the split factor.
            if settings.patch_budget > 0 && !mirror {
                let dist = cam_local.length().max(1.0);
                let angle = (src.map.radius / dist).min(1.0).asin();
                let share = if view.fov_y > 0.0 {
                    (angle / (0.5 * view.fov_y as f64)).min(1.0).powi(2)
                } else {
                    1.0
                };
                let budget = settings.patch_budget as f64 * share;
                let needed = (sel.clear_count + sel.missing.len() as f64).max(1.0);
                let base = needed / (state.boost * state.boost);
                // No finer than the screen shows (`TerrainSettings::min_quad_px`): a patch's
                // quads span 1 / (QUADS × split) radians at its split distance.
                let cap = if settings.min_quad_px > 0.0 && view.fov_y > 0.0 {
                    let px_per_rad = view.height as f64 / view.fov_y as f64;
                    let unboosted = split / state.boost;
                    px_per_rad / (QUADS as f64 * unboosted * settings.min_quad_px)
                } else {
                    MAX_BOOST
                };
                let want = (budget / base).sqrt().clamp(1.0, cap.clamp(1.0, MAX_BOOST));
                // While much of the view is still streaming in (after a jump), its count falls
                // short of what it needs: then the detail only eases down.
                if want < state.boost || sel.missing.len() * 10 <= sel.draw.len() {
                    state.boost += (want - state.boost) * 0.2;
                }
                max_boost = max_boost.max(state.boost);
            }

            // The far shadow map: the same walk from the camera over the ground the map covers,
            // so ridges outside the view shade the valley kilometers away. Shadows fall away from
            // the sun, so on that side only what lies within the map's reach counts. One level
            // coarser than the view (the map's texels are meters across, and its bias covers the
            // difference), stopping at patches with vertices about half a texel apart; it builds
            // what it lacks a few patches a frame after the view's own, drawing parents meanwhile.
            let far = view.far_shadow;
            let sun = view.sun_dir;
            let far_sel = (!mirror && t.config.cast_shadows && view.sun_shadows && far.enabled)
                .then(|| {
                    let far_visible = move |c: DVec3, r: f64| {
                        let rel = (pos + rot * c - origin).as_vec3();
                        let r = r as f32;
                        far.casts(rel, r)
                            && (rel.dot(sun) + r >= 0.0 || rel.length() <= far.half_extent + r)
                    };
                    let input = SelectInput {
                        camera: cam_local,
                        split_factor: split * FAR_SHADOW_DETAIL,
                        merge_factor: None,
                        visible: &far_visible,
                        min_size: far.texel() as f64 * 0.5 * crate::chunk::QUADS as f64,
                        detail: None,
                    };
                    state.lod.walk(&input)
                });
            steps.mark("patches: far shadow");
            // (Its patches after the view's: the shadows of ridges far off can wait a little.)
            if let Some(far_sel) = &far_sel {
                for &(dist, k) in &far_sel.missing {
                    wants.push((urgency(dist, k) + 2.0, k, false));
                }
            }
            if !mirror {
                // Each patch once, at its most urgent; at most a few hundred (after a jump the
                // nearest go first and the rest are asked for again as those land).
                wants.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut seen = HashSet::new();
                wants.retain(|w| seen.insert(w.1));
                wants.truncate(MAX_QUEUED.max(workers.threads() * 4));
                // (None: a worker panicked holding the queue.)
                if let Some(dropped) = workers.request(*e, state.version, &src, &wants) {
                    for k in dropped {
                        state.building.remove(&k);
                        state.asked.remove(&k);
                    }
                    for &(_, k, _) in &wants {
                        if state.building.insert(k) {
                            state.asked.insert(k, std::time::Instant::now());
                        }
                    }
                }
            }

            steps.mark("patches: ask workers");
            building_total += state.building.len();
            latency_ms = latency_ms.max(state.latency_s * 1000.0);

            // Recycle patches nobody has looked at for a while.
            if !mirror && self.frame.is_multiple_of(60) {
                for k in state.lod.stale(STALE_FRAMES) {
                    if let Some(c) = state.chunks.remove(&k) {
                        self.pool.release(c.slot);
                    }
                    state.lod.unbuild(k);
                }
            }

            steps.mark("patches: recycle");
            // Draw lists.
            let lamps = city
                .and_then(|c| c.planets.get(e))
                .filter(|l| !l.is_empty() && src.map.is_sphere());
            let rot_f = Mat3::from_quat(rot.as_quat());
            let casts = t.config.cast_shadows && view.sun_shadows;
            // The parent takes over at `split` parent sizes; be fully morphed by then.
            let morph = |level: u8| {
                if settings.geomorph && level > 0 {
                    let merge = (split * state.lod.map.node_size(level - 1)) as f32;
                    (merge * 0.75, merge * 0.98)
                } else {
                    (0.0, 0.0)
                }
            };
            for key in &sel.draw {
                let Some(c) = state.chunks.get(key) else {
                    continue;
                };
                let morph = morph(key.level);
                let rel = (pos + rot * c.origin - view.origin).as_vec3();
                let instance = instances.len() as u32;
                let night = match (lamps, self.gpu.as_mut()) {
                    (Some(lamps), Some(gpu)) => {
                        gpu.night
                            .lookup(&gpu_shared.queue, *e, lamps, &state.lod.map, *key)
                    }
                    _ => NO_PAGE,
                };
                instances.push(ChunkInstance {
                    origin: rel.to_array(),
                    planet: pi as u32,
                    lod: [c.spacing as f32, key.level as f32, morph.0, morph.1],
                    night,
                });
                self.draws.push(Draw {
                    slot: c.slot,
                    instance,
                });
                if c.water && !mirror {
                    self.water_draws.push(Draw {
                        slot: c.slot,
                        instance,
                    });
                }
                if casts {
                    let center = (pos + rot * c.center - view.origin).as_vec3();
                    for i in 0..(view.cascades.count as usize).min(4) {
                        if view.cascades.sphere_in_cascade(i, center, c.radius as f32) {
                            self.shadow_draws[i].push(Draw {
                                slot: c.slot,
                                instance,
                            });
                        }
                    }
                }
            }
            for key in far_sel.iter().flat_map(|s| &s.draw) {
                let Some(c) = state.chunks.get(key) else {
                    continue;
                };
                let morph = morph(key.level);
                let rel = (pos + rot * c.origin - view.origin).as_vec3();
                let instance = instances.len() as u32;
                instances.push(ChunkInstance {
                    origin: rel.to_array(),
                    planet: pi as u32,
                    lod: [c.spacing as f32, key.level as f32, morph.0, morph.1],
                    night: NO_PAGE,
                });
                self.far_shadow_draws.push(Draw {
                    slot: c.slot,
                    instance,
                });
            }

            steps.mark("patches: draw lists");
            let p = &t.config.palette;
            let col = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
            let center_rel = (pos - view.origin).as_vec3();
            let (far, far_mean) = far_looks
                .get(e)
                .and_then(|look| {
                    let gpu = self.gpu.as_mut()?;
                    let (layer, mean) = gpu.far_looks.layer(&gpu_shared.queue, *e, look)?;
                    Some((
                        [layer as f32, look.near_tint],
                        [mean[0], mean[1], mean[2], 0.0],
                    ))
                })
                .unwrap_or(([-1.0, 0.0], [0.0; 4]));
            let locked = if t.config.shape.climate.substellar {
                1.0
            } else {
                0.0
            };
            let far = [far[0], far[1], locked, t.config.palette.shore_mats];
            let far_mean = [far_mean[0], far_mean[1], far_mean[2], p.farmland];
            planet_gpu.push(PlanetGpu {
                rot: [
                    rot_f.x_axis.extend(0.0).to_array(),
                    rot_f.y_axis.extend(0.0).to_array(),
                    rot_f.z_axis.extend(0.0).to_array(),
                ],
                center: [
                    center_rel.x,
                    center_rel.y,
                    center_rel.z,
                    t.source.map.radius as f32,
                ],
                colors: [
                    col(p.lush),
                    col(p.dry),
                    col(p.soil),
                    col(p.rock),
                    col(p.sand),
                    col(p.snow),
                    col(p.dark),
                    col(p.ocean),
                    col(p.mud),
                    col([0.33, 0.32, 0.30]),
                ],
                params: [
                    p.snow_below_c,
                    p.rock_slope,
                    if p.vegetation { 1.0 } else { 0.0 },
                    if t.config.ocean_surface && t.config.shape.ocean {
                        1.0
                    } else {
                        0.0
                    },
                ],
                params2: [
                    if t.source.map.is_sphere() { 1.0 } else { 0.0 },
                    t.config.shape.crater_density as f32,
                    t.config.shape.crater_max_radius_m as f32,
                    t.config.shape.crater_seed() as f32,
                ],
                motion: motion.extend(0.0).to_array(),
                params3: [
                    p.regolith,
                    state.rayed as f32,
                    state.young_count as f32,
                    t.config.shape.crater_min_radius_m as f32,
                ],
                rays: state.rays,
                young: state.young,
                params4: [
                    p.night_lights,
                    lamp_handoff,
                    p.lava_glow,
                    melt_latitude(&t.config.shape.climate),
                ],
                far,
                far_mean,
            });
        }
        self.built_last_frame = built_total;
        steps.mark("patches: planet data");

        // Group draws by vertex page (one multi-draw a page), the scene's and the water's
        // nearest first within it, so the prepass's early depth test rejects most of what is
        // behind (the scene pass then shades only what the prepass left: an equal test).
        let page = |d: &Draw| d.slot / SLOTS_PER_PAGE;
        let near =
            |d: &Draw| Vec3::from_array(instances[d.instance as usize].origin).length_squared();
        let front_to_back =
            |a: &Draw, b: &Draw| page(a).cmp(&page(b)).then(near(a).total_cmp(&near(b)));
        self.draws.sort_by(front_to_back);
        self.water_draws.sort_by(front_to_back);
        for s in &mut self.shadow_draws {
            s.sort_by_key(page);
        }
        self.far_shadow_draws.sort_by_key(page);
        self.write_draw_args(gpu_shared, render_settings);

        let mut maps = maps;
        maps.flags[2] = if settings.wireframe_lod { 1.0 } else { 0.0 };
        let gpu = self.gpu.as_mut().expect("gpu");
        let mut regroup = false;
        if !planet_gpu.is_empty() {
            regroup |= gpu.planets.write(
                &gpu_shared.device,
                &gpu_shared.queue,
                bytemuck::cast_slice(&planet_gpu),
            );
        }
        if !instances.is_empty() {
            gpu.instances.write(
                &gpu_shared.device,
                &gpu_shared.queue,
                bytemuck::cast_slice(&instances),
            );
        }
        gpu_shared
            .queue
            .write_buffer(&gpu.maps, 0, bytemuck::bytes_of(&maps));
        if regroup || gpu.group.is_none() {
            self.rebuild_group(gpu_shared);
        }

        let tris_per_patch = (self.gpu.as_ref().map(|g| g.index_count).unwrap_or(0) / 3) as u64;
        self.stats = FeatureStats {
            draw_calls: (self.draws.len()
                + self.shadow_draws.iter().map(Vec::len).sum::<usize>()
                + self.far_shadow_draws.len()) as u32,
            instances: self.draws.len() as u32,
            triangles: self.draws.len() as u64 * tris_per_patch,
            culled: 0,
        };
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("terrain.patches_drawn", self.draws.len() as f64);
            d.set(
                "terrain.far_shadow_patches",
                self.far_shadow_draws.len() as f64,
            );
            d.set("terrain.patches_resident", self.pool.used() as f64);
            d.set("terrain.patches_built", built_total as f64);
            d.set("terrain.patches_missing", missing_total as f64);
            d.set("terrain.patches_building", building_total as f64);
            d.set("terrain.patch_latency_ms", latency_ms);
            d.set("terrain.detail_boost", max_boost);
            if let Some(gpu) = &self.gpu {
                // The hand-made ground's paint map is still baking: until it lands, that ground
                // is painted from its patches' vertices only (a loading screen waits for it).
                d.set(
                    "terrain.paint_map_baking",
                    (gpu.paint_bake.is_some() || gpu.paint_upload.is_some()) as u32 as f64,
                );
                d.set("terrain.night_pages", gpu.night.resident() as f64);
            }
        }
        steps.mark("buffers");
        steps.publish(world);
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {
        let (pipeline, list, draws, indices) = match phase {
            RenderPhase::Opaque => (
                self.gpu.as_ref().map(|g| {
                    if ctx.view.extra_view {
                        &g.extra_view_pipeline
                    } else {
                        &g.pipeline
                    }
                }),
                DrawList::Scene,
                &self.draws,
                None,
            ),
            // The water over the grid only (a skirt would hang a curtain of it).
            RenderPhase::Transparent => (
                self.gpu.as_ref().map(|g| &g.water_pipeline),
                DrawList::Water,
                &self.water_draws,
                Some((QUADS * QUADS * 6) as u32),
            ),
            RenderPhase::Sky => return,
        };
        if draws.is_empty() {
            return;
        }
        let (Some(gpu), Some(pipeline)) = (&self.gpu, pipeline) else {
            return;
        };
        let Some(group) = &gpu.group else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, group, &[]);
        if phase == RenderPhase::Transparent {
            let Some((_, water)) = gpu.water_groups.get(gpu.water_group_now) else {
                return;
            };
            pass.set_bind_group(2, water, &[]);
        }
        self.draw_indices(pass, gpu, list, draws, indices.unwrap_or(gpu.index_count));
    }

    fn needs_scene_copy(&self) -> bool {
        !self.water_draws.is_empty()
    }

    /// The water pass's bind group for this view's scene copies (made anew only when they
    /// are: on a resize, or for another view).
    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if self.water_draws.is_empty() {
            return;
        }
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let key = [ctx.scene_color_copy.clone(), ctx.scene_depth_copy.clone()];
        if let Some(i) = gpu.water_groups.iter().position(|(k, _)| *k == key) {
            gpu.water_group_now = i;
            return;
        }
        let group = ctx
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain water"),
                layout: &gpu.water_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(ctx.scene_color_copy),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                    },
                ],
            });
        // A few views at most (the main view, a mirror); old sizes go.
        if gpu.water_groups.len() >= 4 {
            gpu.water_groups.remove(0);
        }
        gpu.water_groups.push((key, group));
        gpu.water_group_now = gpu.water_groups.len() - 1;
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if self.draws.is_empty() {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(group) = &gpu.group else {
            return;
        };
        pass.set_pipeline(&gpu.prepass_pipeline);
        pass.set_bind_group(1, group, &[]);
        self.draw_list(pass, gpu, DrawList::Scene, &self.draws);
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        if cascade >= 4 || self.shadow_draws[cascade].is_empty() {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(group) = &gpu.group else {
            return;
        };
        pass.set_pipeline(&gpu.shadow_pipeline);
        pass.set_bind_group(1, group, &[]);
        self.draw_list(
            pass,
            gpu,
            DrawList::Shadow(cascade),
            &self.shadow_draws[cascade],
        );
    }

    fn draw_far_shadow(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if self.far_shadow_draws.is_empty() {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(group) = &gpu.group else {
            return;
        };
        pass.set_pipeline(&gpu.shadow_pipeline);
        pass.set_bind_group(1, group, &[]);
        self.draw_list(pass, gpu, DrawList::FarShadow, &self.far_shadow_draws);
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "terrains": self.planets.len(),
            "patches_drawn": self.draws.len(),
            "far_shadow_patches": self.far_shadow_draws.len(),
            "patches_resident": self.pool.used(),
            "patches_built_last_frame": self.built_last_frame,
            "patches_building": self.planets.values().map(|p| p.building.len()).sum::<usize>(),
            "gpu_pages": self.pool.pages.len(),
        })
    }
}

impl TerrainFeature {
    fn draw_list(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        gpu: &Gpu,
        list: DrawList,
        draws: &[Draw],
    ) {
        self.draw_indices(pass, gpu, list, draws, gpu.index_count);
    }

    /// Draw the patches with the first `count` indices (the grid, then the skirts).
    fn draw_indices(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        gpu: &Gpu,
        list: DrawList,
        draws: &[Draw],
        count: u32,
    ) {
        pass.set_index_buffer(gpu.index.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_vertex_buffer(1, gpu.instances.buffer.slice(..));
        let indirect = gpu.draw_args.as_ref().filter(|_| self.multi_draw);
        let first_arg = self.list_args[list.index()] as u64;
        let mut i = 0;
        while i < draws.len() {
            let p = draws[i].slot / SLOTS_PER_PAGE;
            let run = draws[i..]
                .iter()
                .take_while(|d| d.slot / SLOTS_PER_PAGE == p)
                .count();
            pass.set_vertex_buffer(0, self.pool.pages[p as usize].slice(..));
            match indirect {
                Some(args) => pass.multi_draw_indexed_indirect(
                    &args.buffer,
                    (first_arg + i as u64) * std::mem::size_of::<DrawArgs>() as u64,
                    run as u32,
                ),
                None => {
                    for d in &draws[i..i + run] {
                        let base = ((d.slot % SLOTS_PER_PAGE) as usize * VERTS_PER_CHUNK) as i32;
                        pass.draw_indexed(0..count, base, d.instance..d.instance + 1);
                    }
                }
            }
            i += run;
        }
    }

    /// The indirect arguments of every draw list (multi-draw only), in [`DrawList`] order.
    fn write_draw_args(&mut self, gpu_shared: &GpuShared, settings: &RenderSettings) {
        self.multi_draw = settings.multi_draw_indirect
            && gpu_shared
                .device
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE);
        self.args.clear();
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        if !self.multi_draw {
            return;
        }
        let water = (QUADS * QUADS * 6) as u32;
        let lists = [
            (DrawList::Scene, &self.draws, gpu.index_count),
            (DrawList::Water, &self.water_draws, water),
        ]
        .into_iter()
        .chain((0..4).map(|i| (DrawList::Shadow(i), &self.shadow_draws[i], gpu.index_count)))
        .chain(std::iter::once((
            DrawList::FarShadow,
            &self.far_shadow_draws,
            gpu.index_count,
        )));
        for (list, draws, count) in lists {
            self.list_args[list.index()] = self.args.len() as u32;
            self.args.extend(draws.iter().map(|d| DrawArgs {
                index_count: count,
                instance_count: 1,
                first_index: 0,
                base_vertex: ((d.slot % SLOTS_PER_PAGE) as usize * VERTS_PER_CHUNK) as i32,
                first_instance: d.instance,
            }));
        }
        if self.args.is_empty() {
            return;
        }
        gpu.draw_args
            .get_or_insert_with(|| {
                GrowBuffer::new(
                    &gpu_shared.device,
                    "terrain draw arguments",
                    wgpu::BufferUsages::INDIRECT,
                    8192 * std::mem::size_of::<DrawArgs>() as u64,
                )
            })
            .write(
                &gpu_shared.device,
                &gpu_shared.queue,
                bytemuck::cast_slice(&self.args),
            );
    }
}

/// Paint map rows written a frame (2 MB of each layer at 2048 texels a row).
const PAINT_ROWS_PER_FRAME: u32 = 256;

/// Write rows `rows` of a paint map into both layers of its texture.
fn write_paint_rows(
    gpu: &GpuShared,
    tex: &wgpu::Texture,
    map: &PaintMap,
    rows: std::ops::Range<u32>,
) {
    let size = map.size;
    let span = (rows.start * size) as usize..(rows.end * size) as usize;
    for (z, texels) in [&map.paint, &map.tint].into_iter().enumerate() {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: rows.start,
                    z: z as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels[span.clone()]),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(rows.len() as u32),
            },
            wgpu::Extent3d {
                width: size,
                height: rows.len() as u32,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// How the air hides land at a distance, for the quadtree walk (`SelectInput::detail`): its
/// gas, its haze and the weather's height fog, each thinning exponentially with height above
/// the planet's sea level, integrated along the straight path from the camera's height to the
/// patch's (the renderer's own aerial perspective and `height_fog_optical_depth` in
/// lib_frame.wgsl fade the land the same way).
#[derive(Clone, Copy, Debug)]
struct Haze {
    /// Layers: extinction per meter at `base` (green), scale height (m), base height (m).
    layers: [(f64, f64, f64); 3],
    /// The fog never hides more than `FogSettings::max_opacity`.
    fog_floor: f64,
}

impl Haze {
    /// The haze around the planet at `position` (radius `radius`): the sky's air only if it is
    /// this planet's, and the fog with it. None without either.
    fn new(
        atmosphere: Option<&engine_render::Atmosphere>,
        fog: Option<&engine_render::FogSettings>,
        position: DVec3,
        radius: f64,
    ) -> Option<Self> {
        let a = atmosphere.filter(|a| {
            a.enabled
                && a.density_scale > 0.0
                && (a.planet_center - position).length() < radius * 0.01 + 1000.0
        })?;
        let ds = a.density_scale as f64;
        let gas = (
            a.rayleigh_scattering[1] as f64 * ds,
            a.rayleigh_scale_height as f64,
            0.0,
        );
        let dust = (
            (a.mie_scattering[1] + a.mie_absorption[1]) as f64 * ds,
            a.mie_scale_height as f64,
            0.0,
        );
        let mist = fog.map_or((0.0, 1.0, 0.0), |f| {
            (
                f.density.max(0.0) as f64,
                1.0 / f.height_falloff.max(1e-5) as f64,
                f.base_height as f64,
            )
        });
        Some(Self {
            layers: [gas, dust, mist],
            fog_floor: fog.map_or(0.0, |f| 1.0 - f.max_opacity.clamp(0.0, 1.0) as f64),
        })
    }

    /// Optical depth of layer `i` along `len` meters from height `h0` to `h1`.
    fn depth(&self, i: usize, h0: f64, h1: f64, len: f64) -> f64 {
        let (beta, scale, base) = self.layers[i];
        if beta <= 0.0 {
            return 0.0;
        }
        let e = |h: f64| (-((h - base).max(-100.0)) / scale).exp();
        let dh = h1 - h0;
        let mean = if dh.abs() > 0.01 {
            scale * (e(h0) - e(h1)) / dh
        } else {
            e(h0)
        };
        beta * len * mean
    }

    /// The air's transmittance from the camera at height `h0` to land at `h1`, `len` away.
    fn transmittance(&self, h0: f64, h1: f64, len: f64) -> f64 {
        let air = (-(self.depth(0, h0, h1, len) + self.depth(1, h0, h1, len))).exp();
        let fog = (-self.depth(2, h0, h1, len)).exp().max(self.fog_floor);
        air * fog
    }

    /// The share of the split distance that holds there: 1 in clear air, 1 / (1 + k) where
    /// the haze hides everything. The haze's share is squared, as Cesium's fog factor
    /// `1 - exp(-(d ρ)²)` is, so land the air only veils keeps its detail.
    fn detail(&self, h0: f64, h1: f64, len: f64, k: f64) -> f64 {
        let hidden = 1.0 - self.transmittance(h0, h1, len);
        1.0 / (1.0 + k * hidden * hidden)
    }
}

/// The parts of [`TerrainFeature::prepare`], timed for the hitch lists (`render.cpu.terrain:
/// <part>` in `Diagnostics`, under the renderer's own `prepare terrain`).
struct Steps {
    at: std::time::Instant,
    done: Vec<(&'static str, f64)>,
}

impl Steps {
    fn start() -> Self {
        Self {
            at: std::time::Instant::now(),
            done: Vec::new(),
        }
    }

    /// The part since the last mark is done.
    fn mark(&mut self, name: &'static str) {
        let now = std::time::Instant::now();
        let ms = now.duration_since(self.at).as_secs_f64() * 1000.0;
        self.at = now;
        // (A part done once a planet adds up over the planets.)
        match self.done.iter_mut().find(|(n, _)| *n == name) {
            Some(part) => part.1 += ms,
            None => self.done.push((name, ms)),
        }
    }

    fn publish(self, world: &mut World) {
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            for (name, ms) in self.done {
                d.set(&format!("render.cpu.terrain: {name}"), ms);
            }
        }
    }
}

/// A patch built on a worker, for a terrain at a version (patches of an older version, or of
/// a terrain that is gone, are dropped).
struct FinishedPatch {
    planet: Entity,
    version: u64,
    mesh: crate::chunk::ChunkMesh,
}

/// Threads that build terrain patches, so neither the frame nor the render step ever waits
/// for one. The render step hands them the patches it wants each frame, most urgent first
/// ([`PatchWorkers::request`]); a patch nobody asks for any more is dropped before it starts,
/// so after a fast turn or climb the threads build what the view needs now, not what it
/// needed a second ago (Cesium's request culling while moving, Unreal's streaming priority).
/// Finished patches come back through a channel; the render step uploads them a few a frame.
struct PatchWorkers {
    queue: Arc<(Mutex<PatchQueue>, Condvar)>,
    done: Mutex<Receiver<FinishedPatch>>,
    threads: usize,
}

#[derive(Default)]
struct PatchQueue {
    /// Patches asked for and not started: priority (lower first), the planet's version and
    /// shape, and whether it stays queued until built (an edited patch).
    waiting: HashMap<(Entity, NodeKey), (f64, u64, ChunkSource, bool)>,
    /// Patches a thread is building.
    started: HashSet<(Entity, NodeKey)>,
    /// Patches built and not yet taken up by the render step ([`PatchWorkers::taken`]): asked
    /// for again meanwhile (they are still missing from the view), they are not built again.
    built: HashSet<(Entity, NodeKey)>,
}

/// A patch wanted this frame: priority (lower first), the patch, whether it stays queued
/// until built.
type Want = (f64, NodeKey, bool);

impl PatchWorkers {
    /// Half the cores (the frame's own systems and the GPU driver keep the rest).
    fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self::with_threads((cores / 2).clamp(2, 12))
    }

    fn with_threads(threads: usize) -> Self {
        let (done_tx, done) = channel();
        let queue = Arc::new((Mutex::new(PatchQueue::default()), Condvar::new()));
        for i in 0..threads {
            let queue = queue.clone();
            let done_tx = done_tx.clone();
            std::thread::Builder::new()
                .name(format!("terrain-patches-{i}"))
                .spawn(move || {
                    loop {
                        // The most urgent patch (the lock is held only while taking it).
                        let (planet, key, version, src) = {
                            let (lock, ready) = &*queue;
                            let Ok(mut q) = lock.lock() else {
                                return;
                            };
                            while q.waiting.is_empty() {
                                q = match ready.wait(q) {
                                    Ok(q) => q,
                                    Err(_) => return,
                                };
                            }
                            let (&id, _) = q
                                .waiting
                                .iter()
                                .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
                                .expect("not empty");
                            let (_, version, src, _) = q.waiting.remove(&id).expect("queued");
                            q.started.insert(id);
                            (id.0, id.1, version, src)
                        };
                        let mesh = build_chunk(&src, key);
                        if let Ok(mut q) = queue.0.lock() {
                            q.started.remove(&(planet, key));
                            q.built.insert((planet, key));
                        }
                        let done = FinishedPatch {
                            planet,
                            version,
                            mesh,
                        };
                        if done_tx.send(done).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn a terrain patch thread");
        }
        Self {
            queue,
            done: Mutex::new(done),
            threads,
        }
    }

    fn threads(&self) -> usize {
        self.threads
    }

    /// The patches `planet` wants now, in place of what it asked for before: new ones are
    /// queued, queued ones take their new priority, and queued ones it no longer wants are
    /// dropped (returned) unless they stay until built. Patches being built carry on.
    fn request(
        &self,
        planet: Entity,
        version: u64,
        src: &ChunkSource,
        wants: &[Want],
    ) -> Option<Vec<NodeKey>> {
        let (lock, ready) = &*self.queue;
        let mut q = lock.lock().ok()?;
        let wanted: HashSet<NodeKey> = wants.iter().map(|w| w.1).collect();
        let mut dropped = Vec::new();
        q.waiting.retain(|(p, k), job| {
            let keep = *p != planet || job.3 || (wanted.contains(k) && job.1 == version);
            if !keep {
                dropped.push(*k);
            }
            keep
        });
        for &(priority, key, stays) in wants {
            let id = (planet, key);
            if q.started.contains(&id) || q.built.contains(&id) {
                continue;
            }
            match q.waiting.get_mut(&id) {
                Some(job) => {
                    job.0 = priority;
                    job.2 = src.clone();
                    job.3 |= stays;
                }
                None => {
                    q.waiting
                        .insert(id, (priority, version, src.clone(), stays));
                }
            }
        }
        ready.notify_all();
        Some(dropped)
    }

    /// The render step is done with these finished patches (uploaded or thrown away): they may
    /// be built again when asked for.
    fn taken(&self, patches: &[(Entity, NodeKey)]) {
        if patches.is_empty() {
            return;
        }
        if let Ok(mut q) = self.queue.0.lock() {
            for id in patches {
                q.built.remove(id);
            }
        }
    }

    /// Patches finished since the last call.
    fn take_done(&self) -> Vec<FinishedPatch> {
        self.done
            .lock()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default()
    }
}

/// Free the vertex slots of a planet's patches unused for a while (once a frame while the pool
/// is filling up: the scan walks every node the planet knows).
fn free_stale(pool: &mut SlotPool, state: &mut PlanetState) {
    for k in state.lod.stale(30) {
        if let Some(c) = state.chunks.remove(&k) {
            pool.release(c.slot);
        }
        state.lod.unbuild(k);
    }
}

/// Upload a finished patch into a vertex slot. False when there is no room.
fn upload(
    pool: &mut SlotPool,
    state: &mut PlanetState,
    m: crate::chunk::ChunkMesh,
    gpu_shared: &GpuShared,
    max_slots: u32,
) -> bool {
    let slot = match state.chunks.get(&m.key) {
        Some(c) => Some(c.slot),
        None => pool.alloc(&gpu_shared.device, max_slots),
    };
    let Some(slot) = slot else {
        return false;
    };
    pool.write(&gpu_shared.queue, slot, &m.vertices);
    state
        .lod
        .set_built(m.key, m.min_height, m.max_height, m.center, m.radius);
    state.chunks.insert(
        m.key,
        ChunkGpu {
            slot,
            origin: m.origin,
            center: m.center,
            radius: m.radius,
            spacing: m.spacing,
            water: m.water,
        },
    );
    true
}

/// Latitude (radians) up to which rock is hot enough to melt (about 1000 °C at the surface,
/// by the climate's equator-to-pole profile); negative when it is nowhere. The vertices'
/// temperatures stop at 60 °C, so the shader finds the melt from this instead.
fn melt_latitude(c: &crate::shape::Climate) -> f32 {
    const MELT_C: f32 = 1000.0;
    if c.equator_c <= MELT_C {
        return -1.0;
    }
    if c.pole_c >= MELT_C {
        return 2.0;
    }
    // t = pole + (equator − pole) cos^1.6(lat), as `PlanetShape::sample` has it.
    let k = (MELT_C - c.pole_c) / (c.equator_c - c.pole_c);
    k.powf(1.0 / 1.6).acos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::features::{compose_shader, validate_wgsl};

    #[test]
    fn weather_texture_follows_the_grid_as_it_moves() {
        use engine_core::math::DVec2;
        let cell = |x: f64, z: f64| SurfaceCell {
            wetness: ((x * 0.37 + z * 0.11).sin() * 0.5 + 0.5) as f32,
            puddle_m: 0.0,
            snow_m: (x.rem_euclid(7.0) * 0.1) as f32,
            scorch: (z.rem_euclid(5.0) * 0.2) as f32,
        };
        // A 16 x 12 grid of 2 m cells whose values belong to the world spot.
        let mut g = Grid2::new(
            DVec2::new(-16.0, -12.0),
            2.0,
            16,
            12,
            SurfaceCell::default(),
        );
        let fill = |g: &mut Grid2<SurfaceCell>| {
            for y in 0..g.height {
                for x in 0..g.width {
                    let p = g.origin + DVec2::new(x as f64, y as f64) * g.cell as f64;
                    g.data[(y * g.width + x) as usize] = cell(p.x, p.y);
                }
            }
        };
        let mut up = SurfaceUpload::default();
        let mut tex = vec![0u16; 16 * 12 * 4];
        let step = |up: &mut SurfaceUpload, g: &Grid2<SurfaceCell>, tex: &mut Vec<u16>| {
            let rects = up.refresh(g, false);
            for [x, y, w, h] in rects {
                for ty in y..y + h {
                    for tx in x..x + w {
                        let at = ((ty * 16 + tx) * 4) as usize;
                        tex[at..at + 4].copy_from_slice(&up.sent[at..at + 4]);
                    }
                }
            }
        };
        let check = |up: &SurfaceUpload, g: &Grid2<SurfaceCell>, tex: &[u16]| {
            for y in 0..g.height {
                for x in 0..g.width {
                    let c = g.data[(y * g.width + x) as usize];
                    let (tx, ty) = ((x + up.base.x) % 16, (y + up.base.y) % 12);
                    let at = ((ty * 16 + tx) * 4) as usize;
                    assert_eq!(tex[at], f16_bits(c.wetness), "cell {x},{y}");
                    assert_eq!(tex[at + 2], f16_bits(c.snow_m), "cell {x},{y}");
                    assert_eq!(tex[at + 3], f16_bits(c.scorch), "cell {x},{y}");
                }
            }
        };
        fill(&mut g);
        step(&mut up, &g, &mut tex);
        check(&up, &g, &tex);
        // Moves of a few cells each way (only the new strips and a band are sent), a change
        // the bands catch within `SURFACE_UPLOAD_FRAMES` frames, and a jump past the grid.
        for (dx, dy) in [(3, 0), (0, -2), (-5, 4), (1, 1), (0, 0), (-16, 0), (40, 7)] {
            g.origin += DVec2::new(dx as f64, dy as f64) * g.cell as f64;
            fill(&mut g);
            // World-fixed values: right from the first frame after the move.
            step(&mut up, &g, &mut tex);
            check(&up, &g, &tex);
        }
        // A change anywhere arrives within the bands' round.
        g.data[(7 * 16 + 9) as usize].snow_m = 2.5;
        for _ in 0..SURFACE_UPLOAD_FRAMES {
            step(&mut up, &g, &mut tex);
        }
        check(&up, &g, &tex);
        // Without a change, a moved grid re-sends only what came in.
        g.origin += DVec2::new(2.0 * g.cell as f64, 0.0);
        fill(&mut g);
        let sent: u32 = up.refresh(&g, false).iter().map(|r| r[2] * r[3]).sum();
        assert!(sent <= 2 * 12 + 16 * 2, "{sent}");
    }

    #[test]
    fn haze_coarsens_only_land_the_air_hides() {
        let radius = 6.36e6;
        let mut air = engine_render::Atmosphere::earth();
        air.planet_center = DVec3::ZERO;
        air.planet_radius = radius;
        // The terrain's position is its planet's center.
        let at = DVec3::ZERO;
        let clear = engine_render::FogSettings {
            density: 0.0,
            ..Default::default()
        };
        // Someone else's air, or none: no haze.
        assert!(Haze::new(None, Some(&clear), at, radius).is_none());
        let far = DVec3::new(0.0, 3.0 * radius, 0.0);
        assert!(Haze::new(Some(&air), Some(&clear), far, radius).is_none());
        let h = Haze::new(Some(&air), Some(&clear), at, radius).unwrap();
        // Close land keeps its detail; detail falls with distance, never below 1 / (1 + k).
        let d = |len: f64| h.detail(2.0, 0.0, len, 3.0);
        assert!(d(50.0) > 0.99, "{}", d(50.0));
        assert!(d(1000.0) > 0.99, "{}", d(1000.0));
        assert!(d(20_000.0) < d(5_000.0));
        assert!(d(1e7) >= 0.25 - 1e-9);
        // Looking down from high up crosses less air than along the ground.
        assert!(h.detail(30_000.0, 0.0, 30_000.0, 3.0) > d(30_000.0));
        // Fog thickens the haze, but never hides more than its `max_opacity`.
        let fog = engine_render::FogSettings {
            density: 0.002,
            max_opacity: 0.8,
            ..Default::default()
        };
        let foggy = Haze::new(Some(&air), Some(&fog), at, radius).unwrap();
        assert!(foggy.transmittance(2.0, 0.0, 2000.0) < h.transmittance(2.0, 0.0, 2000.0));
        let (deep, air_only) = (
            foggy.transmittance(2.0, 0.0, 1e5),
            h.transmittance(2.0, 0.0, 1e5),
        );
        assert!(deep >= air_only * 0.2 * (1.0 - 1e-6), "{deep} {air_only}");
    }

    #[test]
    fn shaders_validate() {
        validate_wgsl("terrain", &compose_shader(SHADER, true)).unwrap();
        validate_wgsl("terrain shadow", &compose_shader(SHADOW_SHADER, false)).unwrap();
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e9), 0x7c00);
    }

    #[test]
    fn ground_beyond_the_grid_is_like_its_open_rim() {
        use engine_core::math::DVec2;
        let snowy = SurfaceCell {
            wetness: 0.4,
            snow_m: 0.2,
            ..Default::default()
        };
        let mut g = Grid2::new(DVec2::ZERO, 1.0, 64, 48, snowy);
        // A move just uncovered a strip on one side, and a fire melted a corner.
        for y in 0..48 {
            for x in 0..64 {
                if x < 8 || (x > 56 && y > 40) {
                    g.data[(y * 64 + x) as usize] = SurfaceCell::default();
                }
            }
        }
        let [wet, snow] = beyond_grid(&g);
        assert_eq!((wet, snow), (0.4, 0.2));
        // An empty grid says nothing.
        let empty = Grid2::new(DVec2::ZERO, 1.0, 1, 1, SurfaceCell::default());
        assert_eq!(beyond_grid(&empty), [0.0, 0.0]);
    }

    #[test]
    fn shader_structs_match_the_buffers() {
        use wgpu::naga;
        for (label, code, maps) in [
            ("terrain", compose_shader(SHADER, true), true),
            (
                "terrain shadow",
                compose_shader(SHADOW_SHADER, false),
                false,
            ),
        ] {
            let module = naga::front::wgsl::parse_str(&code).unwrap();
            let size = |name: &str| {
                module
                    .types
                    .iter()
                    .find(|(_, t)| t.name.as_deref() == Some(name))
                    .map(|(_, t)| t.inner.size(module.to_ctx()) as usize)
            };
            let planet = size("PlanetGpu");
            assert_eq!(planet, Some(std::mem::size_of::<PlanetGpu>()), "{label}");
            if maps {
                assert_eq!(
                    size("Maps"),
                    Some(std::mem::size_of::<MapsGpu>()),
                    "{label}"
                );
                assert_eq!(
                    size("Print"),
                    Some(std::mem::size_of::<PrintGpu>()),
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn workers_build_the_most_urgent_first_and_drop_what_is_no_longer_wanted() {
        use crate::cube::{SurfaceMap, TerrainDomain};
        use crate::edits::EditLayer;
        use crate::shape::PlanetShape;
        let shape = PlanetShape::moon(1);
        let src = ChunkSource {
            map: SurfaceMap::new(TerrainDomain::Sphere, shape.radius_m),
            shape: Arc::new(shape),
            edits: Arc::new(EditLayer::default()),
            ocean_surface: false,
        };
        let planet = Entity::from_raw_u32(7).expect("an entity");
        let key = |x: u32| NodeKey {
            face: 2,
            level: 8,
            x,
            y: 3,
        };
        let wait_for = |w: &PatchWorkers, n: usize| {
            let mut got = Vec::new();
            let start = std::time::Instant::now();
            while got.len() < n && start.elapsed().as_secs() < 60 {
                got.extend(w.take_done().into_iter().map(|f| f.mesh.key.x));
                std::thread::yield_now();
            }
            got
        };
        let ask = |w: &PatchWorkers, p: Entity, v: u64, src: &ChunkSource, wants: &[Want]| {
            w.request(p, v, src, wants).expect("the queue")
        };
        // One thread: the order is the priority's, whatever the order asked in.
        let w = PatchWorkers::with_threads(1);
        ask(
            &w,
            planet,
            1,
            &src,
            &[
                (3.0, key(1), false),
                (0.0, key(0), false),
                (1.0, key(2), false),
                (2.0, key(3), false),
            ],
        );
        assert_eq!(wait_for(&w, 4), [0, 2, 3, 1]);

        // Asked for 40, then only for two of them: the rest are dropped before they start
        // (the thread may have started the first already), except a patch kept until built
        // (an edit).
        let w = PatchWorkers::with_threads(1);
        let many: Vec<Want> = (0..40).map(|i| (i as f64, key(i), i == 30)).collect();
        ask(&w, planet, 1, &src, &many);
        let dropped = ask(
            &w,
            planet,
            1,
            &src,
            &[(0.5, key(5), false), (0.6, key(6), false)],
        );
        assert!(dropped.len() >= 35, "{dropped:?}");
        assert!(!dropped.contains(&key(5)) && !dropped.contains(&key(30)));
        let mut expected: Vec<u32> = (0..40).filter(|&i| !dropped.contains(&key(i))).collect();
        let mut built = wait_for(&w, expected.len());
        built.sort();
        expected.sort();
        assert_eq!(built, expected);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(w.take_done().is_empty());

        // A patch built but not yet uploaded is still missing from the view, so it is asked
        // for again every frame: it is not built again until the render step has taken it
        // (rebuilding it every frame grew memory without bound at a slow stop).
        let w = PatchWorkers::with_threads(2);
        ask(&w, planet, 1, &src, &[(0.0, key(7), false)]);
        let first = wait_for(&w, 1);
        assert_eq!(first, [7]);
        for _ in 0..20 {
            ask(&w, planet, 1, &src, &[(0.0, key(7), false)]);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(w.take_done().is_empty(), "built again before it was taken");
        w.taken(&[(planet, key(7))]);
        ask(&w, planet, 1, &src, &[(0.0, key(7), false)]);
        assert_eq!(wait_for(&w, 1), [7]);
    }

    #[test]
    fn gpu_structs_match_shader_sizes() {
        assert_eq!(
            std::mem::size_of::<PlanetGpu>(),
            16 * (3 + 1 + 10 + 7 + MAX_RAYED + MAX_YOUNG)
        );
        assert_eq!(std::mem::size_of::<MapsGpu>(), 16 * 14);
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 56);
    }
}
