//! Drawing terrains: quadtree selection, parallel patch building, a pool of GPU vertex
//! slots, shadows, and the ground maps (footprints, wetness, puddles, snow) the shader reads.

use std::collections::HashMap;
use std::path::PathBuf;

use bytemuck::{Pod, Zeroable};
use engine_core::diagnostics::Diagnostics;
use engine_core::math::{DQuat, DVec3, Mat3, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::ComputeTaskPool;
use engine_render::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase,
};
use engine_render::frame::ViewData;
use engine_render::settings::RenderSettings;
use engine_world::{Grid2, SurfaceCell, SurfaceLayers};

use crate::chunk::{ChunkSource, TerrainVertex, VERTS_PER_CHUNK, build_chunk, chunk_indices};
use crate::cube::NodeKey;
use crate::deform::DeformationMap;
use crate::lod::{Lod, SelectInput};
use crate::materials::{self, BakedMaterials, MATERIAL_COUNT};
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
}

impl ChunkInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![5 => Float32x3, 6 => Uint32, 7 => Float32x4];
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
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    /// Depth, normals and motion for ambient occlusion, contact shadows and anti-aliasing.
    prepass_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    index: wgpu::Buffer,
    index_count: u32,
    instances: GrowBuffer,
    planets: GrowBuffer,
    maps: wgpu::Buffer,
    sampler: wgpu::Sampler,
    deform_tex: Option<(wgpu::Texture, wgpu::TextureView, u32)>,
    surface_tex: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
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
    group: Option<wgpu::BindGroup>,
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
}

struct PlanetState {
    lod: Lod,
    version: u64,
    edits_seen: usize,
    chunks: HashMap<NodeKey, ChunkGpu>,
    /// Built patches to rebuild in place (edited), nearest first when drained.
    rebuild: Vec<NodeKey>,
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
            last_position: None,
            rays,
            rayed,
            young,
            young_count,
            boost: 1.0,
        }
    }
}

