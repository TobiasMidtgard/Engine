//! The render feature that draws characters: skin, eyes and hair, each with a depth prepass
//! (so ambient occlusion, reflections and anti-aliasing see them) and shadows.
//!
//! Each character keeps its own GPU buffers, rebuilt when its [`BuiltCharacter`] changes, and a
//! small uniform (`GpuCharacter`) refreshed every frame: where it is relative to the camera and
//! the colors its [`Appearance`] asks for. Hair sets each have a uniform and three storage
//! buffers (points, their attributes, and each hair's root) that the strand shaders expand into
//! ribbons.
//!
//! People with a `SkeletonPose` are skinned on the GPU: a pose buffer per character holds one
//! matrix per bone (from [`Rig::palette`]) for this frame and the last, the expression channel
//! weights from its `FaceControls`, and the gaze. Skin vertices carry four bones and weights;
//! hairs ride the skin vertex they grow from; eyes ride the head and turn to the gaze.
//!
//! Hair that moves also follows its guide (see [`crate::hair_motion`]): its set has a motion
//! buffer with every guide point's displacement this frame and the last, written each frame from
//! the person's `HairGuidesPosed`.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_render::features::{FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase};
use engine_render::frame::ViewData;
use engine_render::settings::RenderSettings;
use engine_render::shadows::SKY_OCCLUSION_VIEW;
use engine_render::wgpu;
use wgpu::util::DeviceExt;

use engine_world::animation::SkeletonPose;
use engine_world::character::{FaceControls, HandControls};
use engine_world::{HairGuides, HairGuidesPosed};

use crate::appearance::{Appearance, AppearanceExt, BeardStyle, HairStyle};
use crate::build::CharacterMeshes;
use crate::clothes::Cut;
use crate::face::{self, MAX_CHANNELS};
use crate::lod::{self, CharacterLod};
use crate::mesh::Landmarks;
use crate::rig::Rig;
use crate::strands::StrandKind;
use crate::{BuiltCharacter, CharacterDebug};

pub const COMMON_WGSL: &str = include_str!("shaders/common.wgsl");
pub const SKIN_WGSL: &str = include_str!("shaders/skin.wgsl");
pub const EYE_WGSL: &str = include_str!("shaders/eye.wgsl");
pub const STRAND_DATA_WGSL: &str = include_str!("shaders/strand_data.wgsl");
pub const STRANDS_WGSL: &str = include_str!("shaders/strands.wgsl");
pub const SHADOW_SKIN_WGSL: &str = include_str!("shaders/shadow_skin.wgsl");
pub const SHADOW_STRANDS_WGSL: &str = include_str!("shaders/shadow_strands.wgsl");

/// Full sources of each shader module (before `compose_shader`), with whether it needs the
/// frame library.
pub fn shader_sources() -> [(&'static str, String, bool); 5] {
    [
        (
            "character skin",
            format!("{COMMON_WGSL}\n{SKIN_WGSL}"),
            true,
        ),
        ("character eyes", format!("{COMMON_WGSL}\n{EYE_WGSL}"), true),
        (
            "character strands",
            format!("{COMMON_WGSL}\n{STRAND_DATA_WGSL}\n{STRANDS_WGSL}"),
            true,
        ),
        (
            "character skin shadow",
            format!("{COMMON_WGSL}\n{SHADOW_SKIN_WGSL}"),
            false,
        ),
        (
            "character strand shadow",
            format!("{COMMON_WGSL}\n{STRAND_DATA_WGSL}\n{SHADOW_STRANDS_WGSL}"),
            false,
        ),
    ]
}

/// Per-character uniform; must match `struct Character` in common.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuCharacter {
    pub model: [[f32; 4]; 4],
    pub prev_model: [[f32; 4]; 4],
    pub skin: [f32; 4],
    pub skin2: [f32; 4],
    pub look: [f32; 4],
    pub hair: [f32; 4],
    pub brows: [f32; 4],
    pub beard: [f32; 4],
    pub eye: [f32; 4],
    pub eye2: [f32; 4],
    pub rest_eye_l: [f32; 4],
    pub rest_eye_r: [f32; 4],
    pub rest_head: [f32; 4],
    pub rest_mouth: [f32; 4],
    /// The outfit: top color and style, bottom color and style, accent color and place.
    pub top: [f32; 4],
    pub bottom: [f32; 4],
    pub accent: [f32; 4],
    /// A skirt's left thigh capsule: hip joint and radius, knee joint and radius (zero without).
    pub thigh: [f32; 4],
    pub knee: [f32; 4],
    /// Each hand's frame for curling the fingers, left then right (`HandFrame::gpu`; zero
    /// without hands).
    pub hands: [[[f32; 4]; 5]; 2],
    /// Where the lids cross each eye: left upper, left lower, right upper, right lower, each
    /// `y = a + b x + c x²` in the eye's own units (`eyes::lid_curves`).
    pub lids: [[f32; 4]; 4],
    /// The hair shadow grid's placement (`ShadowGrid::placement`; zero without one).
    pub hair_grid: [[f32; 4]; 2],
}

