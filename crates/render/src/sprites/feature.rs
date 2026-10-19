//! The render feature that draws [`Sprite`]s and [`TileMap`]s.
//!
//! Per frame it gathers the sprites (culled against the camera and each shadow cascade),
//! builds one camera-relative instance per sprite (quad corner, width and height vectors, last
//! frame's placement for motion vectors, frame rectangle, tint, material values), sorts them
//! and draws each run that shares an image with one instanced 4-vertex strip. Tile maps keep
//! their tiles on the GPU (uploaded when the map, its atlas or its image change), sorted into
//! chunks of [`CHUNK`] × [`CHUNK`] tiles; each frame only the map's placement (a uniform at a
//! dynamic offset) and the list of visible chunk ranges are new.
//!
//! Images come from the shared image cache ([`GpuShared::image_texture`]); a sprite whose
//! image is still loading is not drawn. Pipelines are built the first frame anything is drawn,
//! so scenes without sprites pay nothing.
//!
//! Order: cut-out sprites and maps draw in the opaque phase (and the depth prepass and the sun
//! shadow cascades), by layer and then by image so runs batch; blended and additive ones in the
//! transparent phase, by layer and then back to front.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use wgpu::util::DeviceExt;

use super::{
    Billboard, Parallax, PixelGrid, Sprite, SpriteAlpha, SpriteFilter, TILE_FLIP_DIAGONAL,
    TILE_FLIP_X, TILE_FLIP_Y, TILE_FRAME_MASK, TileMap, camera_pixel_grid, unit_d,
};
use crate::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase,
};
use crate::frame::ViewData;
use crate::image_cache::{ImageTexture, upload_image};
use crate::settings::RenderSettings;
use crate::shadows::MAX_CASCADES;
use crate::texture::Image;

/// Tiles per side of a tile map chunk (the unit of culling).
pub const CHUNK: u32 = 32;
/// Most copies of a repeating [`Parallax`] layer drawn.
const MAX_REPEATS: i64 = 256;
/// Texture groups and uploaded tile maps unused for this many frames are released.
const EVICT_AFTER_FRAMES: u64 = 600;

const SHADER_COMMON: &str = include_str!("../shaders/sprite_common.wgsl");
const SHADER_SCENE: &str = include_str!("../shaders/sprite.wgsl");
const SHADER_SHADOW: &str = include_str!("../shaders/sprite_shadow.wgsl");

/// The scene/prepass shader (composed with the frame library) and the shadow shader (without).
pub(crate) fn shader_sources() -> [(&'static str, String, bool); 2] {
    [
        ("sprite", format!("{SHADER_COMMON}\n{SHADER_SCENE}"), true),
        (
            "sprite_shadow",
            format!("{SHADER_COMMON}\n{SHADER_SHADOW}"),
            false,
        ),
    ]
}

// Flag bits, as in `sprite_common.wgsl`.
const FLAG_FILTER_NEAREST: u32 = 1;
const FLAG_FILTER_PIXEL_ART: u32 = 2;
const FLAG_ALPHA_SHIFT: u32 = 2;
const ALPHA_BLEND: u32 = 0;
const ALPHA_ADDITIVE: u32 = 1;
const ALPHA_CUTOUT: u32 = 2;
const FLAG_LIT: u32 = 16;
const FLAG_NORMAL_MAP: u32 = 32;
const FLAG_FLIP_X: u32 = 64;
const FLAG_FLIP_Y: u32 = 128;
const FLAG_FLIP_DIAGONAL: u32 = 256;

/// One sprite on the GPU (`SpriteIn` in `sprite_common.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SpriteInstance {
    /// Camera-relative bottom-left corner.
    pos: [f32; 3],
    flags: u32,
    /// Width vector, alpha cutoff.
    ax: [f32; 4],
    /// Height vector, roughness.
    ay: [f32; 4],
    /// Last frame's corner (relative to last frame's camera), translucency.
    prev_pos: [f32; 4],
    /// Last frame's width vector, emissive.
    prev_ax: [f32; 4],
    /// Last frame's height vector, roundness.
    prev_ay: [f32; 4],
    /// Frame rectangle (top-left, bottom-right texture coordinates).
    rect: [f32; 4],
    color: [f32; 4],
}

impl SpriteInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Uint32, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// One tile on the GPU (`SpriteTileIn`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct TileInstance {
    /// Column | row << 16 (row 0 at the bottom), flip flags.
    tile: [u32; 2],
    /// The tile's texture rectangle.
    rect: [f32; 4],
}

impl TileInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Uint32x2, 1 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// A drawn tile map's placement and material (`SpriteTileLayer`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuTileLayer {
    m: [[f32; 4]; 3],
    p: [[f32; 4]; 3],
    tile: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    flags: [u32; 4],
}

/// A tile map's tiles on the GPU.
struct GpuTileMap {
    buffer: Option<wgpu::Buffer>,
    /// `(first, count, center in map space, radius)` per chunk, in buffer order.
    chunks: Vec<Chunk>,
    tiles: u32,
    /// What the upload was built from: the map's change tick and the image's size.
    tick: u32,
    image_size: [u32; 2],
    /// Last frame's placement: the map's corner in the world and its axes.
    previous: Option<(DVec3, Mat3, u64)>,
    last_used: u64,
}

#[derive(Debug, Clone, Copy)]
struct Chunk {
    first: u32,
    count: u32,
    center: Vec3,
    radius: f32,
}

/// A bind group of an image, its normal map and the sampler.
struct TextureGroup {
    group: wgpu::BindGroup,
    key: TextureKey,
    /// Uploads bound (image, normal map), to rebuild when either is uploaded again.
    bound: (Option<u64>, Option<u64>),
    last_used: u64,
}

/// Images of a group by canonical index (`u32::MAX` = none).
type TextureKey = (u32, u32);

/// What an item draws.
#[derive(Debug, Clone, Copy)]
enum ItemKind {
    /// A sprite: its instance in `sprite_data`.
    Sprite(u32),
    /// A tile map copy: its layer uniform, its map, and its visible chunk ranges for the camera
    /// and each cascade (`(start, len)` into `ranges`).
    Tiles {
        layer: u32,
        map: Entity,
        camera: (u32, u32),
        cascades: [(u32, u32); MAX_CASCADES],
    },
}