#[derive(Clone, Copy)]
struct Draw {
    slot: u32,
    instance: u32,
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
        if let Some(s) = self.free.pop() {
            return Some(s);
        }
        if self.capacity + SLOTS_PER_PAGE > max.max(SLOTS_PER_PAGE) {
            return None;
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
            .extend((page * SLOTS_PER_PAGE + 1..(page + 1) * SLOTS_PER_PAGE).rev());
        Some(page * SLOTS_PER_PAGE)
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
    shadow_draws: [Vec<Draw>; 4],
    /// Patches drawn into the far shadow map (big shadows beyond the cascades).
    far_shadow_draws: Vec<Draw>,
    deform_generation: u64,
    surface_frame: u64,
    /// Wetness and snow depth beyond the surface layers' grid (see [`beyond_grid`]).
    surface_far: [f32; 2],
    frame: u64,
    stats: FeatureStats,
    built_last_frame: usize,
}

/// How wet and snowy the ground is beyond the weather's grid around the player: like the open
/// ground along the grid's rim. The upper quartile of its last few cells, so shelters, fires
/// and the strip a move has just uncovered (empty until the weather fills it) don't count.
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
        let (Some(deform), Some(surface)) = (&gpu.deform_tex, &gpu.surface_tex) else {
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

    /// Upload changed parts of the footprint grid and the weather surface layers.
    fn upload_maps(
        &mut self,
        world: &mut World,
        gpu_shared: &GpuShared,
        view: &ViewData,
    ) -> MapsGpu {
        let mut maps = MapsGpu::default();
        let mut recreate = false;
        let gpu = self.gpu.as_mut().expect("gpu");
        // Footprints.
        if let Some(mut def) = world.get_resource_mut::<DeformationMap>() {
            let size = def.size;
            let fresh = gpu.deform_tex.as_ref().is_none_or(|t| t.2 != size);
            if fresh {
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
                recreate = true;
                def.dirty = Some((0, 0, size, size));
            }
            if fresh || def.generation != self.deform_generation {
                self.deform_generation = def.generation;
            }
            if let Some((x0, y0, x1, y1)) = def.dirty.take() {
                let (w, h) = (x1 - x0, y1 - y0);
                let mut data = Vec::with_capacity((w * h * 2) as usize);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * size + x) as usize;
                        data.push(f16_bits(def.depth[i]));
                        data.push(f16_bits(def.flatten[i]));
                    }
                }
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
            maps.flags[0] = 1.0;
        }
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
            if fresh || self.frame >= self.surface_frame + 6 {
                self.surface_frame = self.frame;
                self.surface_far = beyond_grid(g);
                let mut data = Vec::with_capacity(g.data.len() * 4);
                for c in &g.data {
                    data.push(f16_bits(c.wetness));
                    data.push(f16_bits(c.puddle_m));
                    data.push(f16_bits(c.snow_m));
                    data.push(f16_bits(c.scorch));
                }
                let tex = &gpu.surface_tex.as_ref().expect("texture").0;
                gpu_shared.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&data),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(g.width * 8),
                        rows_per_image: Some(g.height),
                    },
                    wgpu::Extent3d {
                        width: g.width,
                        height: g.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            maps.surface = [
                (g.origin.x - view.origin.x) as f32,
                (g.origin.y - view.origin.z) as f32,
                g.cell,
                g.width as f32,
            ];
            maps.flags[1] = 1.0;
            maps.far = [self.surface_far[0], 0.0, self.surface_far[1], 0.0];
        }
        // Placeholders so the bind group is always complete.
        for (label, slot) in [("deform", 0usize), ("surface", 1usize)] {
            let missing = if slot == 0 {
                gpu.deform_tex.is_none()
            } else {
                gpu.surface_tex.is_none()
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
                    format: if slot == 0 {
                        wgpu::TextureFormat::Rg16Float
                    } else {
                        wgpu::TextureFormat::Rgba16Float
                    },
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let v = tex.create_view(&Default::default());
                if slot == 0 {
                    gpu.deform_tex = Some((tex, v, 0));
                } else {
                    gpu.surface_tex = Some((tex, v, 0, 0));
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
            ],
        });
        let buffers = [Some(TerrainVertex::layout()), Some(ChunkInstance::layout())];
        let shader = gpu.shader("terrain", SHADER, true);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
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
            depth_stencil: Some(gpu.depth_state(true)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.hdr,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
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
            prepass_pipeline,
            shadow_pipeline,
            layout,
            index,
            index_count: indices.len() as u32,
            instances: GrowBuffer::new(
                device,
                "terrain instances",
                wgpu::BufferUsages::VERTEX,
                4096 * std::mem::size_of::<ChunkInstance>() as u64,
            ),
            planets: GrowBuffer::new(
                device,
                "terrain planets",
                wgpu::BufferUsages::STORAGE,
                8 * std::mem::size_of::<PlanetGpu>() as u64,
            ),
            maps,
            sampler,
            deform_tex: None,
            surface_tex: None,
            mat_albedo,
            mat_normal,
            mat_info,
            mat_sampler,
            mat_size: 0,
            mat_key: None,
            mat_scanned: 0,
            group: None,
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu_shared: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.frame += 1;
        self.draws.clear();
        for s in &mut self.shadow_draws {
            s.clear();
        }
        self.far_shadow_draws.clear();
        if self.gpu.is_none() {
            return;
        }
        let settings = world
            .get_resource::<TerrainSettings>()
            .cloned()
            .unwrap_or_default();
        let maps = self.upload_maps(world, gpu_shared, view);
        let Some(terrains) = world.get_resource::<Terrains>() else {
            return;
        };
        if !terrains.entries.is_empty() {
            let scans = settings.scan_folder(world);
            self.upload_materials(gpu_shared, settings.material_resolution, scans);
        }
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
        let mut planet_gpu = Vec::with_capacity(entities.len());
        let mut instances: Vec<ChunkInstance> = Vec::new();
        let mut built_total = 0usize;
        let mut max_boost = 1.0f64;
        let max_slots = settings.max_patches as u32;

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
            state.last_position = Some(pos);
            let cam_local = rot.inverse() * (view.origin - pos);
            let frustum = view.frustum;
            let origin = view.origin;
            let visible = move |c: DVec3, r: f64| {
                let rel = (pos + rot * c - origin).as_vec3();
                frustum.intersects_sphere(rel, r as f32)
            };
            let split = settings.split_factor * t.config.detail.max(0.05) as f64 * state.boost;

            // Select; after a jump (many patches missing) build until the view is complete.
            let mut sel;
            let mut rounds = 0;
            loop {
                sel = state.lod.select(&SelectInput {
                    camera: cam_local,
                    split_factor: split,
                    visible: &visible,
                    min_size: 0.0,
                });
                let catch_up =
                    sel.missing.len() > settings.catch_up_threshold || state.chunks.is_empty();
                let mut batch: Vec<NodeKey> = if catch_up {
                    sel.missing.iter().map(|m| m.1).collect()
                } else {
                    sel.missing
                        .iter()
                        .take(settings.builds_per_frame)
                        .map(|m| m.1)
                        .collect()
                };
                // Edited patches (a few per frame).
                if !catch_up && batch.len() < settings.builds_per_frame {
                    let room = settings.builds_per_frame - batch.len();
                    let n = room.min(state.rebuild.len());
                    batch.extend(state.rebuild.drain(..n));
                }
                if batch.is_empty() {
                    break;
                }
                built_total +=
                    build_into(&mut self.pool, state, &src, &batch, gpu_shared, max_slots);
                rounds += 1;
                if !catch_up || rounds > 40 {
                    // One more pass picks up what was just built.
                    sel = state.lod.select(&SelectInput {
                        camera: cam_local,
                        split_factor: split,
                        visible: &visible,
                        min_size: 0.0,
                    });
                    break;
                }
            }

            // Few patches for the planet's share of the view (seen from orbit): split finer,
            // easing toward the detail that fills the budget. The patch count grows with the
            // square of the split factor.
            if settings.patch_budget > 0 {
                let dist = cam_local.length().max(1.0);
                let angle = (src.map.radius / dist).min(1.0).asin();
                let share = if view.fov_y > 0.0 {
                    (angle / (0.5 * view.fov_y as f64)).min(1.0).powi(2)
                } else {
                    1.0
                };
                let budget = settings.patch_budget as f64 * share;
                let needed = (sel.draw.len() + sel.missing.len()).max(1) as f64;
                let base = needed / (state.boost * state.boost);
                let want = (budget / base).sqrt().clamp(1.0, MAX_BOOST);
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
            let far_sel = (t.config.cast_shadows && view.sun_shadows && far.enabled).then(|| {
                let far_visible = move |c: DVec3, r: f64| {
                    let rel = (pos + rot * c - origin).as_vec3();
                    let r = r as f32;
                    far.casts(rel, r)
                        && (rel.dot(sun) + r >= 0.0 || rel.length() <= far.half_extent + r)
                };
                let input = SelectInput {
                    camera: cam_local,
                    split_factor: split * FAR_SHADOW_DETAIL,
                    visible: &far_visible,
                    min_size: far.texel() as f64 * 0.5 * crate::chunk::QUADS as f64,
                };
                let mut far_sel = state.lod.walk(&input);
                let batch: Vec<NodeKey> = far_sel
                    .missing
                    .iter()
                    .take((settings.builds_per_frame / 2).max(1))
                    .map(|m| m.1)
                    .collect();
                if !batch.is_empty() {
                    built_total +=
                        build_into(&mut self.pool, state, &src, &batch, gpu_shared, max_slots);
                    far_sel = state.lod.walk(&input);
                }
                far_sel
            });

            // Recycle patches nobody has looked at for a while.
            if self.frame.is_multiple_of(60) {
                for k in state.lod.stale(STALE_FRAMES) {
                    if let Some(c) = state.chunks.remove(&k) {
                        self.pool.release(c.slot);
                    }
                    state.lod.unbuild(k);
                }
            }

            // Draw lists.
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
                instances.push(ChunkInstance {
                    origin: rel.to_array(),
                    planet: pi as u32,
                    lod: [c.spacing as f32, key.level as f32, morph.0, morph.1],
                });
                self.draws.push(Draw {
                    slot: c.slot,
                    instance,
                });
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
                });
                self.far_shadow_draws.push(Draw {
                    slot: c.slot,
                    instance,
                });
            }

            let p = &t.config.palette;
            let col = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
            let center_rel = (pos - view.origin).as_vec3();
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
            });
        }
        self.built_last_frame = built_total;

        // Group draws by vertex page.
        self.draws.sort_by_key(|d| d.slot / SLOTS_PER_PAGE);
        for s in &mut self.shadow_draws {
            s.sort_by_key(|d| d.slot / SLOTS_PER_PAGE);
        }
        self.far_shadow_draws
            .sort_by_key(|d| d.slot / SLOTS_PER_PAGE);

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
            d.set("terrain.detail_boost", max_boost);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque || self.draws.is_empty() {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(group) = &gpu.group else {
            return;
        };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(1, group, &[]);
        self.draw_list(pass, gpu, &self.draws);
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
        self.draw_list(pass, gpu, &self.draws);
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
        self.draw_list(pass, gpu, &self.shadow_draws[cascade]);
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
        self.draw_list(pass, gpu, &self.far_shadow_draws);
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
            "gpu_pages": self.pool.pages.len(),
        })
    }
}