impl GpuCharacter {
    /// Colors and landmarks of an appearance (the matrices are filled in per frame).
    pub fn new(a: &Appearance, rest: &Landmarks, debug: u32) -> Self {
        // Wrinkles: from 30 on, deeper with age, plus the slider.
        let age_lines = ((a.age - 28.0) / 45.0).clamp(0.0, 1.0);
        let wrinkles = (age_lines + a.skin.wrinkles * 0.5).clamp(0.0, 1.0);
        let male = a.sex.clamp(0.0, 1.0);
        let shadow = match a.beard_style() {
            BeardStyle::None => 0.35 * male * ((a.age - 16.0) / 6.0).clamp(0.0, 1.0),
            BeardStyle::Stubble => 0.9,
            _ => 0.7,
        } * (0.4 + 0.6 * a.beard_color().melanin);
        let scalp_tint = match a.hair_style() {
            HairStyle::Bald => 0.0,
            HairStyle::Buzz => 0.85,
            // Slicked into a bun, the scalp hardly shows between the hairs.
            HairStyle::Bun => 0.7,
            HairStyle::Short => 0.45,
            // Long hair lies flat over its roots: what shows between them reads as hair.
            _ => 0.85,
        } * (1.0 - a.hair.color.grey * 0.5);
        let hc = a.hair_color();
        let bc = a.brow_color();
        let dc = a.beard_color();
        let e = |p: Vec3, w: f32| [p.x, p.y, p.z, w];
        let o = &a.outfit;
        let cut = Cut::of(a).ids();
        Self {
            model: Mat4::IDENTITY.to_cols_array_2d(),
            prev_model: Mat4::IDENTITY.to_cols_array_2d(),
            skin: [
                a.skin.melanin,
                a.skin.redness,
                a.skin.undertone,
                a.skin.oiliness,
            ],
            skin2: [a.skin.freckles, a.skin.moles, wrinkles, a.skin.pores],
            look: [a.age, male, shadow, (a.seed % 1000) as f32 / 1000.0],
            hair: [hc.melanin, hc.red, hc.grey, scalp_tint],
            brows: [bc.melanin, bc.red, bc.grey, 0.0],
            beard: [dc.melanin, dc.red, dc.grey, a.beard.density],
            eye: [
                a.eyes.melanin,
                a.eyes.lipochrome,
                a.eyes.limbal_ring,
                a.eyes.pupil,
            ],
            eye2: [a.eyes.redness, debug as f32, 0.0, 0.0],
            rest_eye_l: e(rest.eyes[0], rest.eye_radius),
            rest_eye_r: e(rest.eyes[1], rest.eye_radius),
            rest_head: e(rest.head, 0.0),
            rest_mouth: e(rest.mouth, 0.0),
            top: [o.top_color[0], o.top_color[1], o.top_color[2], cut[0]],
            bottom: [
                o.bottom_color[0],
                o.bottom_color[1],
                o.bottom_color[2],
                cut[1],
            ],
            accent: [o.accent[0], o.accent[1], o.accent[2], cut[2]],
            thigh: [0.0; 4],
            knee: [0.0; 4],
            hands: [[[0.0; 4]; 5]; 2],
            lids: [[0.0; 4]; 4],
            hair_grid: [[0.0; 4]; 2],
        }
    }
}

/// Skin vertex; must match `SkinIn` in skin.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SkinVertex {
    position_curvature: [f32; 4],
    normal: [f32; 4],
    rest: [f32; 4],
    masks: [f32; 12],
    bones: [u8; 4],
    weights: [u8; 4],
}

const SKIN_ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4,
    6 => Uint8x4, 7 => Unorm8x4
];

/// The fixed part of the pose buffer; must match `struct Pose` in common.wgsl (its bone
/// matrices follow: this frame's, then last frame's).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuPose {
    info: [u32; 4],
    gaze: [f32; 4],
    eye_l: [f32; 4],
    eye_r: [f32; 4],
    grip: [f32; 4],
    weights: [[f32; 4]; MAX_CHANNELS / 4],
    prev_weights: [[f32; 4]; MAX_CHANNELS / 4],
}

/// Eye vertex; must match `EyeIn` in eye.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct EyeVertex {
    position: [f32; 4],
    normal: [f32; 4],
    local: [f32; 4],
}

const EYE_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

