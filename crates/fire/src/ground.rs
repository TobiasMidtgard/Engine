//! Painted ground: a rectangle of terrain whose surface is blended per pixel from tiling
//! procedural layers ([`GroundKind`]: meadow litter, dirt, gravel, mud, ash) by painted
//! shapes with soft, ragged, height-aware edges, instead of hard-edged colored boxes. It lies
//! over the ground (the `Ground` contract), hills and all.
//!
//! The layer textures (albedo with height, normal, roughness, occlusion; 1K each, mipmapped)
//! are generated once, the first time a [`PaintedGround`] is drawn, and shared by all of them.
//!
//! Burnt ground chars: each ground keeps a map of the fire field's scorch under it, and the
//! litter and dirt turn to black char flecked with ash where the fire has been, fading as the
//! field forgets the fire.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, DVec3, Vec2};
use engine_core::prelude::*;
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, Image, RenderFeature, RenderPhase, RenderSettings,
    ViewData,
};

use engine_world::Ground;

use crate::field::{FireField, TILE};
use crate::fuel::PatchShape;
pub use crate::textures::GroundKind;

pub const GROUND_SHADER: &str = include_str!("shaders/ground.wgsl");

/// Most paints one ground takes.
pub const MAX_PAINTS: usize = 12;
/// Texels along each side of a layer texture.
const LAYER_SIZE: u32 = 1024;
/// Grid cells along each side of the ground mesh (must match `GRID` in `ground.wgsl`).
const GRID: u32 = 48;
/// Largest scorch map side, texels.
const MAX_SCORCH: u32 = 1024;

/// One painted area of a [`PaintedGround`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundPaint {
    pub kind: GroundKind,
    /// Center, meters from the ground's center (X, Z).
    pub center: [f32; 2],
    pub shape: PatchShape,
    /// Width of the soft blend at the edge, meters.
    pub blend_m: f32,
    /// How far noise pushes the edge in and out, meters.
    pub ragged_m: f32,
    /// How fully it covers what's under it, 0..1.
    pub strength: f32,
}

impl Default for GroundPaint {
    fn default() -> Self {
        Self {
            kind: GroundKind::Dirt,
            center: [0.0, 0.0],
            shape: PatchShape::Disc { radius: 1.0 },
            blend_m: 0.6,
            ragged_m: 0.3,
            strength: 1.0,
        }
    }
}

impl GroundPaint {
    pub fn new(kind: GroundKind, center: [f32; 2], shape: PatchShape) -> Self {
        Self {
            kind,
            center,
            shape,
            ..Default::default()
        }
    }

    pub fn with_edge(mut self, blend_m: f32, ragged_m: f32) -> Self {
        self.blend_m = blend_m;
        self.ragged_m = ragged_m;
        self
    }

    pub fn with_strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }
}

/// A rectangle of painted ground around its entity (no rotation), draped over the ground
/// (`Ground`; flat at the entity's height where there is none) and drawn `lift_m` above it so
/// it covers the terrain under it. Its outer edge frays out over `edge_m`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaintedGround {
    /// Half extents, meters.
    pub half_extents: [f32; 2],
    /// Material everywhere no paint covers.
    pub base: GroundKind,
    /// Painted areas, later ones over earlier ones (at most [`MAX_PAINTS`]).
    pub paints: Vec<GroundPaint>,
    /// Width of the frayed outer edge, meters.
    pub edge_m: f32,
    /// Height above the entity, meters.
    pub lift_m: f32,
}