impl TerrainFeature {
    fn draw_list(&self, pass: &mut wgpu::RenderPass<'_>, gpu: &Gpu, draws: &[Draw]) {
        pass.set_index_buffer(gpu.index.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_vertex_buffer(1, gpu.instances.buffer.slice(..));
        let mut page = u32::MAX;
        for d in draws {
            let p = d.slot / SLOTS_PER_PAGE;
            if p != page {
                page = p;
                pass.set_vertex_buffer(0, self.pool.pages[p as usize].slice(..));
            }
            let base = ((d.slot % SLOTS_PER_PAGE) as usize * VERTS_PER_CHUNK) as i32;
            pass.draw_indexed(0..gpu.index_count, base, d.instance..d.instance + 1);
        }
    }
}

/// Build `batch` into vertex slots, first freeing patches unused for a while if the pool is
/// filling up. Returns how many were built.
fn build_into(
    pool: &mut SlotPool,
    state: &mut PlanetState,
    src: &ChunkSource,
    batch: &[NodeKey],
    gpu_shared: &GpuShared,
    max_slots: u32,
) -> usize {
    if pool.used() + batch.len() as u32 + SLOTS_PER_PAGE > max_slots {
        for k in state.lod.stale(30) {
            if let Some(c) = state.chunks.remove(&k) {
                pool.release(c.slot);
            }
            state.lod.unbuild(k);
        }
    }
    let meshes = build_parallel(src, batch);
    let built = meshes.len();
    for m in meshes {
        let slot = match state.chunks.get(&m.key) {
            Some(c) => Some(c.slot),
            None => pool.alloc(&gpu_shared.device, max_slots),
        };
        let Some(slot) = slot else {
            break;
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
            },
        );
    }
    built
}

/// Build patches on the compute pool.
fn build_parallel(src: &ChunkSource, keys: &[NodeKey]) -> Vec<crate::chunk::ChunkMesh> {
    if keys.len() <= 1 {
        return keys.iter().map(|k| build_chunk(src, *k)).collect();
    }
    engine_core::tasks::init_task_pools();
    ComputeTaskPool::get().scope(|s| {
        for k in keys {
            let src = src.clone();
            let k = *k;
            s.spawn(async move { build_chunk(&src, k) });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::features::{compose_shader, validate_wgsl};

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
            }
        }
    }

    #[test]
    fn gpu_structs_match_shader_sizes() {
        assert_eq!(
            std::mem::size_of::<PlanetGpu>(),
            16 * (3 + 1 + 10 + 4 + MAX_RAYED + MAX_YOUNG)
        );
        assert_eq!(std::mem::size_of::<MapsGpu>(), 64);
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 48);
    }
}