struct GpuStrands {
    group: wgpu::BindGroup,
    params: wgpu::Buffer,
    info: [u32; 4],
    kind: StrandKind,
    hairs: u32,
    points: u32,
    /// This frame's detail (`lod::strands`): hairs and vertices per hair drawn, and the
    /// stride and width written to `params`.
    drawn: u32,
    vertices: u32,
    lod: (u32, u32),
    /// Hair that moves: guide point displacements, this frame's then last frame's.
    motion: Option<GuideMotion>,
}

struct GuideMotion {
    buffer: wgpu::Buffer,
    /// Last frame's displacements (the buffer's second half next frame).
    last: Vec<[f32; 4]>,
}

/// What a character's pose buffer held last frame.
struct LastPose {
    palette: Vec<Mat4>,
    weights: [f32; MAX_CHANNELS],
    gaze: [f32; 2],
    grip: [f32; 2],
    /// The pose skeleton the bone map was made for (its names), and the map.
    names: Option<Arc<[String]>>,
    map: Vec<Option<usize>>,
}

impl Default for LastPose {
    fn default() -> Self {
        Self {
            palette: Vec::new(),
            weights: [0.0; MAX_CHANNELS],
            gaze: [0.0; 2],
            grip: [0.0; 2],
            names: None,
            map: Vec::new(),
        }
    }
}

struct GpuMeshes {
    version: u64,
    uniform: wgpu::Buffer,
    pose: wgpu::Buffer,
    bones: usize,
    last: LastPose,
    group: wgpu::BindGroup,
    skin_vertices: wgpu::Buffer,
    skin_indices: wgpu::Buffer,
    skin_count: u32,
    eye_vertices: wgpu::Buffer,
    eye_indices: wgpu::Buffer,
    eye_count: u32,
    /// The skin seen from afar (`SkinMesh::coarse_indices`); empty buffer when there is none.
    coarse_indices: wgpu::Buffer,
    coarse_count: u32,
    /// Draw the coarse skin this frame.
    coarse: bool,
    strands: Vec<GpuStrands>,
    /// Last frame's model matrix (camera-relative), for motion vectors.
    last_model: Option<(Mat4, DVec3)>,
    seen: bool,
}

impl GpuMeshes {
    /// The skin, coarse from afar.
    fn draw_skin(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.skin_vertices.slice(..));
        let (indices, count) = if self.coarse {
            (&self.coarse_indices, self.coarse_count)
        } else {
            (&self.skin_indices, self.skin_count)
        };
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..count, 0, 0..1);
    }

    /// Pick this frame's detail for a person `distance` m from the camera.
    fn choose_detail(&mut self, gpu: &GpuShared, distance: f32) {
        self.coarse = self.coarse_count > 0 && distance > lod::COARSE_SKIN;
        for s in &mut self.strands {
            let (drawn, stride, width) = lod::strands(s.kind, s.hairs, s.points, distance);
            s.drawn = drawn;
            s.vertices = lod::strand_vertices(s.points, stride);
            if s.lod != (drawn, stride) {
                s.lod = (drawn, stride);
                let params = GpuStrandParams {
                    info: s.info,
                    lod: [width, stride as f32, 0.0, 0.0],
                };
                gpu.queue
                    .write_buffer(&s.params, 0, bytemuck::bytes_of(&params));
            }
        }
    }

    /// Triangles drawn this frame.
    fn triangles(&self) -> u64 {
        let skin = if self.coarse {
            self.coarse_count
        } else {
            self.skin_count
        };
        let hair: u64 = self
            .strands
            .iter()
            .map(|s| s.drawn as u64 * (s.vertices / 3) as u64)
            .sum();
        (skin / 3 + self.eye_count / 3) as u64 + hair
    }
}

/// Strand set parameters; must match `StrandParams` in strand_data.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuStrandParams {
    info: [u32; 4],
    lod: [f32; 4],
}

struct Pipelines {
    char_layout: wgpu::BindGroupLayout,
    strand_layout: wgpu::BindGroupLayout,
    shadow_sampler: wgpu::Sampler,
    skin: wgpu::RenderPipeline,
    skin_prepass: wgpu::RenderPipeline,
    skin_shadow: wgpu::RenderPipeline,
    eye: wgpu::RenderPipeline,
    eye_prepass: wgpu::RenderPipeline,
    strands: wgpu::RenderPipeline,
    strands_prepass: wgpu::RenderPipeline,
    strands_shadow: wgpu::RenderPipeline,
}