#[derive(Debug, Clone, Copy)]
struct Item {
    kind: ItemKind,
    group: u32,
    layer: i32,
    depth: f32,
    /// Ties: entity bits, then the copy.
    tie: (u64, i64),
    cutout: bool,
    visible: bool,
    casts: bool,
    center: Vec3,
    radius: f32,
}

/// A recorded draw.
#[derive(Debug, Clone, Copy)]
enum Draw {
    Sprites {
        group: u32,
        first: u32,
        count: u32,
    },
    Tiles {
        group: u32,
        map: Entity,
        layer: u32,
        ranges: (u32, u32),
    },
}

struct Pipelines {
    sprite_opaque: wgpu::RenderPipeline,
    sprite_blend: wgpu::RenderPipeline,
    sprite_prepass: wgpu::RenderPipeline,
    sprite_shadow: wgpu::RenderPipeline,
    tile_opaque: wgpu::RenderPipeline,
    tile_blend: wgpu::RenderPipeline,
    tile_prepass: wgpu::RenderPipeline,
    tile_shadow: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    layer_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: (wgpu::Texture, wgpu::TextureView),
    flat_normal: (wgpu::Texture, wgpu::TextureView),
    instances: GrowBuffer,
    layers: GrowBuffer,
    layer_group: wgpu::BindGroup,
    /// Bytes between layer uniforms (the device's uniform offset alignment).
    layer_stride: u64,
}

/// A sprite's placement last frame (motion vectors).
#[derive(Debug, Clone, Copy)]
struct Previous {
    corner: DVec3,
    ax: Vec3,
    ay: Vec3,
    frame: u64,
}

type SpriteQuery = (
    Entity,
    &'static GlobalTransform,
    &'static Sprite,
    Option<&'static Parallax>,
);
type TileQuery = (
    Entity,
    &'static GlobalTransform,
    Ref<'static, TileMap>,
    Option<&'static Parallax>,
);

/// Draws [`Sprite`] and [`TileMap`] entities. Added by `RenderPlugin`.
#[derive(Default)]
pub struct SpriteFeature {
    pipelines: Option<Pipelines>,
    sprite_query: Option<QueryState<SpriteQuery>>,
    tile_query: Option<QueryState<TileQuery>>,
    groups: Vec<Option<TextureGroup>>,
    group_ids: HashMap<TextureKey, u32>,
    free_groups: Vec<u32>,
    /// This frame's groups by key: `(id, has normal map)`, or `None` while an image loads.
    group_memo: HashMap<TextureKey, Option<(u32, bool)>>,
    maps: HashMap<Entity, GpuTileMap>,
    previous: HashMap<Entity, Previous>,
    prev_origin: Option<DVec3>,
    frame: u64,
    items: Vec<Item>,
    order: Vec<u32>,
    sprite_data: Vec<SpriteInstance>,
    instance_data: Vec<SpriteInstance>,
    layer_data: Vec<u8>,
    ranges: Vec<(u32, u32)>,
    opaque: Vec<Draw>,
    transparent: Vec<Draw>,
    shadow: [Vec<Draw>; MAX_CASCADES],
    stats: FeatureStats,
}

/// The view's camera axes (right, up, forward) as unit f64 vectors and f32.
struct CameraAxes {
    right: Vec3,
    up: Vec3,
    right_d: DVec3,
    up_d: DVec3,
}

impl CameraAxes {
    fn of(view: &ViewData) -> Self {
        let right = view.rotation * Vec3::X;
        let up = view.rotation * Vec3::Y;
        Self {
            right,
            up,
            right_d: unit_d(right, DVec3::X),
            up_d: unit_d(up, DVec3::Y),
        }
    }
}

fn filter_bits(filter: SpriteFilter) -> u32 {
    match filter {
        SpriteFilter::Linear => 0,
        SpriteFilter::Nearest => FLAG_FILTER_NEAREST,
        SpriteFilter::PixelArt => FLAG_FILTER_PIXEL_ART,
    }
}

/// Alpha flag bits and cutoff.
fn alpha_bits(alpha: SpriteAlpha) -> (u32, f32) {
    match alpha {
        SpriteAlpha::Blend => (ALPHA_BLEND << FLAG_ALPHA_SHIFT, 0.0),
        SpriteAlpha::Additive => (ALPHA_ADDITIVE << FLAG_ALPHA_SHIFT, 0.0),
        SpriteAlpha::Cutout(t) => (ALPHA_CUTOUT << FLAG_ALPHA_SHIFT, t.clamp(0.0, 1.0)),
    }
}

/// Where a [`Parallax`] layer is drawn: its offset from its own position for a camera at
/// `origin`.
fn parallax_offset(parallax: Option<&Parallax>, origin: DVec3) -> DVec3 {
    parallax.map_or(DVec3::ZERO, |p| {
        DVec3::new(
            origin.x * (1.0 - p.factor[0] as f64),
            origin.y * (1.0 - p.factor[1] as f64),
            0.0,
        )
    })
}

/// The copies `k` (offset `k × repeat_x` along `axis`) of a repeating layer whose base copy
/// is centered at camera-relative `center` and reaches `extent` meters along `axis` from its
/// center, that the view can see.
fn repeat_copies(
    view: &ViewData,
    parallax: Option<&Parallax>,
    center: Vec3,
    axis: Vec3,
    extent: f32,
) -> std::ops::RangeInclusive<i64> {
    let Some(p) = parallax.filter(|p| p.repeat_x > 1e-6) else {
        return 0..=0;
    };
    let right = view.rotation * Vec3::X;
    let along = axis.normalize_or_zero().dot(right);
    if along.abs() < 0.1 {
        return 0..=0;
    }
    // Half the view's width at the layer's depth.
    let sx = view.unjittered_proj.x_axis.x.abs().max(1e-6);
    let half = if view.orthographic {
        1.0 / sx
    } else {
        let depth = view.view_depth(center);
        if depth <= 0.0 {
            return 0..=0;
        }
        depth / sx
    };
    let p0 = center.dot(right);
    let step = p.repeat_x * along;
    let reach = half + extent;
    let (a, b) = ((-reach - p0) / step, (reach - p0) / step);
    let (lo, hi) = (a.min(b).floor() as i64, a.max(b).ceil() as i64);
    let lo = lo.max(-MAX_REPEATS);
    lo..=hi.min(lo + MAX_REPEATS)
}

