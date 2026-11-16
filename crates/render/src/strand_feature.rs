//! The strand renderer (characters week R2): draws every [`StrandInstance`] with the shape of
//! the 2023-2025 shipped hair renderers (see `shaders/strands_raster.wgsl`).
//!
//! A frame, in the renderer's order:
//!
//! 1. `prepare` copies the instances out of the world: transform, level of detail, guides.
//! 2. `prepare_frame` uploads new grooms (strands shuffled once, so any prefix of them spreads
//!    over the whole groom and a level of detail is a prefix), the instances and the guides'
//!    offsets from rest.
//! 3. `compute`: every drawn point posed by its guides (`strands_pose.wgsl`), its fibre added to
//!    its instance's density volume (64³ cells over the groom).
//! 4. `draw_shadow`/`draw_inset_shadow`: the strands as hashed ribbons into the sun's maps.
//! 5. `before_scene` (the prepass, ambient occlusion and fog are ready): the farthest opaque
//!    depth of every 8x8 tile; every point placed on screen; the points a pixel can show shaded
//!    once (Marschner/Karis fibre model, the shadow maps, the light through the hair from the
//!    density volume, capsule occlusion, sky, point lights, fog), which leaves out whatever
//!    stands behind a hill or a wall and the far side of a coat whose body fills whole tiles;
//!    the segments counted per tile (those in front of something in it), summed, the tiles
//!    listed and filled; each tile rasterized in compute with box-filtered coverage, four
//!    nearest fibres per pixel and the rest as one, tested against the prepass depth; then the
//!    hair's motion over the prepass's where it covers a pixel.
//! 6. `draw(Transparent)`: the hair over the scene, premultiplied, a quad per touched tile.
//!
//! Levels of detail ([`strands_wanted`]): up close every strand; further out a share of them,
//! each wider by the inverse share so the hair keeps its cover; far away as many as its pixels
//! can show apart, and enough of them that none is drawn wider than a pixel (a coat of 200 000
//! hairs twenty pixels tall draws a thousand, not twelve thousand); past a few pixels nothing.
//! The number follows the size on screen without steps: the last strand drawn comes in by
//! degrees. `StrandLod::Cards` and `Shells` draw strands for now (cards and shells come later);
//! `Hidden` draws nothing. Extra views (editor splits, portals, mirrors) draw no hair yet.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use engine_assets::Assets;
use engine_core::math::{Mat3, Vec3};
use engine_core::prelude::*;
use engine_world::HairGuidesPosed;

use crate::features::{FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase};
use crate::frame::ViewData;
use crate::settings::{RenderSettings, shade_skip};
use crate::strands::{StrandFiber, StrandGroom, StrandInstance, StrandLod};

const COMMON: &str = include_str!("shaders/strands_common.wgsl");
const POSE: &str = include_str!("shaders/strands_pose.wgsl");
const RASTER: &str = include_str!("shaders/strands_raster.wgsl");
const COMPOSITE: &str = include_str!("shaders/strands_composite.wgsl");
const SHADOW: &str = include_str!("shaders/strands_shadow.wgsl");

/// Instances drawn at once, nearest first.
pub(crate) const MAX_INSTANCES: usize = 32;
/// Cells along each side of an instance's density volume.
pub(crate) const VOXELS: u32 = 64;
/// Points drawn in a frame; past it every instance draws a smaller share.
pub(crate) const MAX_POINTS: usize = 2 << 20;
/// Tile side (px), as `STRAND_TILE` in strands_common.wgsl.
const TILE: u32 = 8;
const WORKGROUP: u32 = 256;
/// Segment references per tile list, at most and at least.
const MAX_REFS: u64 = 16 << 20;
const MIN_REFS: u64 = 1 << 20;
/// A groom smaller than this on screen (its bounds' height, px) is not drawn; at
/// [`FULL_PX`] and above every strand is.
const MIN_PX: f32 = 3.0;
const FULL_PX: f32 = 500.0;
/// The share of strands the near rule draws never goes below this (wider strands make up the
/// cover), and an instance's density volume is filled from at least this share of its groom
/// however few strands it draws.
const MIN_SHARE: f32 = 1.0 / 16.0;
/// Far away a groom draws this many strands for every pixel of the square its bounds are
/// tall: enough that every pixel of it blends several (with one a pixel a deer's coat a
/// hundred pixels tall is twice as grainy as with the near rule's five times as many, and its
/// skin shows between them; with four it is as smooth)...
const STRANDS_PER_PX2: f32 = 4.0;
/// (...and with `shade_skip::STRANDS_SPARSE` this many, for the PC to tell what the finer
/// number costs.)
const SPARSE_PER_PX2: f32 = 1.0;
/// ...and enough of them that its thickest fibre, widened to keep the cover, stays within
/// this half width (px): no strand is drawn wider than a pixel...
const MAX_WIDE_HALF_PX: f32 = 0.5;
/// ...and never fewer than this.
const MIN_STRANDS: f32 = 16.0;
/// Strand slots an instance takes for the strands it draws by its pixels, and the most it
/// keeps before it gives some back (both times the strands drawn): room for the number to
/// drift with the size on screen while every strand's slot, and its history, stays put.
const SLOT_ROOM: f32 = 1.25;
const SLOT_KEEP: f32 = 1.6;
/// How opaque one fibre is across its width: hair, fur.
const OPACITY: [f32; 2] = [0.85, 0.9];
/// Strands cast into the cascades: one in this many, as much wider.
const CASCADE_SHARE: u32 = 4;
/// Grooms nobody drew in this many frames leave the GPU.
const EVICT_FRAMES: u64 = 600;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GpuParams {
    size: [u32; 4],
    misc: [u32; 4],
    tiles: [u32; 4],
    screen: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default, Debug)]