/// Draws every entity with a [`BuiltCharacter`].
#[derive(Default)]
pub struct CharacterFeature {
    pipelines: Option<Pipelines>,
    characters: HashMap<Entity, GpuMeshes>,
    query: Option<QueryState<CharacterItem>>,
    stats: FeatureStats,
    /// Scratch for this frame's bone matrices and pose buffer bytes.
    palette: Vec<Mat4>,
    pose_bytes: Vec<u8>,
    motion: Vec<[f32; 4]>,
    /// Skin scatters light through the renderer's subsurface pass this frame (people drawn and
    /// the pass on); otherwise the skin shader imitates it on its own.
    subsurface: bool,
}

type CharacterItem = (
    Entity,
    &'static GlobalTransform,
    &'static BuiltCharacter,
    Option<&'static SkeletonPose>,
    Option<&'static FaceControls>,
    Option<&'static HandControls>,
    Option<&'static HairGuides>,
    Option<&'static HairGuidesPosed>,
);

enum Pass {
    Scene,
    Prepass,
    Shadow,
}

#[allow(clippy::too_many_arguments)]
fn pipeline(
    gpu: &GpuShared,
    label: &str,
    module: &wgpu::ShaderModule,
    (vs, fs): (&str, Option<&str>),
    layouts: &[Option<&wgpu::BindGroupLayout>],
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
    pass: Pass,
    cull: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    let device = &gpu.device;
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    let scene_targets = [Some(wgpu::ColorTargetState {
        format: gpu.formats.hdr,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let prepass_targets = gpu.prepass_targets();
    let (targets, depth): (&[Option<wgpu::ColorTargetState>], _) = match pass {
        Pass::Scene => (&scene_targets, gpu.depth_state(true)),
        Pass::Prepass => (&prepass_targets, gpu.depth_state(true)),
        Pass::Shadow => (&[], gpu.shadow_depth_state()),
    };
    let shadow = matches!(pass, Pass::Shadow);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            compilation_options: Default::default(),
            buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: if shadow { None } else { cull },
            unclipped_depth: shadow && gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            ..Default::default()
        },
        depth_stencil: Some(depth),
        multisample: Default::default(),
        fragment: fs.map(|fs| wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl CharacterFeature {
    fn upload(p: &Pipelines, gpu: &GpuShared, m: &CharacterMeshes, version: u64) -> GpuMeshes {
        let device = &gpu.device;
        let n_skin = m.skin.skin_vertices();
        let part = |i: usize| {
            i.checked_sub(n_skin)
                .and_then(|e| m.skin.extra_parts.get(e))
                .copied()
                .unwrap_or(0)
        };
        let skin: Vec<SkinVertex> = (0..m.skin.positions.len())
            .map(|i| {
                let p = m.skin.positions[i];
                let n = m.skin.normals[i];
                let r = m.skin.rest[i];
                let mut masks = [0.0; 12];
                masks.copy_from_slice(&m.skin.masks[i * 12..i * 12 + 12]);
                SkinVertex {
                    position_curvature: [p.x, p.y, p.z, m.skin.curvature[i]],
                    normal: [n.x, n.y, n.z, part(i) as f32],
                    rest: [r.x, r.y, r.z, 0.0],
                    masks,
                    bones: m.skin.bones.get(i).copied().unwrap_or_default(),
                    weights: m.skin.weights.get(i).copied().unwrap_or([255, 0, 0, 0]),
                }
            })
            .collect();
        let eyes: Vec<EyeVertex> = m
            .eyes
            .vertices
            .iter()
            .map(|v| EyeVertex {
                position: [v.position.x, v.position.y, v.position.z, 1.0],
                normal: [v.normal.x, v.normal.y, v.normal.z, 0.0],
                local: [v.local.x, v.local.y, v.local.z, 0.0],
            })
            .collect();
        let buffer = |label: &str, data: &[u8], usage| {
            // wgpu refuses empty buffers.
            let fallback = [0u8; 16];
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: if data.is_empty() { &fallback } else { data },
                usage,
            })
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("character uniform"),
            size: std::mem::size_of::<GpuCharacter>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bones = m.rig.as_ref().map_or(1, |r| r.bones.len()).max(1);
        let pose = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("character pose"),
            size: (std::mem::size_of::<GpuPose>() + 2 * bones * 64) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let morph_offsets = buffer(
            "character expression offsets",
            bytemuck::cast_slice(&m.face.offsets),
            wgpu::BufferUsages::STORAGE,
        );
        let morph_entries = buffer(
            "character expression shapes",
            bytemuck::cast_slice(&m.face.entries),
            wgpu::BufferUsages::STORAGE,
        );
        // The hair shadow grid, for the skin and the hair (a single empty cell without one).
        let (shadow_dims, shadow_cells) = match &m.hair_shadow {
            Some(g) => (g.dims, g.density.as_slice()),
            None => ([1, 1, 1], &[0u8, 0][..]),
        };
        let shadow_texture = device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("character hair shadow"),
                size: wgpu::Extent3d {
                    width: shadow_dims[0],
                    height: shadow_dims[1],
                    depth_or_array_layers: shadow_dims[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rg8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            shadow_cells,
        );
        let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("character"),
            layout: &p.char_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pose.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: morph_offsets.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: morph_entries.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&p.shadow_sampler),
                },
            ],
        });
        let pack = |b: [u8; 4]| u32::from_le_bytes(b);
        let mut strands = Vec::new();
        for (si, set) in m.strands.iter().enumerate() {
            let hairs = set.strand_count() as u32;
            if hairs == 0 || set.points_per < 2 {
                continue;
            }
            let guides = set.guide_count();
            let moves = guides > 0 && set.follow.len() == hairs as usize;
            let info: [u32; 4] = [
                set.points_per,
                set.kind as u32,
                hairs,
                if moves { guides as u32 } else { 0 },
            ];
            let params = buffer(
                "character strand params",
                bytemuck::bytes_of(&GpuStrandParams {
                    info,
                    lod: [1.0, 1.0, 0.0, 0.0],
                }),
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            );
            let points = buffer(
                "character strand points",
                bytemuck::cast_slice(&set.points),
                wgpu::BufferUsages::STORAGE,
            );
            let attrs = buffer(
                "character strand attributes",
                bytemuck::cast_slice(&set.attrs),
                wgpu::BufferUsages::STORAGE,
            );
            let roots: Vec<[u32; 4]> = (0..hairs as usize)
                .map(|h| {
                    let v = m
                        .strand_roots
                        .get(si)
                        .and_then(|r| r.get(h))
                        .copied()
                        .unwrap_or(u32::MAX);
                    let at = v as usize;
                    [
                        v,
                        pack(m.skin.bones.get(at).copied().unwrap_or_default()),
                        pack(m.skin.weights.get(at).copied().unwrap_or([255, 0, 0, 0])),
                        if moves {
                            set.follow[h].guides[0]
                        } else {
                            u32::MAX
                        },
                    ]
                })
                .collect();
            let roots = buffer(
                "character strand roots",
                bytemuck::cast_slice(&roots),
                wgpu::BufferUsages::STORAGE,
            );
            // Per hair that moves: its three guides and their weights (bytes, 0..255).
            let follow: Vec<[u32; 4]> = if moves {
                set.follow
                    .iter()
                    .map(|f| {
                        let w = f.weights.map(|w| (w.clamp(0.0, 1.0) * 255.0).round() as u8);
                        let [a, b, c] = f.guides;
                        [a, b, c, u32::from_le_bytes([w[0], w[1], w[2], 0])]
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let follow = buffer(
                "character strand follow",
                bytemuck::cast_slice(&follow),
                wgpu::BufferUsages::STORAGE,
            );
            // Every guide point's displacement, this frame's then last frame's (none yet).
            let guide_points = guides * set.points_per as usize;
            let motion = moves.then(|| GuideMotion {
                buffer: buffer(
                    "character strand motion",
                    &vec![0u8; 2 * guide_points * 16],
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                ),
                last: vec![[0.0; 4]; guide_points],
            });
            let still = motion
                .is_none()
                .then(|| buffer("character strand motion", &[], wgpu::BufferUsages::STORAGE));
            let motion_binding = motion
                .as_ref()
                .map_or_else(|| still.as_ref().unwrap(), |m| &m.buffer)
                .as_entire_binding();
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("character strands"),
                layout: &p.strand_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: points.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: attrs.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: roots.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: motion_binding,
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: follow.as_entire_binding(),
                    },
                ],
            });
            strands.push(GpuStrands {
                group,
                params,
                info,
                kind: set.kind,
                hairs,
                points: set.points_per,
                drawn: hairs,
                vertices: lod::strand_vertices(set.points_per, 1),
                lod: (hairs, 1),
                motion,
            });
        }
        GpuMeshes {
            version,
            uniform,
            pose,
            bones,
            last: LastPose::default(),
            group,
            skin_vertices: buffer(
                "character skin vertices",
                bytemuck::cast_slice(&skin),
                wgpu::BufferUsages::VERTEX,
            ),
            skin_indices: buffer(
                "character skin indices",
                bytemuck::cast_slice(&m.skin.indices),
                wgpu::BufferUsages::INDEX,
            ),
            skin_count: m.skin.indices.len() as u32,
            eye_vertices: buffer(
                "character eye vertices",
                bytemuck::cast_slice(&eyes),
                wgpu::BufferUsages::VERTEX,
            ),
            eye_indices: buffer(
                "character eye indices",
                bytemuck::cast_slice(&m.eyes.indices),
                wgpu::BufferUsages::INDEX,
            ),
            eye_count: m.eyes.indices.len() as u32,
            coarse_indices: buffer(
                "character coarse skin indices",
                bytemuck::cast_slice(&m.skin.coarse_indices),
                wgpu::BufferUsages::INDEX,
            ),
            coarse_count: m.skin.coarse_indices.len() as u32,
            coarse: false,
            strands,
            last_model: None,
            seen: true,
        }
    }

    fn draw_meshes(&self, pass: &mut wgpu::RenderPass<'_>, which: Pass) {
        let Some(p) = &self.pipelines else {
            return;
        };
        for c in self.characters.values().filter(|c| c.seen) {
            pass.set_bind_group(1, &c.group, &[]);
            match which {
                Pass::Scene | Pass::Prepass => {
                    let scene = matches!(which, Pass::Scene);
                    pass.set_pipeline(if scene { &p.skin } else { &p.skin_prepass });
                    c.draw_skin(pass);
                    pass.set_pipeline(if scene { &p.eye } else { &p.eye_prepass });
                    pass.set_vertex_buffer(0, c.eye_vertices.slice(..));
                    pass.set_index_buffer(c.eye_indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..c.eye_count, 0, 0..1);
                    pass.set_pipeline(if scene {
                        &p.strands
                    } else {
                        &p.strands_prepass
                    });
                }
                Pass::Shadow => {
                    pass.set_pipeline(&p.skin_shadow);
                    c.draw_skin(pass);
                    pass.set_pipeline(&p.strands_shadow);
                }
            }
            for s in c.strands.iter().filter(|s| s.drawn > 0) {
                pass.set_bind_group(2, &s.group, &[]);
                pass.draw(0..s.vertices, 0..s.drawn);
            }
        }
    }
}