impl Default for PaintedGround {
    fn default() -> Self {
        Self {
            half_extents: [10.0, 10.0],
            base: GroundKind::Dirt,
            paints: Vec::new(),
            edge_m: 1.5,
            lift_m: 0.012,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuPaint {
    shape: [f32; 4],
    look: [f32; 4],
}

/// GPU layout of `GroundParams` in `ground.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuGroundParams {
    origin: [f32; 4],
    extent: [f32; 4],
    tiles: [[f32; 4]; 2],
    paints: [GpuPaint; MAX_PAINTS],
}

impl GpuGroundParams {
    fn new(ground: &PaintedGround, rel: [f32; 3]) -> Self {
        let mut paints = [GpuPaint::zeroed(); MAX_PAINTS];
        for (g, p) in paints.iter_mut().zip(&ground.paints) {
            g.shape = match p.shape {
                PatchShape::Disc { radius } => [p.center[0], p.center[1], radius, -1.0],
                PatchShape::Rect { half_extents } => {
                    [p.center[0], p.center[1], half_extents[0], half_extents[1]]
                }
            };
            g.look = [
                p.kind.layer() as f32,
                p.blend_m,
                p.ragged_m,
                p.strength.clamp(0.0, 1.0),
            ];
        }
        let mut tiles = [[1.0; 4]; 2];
        for k in GroundKind::ALL {
            let i = k.layer() as usize;
            tiles[i / 4][i % 4] = k.tile_m();
        }
        Self {
            origin: [
                rel[0],
                rel[1],
                rel[2],
                ground.paints.len().min(MAX_PAINTS) as f32,
            ],
            extent: [
                ground.half_extents[0],
                ground.half_extents[1],
                ground.edge_m,
                ground.base.layer() as f32,
            ],
            tiles,
            paints,
        }
    }
}

struct GroundGpu {
    entity: Entity,
    params: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
    visible: bool,
    scorch: Option<(wgpu::Texture, wgpu::TextureView)>,
    scorch_size: [u32; 2],
    scorch_key: (u64, [i64; 2]),
    /// Ground height above the entity at each mesh vertex (R32Float, `GRID + 1` square).
    heights: Option<(wgpu::Texture, wgpu::TextureView)>,
    heights_key: [i64; 5],
}

/// Ground height above `at.y` at each of the ground mesh's `(GRID + 1)²` vertices.
fn ground_heights(ground: Option<&Ground>, at: DVec3, half: [f32; 2]) -> Vec<f32> {
    let n = GRID + 1;
    let hint = at.y + 2.0 + half[0].max(half[1]) as f64;
    // The ground is dear to ask (a planet's), so a row per task.
    let mut heights = vec![0.0f32; (n * n) as usize];
    engine_core::tasks::par_chunks_mut(&mut heights, n as usize, |row, out| {
        for (col, h) in out.iter_mut().enumerate() {
            let f = DVec2::new(col as f64, row as f64) / GRID as f64 * 2.0 - 1.0;
            let x = at.x + f.x * half[0] as f64;
            let z = at.z + f.y * half[1] as f64;
            *h = ground
                .and_then(|g| g.height(DVec3::new(x, hint, z)))
                .filter(|h| h.is_finite())
                .map_or(0.0, |h| (h - at.y) as f32);
        }
    });
    heights
}

/// The scorch map of a ground: one texel per fire cell over its rectangle (`min`, `size`
/// texels of `cell` m), R = scorch. Only tiles that overlap and carry scorch are read.
fn scorch_texels(field: &FireField, min: DVec2, size: [u32; 2], cell: f64) -> Vec<u8> {
    let mut data = vec![0u8; (size[0] * size[1]) as usize];
    let tile_size = field.tile_size();
    let max = min + DVec2::new(size[0] as f64, size[1] as f64) * cell;
    for t in &field.tiles {
        let o = field.tile_origin(t.key);
        if o.x > max.x || o.y > max.y || o.x + tile_size < min.x || o.y + tile_size < min.y {
            continue;
        }
        if !t.cells.iter().any(|c| c.scorch > 0.0) {
            continue;
        }
        for (i, c) in t.cells.iter().enumerate() {
            if c.scorch <= 0.0 {
                continue;
            }
            let local = DVec2::new((i as i32 % TILE) as f64, (i as i32 / TILE) as f64);
            let p = o + (local + 0.5) * field.cell_m as f64;
            let q = ((p - min) / cell).floor();
            if q.x < 0.0 || q.y < 0.0 || q.x >= size[0] as f64 || q.y >= size[1] as f64 {
                continue;
            }
            let k = (q.y as u32 * size[0] + q.x as u32) as usize;
            data[k] = data[k].max((c.scorch.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
    }
    data
}

/// Draws every [`PaintedGround`] (added by `FirePlugin`).
#[derive(Default)]
pub struct GroundFeature {
    pipeline: Option<wgpu::RenderPipeline>,
    layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    /// Albedo and detail layer arrays, made on first use.
    layers: Option<(wgpu::TextureView, wgpu::TextureView)>,
    grounds: Vec<GroundGpu>,
    stats: FeatureStats,
}

/// One ground layer image and its smaller mip levels.
struct Layer {
    image: Image,
    mips: Vec<Vec<u8>>,
}

/// Every [`GroundKind`]'s albedo (sRGB) and detail layers.
struct Layers {
    albedo: Vec<Layer>,
    detail: Vec<Layer>,
}

/// Make the ground layers (a second or so on the CPU: kept off the render frame).
fn make_layers() -> Layers {
    let (albedo, detail) = GroundKind::ALL
        .iter()
        .map(|k| {
            let (a, d) = k.generate(LAYER_SIZE, 0x6a0d + k.layer() * 977);
            let layer = |image: Image, srgb: bool| Layer {
                mips: image.mip_chain(srgb),
                image,
            };
            (layer(a, true), layer(d, false))
        })
        .unzip();
    Layers { albedo, detail }
}

/// Ground layers made ahead of time ([`prefetch_layers`]).
#[derive(Default)]
struct Prefetch {
    layers: Mutex<Option<Layers>>,
    done: AtomicBool,
}

static PREFETCH: OnceLock<Prefetch> = OnceLock::new();

/// Start making the ground layers on a thread of their own, so the painted ground's first
/// frame finds them made. Later calls do nothing.
pub(crate) fn prefetch_layers() {
    if PREFETCH.set(Prefetch::default()).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("fire ground layers".into())
        .spawn(|| {
            let p = PREFETCH.get().expect("set before the thread starts");
            let layers = make_layers();
            *p.layers.lock().unwrap_or_else(|e| e.into_inner()) = Some(layers);
            p.done.store(true, Ordering::Release);
        })
        .expect("spawn the ground layer thread");
}

/// Whether the layers [`prefetch_layers`] started are made (false if it wasn't called).
pub(crate) fn layers_ready() -> bool {
    PREFETCH
        .get()
        .is_some_and(|p| p.done.load(Ordering::Acquire))
}

/// Whether a prefetch of the layers is still under way.
pub(crate) fn layers_pending() -> bool {
    PREFETCH
        .get()
        .is_some_and(|p| !p.done.load(Ordering::Acquire))
}

/// The layers made ahead of time, if they are done (taken out of the store).
fn take_prefetched() -> Option<Layers> {
    let p = PREFETCH.get()?;
    if !p.done.load(Ordering::Acquire) {
        return None;
    }
    p.layers.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Upload `layers` (all the same size, with mips) as one 2D array texture.
fn layer_array(gpu: &GpuShared, label: &str, layers: &[Layer], srgb: bool) -> wgpu::TextureView {
    let (w, h) = (layers[0].image.width, layers[0].image.height);
    let levels = layers[0].image.full_mip_count();
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: layers.len() as u32,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, Layer { image, mips }) in layers.iter().enumerate() {
        for level in 0..levels {
            let data: &[u8] = if level == 0 {
                &image.data
            } else {
                &mips[level as usize - 1]
            };
            let (lw, lh) = image.level_size(level);
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(lw * 4),
                    rows_per_image: Some(lh),
                },
                wgpu::Extent3d {
                    width: lw,
                    height: lh,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

impl GroundFeature {
    fn ensure_layers(&mut self, gpu: &GpuShared) {
        if self.layers.is_some() {
            return;
        }
        let started = std::time::Instant::now();
        let Layers { albedo, detail } = take_prefetched().unwrap_or_else(make_layers);
        self.layers = Some((
            layer_array(gpu, "ground albedo layers", &albedo, true),
            layer_array(gpu, "ground detail layers", &detail, false),
        ));
        tracing::debug!(
            "fire: generated ground layers in {:.2} s",
            started.elapsed().as_secs_f32()
        );
    }
}

impl RenderFeature for GroundFeature {
    fn name(&self) -> &str {
        "painted ground"
    }

    /// Before other opaque features: it is the floor everything stands on.
    fn order(&self) -> i32 {
        -10
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let frag = wgpu::ShaderStages::FRAGMENT;
        let array = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("painted ground"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                array(1),
                array(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: frag,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = gpu.shader("painted ground", GROUND_SHADER, true);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("painted ground"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("painted ground"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_ground"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_ground"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
        self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("painted ground"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        }));
        self.layout = Some(layout);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.stats = FeatureStats::default();
        let mut q = world.query::<(Entity, &PaintedGround, &GlobalTransform)>();
        let grounds: Vec<(Entity, PaintedGround, DVec3)> = q
            .iter(world)
            .map(|(e, g, t)| (e, g.clone(), t.translation()))
            .collect();
        if grounds.is_empty() {
            self.grounds.clear();
            return;
        }
        if self.layout.is_none() {
            return;
        }
        self.ensure_layers(gpu);
        let (Some(layout), Some(sampler), Some((albedo, detail))) =
            (&self.layout, &self.sampler, &self.layers)
        else {
            return;
        };
        self.grounds
            .retain(|g| grounds.iter().any(|(e, ..)| *e == g.entity));
        let terrain = world.get_resource::<Ground>().cloned();
        let field = world.get_resource::<FireField>();
        let field_version = field.map_or(0, |f| {
            f.tiles
                .iter()
                .map(|t| t.version)
                .sum::<u64>()
                .wrapping_add(f.tiles.len() as u64 * 1_000_003)
        });
        for (entity, ground, at) in grounds {
            let rel = (at + DVec3::Y * ground.lift_m as f64 - view.origin).as_vec3();
            let half = Vec2::from(ground.half_extents);
            let visible = view.frustum.intersects_sphere(rel, half.length() + 1.0);
            let params = GpuGroundParams::new(&ground, rel.to_array());
            let i = match self.grounds.iter().position(|g| g.entity == entity) {
                Some(i) => i,
                None => {
                    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("painted ground params"),
                        size: std::mem::size_of::<GpuGroundParams>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.grounds.push(GroundGpu {
                        entity,
                        params: buffer,
                        group: None,
                        visible,
                        scorch: None,
                        scorch_size: [0, 0],
                        scorch_key: (u64::MAX, [0, 0]),
                        heights: None,
                        heights_key: [i64::MAX; 5],
                    });
                    self.grounds.len() - 1
                }
            };
            let g = &mut self.grounds[i];
            g.visible = visible;
            // Scorch map over the ground's rectangle, at the fire field's cell size.
            let cell = field.map_or(0.5, |f| f.cell_m as f64).max(0.05);
            let min = DVec2::new(at.x, at.z) - Vec2::from(ground.half_extents).as_dvec2();
            let size = [
                ((ground.half_extents[0] as f64 * 2.0 / cell).ceil() as u32).clamp(1, MAX_SCORCH),
                ((ground.half_extents[1] as f64 * 2.0 / cell).ceil() as u32).clamp(1, MAX_SCORCH),
            ];
            let texel = DVec2::new(
                ground.half_extents[0] as f64 * 2.0 / size[0] as f64,
                ground.half_extents[1] as f64 * 2.0 / size[1] as f64,
            );
            let key = (
                field_version,
                [(min.x * 64.0) as i64, (min.y * 64.0) as i64],
            );
            if g.scorch.is_none() || g.scorch_size != size {
                let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("painted ground scorch"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                g.scorch = Some((texture, view));
                g.scorch_size = size;
                g.scorch_key = (u64::MAX, [0, 0]);
                g.group = None;
            }
            if g.scorch_key != key
                && let Some((texture, _)) = &g.scorch
            {
                let data = match field {
                    Some(f) => scorch_texels(f, min, size, texel.x.min(texel.y)),
                    None => vec![0u8; (size[0] * size[1]) as usize],
                };
                gpu.queue.write_texture(
                    texture.as_image_copy(),
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size[0]),
                        rows_per_image: Some(size[1]),
                    },
                    wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                );
                g.scorch_key = key;
            }
            // The ground under the rectangle, sampled again whenever it moves or resizes.
            let mm = |v: f64| (v * 1000.0).round() as i64;
            let heights_key = [
                mm(at.x),
                mm(at.y),
                mm(at.z),
                mm(ground.half_extents[0] as f64),
                mm(ground.half_extents[1] as f64),
            ];
            if g.heights.is_none() || g.heights_key != heights_key {
                let n = GRID + 1;
                let extent = wgpu::Extent3d {
                    width: n,
                    height: n,
                    depth_or_array_layers: 1,
                };
                let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("painted ground heights"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let heights = ground_heights(terrain.as_ref(), at, ground.half_extents);
                gpu.queue.write_texture(
                    texture.as_image_copy(),
                    bytemuck::cast_slice(&heights),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(n * 4),
                        rows_per_image: Some(n),
                    },
                    extent,
                );
                let view = texture.create_view(&Default::default());
                g.heights = Some((texture, view));
                g.heights_key = heights_key;
                g.group = None;
            }
            if g.group.is_none()
                && let (Some((_, scorch)), Some((_, heights))) = (&g.scorch, &g.heights)
            {
                g.group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("painted ground"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: g.params.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(albedo),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(detail),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(scorch),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::Sampler(&gpu.linear_clamp),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(heights),
                        },
                    ],
                }));
            }
            gpu.queue
                .write_buffer(&g.params, 0, bytemuck::bytes_of(&params));
            if visible {
                self.stats.draw_calls += 1;
                self.stats.triangles += (GRID * GRID * 2) as u64;
            }
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        let Some(p) = &self.pipeline else {
            return;
        };
        for g in self.grounds.iter().filter(|g| g.visible) {
            let Some(group) = &g.group else {
                continue;
            };
            pass.set_pipeline(p);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..GRID * GRID * 6, 0..1);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn shader_validates_and_params_match_wgsl() {
        validate_wgsl("ground", &compose_shader(GROUND_SHADER, true)).unwrap();
        // origin + extent + 2 tiles + 12 paints × 2 vec4.
        assert_eq!(
            std::mem::size_of::<GpuGroundParams>(),
            16 * (4 + MAX_PAINTS * 2)
        );
        assert!(GROUND_SHADER.contains(&format!("const GRID: u32 = {GRID}u;")));
        assert!(GROUND_SHADER.contains(&format!("const MAX_PAINTS: u32 = {MAX_PAINTS}u;")));
    }

    #[test]
    fn heights_follow_the_ground_under_the_rectangle() {
        struct Ramp;
        impl engine_world::GroundQuery for Ramp {
            fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
                Some(engine_world::GroundSample {
                    height: 3.0 + 0.25 * p.x,
                    normal: engine_core::math::Vec3::Y,
                    surface: engine_world::SurfaceId(3),
                })
            }
        }
        let ramp = Ground(std::sync::Arc::new(Ramp));
        let h = ground_heights(Some(&ramp), DVec3::new(4.0, 3.0, 0.0), [8.0, 2.0]);
        let n = (GRID + 1) as usize;
        assert_eq!(h.len(), n * n);
        // Corners at x = -4 and 12: the ramp is 1 m below and 3 m above the entity there.
        assert!((h[0] - (3.0 + 0.25 * -4.0 - 3.0)).abs() < 1e-5, "{}", h[0]);
        assert!(
            (h[n - 1] - (3.0 + 0.25 * 12.0 - 3.0)).abs() < 1e-5,
            "{}",
            h[n - 1]
        );
        // No ground: flat at the entity.
        assert!(
            ground_heights(None, DVec3::ZERO, [1.0, 1.0])
                .iter()
                .all(|&v| v == 0.0)
        );
    }

    #[test]
    fn scorch_lands_where_the_fire_burnt_wherever_the_field_starts() {
        use engine_core::math::IVec2;
        // A floating-origin shift moves the field's origin off zero.
        let mut field = FireField::new(0.5);
        field.origin = DVec2::new(103.0, -41.5);
        let mut fuel = |_: DVec3| (crate::fuel::CellFuel::NONE, 0.0);
        let i = field.ensure_tile(IVec2::ZERO, 0.0, &mut fuel).unwrap();
        field.tiles[i].cell_mut(IVec2::new(3, 5)).scorch = 1.0;
        // That cell's center is at origin + (3.5, 5.5) cells.
        let center = field.origin + DVec2::new(3.5, 5.5) * 0.5;
        let min = center - DVec2::splat(2.0);
        let data = scorch_texels(&field, min, [8, 8], 0.5);
        assert_eq!(data[4 * 8 + 4], 255, "{data:?}");
        assert_eq!(data.iter().filter(|&&v| v > 0).count(), 1);
    }

    #[test]
    fn paints_pack_in_order() {
        let g = PaintedGround {
            base: GroundKind::Litter,
            paints: vec![
                GroundPaint::new(
                    GroundKind::Gravel,
                    [3.0, 0.0],
                    PatchShape::Rect {
                        half_extents: [1.5, 20.0],
                    },
                ),
                GroundPaint::new(
                    GroundKind::Ash,
                    [-2.0, 1.0],
                    PatchShape::Disc { radius: 0.7 },
                ),
            ],
            ..Default::default()
        };
        let p = GpuGroundParams::new(&g, [0.0; 3]);
        assert_eq!(p.origin[3], 2.0);
        assert_eq!(p.extent[3], GroundKind::Litter.layer() as f32);
        assert_eq!(p.paints[0].shape, [3.0, 0.0, 1.5, 20.0]);
        assert_eq!(p.paints[1].shape, [-2.0, 1.0, 0.7, -1.0]);
        assert_eq!(p.paints[1].look[0], GroundKind::Ash.layer() as f32);
    }
}