struct GpuInstance {
    rows: [[f32; 4]; 3],
    inv: [[f32; 4]; 3],
    grid: [f32; 4],
    tint: [f32; 4],
    first: [u32; 4],
    out: [u32; 4],
    more: [u32; 4],
    sphere: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GpuInfo {
    color: [u32; 4],
    follow: [u32; 4],
}

/// A groom as uploaded: where its strands are in the shared arrays.
struct GroomSlot {
    version: u64,
    /// Strands shuffled, as uploaded.
    rest: Vec<[f32; 4]>,
    info: Vec<GpuInfo>,
    /// Guides' rest points (not shuffled: `follow` indexes them).
    guides: Vec<Vec3>,
    points_per: u32,
    strands: u32,
    fiber: StrandFiber,
    thin: bool,
    /// Model-space bounds of the rest points (with their radii).
    lo: Vec3,
    hi: Vec3,
    /// The thickest fibre's radius (m).
    max_radius: f32,
    first_point: u32,
    first_strand: u32,
    last_seen: u64,
}

/// One instance this frame, as `prepare` copied it.
struct Draw {
    entity: Entity,
    groom: u32,
    /// Model -> camera-relative.
    rotation_scale: Mat3,
    translation: Vec3,
    tint: [f32; 3],
    width: f32,
    /// Strands to draw ([`strands_wanted`]), and whether the near rule set the number.
    wanted: f32,
    stepped: bool,
    distance: f32,
    /// Posed guide points, when they match the groom.
    guides: Option<Vec<Vec3>>,
}

/// What the frame draws, as `prepare_frame` laid it out.
struct FrameDraws {
    instances: Vec<GpuInstance>,
    /// Points drawn by the largest instance (the point passes' dispatch width), and the
    /// points of the instance that poses the most to fill its volume (the pose pass's).
    widest: u32,
    widest_pose: u32,
    tiles: u32,
    tiles_x: u32,
    tiles_y: u32,
    /// Points and strands drawn, and the strands in the drawn instances' grooms.
    points: u64,
    strands: u64,
    groom_strands: u64,
}

struct Pipelines {
    pose_layout: wgpu::BindGroupLayout,
    raster_layout: wgpu::BindGroupLayout,
    args_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    shadow_layout: wgpu::BindGroupLayout,
    pose: wgpu::ComputePipeline,
    tile_depth: wgpu::ComputePipeline,
    project: wgpu::ComputePipeline,
    shade: wgpu::ComputePipeline,
    count: wgpu::ComputePipeline,
    scan_tiles: wgpu::ComputePipeline,
    scan_blocks: wgpu::ComputePipeline,
    scan_finish: wgpu::ComputePipeline,
    args: wgpu::ComputePipeline,
    fill: wgpu::ComputePipeline,
    raster: wgpu::ComputePipeline,
    composite: wgpu::RenderPipeline,
    motion: wgpu::RenderPipeline,
    shadow_cascade: wgpu::RenderPipeline,
    shadow_inset: wgpu::RenderPipeline,
}

/// Buffers sized for the frame's points, tiles and grooms.
#[derive(Default)]
struct Buffers {
    params: Option<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    rest: Option<wgpu::Buffer>,
    info: Option<wgpu::Buffer>,
    guide_offsets: Option<wgpu::Buffer>,
    guide_capacity: u64,
    posed: Option<wgpu::Buffer>,
    history: Option<wgpu::Buffer>,
    lit: Option<wgpu::Buffer>,
    point_capacity: u64,
    voxels: Option<wgpu::Buffer>,
    voxel_capacity: u64,
    tiles: Option<wgpu::Buffer>,
    tile_capacity: u64,
    refs: Option<wgpu::Buffer>,
    ref_capacity: u64,
    args: Option<wgpu::Buffer>,
    color: Option<(wgpu::Texture, wgpu::TextureView)>,
    motion: Option<(wgpu::Texture, wgpu::TextureView)>,
    size: (u32, u32),
}

struct Groups {
    pose: wgpu::BindGroup,
    raster: wgpu::BindGroup,
    args: wgpu::BindGroup,
    composite: wgpu::BindGroup,
    shadow: wgpu::BindGroup,
}

/// The strand renderer. Registered by the render plugin.
#[derive(Default)]
pub(crate) struct StrandFeature {
    pipelines: Option<Pipelines>,
    grooms: HashMap<u32, GroomSlot>,
    /// The shared groom arrays need building again (a groom came, changed or left).
    grooms_dirty: bool,
    /// Grooms waiting for `prepare_frame` (new or changed this frame).
    incoming: Vec<(u32, u64, StrandGroom)>,
    draws: Option<Vec<Draw>>,
    /// Last frame's layout (entity, groom, point slots): history is valid while it holds.
    layout: Vec<(Entity, u32, u32)>,
    /// Strand slots each instance had last frame ([`strand_slots`]).
    slots: HashMap<Entity, u32>,
    buffers: Buffers,
    groups: Option<Groups>,
    frame: Option<FrameDraws>,
    frame_index: u32,
    frames: u64,
    stats: FeatureStats,
    warned: bool,
}

impl StrandFeature {
    fn upload_groom(&mut self, index: u32, version: u64, groom: StrandGroom) {
        let per = groom.points_per as usize;
        let strands = groom.strand_count();
        let order = spread_order(&groom);
        let default_rough = match groom.fiber {
            StrandFiber::Human => 0.33,
            StrandFiber::Fur => 0.42,
        };
        let mut rest = Vec::with_capacity(groom.points.len());
        let mut info = Vec::with_capacity(strands);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut max_radius = 0.0f32;
        for &s in &order {
            let s = s as usize;
            for p in &groom.points[s * per..(s + 1) * per] {
                let v = Vec3::new(p[0], p[1], p[2]);
                lo = lo.min(v - p[3]);
                hi = hi.max(v + p[3]);
                max_radius = max_radius.max(p[3]);
                rest.push(*p);
            }
            let root = groom.roots[s];
            let tip =
                groom
                    .tips
                    .get(s)
                    .copied()
                    .unwrap_or([root[0], root[1], root[2], default_rough]);
            let pack = |a: f32, b: f32| f16_bits(a) | (f16_bits(b) << 16);
            let follow = groom.follow.get(s).copied().unwrap_or_default();
            let unorm = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u32;
            info.push(GpuInfo {
                color: [
                    pack(root[0], root[1]),
                    pack(root[2], root[3]),
                    pack(tip[0], tip[1]),
                    pack(tip[2], tip[3]),
                ],
                follow: [
                    follow.guides[0],
                    follow.guides[1],
                    follow.guides[2],
                    unorm(follow.weights[0]) | (unorm(follow.weights[1]) << 16),
                ],
            });
        }
        if rest.is_empty() {
            (lo, hi) = (Vec3::ZERO, Vec3::ZERO);
        }
        self.grooms.insert(
            index,
            GroomSlot {
                version,
                rest,
                info,
                guides: groom
                    .guides
                    .iter()
                    .map(|g| Vec3::new(g[0], g[1], g[2]))
                    .collect(),
                points_per: groom.points_per,
                strands: strands as u32,
                fiber: groom.fiber,
                thin: groom.thin,
                lo,
                hi,
                max_radius,
                first_point: 0,
                first_strand: 0,
                last_seen: self.frames,
            },
        );
        self.grooms_dirty = true;
    }

    /// The groom arrays again, every groom in turn.
    fn rebuild_grooms(&mut self, gpu: &GpuShared) {
        let mut rest: Vec<[f32; 4]> = Vec::new();
        let mut info: Vec<GpuInfo> = Vec::new();
        let mut keys: Vec<u32> = self.grooms.keys().copied().collect();
        keys.sort_unstable();
        for k in keys {
            let g = self.grooms.get_mut(&k).expect("listed");
            g.first_point = rest.len() as u32;
            g.first_strand = info.len() as u32;
            rest.extend_from_slice(&g.rest);
            info.extend_from_slice(&g.info);
        }
        if rest.is_empty() {
            rest.push([0.0; 4]);
            info.push(GpuInfo::default());
        }
        use wgpu::util::DeviceExt;
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        self.buffers.rest = Some(gpu.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("strand rest points"),
                contents: bytemuck::cast_slice(&rest),
                usage: storage,
            },
        ));
        self.buffers.info = Some(gpu.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("strand info"),
                contents: bytemuck::cast_slice(&info),
                usage: storage,
            },
        ));
        self.groups = None;
        self.grooms_dirty = false;
    }
}