impl RenderFeature for CharacterFeature {
    fn name(&self) -> &str {
        "character"
    }

    fn needs_subsurface(&self) -> bool {
        self.subsurface
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let char_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("character"),
            entries: &[
                uniform_entry(0),
                storage_entry(1),
                storage_entry(2),
                storage_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let strand_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("character strands"),
            entries: &[
                uniform_entry(0),
                storage_entry(1),
                storage_entry(2),
                storage_entry(3),
                storage_entry(4),
                storage_entry(5),
            ],
        });
        let [
            skin_src,
            eye_src,
            strand_src,
            skin_shadow_src,
            strand_shadow_src,
        ] = shader_sources();
        let skin_mod = gpu.shader(skin_src.0, &skin_src.1, skin_src.2);
        let eye_mod = gpu.shader(eye_src.0, &eye_src.1, eye_src.2);
        let strand_mod = gpu.shader(strand_src.0, &strand_src.1, strand_src.2);
        let skin_shadow_mod = gpu.shader(skin_shadow_src.0, &skin_shadow_src.1, skin_shadow_src.2);
        let strand_shadow_mod = gpu.shader(
            strand_shadow_src.0,
            &strand_shadow_src.1,
            strand_shadow_src.2,
        );
        let skin_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkinVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &SKIN_ATTRIBUTES,
        })];
        let eye_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<EyeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &EYE_ATTRIBUTES,
        })];
        let scene = [Some(&gpu.frame_layout), Some(&char_layout)];
        let scene_strands = [
            Some(&gpu.frame_layout),
            Some(&char_layout),
            Some(&strand_layout),
        ];
        let shadow = [Some(&gpu.shadow_view_layout), Some(&char_layout)];
        let shadow_strands = [
            Some(&gpu.shadow_view_layout),
            Some(&char_layout),
            Some(&strand_layout),
        ];
        let back = Some(wgpu::Face::Back);
        self.pipelines = Some(Pipelines {
            skin: pipeline(
                gpu,
                "character skin",
                &skin_mod,
                ("vs_skin", Some("fs_skin")),
                &scene,
                &skin_buffers,
                Pass::Scene,
                back,
            ),
            skin_prepass: pipeline(
                gpu,
                "character skin prepass",
                &skin_mod,
                ("vs_skin", Some("fs_skin_prepass")),
                &scene,
                &skin_buffers,
                Pass::Prepass,
                back,
            ),
            skin_shadow: pipeline(
                gpu,
                "character skin shadow",
                &skin_shadow_mod,
                ("vs_skin_shadow", None),
                &shadow,
                &skin_buffers,
                Pass::Shadow,
                None,
            ),
            eye: pipeline(
                gpu,
                "character eyes",
                &eye_mod,
                ("vs_eye", Some("fs_eye")),
                &scene,
                &eye_buffers,
                Pass::Scene,
                back,
            ),
            eye_prepass: pipeline(
                gpu,
                "character eyes prepass",
                &eye_mod,
                ("vs_eye", Some("fs_eye_prepass")),
                &scene,
                &eye_buffers,
                Pass::Prepass,
                back,
            ),
            strands: pipeline(
                gpu,
                "character strands",
                &strand_mod,
                ("vs_strand", Some("fs_strand")),
                &scene_strands,
                &[],
                Pass::Scene,
                None,
            ),
            strands_prepass: pipeline(
                gpu,
                "character strands prepass",
                &strand_mod,
                ("vs_strand", Some("fs_strand_prepass")),
                &scene_strands,
                &[],
                Pass::Prepass,
                None,
            ),
            strands_shadow: pipeline(
                gpu,
                "character strands shadow",
                &strand_shadow_mod,
                ("vs_strand_shadow", Some("fs_strand_shadow")),
                &shadow_strands,
                &[],
                Pass::Shadow,
                None,
            ),
            char_layout,
            strand_layout,
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("character hair shadow"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
        let Some(p) = &self.pipelines else {
            return;
        };
        // The renderer runs the pass for perspective views only.
        let subsurface = settings.subsurface_width > 0.0
            && !view.orthographic
            && world
                .get_resource::<crate::CharacterSubsurface>()
                .is_some_and(|s| s.pass);
        self.stats = FeatureStats::default();
        let debug = world
            .get_resource::<CharacterDebug>()
            .map_or(0, |d| d.view as u32);
        let lod_scale = world
            .get_resource::<CharacterLod>()
            .map_or(1.0, |l| l.distance_scale);
        let query = self.query.get_or_insert_with(|| world.query());
        for c in self.characters.values_mut() {
            c.seen = false;
        }
        for (entity, transform, built, skeleton, face, hands, guides, posed) in query.iter(world) {
            let entry = self.characters.entry(entity);
            let c = match entry {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let c = o.into_mut();
                    if c.version != built.version {
                        let last = c.last_model;
                        *c = Self::upload(p, gpu, &built.meshes, built.version);
                        c.last_model = last;
                    }
                    c
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(Self::upload(p, gpu, &built.meshes, built.version))
                }
            };
            c.seen = true;
            let world_pos = transform.translation();
            let rel = (world_pos - view.origin).as_vec3();
            let model =
                Mat4::from_scale_rotation_translation(transform.scale(), transform.rotation(), rel);
            // Last frame's placement relative to last frame's camera.
            let prev_model = match c.last_model {
                Some((m, at)) => Mat4::from_cols(
                    m.x_axis,
                    m.y_axis,
                    m.z_axis,
                    (at - view.prev_origin).as_vec3().extend(1.0),
                ),
                None => Mat4::from_scale_rotation_translation(
                    transform.scale(),
                    transform.rotation(),
                    (world_pos - view.prev_origin).as_vec3(),
                ),
            };
            c.last_model = Some((model, world_pos));
            let mut u = built.uniform;
            u.eye2[1] = debug as f32;
            u.eye2[2] = if subsurface { 1.0 } else { 0.0 };
            u.model = model.to_cols_array_2d();
            u.prev_model = prev_model.to_cols_array_2d();
            gpu.queue
                .write_buffer(&c.uniform, 0, bytemuck::bytes_of(&u));
            write_pose(
                gpu,
                c,
                &built.meshes,
                skeleton,
                (face, hands),
                &mut self.palette,
                &mut self.pose_bytes,
            );
            write_motion(gpu, c, guides, posed, skeleton, &mut self.motion);
            let distance = rel.length() * lod_scale;
            c.choose_detail(gpu, if lod_scale > 0.0 { distance } else { 0.0 });
            self.stats.instances += 1;
            self.stats.triangles += c.triangles();
            self.stats.draw_calls += 2 + c.strands.iter().filter(|s| s.drawn > 0).count() as u32;
        }
        self.characters.retain(|_, c| c.seen);
        self.subsurface = subsurface && !self.characters.is_empty();
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        self.draw_meshes(pass, Pass::Prepass);
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase == RenderPhase::Opaque {
            self.draw_meshes(pass, Pass::Scene);
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        if cascade == SKY_OCCLUSION_VIEW {
            return;
        }
        self.draw_meshes(pass, Pass::Shadow);
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "characters": self.characters.len(),
            "triangles": self.stats.triangles,
        })
    }
}