/// Push the chunk ranges of a map passing `keep` into `ranges`, merging neighbours; returns
/// `(start, len)` into `ranges`.
fn chunk_ranges(
    chunks: &[Chunk],
    ranges: &mut Vec<(u32, u32)>,
    mut keep: impl FnMut(&Chunk) -> bool,
) -> (u32, u32) {
    let start = ranges.len();
    for c in chunks.iter().filter(|c| c.count > 0) {
        if !keep(c) {
            continue;
        }
        let joins = ranges.len() > start && ranges.last().is_some_and(|l| l.0 + l.1 == c.first);
        match ranges.last_mut() {
            Some(last) if joins => last.1 += c.count,
            _ => ranges.push((c.first, c.count)),
        }
    }
    (start as u32, (ranges.len() - start) as u32)
}

/// The tiles of `map` as GPU instances in chunk order, and the chunks.
fn build_tiles(map: &TileMap, image_size: [u32; 2]) -> (Vec<TileInstance>, Vec<Chunk>) {
    let [w, h] = map.size;
    let (w, h) = (w.min(u16::MAX as u32), h.min(u16::MAX as u32));
    let [tw, th] = map.tile_size;
    let mut tiles = Vec::new();
    let mut chunks = Vec::new();
    let mut rects: HashMap<u32, Option<[f32; 4]>> = HashMap::new();
    for cy in 0..h.div_ceil(CHUNK) {
        for cx in 0..w.div_ceil(CHUNK) {
            let first = tiles.len() as u32;
            let (x0, y0) = (cx * CHUNK, cy * CHUNK);
            let (x1, y1) = ((x0 + CHUNK).min(w), (y0 + CHUNK).min(h));
            // `y` counts rows from the bottom; the map's rows run from the top.
            for y in y0..y1 {
                let row = (h - 1 - y) as usize;
                for x in x0..x1 {
                    let value = map
                        .tiles
                        .get(row * map.size[0] as usize + x as usize)
                        .copied()
                        .unwrap_or(0);
                    let frame = value & TILE_FRAME_MASK;
                    if frame == 0 {
                        continue;
                    }
                    let rect = *rects
                        .entry(frame)
                        .or_insert_with(|| map.atlas.frame_uv(frame - 1, image_size));
                    let Some(rect) = rect else { continue };
                    let mut flags = 0;
                    if value & TILE_FLIP_X != 0 {
                        flags |= FLAG_FLIP_X;
                    }
                    if value & TILE_FLIP_Y != 0 {
                        flags |= FLAG_FLIP_Y;
                    }
                    if value & TILE_FLIP_DIAGONAL != 0 {
                        flags |= FLAG_FLIP_DIAGONAL;
                    }
                    tiles.push(TileInstance {
                        tile: [x | (y << 16), flags],
                        rect,
                    });
                }
            }
            let size = Vec2::new((x1 - x0) as f32 * tw, (y1 - y0) as f32 * th);
            chunks.push(Chunk {
                first,
                count: tiles.len() as u32 - first,
                center: Vec3::new(
                    x0 as f32 * tw + size.x * 0.5,
                    y0 as f32 * th + size.y * 0.5,
                    0.0,
                ),
                radius: size.length() * 0.5,
            });
        }
    }
    (tiles, chunks)
}

/// Rows of an affine map space → camera-relative transform (`m` in `GpuTileLayer`).
fn rows(axes: Mat3, translation: Vec3) -> [[f32; 4]; 3] {
    let m = Mat4::from_cols(
        axes.x_axis.extend(0.0),
        axes.y_axis.extend(0.0),
        axes.z_axis.extend(0.0),
        translation.extend(1.0),
    );
    [
        m.row(0).to_array(),
        m.row(1).to_array(),
        m.row(2).to_array(),
    ]
}