/// `v` as a half float's bits (round to nearest; tiny values flush to 0, huge ones clamp), as
/// WGSL's `unpack2x16float` reads them.
fn f16_bits(v: f32) -> u32 {
    let x = v.to_bits();
    let sign = (x >> 16) & 0x8000;
    let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
    let mant = x & 0x7f_ffff;
    if v.is_nan() {
        0
    } else if exp <= 0 {
        sign
    } else if exp >= 31 {
        sign | 0x7bff
    } else {
        (sign | ((exp as u32) << 10) | (mant >> 13)) + ((mant >> 12) & 1)
    }
}

/// A buffer of at least `size` bytes in `slot` (a power of two, grown when short): true when
/// it was made again.
fn ensure(
    gpu: &GpuShared,
    slot: &mut Option<wgpu::Buffer>,
    capacity: &mut u64,
    size: u64,
    label: &'static str,
    usage: wgpu::BufferUsages,
) -> bool {
    if slot.is_some() && *capacity >= size {
        return false;
    }
    let size = size.max(256).next_power_of_two();
    *slot = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    }));
    *capacity = size;
    true
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, storage: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: if storage {
            wgpu::ShaderStages::COMPUTE
        } else {
            wgpu::ShaderStages::FRAGMENT
        },
        ty: if storage {
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            }
        } else {
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            }
        },
        count: None,
    }
}

/// The WGSL of a strand shader: the shared types, then its own.
pub(crate) fn strand_source(own: &str) -> String {
    format!("{COMMON}\n{own}")
}

/// Every strand shader composed as the renderer compiles it, for validation tests.
#[cfg(test)]
pub(crate) fn composed_shaders() -> Vec<(&'static str, String)> {
    use crate::features::compose_shader;
    vec![
        ("strands pose", compose_shader(&strand_source(POSE), false)),
        (
            "strands raster",
            compose_shader(&strand_source(RASTER), true),
        ),
        (
            "strands composite",
            compose_shader(&strand_source(COMPOSITE), true),
        ),
        (
            "strands shadow",
            compose_shader(&strand_source(SHADOW), false),
        ),
    ]
}