/// The meshes a character feature draws, shared with the build system.
pub type SharedMeshes = Arc<CharacterMeshes>;

/// Fill the motion buffers of a character's hair that moves: this frame's guide displacements
/// (from its `HairGuidesPosed`) and last frame's.
fn write_motion(
    gpu: &GpuShared,
    c: &mut GpuMeshes,
    guides: Option<&HairGuides>,
    posed: Option<&HairGuidesPosed>,
    skeleton: Option<&SkeletonPose>,
    scratch: &mut Vec<[f32; 4]>,
) {
    // Only guides grown from this build match its hairs.
    let guides = guides.filter(|g| g.version == c.version as u32);
    for m in c.strands.iter_mut().filter_map(|s| s.motion.as_mut()) {
        match guides {
            Some(g) => crate::hair_motion::guide_motion(g, posed, skeleton, scratch),
            None => scratch.clear(),
        }
        // Guides for another build (or none yet): the hair rides the head.
        scratch.resize(m.last.len(), [0.0; 4]);
        let mut bytes = Vec::with_capacity(scratch.len() * 32);
        bytes.extend_from_slice(bytemuck::cast_slice(scratch.as_slice()));
        bytes.extend_from_slice(bytemuck::cast_slice(m.last.as_slice()));
        gpu.queue.write_buffer(&m.buffer, 0, &bytes);
        m.last.clear();
        m.last.extend_from_slice(scratch);
    }
}