impl SpriteFeature {
    fn build_pipelines(gpu: &GpuShared) -> Pipelines {
        let device = &gpu.device;
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite textures"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layer_size = std::mem::size_of::<GpuTileLayer>() as u64;
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile layer"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(layer_size),
                },
                count: None,
            }],
        });
        // Trilinear and anisotropic; nearest and pixel-art filtering happen in the shader.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let white = upload_image(gpu, &Image::solid([255; 4], true), true, false)
            .expect("1x1 white sprite texture");
        let flat_normal = upload_image(
            gpu,
            &Image::solid([128, 128, 255, 255], false),
            false,
            false,
        )
        .expect("1x1 flat normal texture");

        let [(_, scene_src, _), (_, shadow_src, _)] = shader_sources();
        let scene = gpu.shader("sprite", &scene_src, true);
        let shadow = gpu.shader("sprite shadow", &shadow_src, false);
        let layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let sprite_layout = layout("sprite", &[Some(&gpu.frame_layout), Some(&texture_layout)]);
        let tile_layout = layout(
            "tile map",
            &[
                Some(&gpu.frame_layout),
                Some(&texture_layout),
                Some(&layer_layout),
            ],
        );
        let sprite_shadow_layout = layout(
            "sprite shadow",
            &[Some(&gpu.shadow_view_layout), Some(&texture_layout)],
        );
        let tile_shadow_layout = layout(
            "tile map shadow",
            &[
                Some(&gpu.shadow_view_layout),
                Some(&texture_layout),
                Some(&layer_layout),
            ],
        );
        let strip = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        };
        let hdr_target = |blend: Option<wgpu::BlendState>| {
            [Some(wgpu::ColorTargetState {
                format: gpu.formats.hdr,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })]
        };
        let opaque_targets = hdr_target(None);
        let blend_targets = hdr_target(Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let prepass_targets = gpu.prepass_targets();
        let sprite_buffers = [Some(SpriteInstance::layout())];
        let tile_buffers = [Some(TileInstance::layout())];
        #[allow(clippy::too_many_arguments)]
        let scene_pipeline = |label: &str,
                              layout: &wgpu::PipelineLayout,
                              vs: &str,
                              buffers: &[Option<wgpu::VertexBufferLayout>],
                              fs: &str,
                              targets: &[Option<wgpu::ColorTargetState>],
                              depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &scene,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: strip,
                depth_stencil: Some(gpu.depth_state(depth_write)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &scene,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let shadow_primitive = wgpu::PrimitiveState {
            unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            ..strip
        };
        let shadow_pipeline =
            |label: &str,
             layout: &wgpu::PipelineLayout,
             vs: &str,
             buffers: &[Option<wgpu::VertexBufferLayout>]| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shadow,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers,
                    },
                    primitive: shadow_primitive,
                    depth_stencil: Some(gpu.shadow_depth_state()),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shadow,
                        entry_point: Some("fs_sprite_shadow"),
                        compilation_options: Default::default(),
                        targets: &[],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let s = &sprite_layout;
        let t = &tile_layout;
        let alignment = device.limits().min_uniform_buffer_offset_alignment.max(16) as u64;
        let layer_stride = layer_size.div_ceil(alignment) * alignment;
        let instances = GrowBuffer::new(
            device,
            "sprite instances",
            wgpu::BufferUsages::VERTEX,
            256 * std::mem::size_of::<SpriteInstance>() as u64,
        );
        let layers = GrowBuffer::new(
            device,
            "tile layers",
            wgpu::BufferUsages::UNIFORM,
            layer_stride * 8,
        );
        let layer_group = create_layer_group(device, &layer_layout, &layers.buffer, layer_size);
        Pipelines {
            sprite_opaque: scene_pipeline(
                "sprite cutout",
                s,
                "vs_sprite",
                &sprite_buffers,
                "fs_sprite",
                &opaque_targets,
                true,
            ),
            sprite_blend: scene_pipeline(
                "sprite blend",
                s,
                "vs_sprite",
                &sprite_buffers,
                "fs_sprite",
                &blend_targets,
                false,
            ),
            sprite_prepass: scene_pipeline(
                "sprite prepass",
                s,
                "vs_sprite",
                &sprite_buffers,
                "fs_sprite_prepass",
                &prepass_targets,
                true,
            ),
            sprite_shadow: shadow_pipeline(
                "sprite shadow",
                &sprite_shadow_layout,
                "vs_sprite_shadow",
                &sprite_buffers,
            ),
            tile_opaque: scene_pipeline(
                "tile map cutout",
                t,
                "vs_tile",
                &tile_buffers,
                "fs_sprite",
                &opaque_targets,
                true,
            ),
            tile_blend: scene_pipeline(
                "tile map blend",
                t,
                "vs_tile",
                &tile_buffers,
                "fs_sprite",
                &blend_targets,
                false,
            ),
            tile_prepass: scene_pipeline(
                "tile map prepass",
                t,
                "vs_tile",
                &tile_buffers,
                "fs_sprite_prepass",
                &prepass_targets,
                true,
            ),
            tile_shadow: shadow_pipeline(
                "tile map shadow",
                &tile_shadow_layout,
                "vs_tile_shadow",
                &tile_buffers,
            ),
            texture_layout,
            layer_layout,
            sampler,
            white,
            flat_normal,
            instances,
            layers,
            layer_group,
            layer_stride,
        }
    }

    /// The texture group of an image and a normal map: `(id, has a normal map)`, or `None`
    /// while the image is still loading.
    fn texture_group(
        &mut self,
        gpu: &GpuShared,
        images: Option<&Assets<Image>>,
        image: Option<Handle<Image>>,
        normal: Option<Handle<Image>>,
    ) -> Option<(u32, bool)> {
        let canonical = |h: Option<Handle<Image>>| match (h, images) {
            (Some(h), Some(images)) => images.canonical(h).index(),
            (Some(h), None) => h.index(),
            (None, _) => u32::MAX,
        };
        let key = (canonical(image), canonical(normal));
        if let Some(memo) = self.group_memo.get(&key) {
            return *memo;
        }
        let result = self.build_group(gpu, images, image, normal, key);
        self.group_memo.insert(key, result);
        result
    }

    fn build_group(
        &mut self,
        gpu: &GpuShared,
        images: Option<&Assets<Image>>,
        image: Option<Handle<Image>>,
        normal: Option<Handle<Image>>,
        key: TextureKey,
    ) -> Option<(u32, bool)> {
        let pipelines = self.pipelines.as_ref()?;
        let base: Option<Arc<ImageTexture>> = match image {
            Some(h) => Some(gpu.image_texture(images?, h, true)?),
            None => None,
        };
        // A normal map that is still loading: draw without it meanwhile.
        let normal_tex: Option<Arc<ImageTexture>> =
            normal.and_then(|h| gpu.image_texture(images?, h, false));
        let bound = (
            base.as_ref().map(|t| t.id),
            normal_tex.as_ref().map(|t| t.id),
        );
        let id = match self.group_ids.get(&key) {
            Some(id) => *id,
            None => {
                let id = self.free_groups.pop().unwrap_or(self.groups.len() as u32);
                self.group_ids.insert(key, id);
                id
            }
        };
        if self.groups.len() <= id as usize {
            self.groups.resize_with(id as usize + 1, || None);
        }
        let frame = self.frame;
        match &mut self.groups[id as usize] {
            Some(g) if g.bound == bound => g.last_used = frame,
            slot => {
                let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("sprite textures"),
                    layout: &pipelines.texture_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                base.as_ref().map_or(&pipelines.white.1, |t| &t.view),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(
                                normal_tex
                                    .as_ref()
                                    .map_or(&pipelines.flat_normal.1, |t| &t.view),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&pipelines.sampler),
                        },
                    ],
                });
                *slot = Some(TextureGroup {
                    group,
                    key,
                    bound,
                    last_used: frame,
                });
            }
        }
        Some((id, normal_tex.is_some()))
    }

    /// Release texture groups and tile maps nobody drew for a while.
    fn evict(&mut self) {
        let frame = self.frame;
        for id in 0..self.groups.len() {
            let stale = self.groups[id]
                .as_ref()
                .is_some_and(|g| g.last_used + EVICT_AFTER_FRAMES < frame);
            if stale && let Some(g) = self.groups[id].take() {
                self.group_ids.remove(&g.key);
                self.free_groups.push(id as u32);
            }
        }
        self.maps
            .retain(|_, m| m.last_used + EVICT_AFTER_FRAMES >= frame);
        self.previous.retain(|_, p| p.frame + 1 >= frame);
    }

    fn clear_frame(&mut self) {
        self.items.clear();
        self.order.clear();
        self.sprite_data.clear();
        self.instance_data.clear();
        self.layer_data.clear();
        self.ranges.clear();
        self.opaque.clear();
        self.transparent.clear();
        for s in &mut self.shadow {
            s.clear();
        }
        self.group_memo.clear();
    }

    /// Gather this frame's sprites into `items` and `sprite_data`. Returns the number culled.
    #[allow(clippy::too_many_arguments)]
    fn gather_sprites(
        &mut self,
        world: &World,
        query: &mut QueryState<SpriteQuery>,
        gpu: &GpuShared,
        view: &ViewData,
        grid: Option<&PixelGrid>,
        prev_origin: DVec3,
    ) -> u32 {
        let images = world.get_resource::<Assets<Image>>();
        let axes = CameraAxes::of(view);
        let frame = self.frame;
        let mut culled = 0;
        for (entity, gt, sprite, parallax) in query.iter(world) {
            if !sprite.visible {
                continue;
            }
            let image_size = match sprite.image {
                Some(h) => match images.and_then(|i| i.get(h)) {
                    Some(image) => [image.width, image.height],
                    None => continue,
                },
                None => [1, 1],
            };
            let Some(rect) = sprite.atlas.frame_uv(sprite.frame, image_size) else {
                continue;
            };
            let Some((group, has_normal)) =
                self.texture_group(gpu, images, sprite.image, sprite.normal_map)
            else {
                continue;
            };
            let [w, h] = sprite.size;
            let scale = gt.scale();
            let (ax, ay) = match sprite.billboard {
                Billboard::Fixed => (
                    gt.transform_vector(Vec3::X * w),
                    gt.transform_vector(Vec3::Y * h),
                ),
                Billboard::FaceCamera => (axes.right * (w * scale.x), axes.up * (h * scale.y)),
                Billboard::Upright => {
                    let up = gt.up();
                    let right = (axes.right - up * axes.right.dot(up)).normalize_or(gt.right());
                    (right * (w * scale.x), up * (h * scale.y))
                }
            };
            let anchor_offset = ax * sprite.anchor[0] + ay * sprite.anchor[1];
            let base_corner = gt.translation() + parallax_offset(parallax, view.origin)
                - anchor_offset.as_dvec3();
            let (alpha, cutoff) = alpha_bits(sprite.alpha);
            let cutout = matches!(sprite.alpha, SpriteAlpha::Cutout(_));
            let mut flags = filter_bits(sprite.filter) | alpha;
            if sprite.lit {
                flags |= FLAG_LIT;
            }
            if has_normal && sprite.lit {
                flags |= FLAG_NORMAL_MAP;
            }
            if sprite.flip_x {
                flags |= FLAG_FLIP_X;
            }
            if sprite.flip_y {
                flags |= FLAG_FLIP_Y;
            }
            let prev = match self.previous.insert(
                entity,
                Previous {
                    corner: base_corner,
                    ax,
                    ay,
                    frame,
                },
            ) {
                Some(p) if p.frame + 1 == frame => p,
                _ => Previous {
                    corner: base_corner,
                    ax,
                    ay,
                    frame,
                },
            };
            let radius = 0.5 * (ax.length() + ay.length());
            let base_center = (base_corner - view.origin).as_vec3() + (ax + ay) * 0.5;
            let copy_axis = if sprite.billboard == Billboard::Fixed {
                gt.right()
            } else {
                Vec3::X
            };
            let casts = cutout && sprite.cast_shadows && view.sun_shadows;
            for copy in repeat_copies(view, parallax, base_center, copy_axis, radius) {
                let shift = parallax.map_or(DVec3::ZERO, |p| {
                    (copy_axis * p.repeat_x).as_dvec3() * copy as f64
                });
                let mut corner = base_corner + shift;
                if let Some(grid) = grid {
                    corner = grid.snap_point(corner, axes.right_d, axes.up_d);
                }
                let rel = (corner - view.origin).as_vec3();
                let center = rel + (ax + ay) * 0.5;
                let visible = view.frustum.intersects_sphere(center, radius);
                if !visible && !casts {
                    culled += 1;
                    continue;
                }
                let prev_rel = (prev.corner + shift - prev_origin).as_vec3();
                let index = self.sprite_data.len() as u32;
                self.sprite_data.push(SpriteInstance {
                    pos: rel.to_array(),
                    flags,
                    ax: ax.extend(cutoff).to_array(),
                    ay: ay.extend(sprite.roughness).to_array(),
                    prev_pos: prev_rel.extend(sprite.translucency).to_array(),
                    prev_ax: prev.ax.extend(sprite.emissive.max(0.0)).to_array(),
                    prev_ay: prev.ay.extend(sprite.roundness.max(0.0)).to_array(),
                    rect,
                    color: sprite.color,
                });
                self.items.push(Item {
                    kind: ItemKind::Sprite(index),
                    group,
                    layer: sprite.layer,
                    depth: view.view_depth(center),
                    tie: (entity.to_bits(), copy),
                    cutout,
                    visible,
                    casts,
                    center,
                    radius,
                });
                if !visible {
                    culled += 1;
                }
            }
        }
        culled
    }

    /// Gather this frame's tile maps (uploading changed ones) into `items`, with their layer
    /// uniforms and chunk ranges. Returns the number of maps culled.
    #[allow(clippy::too_many_arguments)]
    fn gather_tile_maps(
        &mut self,
        world: &World,
        query: &mut QueryState<TileQuery>,
        gpu: &GpuShared,
        view: &ViewData,
        grid: Option<&PixelGrid>,
        prev_origin: DVec3,
    ) -> u32 {
        let images = world.get_resource::<Assets<Image>>();
        let axes = CameraAxes::of(view);
        let frame = self.frame;
        let Some(stride) = self.pipelines.as_ref().map(|p| p.layer_stride) else {
            return 0;
        };
        let cascades = if view.sun_shadows {
            (view.cascades.count as usize).min(MAX_CASCADES)
        } else {
            0
        };
        let mut culled = 0;
        for (entity, gt, map, parallax) in query.iter(world) {
            if !map.visible || map.size[0] == 0 || map.size[1] == 0 {
                continue;
            }
            let image_size = match map.image {
                Some(h) => match images.and_then(|i| i.get(h)) {
                    Some(image) => [image.width, image.height],
                    None => continue,
                },
                None => [1, 1],
            };
            let Some((group, has_normal)) =
                self.texture_group(gpu, images, map.image, map.normal_map)
            else {
                continue;
            };
            // Upload the tiles when the map (or its image's size) changed.
            let tick = map.last_changed().get();
            let gpu_map = self.maps.entry(entity).or_insert_with(|| GpuTileMap {
                buffer: None,
                chunks: Vec::new(),
                tiles: 0,
                tick: tick.wrapping_add(1),
                image_size: [0, 0],
                previous: None,
                last_used: frame,
            });
            if gpu_map.tick != tick || gpu_map.image_size != image_size {
                let (tiles, chunks) = build_tiles(&map, image_size);
                gpu_map.buffer = (!tiles.is_empty()).then(|| {
                    gpu.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tile map"),
                            contents: bytemuck::cast_slice(&tiles),
                            usage: wgpu::BufferUsages::VERTEX,
                        })
                });
                gpu_map.tiles = tiles.len() as u32;
                gpu_map.chunks = chunks;
                gpu_map.tick = tick;
                gpu_map.image_size = image_size;
            }
            gpu_map.last_used = frame;
            if gpu_map.buffer.is_none() {
                continue;
            }

            // Placement: map space (meters, the bottom-left corner at 0) → world.
            let extent = map.extent();
            let anchor = Vec3::new(map.anchor[0] * extent[0], map.anchor[1] * extent[1], 0.0);
            let axes_m = Mat3::from_cols(
                gt.transform_vector(Vec3::X),
                gt.transform_vector(Vec3::Y),
                gt.transform_vector(Vec3::Z),
            );
            let base_corner =
                gt.transform_point(-anchor.as_dvec3()) + parallax_offset(parallax, view.origin);
            let (prev_corner, prev_axes) = match gpu_map.previous {
                Some((c, a, f)) if f + 1 == frame => (c, a),
                _ => (base_corner, axes_m),
            };
            gpu_map.previous = Some((base_corner, axes_m, frame));
            let max_scale = gt.scale().abs().max_element();
            let (alpha, cutoff) = alpha_bits(map.alpha);
            let cutout = matches!(map.alpha, SpriteAlpha::Cutout(_));
            let mut flags = filter_bits(map.filter) | alpha;
            if map.lit {
                flags |= FLAG_LIT;
            }
            if has_normal && map.lit {
                flags |= FLAG_NORMAL_MAP;
            }
            let casts = cutout && map.cast_shadows && cascades > 0;
            let half_extent = Vec2::from(extent).length() * 0.5 * max_scale;
            let base_center = (base_corner - view.origin).as_vec3()
                + axes_m * Vec3::new(extent[0] * 0.5, extent[1] * 0.5, 0.0);
            let copy_axis = gt.right();
            let gpu_map = &self.maps[&entity];
            for copy in repeat_copies(view, parallax, base_center, copy_axis, half_extent) {
                let shift = parallax.map_or(DVec3::ZERO, |p| {
                    (copy_axis * p.repeat_x).as_dvec3() * copy as f64
                });
                let mut corner = base_corner + shift;
                if let Some(grid) = grid {
                    corner = grid.snap_point(corner, axes.right_d, axes.up_d);
                }
                let translation = (corner - view.origin).as_vec3();
                let to_rel = |c: &Chunk| translation + axes_m * c.center;
                let radius = |c: &Chunk| c.radius * max_scale;
                let camera = chunk_ranges(&gpu_map.chunks, &mut self.ranges, |c| {
                    view.frustum.intersects_sphere(to_rel(c), radius(c))
                });
                let mut shadow = [(0, 0); MAX_CASCADES];
                if casts {
                    for (i, s) in shadow.iter_mut().enumerate().take(cascades) {
                        *s = chunk_ranges(&gpu_map.chunks, &mut self.ranges, |c| {
                            view.cascades.sphere_in_cascade(i, to_rel(c), radius(c))
                        });
                    }
                }
                if camera.1 == 0 && shadow.iter().all(|s| s.1 == 0) {
                    culled += 1;
                    continue;
                }
                let prev_translation = (prev_corner + shift - prev_origin).as_vec3();
                let layer = GpuTileLayer {
                    m: rows(axes_m, translation),
                    p: rows(prev_axes, prev_translation),
                    tile: [map.tile_size[0], map.tile_size[1], 0.0, 0.0],
                    color: map.color,
                    params: [
                        cutoff,
                        map.roughness,
                        map.translucency,
                        map.emissive.max(0.0),
                    ],
                    flags: [flags, 0, 0, 0],
                };
                let index = (self.layer_data.len() as u64 / stride) as u32;
                self.layer_data
                    .extend_from_slice(bytemuck::bytes_of(&layer));
                self.layer_data
                    .resize(((index + 1) as u64 * stride) as usize, 0);
                let center = base_center + shift.as_vec3();
                self.items.push(Item {
                    kind: ItemKind::Tiles {
                        layer: index,
                        map: entity,
                        camera,
                        cascades: shadow,
                    },
                    group,
                    layer: map.layer,
                    depth: view.view_depth(center),
                    tie: (entity.to_bits(), copy),
                    cutout,
                    visible: camera.1 > 0,
                    casts,
                    center,
                    radius: half_extent,
                });
            }
        }
        culled
    }

    /// Append the draw of `item` to `draws` (sprites join the previous run when they can),
    /// with `ranges` the chunk ranges for tile maps.
    fn push_draw(
        draws: &mut Vec<Draw>,
        instances: &mut Vec<SpriteInstance>,
        sprite_data: &[SpriteInstance],
        item: &Item,
        ranges: (u32, u32),
    ) {
        match item.kind {
            ItemKind::Sprite(index) => {
                let at = instances.len() as u32;
                instances.push(sprite_data[index as usize]);
                if let Some(Draw::Sprites {
                    group,
                    first,
                    count,
                }) = draws.last_mut()
                    && *group == item.group
                    && *first + *count == at
                {
                    *count += 1;
                    return;
                }
                draws.push(Draw::Sprites {
                    group: item.group,
                    first: at,
                    count: 1,
                });
            }
            ItemKind::Tiles { layer, map, .. } => {
                if ranges.1 > 0 {
                    draws.push(Draw::Tiles {
                        group: item.group,
                        map,
                        layer,
                        ranges,
                    });
                }
            }
        }
    }

    fn draw_list(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        draws: &[Draw],
        sprite_pipeline: &wgpu::RenderPipeline,
        tile_pipeline: &wgpu::RenderPipeline,
    ) {
        let Some(p) = &self.pipelines else {
            return;
        };
        // 0 = none, 1 = sprites, 2 = tiles.
        let mut bound = 0;
        let mut group_bound = u32::MAX;
        for draw in draws {
            match *draw {
                Draw::Sprites {
                    group,
                    first,
                    count,
                } => {
                    let Some(g) = self.groups.get(group as usize).and_then(Option::as_ref) else {
                        continue;
                    };
                    if bound != 1 {
                        pass.set_pipeline(sprite_pipeline);
                        pass.set_vertex_buffer(0, p.instances.buffer.slice(..));
                        bound = 1;
                        group_bound = u32::MAX;
                    }
                    if group != group_bound {
                        pass.set_bind_group(1, &g.group, &[]);
                        group_bound = group;
                    }
                    pass.draw(0..4, first..first + count);
                }
                Draw::Tiles {
                    group,
                    map,
                    layer,
                    ranges,
                } => {
                    let (Some(g), Some(buffer)) = (
                        self.groups.get(group as usize).and_then(Option::as_ref),
                        self.maps.get(&map).and_then(|m| m.buffer.as_ref()),
                    ) else {
                        continue;
                    };
                    if bound != 2 {
                        pass.set_pipeline(tile_pipeline);
                        bound = 2;
                        group_bound = u32::MAX;
                    }
                    if group != group_bound {
                        pass.set_bind_group(1, &g.group, &[]);
                        group_bound = group;
                    }
                    let offset = (layer as u64 * p.layer_stride) as u32;
                    pass.set_bind_group(2, &p.layer_group, &[offset]);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    let (start, len) = ranges;
                    for &(first, count) in &self.ranges[start as usize..(start + len) as usize] {
                        pass.draw(0..4, first..first + count);
                    }
                }
            }
        }
    }

    /// Instances and draw calls a list makes (stats).
    fn count(&self, draws: &[Draw]) -> (u32, u32) {
        let mut calls = 0;
        let mut instances = 0;
        for d in draws {
            match *d {
                Draw::Sprites { count, .. } => {
                    calls += 1;
                    instances += count;
                }
                Draw::Tiles { ranges, .. } => {
                    let r = &self.ranges[ranges.0 as usize..(ranges.0 + ranges.1) as usize];
                    calls += r.len() as u32;
                    instances += r.iter().map(|r| r.1).sum::<u32>();
                }
            }
        }
        (calls, instances)
    }
}