impl Pipelines {
    fn new(gpu: &GpuShared) -> Self {
        use wgpu::ShaderStages as S;
        let c = S::COMPUTE;
        let pose_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands pose"),
            entries: &[
                uniform_entry(0, c),
                storage_entry(1, c, true),
                storage_entry(2, c, true),
                storage_entry(3, c, true),
                storage_entry(4, c, true),
                storage_entry(5, c, false),
                storage_entry(6, c, false),
            ],
        });
        let raster_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands raster"),
            entries: &[
                uniform_entry(0, c),
                storage_entry(1, c, true),
                storage_entry(2, c, true),
                storage_entry(3, c, true),
                storage_entry(4, c, true),
                storage_entry(5, c, false),
                storage_entry(6, c, false),
                storage_entry(7, c, false),
                storage_entry(8, c, false),
                texture_entry(10, true),
                texture_entry(11, true),
            ],
        });
        let args_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands args"),
            entries: &[storage_entry(0, c, false)],
        });
        let composite_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands composite"),
            entries: &[
                uniform_entry(0, S::VERTEX_FRAGMENT),
                storage_entry(1, S::VERTEX, true),
                texture_entry(2, false),
                texture_entry(3, false),
            ],
        });
        let shadow_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands shadow"),
            entries: &[
                uniform_entry(0, S::VERTEX),
                storage_entry(1, S::VERTEX, true),
                storage_entry(2, S::VERTEX, true),
            ],
        });

        let pose_module = gpu.shader("strands pose", &strand_source(POSE), false);
        let raster_module = gpu.shader("strands raster", &strand_source(RASTER), true);
        let composite_module = gpu.shader("strands composite", &strand_source(COMPOSITE), true);
        let shadow_module = gpu.shader("strands shadow", &strand_source(SHADOW), false);

        let pose_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands pose"),
            bind_group_layouts: &[Some(&pose_layout)],
            immediate_size: 0,
        });
        let raster_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands raster"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&raster_layout)],
            immediate_size: 0,
        });
        let args_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands args"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&raster_layout),
                Some(&args_layout),
            ],
            immediate_size: 0,
        });
        let composite_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands composite"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&composite_layout)],
            immediate_size: 0,
        });
        let shadow_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&shadow_layout)],
            immediate_size: 0,
        });
        let compute = |layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, entry: &str| {
            gpu.compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pose = compute(&pose_pl, &pose_module, "cs_pose");
        let tile_depth = compute(&raster_pl, &raster_module, "cs_tile_depth");
        let project = compute(&raster_pl, &raster_module, "cs_project");
        let shade = compute(&raster_pl, &raster_module, "cs_shade");
        let count = compute(&raster_pl, &raster_module, "cs_count");
        let scan_tiles = compute(&raster_pl, &raster_module, "cs_scan_tiles");
        let scan_blocks = compute(&raster_pl, &raster_module, "cs_scan_blocks");
        let scan_finish = compute(&raster_pl, &raster_module, "cs_scan_finish");
        let args = compute(&args_pl, &raster_module, "cs_args");
        let fill = compute(&raster_pl, &raster_module, "cs_fill");
        let raster = compute(&raster_pl, &raster_module, "cs_raster");

        let tile_pipeline = |fs: &str,
                             format: wgpu::TextureFormat,
                             blend: Option<wgpu::BlendState>,
                             depth: bool| {
            gpu.render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&composite_pl),
                vertex: wgpu::VertexState {
                    module: &composite_module,
                    entry_point: Some("vs_tile"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: depth.then(|| wgpu::DepthStencilState {
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    ..gpu.depth_state(false)
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &composite_module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let composite = tile_pipeline(
            "fs_composite",
            gpu.formats.hdr,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            true,
        );
        let motion = tile_pipeline("fs_motion", gpu.formats.motion, None, false);
        let shadow = |vs: &str| {
            gpu.render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(vs),
                layout: Some(&shadow_pl),
                vertex: wgpu::VertexState {
                    module: &shadow_module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(gpu.shadow_depth_state()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shadow_module,
                    entry_point: Some("fs_shadow"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            shadow_cascade: shadow("vs_shadow_cascade"),
            shadow_inset: shadow("vs_shadow_inset"),
            pose_layout,
            raster_layout,
            args_layout,
            composite_layout,
            shadow_layout,
            pose,
            tile_depth,
            project,
            shade,
            count,
            scan_tiles,
            scan_blocks,
            scan_finish,
            args,
            fill,
            raster,
            composite,
            motion,
        }
    }
}

/// The share of a groom's strands the near rule draws when its bounds are `px` pixels tall:
/// all of them from [`FULL_PX`], fewer below (in steps of 1/16, so the layout seldom changes),
/// none under [`MIN_PX`].
pub(crate) fn strand_share(px: f32) -> f32 {
    if px < MIN_PX {
        return 0.0;
    }
    let share = (px / FULL_PX).clamp(MIN_SHARE, 1.0);
    (share * 16.0).ceil() / 16.0
}

/// The order a groom's strands are stored in: any first n of them spread evenly over the
/// groom, so a groom that draws a share of its strands covers every part of itself alike.
///
/// The roots are sorted along a curve that fills their bounds (Morton order: neighbors on the
/// curve are neighbors in space), and the strands are taken from that line at evenly spaced
/// places, halving the spaces each round (the bit-reversed index). The first n are then one
/// strand from each of n patches of about the same number of roots, where a shuffle (as it was
/// until 17 Nov 2026) leaves some patches bare and others crowded: at two strands a pixel,
/// which is what a coat far away draws, that showed as a mottled pelt.
pub(crate) fn spread_order(groom: &StrandGroom) -> Vec<u32> {
    let per = (groom.points_per as usize).max(1);
    let strands = groom.strand_count();
    let root = |s: usize| {
        let p = groom.points[s * per];
        Vec3::new(p[0], p[1], p[2])
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for s in 0..strands {
        lo = lo.min(root(s));
        hi = hi.max(root(s));
    }
    // 21 bits an axis, interleaved.
    let scale = Vec3::splat(2_097_151.0) / (hi - lo).max(Vec3::splat(1e-9));
    let part = |v: f32| {
        let mut x = (v.clamp(0.0, 2_097_151.0) as u64) & 0x1f_ffff;
        x = (x | (x << 32)) & 0x001f_0000_0000_ffff;
        x = (x | (x << 16)) & 0x001f_0000_ff00_00ff;
        x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
        x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
        (x | (x << 2)) & 0x1249_2492_4924_9249
    };
    let mut line: Vec<(u64, u32)> = (0..strands)
        .map(|s| {
            let q = (root(s) - lo) * scale;
            (part(q.x) | (part(q.y) << 1) | (part(q.z) << 2), s as u32)
        })
        .collect();
    line.sort_unstable();
    let bits = strands.next_power_of_two().trailing_zeros();
    let mut order = Vec::with_capacity(strands);
    for i in 0..(1u64 << bits) {
        let at = if bits == 0 {
            0
        } else {
            (i.reverse_bits() >> (64 - bits)) as usize
        };
        if at < strands {
            order.push(line[at].1);
        }
    }
    order
}

/// The near rule's number: [`strand_share`] of a groom's `strands`, whole.
fn near_strands(strands: u32, px: f32) -> f32 {
    let all = strands as f32;
    (all * strand_share(px)).ceil().min(all)
}

/// How many of a groom's `strands` are drawn when its bounds are `px` pixels tall and its
/// thickest fibre `half_px` pixels wide either side of its axis (0: not drawn), and whether
/// the near rule set the number (a whole number that changes in steps).
///
/// The near rule ([`strand_share`]) thins a groom by its height alone, and never below a
/// sixteenth: right for a head of hair, but a coat of 200 000 hairs on an animal twenty
/// pixels tall then draws 12 500 of them into a few hundred pixels, and a herd of those was
/// most of the frame (30 ms on an RTX 5080, 16 Nov 2026). So far away the pixels set the
/// number: [`STRANDS_PER_PX2`] strands for every pixel of the square the bounds are tall, and
/// as many as it takes for the widened strands to keep the groom's cover without one of them
/// passing a pixel in width ([`MAX_WIDE_HALF_PX`]). Whichever rule asks for fewer wins: a head
/// of hair (20 000 strands) stays the near rule's down to 18 px, a coat of 200 000 hairs
/// down to about 110 px (at 50 px it draws 10 000 strands for the near rule's 12 500, at
/// 30 px 3 600, at 20 px 1 600, each as much wider).
pub(crate) fn strands_wanted(strands: u32, px: f32, half_px: f32, per_px2: f32) -> (f32, bool) {
    let near = near_strands(strands, px);
    if near <= 0.0 {
        return (0.0, true);
    }
    let all = strands as f32;
    let cover = all * half_px.max(0.0) / MAX_WIDE_HALF_PX;
    let seen = (per_px2 * px * px).max(cover).max(MIN_STRANDS);
    if seen < near {
        (seen.max(1.0), false)
    } else {
        (near, true)
    }
}

/// `count` strands or fewer, in steps of a sixteenth of the power of two under it (whole
/// numbers under 32): a number that stands still while `count` drifts a little.
fn stepped_count(count: f32) -> f32 {
    let step = 2f32.powi(count.max(1.0).log2().floor() as i32 - 4).max(1.0);
    ((count / step).floor() * step).max(1.0)
}

/// Strand slots for an instance that draws `drawn` of its groom's `strands` by its pixels,
/// having had `before` last frame: the same while they hold the strands drawn without much
/// to spare, else the strands drawn and some room.
fn strand_slots(drawn: u32, strands: u32, before: Option<u32>) -> u32 {
    if let Some(before) = before
        && before >= drawn
        && before <= strands
        && before as f32 <= drawn as f32 * SLOT_KEEP
    {
        return before;
    }
    ((drawn as f32 * SLOT_ROOM).ceil() as u32).clamp(drawn, strands.max(drawn))
}

impl RenderFeature for StrandFeature {
    fn name(&self) -> &str {
        "strands"
    }

    fn order(&self) -> i32 {
        // After the other transparent features' opaque work, before particles.
        50
    }

    fn init(&mut self, gpu: &GpuShared) {
        if gpu.warm_pipelines {
            self.pipelines = Some(Pipelines::new(gpu));
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        _gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
        if view.extra_view {
            return;
        }
        self.draws = None;
        self.frames += 1;
        if settings.debug_skip & shade_skip::STRANDS != 0 {
            // (No draws, not no word: the slots and their history end here.)
            self.draws = Some(Vec::new());
            return;
        }
        let near_count = settings.debug_skip
            & (shade_skip::STRANDS_OLD_WAY | shade_skip::STRANDS_NEAR_COUNT)
            != 0;
        let per_px2 = if settings.debug_skip & shade_skip::STRANDS_SPARSE != 0 {
            SPARSE_PER_PX2
        } else {
            STRANDS_PER_PX2
        };
        let mut q = world.query::<(
            Entity,
            &StrandInstance,
            &GlobalTransform,
            Option<&HairGuidesPosed>,
        )>();
        let Some(assets) = world.get_resource::<Assets<StrandGroom>>() else {
            return;
        };
        let mut draws = Vec::new();
        let mut incoming = Vec::new();
        let origin = view.origin;
        let px_per_m = view.proj.y_axis.y * view.height as f32 * 0.5;
        for (entity, inst, gt, posed) in q.iter(world) {
            if !inst.visible || inst.lod == Some(StrandLod::Hidden) {
                continue;
            }
            let Some(groom) = assets.get(inst.groom) else {
                continue;
            };
            let index = inst.groom.index();
            let version = assets.version(inst.groom);
            let known = self
                .grooms
                .get(&index)
                .is_some_and(|g| g.version == version)
                || incoming
                    .iter()
                    .any(|(i, _, _): &(u32, u64, StrandGroom)| *i == index);
            if !known {
                if !groom.is_valid() {
                    if !self.warned {
                        engine_core::log::warn!(
                            "strands: a groom's arrays don't agree with its points_per; not drawn"
                        );
                        self.warned = true;
                    }
                    continue;
                }
                incoming.push((index, version, groom.clone()));
            }
            // Bounds from the groom itself when it is new this frame.
            let (lo, hi, max_radius) =
                match self.grooms.get(&index).filter(|g| g.version == version) {
                    Some(g) => (g.lo, g.hi, g.max_radius),
                    None => {
                        let mut lo = Vec3::splat(f32::MAX);
                        let mut hi = Vec3::splat(f32::MIN);
                        let mut max_radius = 0.0f32;
                        for p in &groom.points {
                            let v = Vec3::new(p[0], p[1], p[2]);
                            lo = lo.min(v - p[3]);
                            hi = hi.max(v + p[3]);
                            max_radius = max_radius.max(p[3]);
                        }
                        (lo, hi, max_radius)
                    }
                };
            if lo.x > hi.x {
                continue;
            }
            let rotation_scale = Mat3::from_quat(gt.rotation()) * Mat3::from_diagonal(gt.scale());
            let translation = (gt.translation() - origin).as_vec3();
            let center = rotation_scale * ((lo + hi) * 0.5) + translation;
            let radius = (hi - lo).length() * 0.5 * gt.scale().max_element();
            // Hair just off screen still casts its shadow on screen.
            if !view.frustum.intersects_sphere(center, radius + 0.5) {
                continue;
            }
            let distance = center.length().max(view.near);
            // Pixels per meter where the groom is.
            let per_m = if view.orthographic {
                px_per_m
            } else {
                px_per_m / distance
            };
            let px = radius * 2.0 * per_m;
            let width = inst.width_scale.max(0.0);
            let strands = groom.strand_count() as u32;
            let (wanted, stepped) = if near_count {
                (near_strands(strands, px), true)
            } else {
                // (A fibre is as thick as groomed whatever the entity's scale: `cs_project`.)
                strands_wanted(strands, px, max_radius * width * per_m, per_px2)
            };
            if wanted <= 0.0 {
                continue;
            }
            let guides = posed
                .filter(|p| !groom.guides.is_empty() && p.points.len() == groom.guides.len())
                .map(|p| p.points.clone());
            draws.push(Draw {
                entity,
                groom: index,
                rotation_scale,
                translation,
                tint: inst.tint,
                width,
                wanted,
                stepped,
                distance,
                guides,
            });
        }
        draws.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        draws.truncate(MAX_INSTANCES);
        self.incoming = incoming;
        self.draws = Some(draws);
    }

    fn prepare_frame(
        &mut self,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
        _diagnostics: &mut engine_core::diagnostics::Diagnostics,
    ) {
        if view.extra_view {
            return;
        }
        self.frame = None;
        self.stats = FeatureStats::default();
        for (index, version, groom) in std::mem::take(&mut self.incoming) {
            self.upload_groom(index, version, groom);
        }
        let Some(draws) = self.draws.take() else {
            return;
        };
        let frames = self.frames;
        for d in &draws {
            if let Some(g) = self.grooms.get_mut(&d.groom) {
                g.last_seen = frames;
            }
        }
        let before = self.grooms.len();
        self.grooms
            .retain(|_, g| g.last_seen + EVICT_FRAMES >= frames);
        if self.grooms.len() != before {
            self.grooms_dirty = true;
        }
        if draws.is_empty() {
            self.layout.clear();
            self.slots.clear();
            return;
        }
        if self.pipelines.is_none() {
            self.pipelines = Some(Pipelines::new(gpu));
        }
        if self.grooms_dirty || self.buffers.rest.is_none() {
            self.rebuild_grooms(gpu);
        }

        // Points drawn per instance, scaled down together past the budget.
        let wanted: usize = draws
            .iter()
            .filter_map(|d| {
                let g = self.grooms.get(&d.groom)?;
                Some(d.wanted.ceil() as usize * g.points_per as usize)
            })
            .sum();
        let budget = if wanted > MAX_POINTS {
            MAX_POINTS as f32 / wanted as f32
        } else {
            1.0
        };
        // Points the budget leaves over: the room instances may keep beyond what they draw.
        let mut spare = MAX_POINTS.saturating_sub(wanted) as u64;

        let mut instances = Vec::with_capacity(draws.len());
        let mut layout = Vec::with_capacity(draws.len());
        let mut slots_now = HashMap::with_capacity(draws.len());
        let mut offsets: Vec<[f32; 4]> = Vec::new();
        let mut out = 0u32;
        let mut strand_base = 0u32;
        let (mut widest, mut widest_pose) = (0u32, 0u32);
        let (mut drawn_points, mut groom_strands) = (0u64, 0u64);
        for d in &draws {
            let Some(g) = self.grooms.get(&d.groom) else {
                continue;
            };
            // The strands drawn, as a number with a fraction: the last of them is drawn that
            // share of its width, so it comes in by degrees as the groom grows on screen.
            let most = g.strands.max(1);
            let mut count = (d.wanted * budget).clamp(1.0, most as f32);
            let mut drawn = (count.ceil() as u32).clamp(1, most);
            // Its slots: with room where the number follows the pixels, so they stay put
            // while it drifts...
            let mut slots = drawn;
            let mut roomy = false;
            if !d.stepped && budget >= 1.0 {
                let wide = strand_slots(drawn, g.strands, self.slots.get(&d.entity).copied());
                let room = (wide - drawn) as u64 * g.points_per as u64;
                if room <= spare {
                    spare -= room;
                    slots = wide;
                    roomy = true;
                }
            }
            // ...and where there is no room (the budget is spent, and every number moves with
            // every other), the number goes in steps, sixteen to a doubling, so the slots
            // stand still between them as they do under the near rule.
            if !roomy && (!d.stepped || budget < 1.0) {
                count = stepped_count(count);
                drawn = (count as u32).clamp(1, most);
                slots = drawn;
            }
            let last = (count - (drawn - 1) as f32).clamp(0.0, 1.0);
            let points = drawn * g.points_per;
            // The strands that fill its density volume: those drawn, and where they are few
            // (far away) as many as the near rule's least share, as it was filled before the
            // pixels set the number. A few hundred wide strands would leave most of a coat's
            // cells empty and the rest too dense: speckled, too bright self-shadowing.
            let filling = drawn.max(((g.strands as f32 * MIN_SHARE).ceil() as u32).min(most));
            slots_now.insert(d.entity, slots);
            let guide_base = match &d.guides {
                Some(posed) if posed.len() == g.guides.len() => {
                    let base = offsets.len() as u32;
                    offsets.extend(
                        posed
                            .iter()
                            .zip(&g.guides)
                            .map(|(p, r)| [p.x - r.x, p.y - r.y, p.z - r.z, 0.0]),
                    );
                    base
                }
                _ => u32::MAX,
            };
            // The density volume: the groom's bounds, grown for hair the guides move.
            let size = (g.hi - g.lo).max_element() * 1.3 + 1e-3;
            let cell = size / VOXELS as f32;
            let corner = (g.lo + g.hi) * 0.5 - Vec3::splat(size * 0.5);
            let m = d.rotation_scale;
            let inv = m.inverse();
            let row = |r: usize, w: f32| [m.row(r).x, m.row(r).y, m.row(r).z, w];
            let inv_row = |r: usize| [inv.row(r).x, inv.row(r).y, inv.row(r).z, 0.0];
            let center = m * ((g.lo + g.hi) * 0.5) + d.translation;
            let radius = (g.hi - g.lo).length()
                * 0.5
                * m.x_axis
                    .length()
                    .max(m.y_axis.length())
                    .max(m.z_axis.length());
            // Fewer strands, each wider: the same cover.
            let width = d.width * (g.strands as f32 / count);
            let k = instances.len() as u32;
            instances.push(GpuInstance {
                rows: [
                    row(0, d.translation.x),
                    row(1, d.translation.y),
                    row(2, d.translation.z),
                ],
                inv: [inv_row(0), inv_row(1), inv_row(2)],
                grid: [corner.x, corner.y, corner.z, cell],
                tint: [d.tint[0], d.tint[1], d.tint[2], width],
                first: [g.first_point, g.first_strand, g.points_per, drawn],
                out: [out, guide_base, last.to_bits(), k * VOXELS.pow(3)],
                more: [
                    strand_base,
                    matches!(g.fiber, StrandFiber::Fur) as u32,
                    filling,
                    g.thin as u32,
                ],
                sphere: [center.x, center.y, center.z, radius],
            });
            layout.push((d.entity, d.groom, slots * g.points_per));
            out += slots * g.points_per;
            strand_base += drawn;
            widest = widest.max(points);
            widest_pose = widest_pose.max(filling * g.points_per);
            drawn_points += points as u64;
            groom_strands += g.strands as u64;
            self.stats.instances += 1;
        }
        self.slots = slots_now;
        if instances.is_empty() {
            self.layout.clear();
            return;
        }
        // Instances keep their slots while those before them do: from the first whose slots
        // changed on, last frame's positions lie elsewhere and are no history (a far coat
        // that takes new slots costs the hair nearer than it nothing).
        let kept = layout
            .iter()
            .zip(&self.layout)
            .take_while(|(now, before)| now == before)
            .count() as u32;
        self.layout = layout;
        self.frame_index = self.frame_index.wrapping_add(1).max(2);

        let width = view.width.max(1);
        let height = view.height.max(1);
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);
        let tiles = tiles_x * tiles_y;
        let blocks = tiles.div_ceil(1024);
        // Counts, starts, fill cursors, block sums, active list, then two counters, then the
        // farthest opaque depth per tile.
        let tile_words = tiles as u64 * 5 + blocks as u64 + 2;
        let ref_want = (drawn_points * 4).clamp(MIN_REFS, MAX_REFS);

        let b = &mut self.buffers;
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let mut grown = false;
        let points = out as u64;
        if b.posed.is_none() || b.point_capacity < points {
            let cap = points.max(4096).next_power_of_two();
            let make = |label: &'static str, stride: u64| {
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: cap * stride,
                    usage: storage,
                    mapped_at_creation: false,
                })
            };
            b.posed = Some(make("strand posed points", 16));
            b.history = Some(make("strand history", 16));
            b.lit = Some(make("strand lit points", 32));
            b.point_capacity = cap;
            grown = true;
        }
        let voxel_bytes = instances.len() as u64 * VOXELS.pow(3) as u64 * 4;
        grown |= ensure(
            gpu,
            &mut b.voxels,
            &mut b.voxel_capacity,
            voxel_bytes,
            "strand density",
            storage,
        );
        grown |= ensure(
            gpu,
            &mut b.tiles,
            &mut b.tile_capacity,
            tile_words * 4,
            "strand tiles",
            storage,
        );
        if b.refs.is_none() || b.ref_capacity < ref_want {
            let cap = ref_want.next_power_of_two().min(MAX_REFS);
            b.refs = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand tile references"),
                size: cap * 4,
                usage: storage,
                mapped_at_creation: false,
            }));
            b.ref_capacity = cap;
            grown = true;
        }
        let guide_bytes = (offsets.len().max(1) * 16) as u64;
        grown |= ensure(
            gpu,
            &mut b.guide_offsets,
            &mut b.guide_capacity,
            guide_bytes,
            "strand guide offsets",
            storage,
        );
        if b.params.is_none() {
            b.params = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand params"),
                size: std::mem::size_of::<GpuParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            b.instances = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand instances"),
                size: (MAX_INSTANCES * std::mem::size_of::<GpuInstance>()) as u64,
                usage: storage,
                mapped_at_creation: false,
            }));
            b.args = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand indirect"),
                size: 32,
                usage: storage | wgpu::BufferUsages::INDIRECT,
                mapped_at_creation: false,
            }));
            grown = true;
        }
        if b.size != (width, height) || b.color.is_none() {
            let make = |label: &'static str| {
                let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let v = t.create_view(&Default::default());
                (t, v)
            };
            b.color = Some(make("strand color"));
            b.motion = Some(make("strand motion"));
            b.size = (width, height);
            grown = true;
        }

        let params = GpuParams {
            size: [
                tiles_x,
                tiles_y,
                b.ref_capacity as u32,
                instances.len() as u32,
            ],
            misc: [tiles, kept, self.frame_index, VOXELS],
            tiles: [tiles, tiles * 2, tiles * 3, tiles * 3 + blocks],
            screen: [width as f32, height as f32, OPACITY[0], OPACITY[1]],
        };
        let queue = &gpu.queue;
        queue.write_buffer(
            b.params.as_ref().expect("made"),
            0,
            bytemuck::bytes_of(&params),
        );
        queue.write_buffer(
            b.instances.as_ref().expect("made"),
            0,
            bytemuck::cast_slice(&instances),
        );
        if !offsets.is_empty() {
            queue.write_buffer(
                b.guide_offsets.as_ref().expect("made"),
                0,
                bytemuck::cast_slice(&offsets),
            );
        }
        if grown {
            self.groups = None;
        }
        if self.groups.is_none() {
            let p = self.pipelines.as_ref().expect("made");
            let b = &self.buffers;
            fn buf(o: &Option<wgpu::Buffer>) -> wgpu::BindingResource<'_> {
                o.as_ref().expect("made").as_entire_binding()
            }
            let entry = |binding: u32, resource| wgpu::BindGroupEntry { binding, resource };
            let pose = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands pose"),
                layout: &p.pose_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.instances)),
                    entry(2, buf(&b.rest)),
                    entry(3, buf(&b.info)),
                    entry(4, buf(&b.guide_offsets)),
                    entry(5, buf(&b.posed)),
                    entry(6, buf(&b.voxels)),
                ],
            });
            let color = &b.color.as_ref().expect("made").1;
            let motion = &b.motion.as_ref().expect("made").1;
            let raster = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands raster"),
                layout: &p.raster_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.instances)),
                    entry(2, buf(&b.info)),
                    entry(3, buf(&b.posed)),
                    entry(4, buf(&b.voxels)),
                    entry(5, buf(&b.history)),
                    entry(6, buf(&b.lit)),
                    entry(7, buf(&b.tiles)),
                    entry(8, buf(&b.refs)),
                    entry(10, wgpu::BindingResource::TextureView(color)),
                    entry(11, wgpu::BindingResource::TextureView(motion)),
                ],
            });
            let args = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands args"),
                layout: &p.args_layout,
                entries: &[entry(0, buf(&b.args))],
            });
            let composite = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands composite"),
                layout: &p.composite_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.tiles)),
                    entry(2, wgpu::BindingResource::TextureView(color)),
                    entry(3, wgpu::BindingResource::TextureView(motion)),
                ],
            });
            let shadow = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands shadow"),
                layout: &p.shadow_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.instances)),
                    entry(2, buf(&b.posed)),
                ],
            });
            self.groups = Some(Groups {
                pose,
                raster,
                args,
                composite,
                shadow,
            });
        }
        self.stats.draw_calls = 1;
        self.frame = Some(FrameDraws {
            instances,
            widest,
            widest_pose,
            tiles,
            tiles_x,
            tiles_y,
            points: drawn_points,
            strands: strand_base as u64,
            groom_strands,
        });
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        let voxels = self.buffers.voxels.as_ref().expect("made");
        encoder.clear_buffer(
            voxels,
            0,
            Some(f.instances.len() as u64 * VOXELS.pow(3) as u64 * 4),
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("strands pose"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&p.pose);
        pass.set_bind_group(0, &g.pose, &[]);
        pass.dispatch_workgroups(
            f.widest_pose.div_ceil(WORKGROUP),
            f.instances.len() as u32,
            1,
        );
    }

    fn shadow_moving(&self, map: usize, ctx: &FrameContext) -> crate::shadows::ShadowTiles {
        let mut tiles = crate::shadows::ShadowTiles::NONE;
        if map >= crate::shadows::MAX_CASCADES || ctx.view.extra_view {
            return tiles;
        }
        let (Some(f), Some(_), Some(_)) = (&self.frame, &self.pipelines, &self.groups) else {
            return tiles;
        };
        for inst in &f.instances {
            let s = inst.sphere;
            let center = Vec3::new(s[0], s[1], s[2]);
            if ctx.view.cascades.sphere_in_cascade(map, center, s[3]) {
                tiles.add_sphere(ctx.view, map, center, s[3]);
            }
        }
        tiles
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, ctx: &FrameContext) {
        if cascade >= crate::shadows::MAX_CASCADES || ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        let mut bound = false;
        for inst in &f.instances {
            let s = inst.sphere;
            if !ctx
                .view
                .cascades
                .sphere_in_cascade(cascade, Vec3::new(s[0], s[1], s[2]), s[3])
            {
                continue;
            }
            if !bound {
                pass.set_pipeline(&p.shadow_cascade);
                pass.set_bind_group(1, &g.shadow, &[]);
                bound = true;
            }
            let first = inst.more[0];
            let n = inst.first[3].div_ceil(CASCADE_SHARE);
            pass.draw(0..(inst.first[2] - 1) * 6, first..first + n);
        }
    }

    fn draw_inset_shadow(&self, pass: &mut wgpu::RenderPass<'_>, inset: usize, ctx: &FrameContext) {
        if ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g), Some(i)) = (
            &self.frame,
            &self.pipelines,
            &self.groups,
            ctx.view.shadow_insets.get(inset),
        ) else {
            return;
        };
        let mut bound = false;
        for inst in &f.instances {
            let s = inst.sphere;
            if !i.casts(Vec3::new(s[0], s[1], s[2]), s[3]) {
                continue;
            }
            if !bound {
                pass.set_pipeline(&p.shadow_inset);
                pass.set_bind_group(1, &g.shadow, &[]);
                bound = true;
            }
            let first = inst.more[0];
            pass.draw(0..(inst.first[2] - 1) * 6, first..first + inst.first[3]);
        }
    }

    fn before_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        motion: &wgpu::TextureView,
        ctx: &FrameContext,
    ) {
        if ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        let b = &self.buffers;
        encoder.clear_buffer(b.tiles.as_ref().expect("made"), 0, None);
        let args = b.args.as_ref().expect("made");
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("strands"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &g.raster, &[]);
            let points = (f.widest.div_ceil(WORKGROUP), f.instances.len() as u32, 1);
            let tiles = f.tiles.div_ceil(WORKGROUP);
            pass.set_pipeline(&p.tile_depth);
            pass.dispatch_workgroups(f.tiles_x.div_ceil(8), f.tiles_y.div_ceil(8), 1);
            pass.set_pipeline(&p.project);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            pass.set_pipeline(&p.shade);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            pass.set_pipeline(&p.count);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            pass.set_pipeline(&p.scan_tiles);
            pass.dispatch_workgroups(f.tiles.div_ceil(1024), 1, 1);
            pass.set_pipeline(&p.scan_blocks);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&p.scan_finish);
            pass.dispatch_workgroups(tiles, 1, 1);
            pass.set_pipeline(&p.args);
            pass.set_bind_group(2, &g.args, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&p.fill);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            if ctx.settings.debug_skip & shade_skip::STRAND_RASTER == 0 {
                pass.set_pipeline(&p.raster);
                pass.dispatch_workgroups_indirect(args, 0);
            }
        }
        // The hair's motion where it covers most of a pixel.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("strands motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: motion,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&p.motion);
        pass.set_bind_group(0, ctx.frame_group, &[]);
        pass.set_bind_group(1, &g.composite, &[]);
        pass.draw_indirect(args, 12);
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {
        if phase != RenderPhase::Transparent || ctx.view.extra_view {
            return;
        }
        let (Some(_), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        pass.set_pipeline(&p.composite);
        pass.set_bind_group(1, &g.composite, &[]);
        pass.draw_indirect(self.buffers.args.as_ref().expect("made"), 12);
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        let f = self.frame.as_ref();
        serde_json::json!({
            "grooms": self.grooms.len(),
            "instances": f.map_or(0, |f| f.instances.len()),
            // Drawn this frame, and the strands their grooms hold.
            "points": f.map_or(0, |f| f.points),
            "strands": f.map_or(0, |f| f.strands),
            "strands_in_grooms": f.map_or(0, |f| f.groom_strands),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strand_shaders_validate() {
        for (label, code) in composed_shaders() {
            crate::features::validate_wgsl(label, &code).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn gpu_layouts_match_wgsl() {
        assert_eq!(std::mem::size_of::<GpuParams>(), 64);
        assert_eq!(std::mem::size_of::<GpuInstance>(), 192);
        assert_eq!(std::mem::size_of::<GpuInfo>(), 32);
    }

    #[test]
    fn half_floats_round_trip() {
        for v in [0.0f32, 1.0, 0.5, 0.123, 3.75, 1000.0] {
            let b = f16_bits(v) as u16;
            let back = f32::from(HalfBits(b));
            assert!((back - v).abs() <= v * 1e-3, "{v} -> {back}");
        }
        assert_eq!(f16_bits(-2.0), 0xc000);
    }

    struct HalfBits(u16);
    impl From<HalfBits> for f32 {
        fn from(h: HalfBits) -> f32 {
            let h = h.0 as u32;
            let exp = (h >> 10) & 0x1f;
            let mant = h & 0x3ff;
            let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
            if exp == 0 {
                sign * mant as f32 * 2f32.powi(-24)
            } else {
                sign * (1.0 + mant as f32 / 1024.0) * 2f32.powi(exp as i32 - 15)
            }
        }
    }

    #[test]
    fn far_grooms_draw_a_share_of_their_strands() {
        assert_eq!(strand_share(1000.0), 1.0);
        assert_eq!(strand_share(FULL_PX), 1.0);
        assert_eq!(strand_share(1.0), 0.0);
        let mid = strand_share(100.0);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(strand_share(10.0) >= MIN_SHARE);
        // In steps of 1/16.
        assert_eq!((mid * 16.0).fract(), 0.0);
    }

    #[test]
    fn a_dense_coat_far_away_draws_by_its_pixels() {
        // A deer's coat: 200 000 hairs 0.31 mm thick at the root, bounds 2 m across.
        let (all, radius, size) = (200_000u32, 0.00031f32, 2.0f32);
        let at = |px: f32| strands_wanted(all, px, radius * px / size, STRANDS_PER_PX2);
        // Near: the near rule's sixteenths, as before.
        assert_eq!(at(600.0), (200_000.0, true));
        assert_eq!(at(450.0), ((all as f32 * strand_share(450.0)).ceil(), true));
        // Twenty pixels tall: four strands a pixel of its square, not the near rule's 12 500
        // (and with the sparse switch as few as keep the widest strand within a pixel).
        let (far, stepped) = at(20.0);
        assert!(!stepped);
        assert_eq!(far, 1600.0);
        assert_eq!((all as f32 * strand_share(20.0)).ceil(), 12_500.0);
        let sparse = strands_wanted(all, 20.0, radius * 20.0 / size, SPARSE_PER_PX2).0;
        assert!((1239.0..1241.0).contains(&sparse), "{sparse}");
        // A hundred and fifty: the near rule's again.
        assert_eq!(at(150.0), (62_500.0, true));
        // Under the smallest size nothing, as before.
        assert_eq!(at(MIN_PX * 0.9).0, 0.0);
        let mut before = 0.0;
        for px in [
            3.0f32, 4.5, 6.0, 12.0, 20.0, 45.0, 90.0, 150.0, 300.0, 380.0, 500.0,
        ] {
            let (n, stepped) = at(px);
            // Never more than the near rule asks, never fewer than the floor, and more the
            // larger the coat is on screen.
            assert!(n <= (all as f32 * strand_share(px)).ceil(), "{px}: {n}");
            assert!(n >= MIN_STRANDS && n >= before, "{px}: {n} after {before}");
            before = n;
            if !stepped {
                // By its pixels: no strand widened past a pixel, and the pixels' worth.
                let widened = radius * px / size * all as f32 / n;
                assert!(widened <= MAX_WIDE_HALF_PX * 1.001, "{px}: {widened} px");
                assert!(n >= STRANDS_PER_PX2 * px * px, "{px}: {n}");
            }
        }
        // A head of hair (20 000 strands) is the near rule's until it is small.
        assert!(strands_wanted(20_000, 200.0, 0.01, STRANDS_PER_PX2).1);
        assert!(strands_wanted(20_000, 60.0, 0.003, STRANDS_PER_PX2).1);
        assert!(strands_wanted(20_000, 20.0, 0.001, STRANDS_PER_PX2).1);
        assert!(!strands_wanted(20_000, 12.0, 0.0006, STRANDS_PER_PX2).1);
        // A groom of a few strands draws them all.
        assert_eq!(strands_wanted(10, 5.0, 0.0, STRANDS_PER_PX2), (1.0, true));
        assert_eq!(strands_wanted(0, 50.0, 0.0, STRANDS_PER_PX2).0, 0.0);
    }

    #[test]
    fn any_first_strands_spread_over_the_groom() {
        // Roots on a 64 x 64 grid (stored row by row), two points a strand.
        let groom_of = |roots: &[[f32; 2]]| {
            let mut g = StrandGroom {
                points_per: 2,
                ..Default::default()
            };
            for r in roots {
                g.points.push([r[0], r[1], 0.0, 1e-4]);
                g.points.push([r[0], r[1], 0.01, 1e-4]);
                g.roots.push([0.5, 0.5, 0.5, 0.0]);
            }
            g
        };
        let grid: Vec<[f32; 2]> = (0..4096)
            .map(|i| [(i % 64) as f32 / 63.0, (i / 64) as f32 / 63.0])
            .collect();
        let order = spread_order(&groom_of(&grid));
        let mut seen = order.clone();
        seen.sort_unstable();
        assert!(seen.iter().copied().eq(0..4096), "every strand once");
        // The first 16 strands: one in each of the 16 squares 16 roots across; the first 64:
        // one in each of the 64 squares 8 across; the first 100: one or two in each of those.
        for (first, across) in [(16usize, 16u32), (64, 8), (100, 8)] {
            let side = 64 / across;
            let mut held = vec![0u32; (side * side) as usize];
            for &s in &order[..first] {
                held[((s % 64) / across + (s / 64) / across * side) as usize] += 1;
            }
            let (least, most) = (held.iter().min().unwrap(), held.iter().max().unwrap());
            assert!(
                *least >= (first as u32) / (side * side)
                    && *most <= (first as u32).div_ceil(side * side),
                "the first {first}: {least} to {most} a square"
            );
        }
        // Any number of strands, wherever their roots: each once.
        let mut state = 12345u32;
        let mut rand = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32
        };
        let scattered: Vec<[f32; 2]> = (0..3001).map(|_| [rand(), rand() * 0.3]).collect();
        let mut seen = spread_order(&groom_of(&scattered));
        seen.sort_unstable();
        assert!(seen.iter().copied().eq(0..3001));
        assert!(spread_order(&groom_of(&[])).is_empty());
        assert_eq!(spread_order(&groom_of(&[[0.3, 0.1]])), [0]);
    }

    #[test]
    fn strand_slots_stay_while_they_fit() {
        // New: the strands drawn and some room.
        assert_eq!(strand_slots(1000, 200_000, None), 1250);
        // The number drifts: the slots stay.
        assert_eq!(strand_slots(1100, 200_000, Some(1250)), 1250);
        assert_eq!(strand_slots(1250, 200_000, Some(1250)), 1250);
        assert_eq!(strand_slots(800, 200_000, Some(1250)), 1250);
        // More strands than slots, or far fewer: made again.
        assert_eq!(strand_slots(1251, 200_000, Some(1250)), 1564);
        assert_eq!(strand_slots(700, 200_000, Some(1250)), 875);
        // Never more than the groom has, never fewer than drawn.
        assert_eq!(strand_slots(190_000, 200_000, None), 200_000);
        assert_eq!(strand_slots(16, 16, Some(40)), 16);
        assert_eq!(strand_slots(1, 1, None), 1);
        // Without room the number goes in steps, sixteen to a doubling, and never up.
        assert_eq!(stepped_count(1000.0), 992.0);
        assert_eq!(stepped_count(1023.9), 992.0);
        assert_eq!(stepped_count(1024.0), 1024.0);
        assert_eq!(stepped_count(87_500.0), 86_016.0);
        assert_eq!(stepped_count(31.9), 31.0);
        assert_eq!(stepped_count(1.0), 1.0);
    }
}