/// Fill a character's pose buffer: bone matrices from its `SkeletonPose` (identity without
/// one), expression weights from its `FaceControls`, grips from its `HandControls`, and last
/// frame's of each.
fn write_pose(
    gpu: &GpuShared,
    c: &mut GpuMeshes,
    m: &CharacterMeshes,
    skeleton: Option<&SkeletonPose>,
    (face_controls, hands): (Option<&FaceControls>, Option<&HandControls>),
    palette: &mut Vec<Mat4>,
    bytes: &mut Vec<u8>,
) {
    let rig: Option<&Rig> = m.rig.as_ref();
    palette.clear();
    let posed = match (rig, skeleton) {
        (Some(rig), Some(pose)) => {
            let same = c
                .last
                .names
                .as_ref()
                .is_some_and(|n| Arc::ptr_eq(n, &pose.names));
            if !same {
                c.last.map = rig.map(pose);
                c.last.names = Some(pose.names.clone());
            }
            rig.palette(pose, &c.last.map, palette);
            true
        }
        _ => false,
    };
    palette.resize(c.bones, Mat4::IDENTITY);
    if c.last.palette.len() != c.bones {
        c.last.palette = palette.clone();
    }
    let mut weights = [0.0f32; MAX_CHANNELS];
    let mut gaze = [0.0f32; 2];
    if let Some(f) = face_controls {
        face::channel_weights(f, &mut weights);
        gaze = [f.gaze_yaw, f.gaze_pitch];
    }
    let grip = hands.map_or([0.0; 2], |h| [h.grip(0), h.grip(1)]);
    let head = rig
        .and_then(|r| r.bones.iter().position(|b| b == "head"))
        .unwrap_or(0);
    let eyes = m.landmarks.map(|l| l.eyes).unwrap_or([Vec3::ZERO; 2]);
    let e = |p: Vec3| [p.x, p.y, p.z, 0.0];
    let fours = |w: &[f32; MAX_CHANNELS]| {
        let mut out = [[0.0f32; 4]; MAX_CHANNELS / 4];
        for (i, v) in w.iter().enumerate() {
            out[i / 4][i % 4] = *v;
        }
        out
    };
    let header = GpuPose {
        info: [c.bones as u32, posed as u32, head as u32, 0],
        gaze: [gaze[0], gaze[1], c.last.gaze[0], c.last.gaze[1]],
        eye_l: e(eyes[0]),
        eye_r: e(eyes[1]),
        grip: [grip[0], grip[1], c.last.grip[0], c.last.grip[1]],
        weights: fours(&weights),
        prev_weights: fours(&c.last.weights),
    };
    bytes.clear();
    bytes.extend_from_slice(bytemuck::bytes_of(&header));
    for mat in palette.iter().chain(c.last.palette.iter()) {
        bytes.extend_from_slice(bytemuck::bytes_of(&mat.to_cols_array()));
    }
    gpu.queue.write_buffer(&c.pose, 0, bytes);
    c.last.palette.clear();
    c.last.palette.extend_from_slice(palette);
    c.last.weights = weights;
    c.last.gaze = gaze;
    c.last.grip = grip;
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::features::{compose_shader, validate_wgsl};

    #[test]
    fn shaders_validate() {
        for (label, src, with_frame) in shader_sources() {
            validate_wgsl(label, &compose_shader(&src, with_frame))
                .unwrap_or_else(|e| panic!("{label}: {e}"));
        }
    }

    #[test]
    fn gpu_structs_match_wgsl_sizes() {
        assert_eq!(std::mem::size_of::<GpuCharacter>(), 2 * 64 + 33 * 16);
        assert_eq!(std::mem::size_of::<SkinVertex>(), 104);
        assert_eq!(std::mem::size_of::<GpuPose>(), 5 * 16 + 2 * 16 * 16);
        assert_eq!(std::mem::size_of::<EyeVertex>(), 48);
    }
}