fn create_layer_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    size: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tile layers"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size),
            }),
        }],
    })
}

impl RenderFeature for SpriteFeature {
    fn name(&self) -> &str {
        "sprites"
    }

    /// After the meshes: blended sprites draw over blended meshes.
    fn order(&self) -> i32 {
        10
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.frame += 1;
        self.clear_frame();
        let mut sprites = self
            .sprite_query
            .take()
            .unwrap_or_else(|| world.query::<SpriteQuery>());
        let mut tiles = self
            .tile_query
            .take()
            .unwrap_or_else(|| world.query::<TileQuery>());
        let world_ref: &World = world;
        let any =
            sprites.iter(world_ref).next().is_some() || tiles.iter(world_ref).next().is_some();
        if !any {
            // Nothing to draw: no pipelines, no buffers.
            self.sprite_query = Some(sprites);
            self.tile_query = Some(tiles);
            self.stats = FeatureStats::default();
            self.maps.clear();
            self.previous.clear();
            self.prev_origin = None;
            return;
        }
        if self.pipelines.is_none() {
            self.pipelines = Some(Self::build_pipelines(gpu));
        }
        let grid = view
            .camera
            .filter(|_| view.orthographic)
            .and_then(|e| camera_pixel_grid(world_ref, e, view.height));
        let prev_origin = self.prev_origin.unwrap_or(view.origin);
        let mut culled = self.gather_sprites(
            world_ref,
            &mut sprites,
            gpu,
            view,
            grid.as_ref(),
            prev_origin,
        );
        culled +=
            self.gather_tile_maps(world_ref, &mut tiles, gpu, view, grid.as_ref(), prev_origin);
        self.sprite_query = Some(sprites);
        self.tile_query = Some(tiles);
        self.prev_origin = Some(view.origin);

        // Opaque (cut-out): by layer, then image, so runs batch (the depth buffer sorts the
        // rest). Transparent: by layer, then back to front.
        let items = &self.items;
        self.order.extend(0..items.len() as u32);
        self.order.sort_unstable_by(|&a, &b| {
            let (a, b) = (&items[a as usize], &items[b as usize]);
            a.cutout.cmp(&b.cutout).reverse().then_with(|| {
                a.layer.cmp(&b.layer).then_with(|| {
                    if a.cutout {
                        a.group.cmp(&b.group).then(a.tie.cmp(&b.tie))
                    } else {
                        b.depth
                            .total_cmp(&a.depth)
                            .then(a.group.cmp(&b.group))
                            .then(a.tie.cmp(&b.tie))
                    }
                })
            })
        });
        for &i in &self.order {
            let item = &self.items[i as usize];
            if !item.visible {
                continue;
            }
            let ranges = match item.kind {
                ItemKind::Tiles { camera, .. } => camera,
                ItemKind::Sprite(_) => (0, 0),
            };
            let draws = if item.cutout {
                &mut self.opaque
            } else {
                &mut self.transparent
            };
            Self::push_draw(
                draws,
                &mut self.instance_data,
                &self.sprite_data,
                item,
                ranges,
            );
        }
        // Shadows: casters by image, per cascade.
        if view.sun_shadows {
            self.order.retain(|&i| items[i as usize].casts);
            self.order
                .sort_unstable_by_key(|&i| (items[i as usize].group, items[i as usize].tie));
            let cascades = (view.cascades.count as usize).min(MAX_CASCADES);
            for c in 0..cascades {
                for &i in &self.order {
                    let item = &self.items[i as usize];
                    let ranges = match item.kind {
                        ItemKind::Tiles { cascades, .. } => cascades[c],
                        ItemKind::Sprite(_) => {
                            if !view.cascades.sphere_in_cascade(c, item.center, item.radius) {
                                continue;
                            }
                            (0, 0)
                        }
                    };
                    Self::push_draw(
                        &mut self.shadow[c],
                        &mut self.instance_data,
                        &self.sprite_data,
                        item,
                        ranges,
                    );
                }
            }
        }

        // Upload.
        if let Some(p) = &mut self.pipelines {
            p.instances.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&self.instance_data),
            );
            if p.layers.write(&gpu.device, &gpu.queue, &self.layer_data) {
                p.layer_group = create_layer_group(
                    &gpu.device,
                    &p.layer_layout,
                    &p.layers.buffer,
                    std::mem::size_of::<GpuTileLayer>() as u64,
                );
            }
        }
        if self.frame.is_multiple_of(120) {
            self.evict();
        }

        let (calls, instances) = self.count(&self.opaque);
        let (t_calls, t_instances) = self.count(&self.transparent);
        let shadow_calls: u32 = self.shadow.iter().map(|s| self.count(s).0).sum();
        // Opaque draws run twice (depth prepass and scene).
        self.stats = FeatureStats {
            draw_calls: calls * 2 + t_calls + shadow_calls,
            instances: instances + t_instances,
            triangles: 2 * (instances + t_instances) as u64,
            culled,
        };
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        let Some(p) = &self.pipelines else {
            return;
        };
        match phase {
            RenderPhase::Opaque if !self.opaque.is_empty() => {
                self.draw_list(pass, &self.opaque, &p.sprite_opaque, &p.tile_opaque)
            }
            RenderPhase::Transparent if !self.transparent.is_empty() => {
                self.draw_list(pass, &self.transparent, &p.sprite_blend, &p.tile_blend)
            }
            _ => {}
        }
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let Some(p) = &self.pipelines
            && !self.opaque.is_empty()
        {
            self.draw_list(pass, &self.opaque, &p.sprite_prepass, &p.tile_prepass);
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        if let Some(p) = &self.pipelines
            && let Some(draws) = self.shadow.get(cascade)
            && !draws.is_empty()
        {
            self.draw_list(pass, draws, &p.sprite_shadow, &p.tile_shadow);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "pipelines": self.pipelines.is_some(),
            "sprites": self.sprite_data.len(),
            "tile_maps": self.maps.len(),
            "tiles": self.maps.values().map(|m| m.tiles as u64).sum::<u64>(),
            "texture_groups": self.groups.iter().filter(|g| g.is_some()).count(),
            "opaque_draws": self.opaque.len(),
            "transparent_draws": self.transparent.len(),
            "shadow_draws": self.shadow.iter().map(Vec::len).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_upload_in_chunks_and_skip_empty_cells() {
        let mut map = TileMap {
            atlas: crate::sprites::SpriteAtlas::grid(2, 2),
            size: [40, 3],
            tile_size: [0.5, 0.5],
            tiles: vec![0; 120],
            ..Default::default()
        };
        map.set(0, 2, 1); // bottom-left
        map.set(39, 0, 4 | TILE_FLIP_X); // top-right, second chunk
        map.set(5, 1, 0);
        let (tiles, chunks) = build_tiles(&map, [32, 32]);
        assert_eq!(tiles.len(), 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].first, chunks[0].count), (0, 1));
        assert_eq!((chunks[1].first, chunks[1].count), (1, 1));
        // Rows count from the bottom on the GPU.
        assert_eq!(tiles[0].tile, [0, 0]);
        assert_eq!(tiles[1].tile, [39 | (2 << 16), FLAG_FLIP_X]);
        assert_eq!(tiles[1].rect, [0.5, 0.5, 1.0, 1.0]);
        // Chunk bounds in map space.
        assert!((chunks[1].center.x - (32.0 + 4.0) * 0.5).abs() < 1e-6);
    }

    #[test]
    fn chunk_ranges_merge_neighbours() {
        let chunk = |first, count| Chunk {
            first,
            count,
            center: Vec3::ZERO,
            radius: 1.0,
        };
        let chunks = [
            chunk(0, 3),
            chunk(3, 2),
            chunk(5, 0),
            chunk(5, 4),
            chunk(9, 1),
        ];
        let mut ranges = vec![(100, 1)];
        let mut n = 0;
        let r = chunk_ranges(&chunks, &mut ranges, |_| {
            n += 1;
            n != 3
        });
        assert_eq!(r, (1, 2));
        assert_eq!(&ranges[1..], &[(0, 5), (9, 1)]);
    }
}
