//! GPU skinning render feature for [`SkinnedMeshRenderer`].
//!
//! Static data of every skinned mesh in use lives in shared storage buffers (rest vertices,
//! per-vertex morph and neighbor ranges, morph deltas, neighbor lists) plus a vertex buffer of
//! attributes that never change (uv, color, detail coordinates, masks). Each frame:
//!
//! * `prepare` culls instances, uploads joint matrices, morph weights, one *job* per instance
//!   (a 256-byte uniform slot read with a dynamic offset) and instance/material data;
//! * `compute` runs the skin pass for every job, then (in a second pass, so all positions are
//!   final) the tension pass for jobs near the camera;
//! * `draw`/`draw_shadow` draw each instance from its slice of the shared output buffer, one
//!   indexed draw per [`Submesh`](super::Submesh) with that submesh's surface.
//!
//! ## Fur
//!
//! A character whose material has fur ([`SkinMaterial::fur_length`]) is drawn again as
//! *shells* after the opaque pass: copies of its instance (`extra.z` = shell 1..=N,
//! `extra.w` = N) that the vertex shader pushes out along the normal, the tips sagging and
//! combed back, and that the fragment shader cuts into tapering strands on a jittered grid in
//! bind-pose space (so the strands stick to the skin through every shell). Shells thin out with
//! distance ([`SkinMaterial::fur_lod`]); far away the skin alone carries the coat's color.
//!
//! ## Surfaces and textures
//!
//! Each draw binds a *surface* (group 2 in the scene pass, group 1 in the masked shadow pass):
//! base color, normal and metallic/roughness texture views, one trilinear anisotropic
//! repeating sampler, and a small uniform of factors (`GpuSurface`). Surfaces come from
//! [`SkinnedMeshRenderer::surfaces`]; every procedural character uses the shared default
//! surface (id 0: 1×1 neutral textures, flags 0), so the pipeline layout is the same for all.
//! A mesh without submeshes is drawn whole with the renderer's first surface (the default
//! surface when it has none); a submesh whose surface index is out of range uses the default.
//!
//! * **Images** are the renderer's shared GPU copies ([`GpuShared::image_texture`]): the
//!   standard meshes, every character and any other feature that draws an image share one
//!   upload, and with `RenderSettings::release_image_pixels` the renderer may free the image's
//!   CPU pixels once it is on the GPU. Each surface asks for its images on every frame it is
//!   drawn, which keeps them on the GPU.
//! * **Surface bind groups** are cached by what they are built from (canonical image handles
//!   and the factors), so characters with the same materials share them. A group is rebuilt
//!   only when an image it binds gets a new upload (loaded, new pixels, removed).
//! * Groups unused for `EVICT_AFTER_FRAMES` (600) frames are released.
//!
//! Single-sided and double-sided surfaces use two scene pipelines (back-face culling on/off;
//! double-sided ones shade the side the camera sees). Shadows are never culled, like the
//! standard mesh shadows; alpha-masked surfaces cast them through a pipeline that discards by
//! base color alpha.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase,
};
use engine_render::frame::ViewData;
use engine_render::mesh_feature::MeshInstance;
use engine_render::settings::RenderSettings;
use engine_render::shadows::MAX_CASCADES;
use engine_render::wgpu;
use wgpu::util::DeviceExt;

use engine_render::{AlphaMode, Image, ImageTexture, StandardMaterial};

use super::{SkinMaterial, SkinPose, SkinnedMesh, SkinnedMeshRenderer};

/// Compute shader: morphs + linear blend skinning, and cloth tension.
pub const SKIN_COMPUTE_WGSL: &str = include_str!("shaders/skin_compute.wgsl");
/// Scene shader: PBR with tension wrinkles, compression bunching, veins and sheen.
pub const SKIN_DRAW_WGSL: &str = include_str!("shaders/skin_draw.wgsl");
/// Shadow depth shader (depth only, and alpha-masked).
pub const SKIN_SHADOW_WGSL: &str = include_str!("shaders/skin_shadow.wgsl");

const WORKGROUP: u32 = 64;
/// Bytes between jobs in the uniform buffer (the largest allowed dynamic offset alignment).
const JOB_STRIDE: u64 = 256;
const JOB_MORPHS: u32 = 1;
const JOB_TENSION: u32 = 2;

/// Texture slots of a surface: base color, normal, metallic/roughness (bindings 1..=3; the
/// uniform is binding 0, the sampler binding 4).
const SURFACE_SLOTS: usize = 3;
/// Slots holding color (sampled as sRGB when the image is sRGB): only base color.
const SURFACE_COLOR_SLOT: [bool; SURFACE_SLOTS] = [true, false, false];
/// 1×1 stand-ins for missing textures: white base color, flat normal, white
/// metallic/roughness (the factors pass through unchanged).
const DEFAULT_TEXELS: [[u8; 4]; SURFACE_SLOTS] = [[255; 4], [128, 128, 255, 255], [255; 4]];
/// Surface bind groups unused for this many frames are released.
const EVICT_AFTER_FRAMES: u64 = 600;

/// `GpuSurface::flags[0]` bits (`SURFACE_*` in the shaders).
const SURFACE_BASE_COLOR: u32 = 1;
const SURFACE_NORMAL: u32 = 2;
const SURFACE_METALLIC_ROUGHNESS: u32 = 4;
const SURFACE_DOUBLE_SIDED: u32 = 8;
/// The surface's roughness and metallic replace the `SkinMaterial`'s skin/cloth choice.
const SURFACE_STANDARD: u32 = 16;
/// Texture-present bit of each slot.
const SURFACE_TEXTURE_FLAGS: [u32; SURFACE_SLOTS] = [
    SURFACE_BASE_COLOR,
    SURFACE_NORMAL,
    SURFACE_METALLIC_ROUGHNESS,
];

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuRest {
    position: [f32; 4],
    normal: [f32; 4],
    weights: [f32; 4],
    joints: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuStatic {
    uv: [f32; 2],
    _pad: [f32; 2],
    color: [f32; 4],
    pattern: [f32; 4],
    axis: [f32; 4],
    mask: [f32; 4],
}

impl GpuStatic {
    // Explicit offsets: `vertex_attr_array!` would pack `color` right after `uv`, skipping
    // the padding.
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
        attribute(2, wgpu::VertexFormat::Float32x2, 0),
        attribute(3, wgpu::VertexFormat::Float32x4, 16),
        attribute(11, wgpu::VertexFormat::Float32x4, 32),
        attribute(12, wgpu::VertexFormat::Float32x4, 48),
        attribute(13, wgpu::VertexFormat::Float32x4, 64),
    ];
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// One morph delta: `position.w` = muscle influence, `normal.w` = target index (bits).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuMorph {
    position: [f32; 4],
    normal: [f32; 4],
}

/// A skinned vertex written by the compute pass: position + tension, normal + activation.
const SKINNED_SIZE: u64 = 32;
const fn attribute(
    shader_location: u32,
    format: wgpu::VertexFormat,
    offset: u64,
) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        format,
        offset,
        shader_location,
    }
}

const SKINNED_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];

fn skinned_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: SKINNED_SIZE,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &SKINNED_ATTRIBUTES,
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuJob {
    vertex_base: u32,
    vertex_count: u32,
    out_base: u32,
    joint_base: u32,
    morph_base: u32,
    flags: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuSkinMaterial {
    tint: [f32; 4],
    /// skin roughness, cloth roughness, subsurface, metallic
    params: [f32; 4],
    /// wrinkles, wrinkle frequency, veins, bunch
    detail: [f32; 4],
    /// vein rgb, sheen
    vein: [f32; 4],
    /// emissive rgb (cd/m²), unused
    emissive: [f32; 4],
    /// fur length (m), strands per meter, droop, comb
    fur: [f32; 4],
    /// fur tip rgb, root brightness
    fur_tip: [f32; 4],
}

impl GpuSkinMaterial {
    fn new(m: &SkinMaterial) -> Self {
        Self {
            tint: m.tint,
            params: [
                m.skin_roughness.clamp(0.045, 1.0),
                m.cloth_roughness.clamp(0.045, 1.0),
                m.subsurface.clamp(0.0, 1.0),
                m.metallic.clamp(0.0, 1.0),
            ],
            detail: [
                m.wrinkles.max(0.0),
                m.wrinkle_frequency.max(0.0),
                m.veins.max(0.0),
                m.bunch,
            ],
            vein: [m.vein_color[0], m.vein_color[1], m.vein_color[2], m.sheen],
            emissive: [
                m.emissive[0] * m.emissive_strength,
                m.emissive[1] * m.emissive_strength,
                m.emissive[2] * m.emissive_strength,
                0.0,
            ],
            fur: [
                m.fur_length.max(0.0),
                m.fur_density.max(1.0),
                m.fur_droop.clamp(0.0, 1.0),
                m.fur_comb.clamp(0.0, 1.0),
            ],
            fur_tip: [
                m.fur_tip[0],
                m.fur_tip[1],
                m.fur_tip[2],
                m.fur_root.clamp(0.0, 1.0),
            ],
        }
    }
}

/// How many fur shells to draw for a character `distance` meters from the camera.
fn fur_shells(m: &SkinMaterial, distance: f32) -> u32 {
    if m.fur_length <= 0.0 || m.fur_shells == 0 {
        return 0;
    }
    let lod = m.fur_lod.max(0.1);
    let k = (2.0 - distance / lod).clamp(0.0, 1.0);
    (m.fur_shells as f32 * k).round() as u32
}

/// Factors of one surface: binding 0 of the surface bind group (`struct SkinSurface` in
/// `skin_draw.wgsl` and `skin_shadow.wgsl`). 64 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct GpuSurface {
    /// Linear RGBA factor (alpha takes part in the cutout).
    base_color: [f32; 4],
    /// perceptual roughness, metallic, normal scale, alpha cutoff (0 = no cutout)
    params: [f32; 4],
    /// uv scale xy, subsurface, reflectance
    extra: [f32; 4],
    /// x: `SURFACE_*` bits
    flags: [u32; 4],
}

impl GpuSurface {
    /// Procedural characters: no textures, the `SkinMaterial` picks roughness and metallic.
    const PROCEDURAL: Self = Self {
        base_color: [1.0; 4],
        params: [0.5, 0.0, 1.0, 0.0],
        extra: [1.0, 1.0, 0.0, 0.5],
        flags: [0; 4],
    };

    fn new(m: &StandardMaterial) -> Self {
        let cutoff = match m.alpha_mode {
            AlphaMode::Opaque => 0.0,
            AlphaMode::Mask(c) => c.clamp(0.0, 1.0),
            // Characters have no transparent pass: blended surfaces are cut out at half alpha.
            AlphaMode::Blend => 0.5,
        };
        let mut flags = SURFACE_STANDARD;
        if m.double_sided {
            flags |= SURFACE_DOUBLE_SIDED;
        }
        for (slot, t) in surface_textures(m).iter().enumerate() {
            if t.is_some() {
                flags |= SURFACE_TEXTURE_FLAGS[slot];
            }
        }
        Self {
            base_color: m.base_color,
            params: [
                m.perceptual_roughness.clamp(0.045, 1.0),
                m.metallic.clamp(0.0, 1.0),
                m.normal_scale,
                cutoff,
            ],
            extra: [
                m.uv_scale[0],
                m.uv_scale[1],
                m.subsurface.clamp(0.0, 1.0),
                m.reflectance,
            ],
            flags: [flags, 0, 0, 0],
        }
    }

    fn double_sided(&self) -> bool {
        self.flags[0] & SURFACE_DOUBLE_SIDED != 0
    }

    fn masked(&self) -> bool {
        self.params[3] > 0.0
    }
}

/// The texture slots a surface binds, in binding order.
fn surface_textures(m: &StandardMaterial) -> [Option<Handle<Image>>; SURFACE_SLOTS] {
    [
        m.base_color_texture,
        m.normal_texture,
        m.metallic_roughness_texture,
    ]
}

/// What a surface bind group is built from: images (canonical handle indices) and factors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SurfaceKey {
    textures: [Option<u32>; SURFACE_SLOTS],
    factors: [u32; 16],
}

/// One cached surface bind group.
struct SurfaceSet {
    group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
    key: SurfaceKey,
    /// The shared texture bound per slot (`ImageTexture::id`); `None` = the default texture (no
    /// image, not loaded yet, or not uploadable).
    bound: [Option<u64>; SURFACE_SLOTS],
    double_sided: bool,
    masked: bool,
    /// Frame its images were last checked (once per frame, however many characters use it).
    checked: u64,
}

/// Surface bind groups shared by every character (see the module docs).
struct SurfaceCache {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    defaults: Vec<(wgpu::Texture, wgpu::TextureView)>,
    /// By id; id 0 is the default (procedural) surface and is never released.
    sets: Vec<Option<SurfaceSet>>,
    ids: HashMap<SurfaceKey, u32>,
    free: Vec<u32>,
    frame: u64,
}

impl SurfaceCache {
    fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skin surface"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<GpuSurface>() as u64
                        ),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Trilinear, repeating, 16x anisotropic where supported (wgpu clamps to 1 otherwise).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("skin surface sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let defaults = DEFAULT_TEXELS
            .iter()
            .enumerate()
            .map(|(slot, texel)| solid_texture(gpu, *texel, SURFACE_COLOR_SLOT[slot]))
            .collect();
        let mut cache = Self {
            layout,
            sampler,
            defaults,
            sets: Vec::new(),
            ids: HashMap::new(),
            free: Vec::new(),
            frame: 0,
        };
        let key = SurfaceKey {
            textures: [None; SURFACE_SLOTS],
            factors: bytemuck::cast(GpuSurface::PROCEDURAL),
        };
        let default = cache.build(
            gpu,
            key,
            [None; SURFACE_SLOTS],
            [None; SURFACE_SLOTS],
            GpuSurface::PROCEDURAL,
        );
        cache.sets.push(Some(default));
        cache
    }

    /// Surface id for a material, asking the renderer for its images and building (or
    /// rebuilding) its bind group when needed.
    fn surface(
        &mut self,
        gpu: &GpuShared,
        material: &StandardMaterial,
        images: Option<&Assets<Image>>,
    ) -> u32 {
        let mut handles = surface_textures(material);
        // Two handles naming the same image (a forwarded reservation) share one upload.
        if let Some(images) = images {
            for h in handles.iter_mut().flatten() {
                *h = images.canonical(*h);
            }
        }
        let factors = GpuSurface::new(material);
        let key = SurfaceKey {
            textures: handles.map(|h| h.map(|h| h.index())),
            factors: bytemuck::cast(factors),
        };
        let frame = self.frame;
        if let Some(&id) = self.ids.get(&key)
            && let Some(Some(set)) = self.sets.get(id as usize)
            && set.checked == frame
        {
            return id;
        }
        // First use this frame: ask for the images (which keeps them on the GPU) and see
        // whether any got a new upload.
        let mut bound = [None; SURFACE_SLOTS];
        let mut textures: [Option<Arc<ImageTexture>>; SURFACE_SLOTS] = Default::default();
        for (slot, handle) in handles.iter().enumerate() {
            let (Some(handle), Some(images)) = (handle, images) else {
                continue;
            };
            if let Some(texture) = gpu.image_texture(images, *handle, SURFACE_COLOR_SLOT[slot]) {
                bound[slot] = Some(texture.id);
                textures[slot] = Some(texture);
            }
        }
        let id = match self.ids.get(&key) {
            Some(&id) => id,
            None => {
                let id = self.free.pop().unwrap_or(self.sets.len() as u32);
                self.ids.insert(key, id);
                id
            }
        };
        if self.sets.len() <= id as usize {
            self.sets.resize_with(id as usize + 1, || None);
        }
        match &mut self.sets[id as usize] {
            Some(set) if set.bound == bound => set.checked = frame,
            _ => {
                // Only slots bound to real images count as textured.
                let mut uniform = factors;
                for (slot, b) in bound.iter().enumerate() {
                    if b.is_none() {
                        uniform.flags[0] &= !SURFACE_TEXTURE_FLAGS[slot];
                    }
                }
                let views = std::array::from_fn(|slot| textures[slot].as_ref().map(|t| &t.view));
                let set = self.build(gpu, key, bound, views, uniform);
                self.sets[id as usize] = Some(set);
            }
        }
        id
    }

    /// A surface binding `views` (the default texture where `None`), whose ids are `bound`.
    fn build(
        &self,
        gpu: &GpuShared,
        key: SurfaceKey,
        bound: [Option<u64>; SURFACE_SLOTS],
        views: [Option<&wgpu::TextureView>; SURFACE_SLOTS],
        uniform: GpuSurface,
    ) -> SurfaceSet {
        let buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skin surface"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let views: [&wgpu::TextureView; SURFACE_SLOTS] =
            std::array::from_fn(|slot| views[slot].unwrap_or(&self.defaults[slot].1));
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skin surface"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        SurfaceSet {
            group,
            _uniform: buffer,
            key,
            bound,
            double_sided: uniform.double_sided(),
            masked: uniform.masked(),
            checked: self.frame,
        }
    }

    /// Release surfaces nobody used for a while.
    fn evict(&mut self) {
        let frame = self.frame;
        for id in 1..self.sets.len() {
            let stale = self.sets[id]
                .as_ref()
                .is_some_and(|s| s.checked + EVICT_AFTER_FRAMES < frame);
            if stale && let Some(set) = self.sets[id].take() {
                self.ids.remove(&set.key);
                self.free.push(id as u32);
            }
        }
    }

    /// Distinct images bound by the cached surfaces.
    fn image_count(&self) -> usize {
        let mut ids: Vec<u64> = self
            .sets
            .iter()
            .flatten()
            .flat_map(|s| s.bound.iter().flatten().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    fn set(&self, id: u32) -> Option<&SurfaceSet> {
        self.sets
            .get(id as usize)
            .and_then(Option::as_ref)
            .or_else(|| self.sets.first().and_then(Option::as_ref))
    }
}

/// A 1×1 texture of one texel (sRGB or linear), for slots without an image.
fn solid_texture(
    gpu: &GpuShared,
    texel: [u8; 4],
    srgb: bool,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("skin default texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &texel,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Where one mesh lives in the shared static buffers.
#[derive(Debug, Clone, Copy)]
struct MeshSlot {
    version: u64,
    vertex_base: u32,
    vertex_count: u32,
    index_base: u32,
    index_count: u32,
    /// The mesh's submeshes in `SkinningFeature::submeshes`.
    submesh_base: u32,
    submesh_count: u32,
    joints: u32,
    morphs: u32,
    center: Vec3,
    radius: f32,
}

/// A submesh's index range in the shared index buffer.
#[derive(Debug, Clone, Copy)]
struct SubmeshSlot {
    first: u32,
    count: u32,
    /// Index into the renderer's `surfaces`.
    surface: u32,
}

/// One indexed draw of an instance: an index range with one surface.
#[derive(Debug, Clone, Copy)]
struct Part {
    first: u32,
    count: u32,
    surface: u32,
    double_sided: bool,
    masked: bool,
}

struct Statics {
    rest: wgpu::Buffer,
    attributes: wgpu::Buffer,
    ranges: wgpu::Buffer,
    morphs: wgpu::Buffer,
    neighbors: wgpu::Buffer,
    indices: wgpu::Buffer,
}

struct Pipelines {
    skin: wgpu::ComputePipeline,
    tension: wgpu::ComputePipeline,
    /// Back-face culled (single-sided surfaces).
    draw: wgpu::RenderPipeline,
    /// No culling; the shader mirrors the normal of back faces.
    draw_double: wgpu::RenderPipeline,
    /// Depth only (opaque surfaces; never culled, like the standard mesh shadows).
    shadow: wgpu::RenderPipeline,
    /// Discards by base color alpha (alpha-masked surfaces).
    shadow_masked: wgpu::RenderPipeline,
    compute_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
}

/// A storage buffer the GPU writes; grows to the next power of two.
struct OutputBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
}

impl OutputBuffer {
    fn new(device: &wgpu::Device, bytes: u64) -> Self {
        let capacity = bytes.max(4096).next_power_of_two();
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("skinned vertices"),
                size: capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            }),
            capacity,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Instance {
    mesh: u32,
    out_base: u32,
    visible: bool,
    casts: bool,
    detail: bool,
    center: Vec3,
    radius: f32,
    /// This instance's draws in `SkinningFeature::parts`.
    part_base: u32,
    part_count: u32,
    /// Its fur shells: instances `shell_first..shell_first + shells` (after every item's own).
    shell_first: u32,
    shells: u32,
}

type SkinQuery = (
    &'static GlobalTransform,
    &'static SkinnedMeshRenderer,
    &'static SkinPose,
);

/// Deforms and draws skinned characters. Added by `AnimationPlugin`.
#[derive(Default)]
pub struct SkinningFeature {
    pipelines: Option<Pipelines>,
    slots: HashMap<u32, MeshSlot>,
    statics: Option<Statics>,
    joints: Option<GrowBuffer>,
    morph_weights: Option<GrowBuffer>,
    jobs: Option<GrowBuffer>,
    instances: Option<GrowBuffer>,
    materials: Option<GrowBuffer>,
    output: Option<OutputBuffer>,
    compute_group: Option<wgpu::BindGroup>,
    material_group: Option<wgpu::BindGroup>,
    surfaces: Option<SurfaceCache>,
    /// Submeshes of every mesh in the static buffers (see `MeshSlot::submesh_base`).
    submeshes: Vec<SubmeshSlot>,
    /// Each mesh's morphs grouped per vertex and its edges (canonical index → version and
    /// data), so a new character arriving rebuilds the static buffers without working out
    /// every other mesh's again.
    derived: HashMap<u32, (u64, Arc<Derived>)>,
    query: Option<QueryState<SkinQuery>>,
    // Per frame.
    items: Vec<Instance>,
    parts: Vec<Part>,
    surface_ids: Vec<u32>,
    job_data: Vec<u8>,
    job_list: Vec<(u32, bool)>,
    joint_data: Vec<[[f32; 4]; 3]>,
    weight_data: Vec<f32>,
    instance_data: Vec<MeshInstance>,
    material_data: Vec<GpuSkinMaterial>,
    /// Items per shadow view: the sun cascades, then the sky occlusion view (always empty).
    shadow: [Vec<u32>; MAX_CASCADES + 1],
    stats: FeatureStats,
}

/// What the static buffers need from a mesh beyond its vertices: per vertex, its morph deltas
/// and its edges (CSR, offsets local to the mesh).
#[derive(Default)]
struct Derived {
    /// Per vertex: first morph, morph count, first neighbor, neighbor count.
    ranges: Vec<[u32; 4]>,
    morphs: Vec<GpuMorph>,
    neighbors: Vec<[u32; 2]>,
}

impl Derived {
    fn of(mesh: &SkinnedMesh) -> Self {
        // Morphs grouped per vertex (CSR), so the skin pass gathers without atomics.
        let n = mesh.vertices.len();
        let mut counts = vec![0u32; n];
        for target in &mesh.morph_targets {
            for d in &target.deltas {
                if let Some(c) = counts.get_mut(d.vertex as usize) {
                    *c += 1;
                }
            }
        }
        let mut first = Vec::with_capacity(n);
        let mut total = 0u32;
        for c in &counts {
            first.push(total);
            total += c;
        }
        let mut morphs = vec![GpuMorph::zeroed(); total as usize];
        let mut fill = first.clone();
        for (t, target) in mesh.morph_targets.iter().enumerate() {
            for d in &target.deltas {
                if let Some(at) = fill.get_mut(d.vertex as usize) {
                    morphs[*at as usize] = GpuMorph {
                        position: d.position.extend(d.influence).to_array(),
                        normal: d.normal.extend(f32::from_bits(t as u32)).to_array(),
                    };
                    *at += 1;
                }
            }
        }
        let adjacency = mesh.neighbors();
        let mut ranges = Vec::with_capacity(n);
        let mut neighbors = Vec::new();
        for (v, adj) in adjacency.iter().enumerate() {
            ranges.push([
                first[v],
                counts[v],
                neighbors.len() as u32,
                adj.len() as u32,
            ]);
            neighbors.extend(adj.iter().map(|(n, l)| [*n, l.to_bits()]));
        }
        Self {
            ranges,
            morphs,
            neighbors,
        }
    }
}

impl SkinningFeature {
    /// Rebuild the shared static buffers from every mesh in `meshes` (canonical index → mesh).
    fn rebuild_statics(&mut self, device: &wgpu::Device, meshes: &[(u32, u64, &SkinnedMesh)]) {
        let mut rest = Vec::new();
        let mut attributes = Vec::new();
        let mut ranges: Vec<[u32; 4]> = Vec::new();
        let mut morphs = Vec::new();
        let mut neighbors: Vec<[u32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        // Meshes no longer drawn give up their derived data.
        self.derived
            .retain(|index, _| meshes.iter().any(|(i, _, _)| i == index));
        self.slots.clear();
        self.submeshes.clear();
        for &(index, version, mesh) in meshes {
            let vertex_base = rest.len() as u32;
            let index_base = indices.len() as u32;
            let count = mesh.vertices.len() as u32;
            for v in &mesh.vertices {
                rest.push(GpuRest {
                    position: v.position.extend(1.0).to_array(),
                    normal: v.normal.extend(0.0).to_array(),
                    weights: v.weights,
                    joints: v.joints.map(u32::from),
                });
                attributes.push(GpuStatic {
                    uv: v.uv.to_array(),
                    _pad: [0.0; 2],
                    color: v.color.to_array(),
                    pattern: v.pattern.extend(v.fur).to_array(),
                    axis: v.axis.extend(0.0).to_array(),
                    mask: v.mask.to_array(),
                });
            }
            let derived = match self.derived.get(&index) {
                Some((v, d)) if *v == version => d.clone(),
                _ => {
                    let d = Arc::new(Derived::of(mesh));
                    self.derived.insert(index, (version, d.clone()));
                    d
                }
            };
            let (morph_base, neighbor_base) = (morphs.len() as u32, neighbors.len() as u32);
            ranges.extend(
                derived
                    .ranges
                    .iter()
                    .map(|r| [r[0] + morph_base, r[1], r[2] + neighbor_base, r[3]]),
            );
            morphs.extend_from_slice(&derived.morphs);
            neighbors.extend_from_slice(&derived.neighbors);
            // Triangles with an out-of-range index become degenerate rather than dropped, so
            // submesh index ranges stay valid.
            if count > 0 {
                for t in mesh.indices.as_chunks::<3>().0 {
                    if t.iter().all(|&i| i < count) {
                        indices.extend_from_slice(t);
                    } else {
                        indices.extend_from_slice(&[0; 3]);
                    }
                }
            }
            let index_count = indices.len() as u32 - index_base;
            let submesh_base = self.submeshes.len() as u32;
            for sm in &mesh.submeshes {
                let first = sm.first_index.min(index_count);
                let end = sm
                    .first_index
                    .saturating_add(sm.index_count)
                    .min(index_count);
                if end > first {
                    self.submeshes.push(SubmeshSlot {
                        first: index_base + first,
                        count: end - first,
                        surface: u32::try_from(sm.surface).unwrap_or(u32::MAX),
                    });
                }
            }
            let bounds = mesh.bounds();
            self.slots.insert(
                index,
                MeshSlot {
                    version,
                    vertex_base,
                    vertex_count: count,
                    index_base,
                    index_count,
                    submesh_base,
                    submesh_count: self.submeshes.len() as u32 - submesh_base,
                    joints: mesh.joints.len() as u32,
                    morphs: mesh.morph_targets.len() as u32,
                    center: bounds.center(),
                    radius: bounds.half_extents().length(),
                },
            );
        }
        // Storage bindings can't be empty.
        fn non_empty<T: Pod + Zeroable>(v: &mut Vec<T>) {
            if v.is_empty() {
                v.push(T::zeroed());
            }
        }
        non_empty(&mut rest);
        non_empty(&mut attributes);
        non_empty(&mut ranges);
        non_empty(&mut morphs);
        non_empty(&mut neighbors);
        non_empty(&mut indices);
        let storage = |label: &str, bytes: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        self.statics = Some(Statics {
            rest: storage("skin rest vertices", bytemuck::cast_slice(&rest)),
            attributes: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skin attributes"),
                contents: bytemuck::cast_slice(&attributes),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            ranges: storage("skin ranges", bytemuck::cast_slice(&ranges)),
            morphs: storage("skin morphs", bytemuck::cast_slice(&morphs)),
            neighbors: storage("skin neighbors", bytemuck::cast_slice(&neighbors)),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skin indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            }),
        });
        self.compute_group = None;
    }

    fn rebuild_compute_group(&mut self, device: &wgpu::Device) {
        let (Some(p), Some(s), Some(jobs), Some(joints), Some(weights), Some(out)) = (
            &self.pipelines,
            &self.statics,
            &self.jobs,
            &self.joints,
            &self.morph_weights,
            &self.output,
        ) else {
            return;
        };
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        self.compute_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skin compute"),
            layout: &p.compute_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &jobs.buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<GpuJob>() as u64),
                    }),
                },
                entry(1, &s.rest),
                entry(2, &joints.buffer),
                entry(3, &weights.buffer),
                entry(4, &s.ranges),
                entry(5, &s.morphs),
                entry(6, &s.neighbors),
                entry(7, &out.buffer),
            ],
        }));
    }

    /// Bind the buffers every instance shares (instances, indices); false before init.
    fn bind_shared(&self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let (Some(s), Some(instances)) = (&self.statics, &self.instances) else {
            return false;
        };
        pass.set_vertex_buffer(2, instances.buffer.slice(..));
        pass.set_index_buffer(s.indices.slice(..), wgpu::IndexFormat::Uint32);
        true
    }

    /// Bind an instance's slices of the skinned and static vertex buffers; false if there is
    /// nothing to draw.
    fn bind_vertices(&self, pass: &mut wgpu::RenderPass<'_>, item: &Instance) -> bool {
        let (Some(s), Some(out)) = (&self.statics, &self.output) else {
            return false;
        };
        let Some(slot) = self.slots.get(&item.mesh) else {
            return false;
        };
        if slot.index_count == 0 || slot.vertex_count == 0 {
            return false;
        }
        let out_start = item.out_base as u64 * SKINNED_SIZE;
        let out_end = out_start + slot.vertex_count as u64 * SKINNED_SIZE;
        let attr_size = std::mem::size_of::<GpuStatic>() as u64;
        let attr_start = slot.vertex_base as u64 * attr_size;
        let attr_end = attr_start + slot.vertex_count as u64 * attr_size;
        pass.set_vertex_buffer(0, out.buffer.slice(out_start..out_end));
        pass.set_vertex_buffer(1, s.attributes.slice(attr_start..attr_end));
        true
    }

    fn parts_of(&self, item: &Instance) -> &[Part] {
        let start = item.part_base as usize;
        &self.parts[start..start + item.part_count as usize]
    }
}

fn storage_entry(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
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

impl RenderFeature for SkinningFeature {
    fn name(&self) -> &str {
        "skinned meshes"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let compute = wgpu::ShaderStages::COMPUTE;
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skin compute"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<GpuJob>() as u64
                        ),
                    },
                    count: None,
                },
                storage_entry(1, true, compute),
                storage_entry(2, true, compute),
                storage_entry(3, true, compute),
                storage_entry(4, true, compute),
                storage_entry(5, true, compute),
                storage_entry(6, true, compute),
                storage_entry(7, false, compute),
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skin materials"),
            entries: &[storage_entry(0, true, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let compute_shader = gpu.shader("skin compute", SKIN_COMPUTE_WGSL, false);
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("skin compute"),
                bind_group_layouts: &[Some(&compute_layout)],
                immediate_size: 0,
            });
        let compute_pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&compute_pipeline_layout),
                module: &compute_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let skin = compute_pipeline("skin");
        let tension = compute_pipeline("tension");

        let surfaces = SurfaceCache::new(gpu);
        let buffers = [
            Some(skinned_layout()),
            Some(GpuStatic::layout()),
            Some(MeshInstance::layout()),
        ];
        let draw_shader = gpu.shader("skin draw", SKIN_DRAW_WGSL, true);
        let draw_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skin draw"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&material_layout),
                Some(&surfaces.layout),
            ],
            immediate_size: 0,
        });
        let draw_pipeline = |label: &str, cull_mode: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&draw_layout),
                vertex: wgpu::VertexState {
                    module: &draw_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &draw_shader,
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
            })
        };
        let draw = draw_pipeline("skin draw", Some(wgpu::Face::Back));
        let draw_double = draw_pipeline("skin draw double-sided", None);

        let shadow_shader = gpu.shader("skin shadow", SKIN_SHADOW_WGSL, false);
        let shadow_primitive = wgpu::PrimitiveState {
            cull_mode: None,
            unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            ..Default::default()
        };
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skin shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout)],
            immediate_size: 0,
        });
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skin shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: shadow_primitive,
            depth_stencil: Some(gpu.shadow_depth_state()),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let shadow_masked_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skin shadow masked"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&surfaces.layout)],
            immediate_size: 0,
        });
        let shadow_masked = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skin shadow masked"),
            layout: Some(&shadow_masked_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow_masked"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: shadow_primitive,
            depth_stencil: Some(gpu.shadow_depth_state()),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shadow_shader,
                entry_point: Some("fs_shadow_masked"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });

        self.joints = Some(GrowBuffer::new(
            device,
            "skin joints",
            wgpu::BufferUsages::STORAGE,
            64 * 48,
        ));
        self.morph_weights = Some(GrowBuffer::new(
            device,
            "skin morph weights",
            wgpu::BufferUsages::STORAGE,
            256,
        ));
        self.jobs = Some(GrowBuffer::new(
            device,
            "skin jobs",
            wgpu::BufferUsages::UNIFORM,
            16 * JOB_STRIDE,
        ));
        self.instances = Some(GrowBuffer::new(
            device,
            "skin instances",
            wgpu::BufferUsages::VERTEX,
            16 * std::mem::size_of::<MeshInstance>() as u64,
        ));
        self.materials = Some(GrowBuffer::new(
            device,
            "skin materials",
            wgpu::BufferUsages::STORAGE,
            16 * std::mem::size_of::<GpuSkinMaterial>() as u64,
        ));
        self.output = Some(OutputBuffer::new(device, 4096 * SKINNED_SIZE));
        self.pipelines = Some(Pipelines {
            skin,
            tension,
            draw,
            draw_double,
            shadow,
            shadow_masked,
            compute_layout,
            material_layout,
        });
        self.surfaces = Some(surfaces);
        self.rebuild_statics(device, &[]);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        if self.pipelines.is_none() {
            return;
        }
        let mut query = self
            .query
            .take()
            .unwrap_or_else(|| world.query::<SkinQuery>());
        let world_ref: &World = world;
        self.items.clear();
        self.job_data.clear();
        self.job_list.clear();
        self.joint_data.clear();
        self.weight_data.clear();
        self.instance_data.clear();
        self.material_data.clear();
        self.parts.clear();
        for s in &mut self.shadow {
            s.clear();
        }
        if let Some(surfaces) = &mut self.surfaces {
            surfaces.frame += 1;
        }
        let Some(assets) = world_ref.get_resource::<Assets<SkinnedMesh>>() else {
            self.query = Some(query);
            self.stats = FeatureStats::default();
            return;
        };

        // Which meshes are in use, and are they all uploaded?
        let mut used: Vec<(Handle<SkinnedMesh>, u64)> = Vec::new();
        for (_, renderer, _) in query.iter(world_ref) {
            let Some(handle) = renderer.mesh.filter(|_| renderer.visible) else {
                continue;
            };
            let handle = assets.canonical(handle);
            let version = assets.version(handle);
            if version > 0 && assets.contains(handle) && !used.iter().any(|u| u.0 == handle) {
                used.push((handle, version));
            }
        }
        let stale = used
            .iter()
            .any(|(h, v)| self.slots.get(&h.index()).is_none_or(|s| s.version != *v));
        if stale {
            used.sort_unstable_by_key(|u| u.0.index());
            let meshes: Vec<(u32, u64, &SkinnedMesh)> = used
                .iter()
                .filter_map(|&(h, v)| assets.get(h).map(|m| (h.index(), v, m)))
                .collect();
            self.rebuild_statics(&gpu.device, &meshes);
        }

        let images = world_ref.get_resource::<Assets<Image>>();
        let shadows = view.sun_shadows;
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let mut out_base = 0u32;
        let mut culled = 0u32;
        // Fur shells per item, appended after the items' own instances.
        let mut furred: Vec<(usize, u32)> = Vec::new();
        for (gt, renderer, pose) in query.iter(world_ref) {
            let Some(handle) = renderer.mesh.filter(|_| renderer.visible) else {
                continue;
            };
            let index = assets.canonical(handle).index();
            let Some(slot) = self.slots.get(&index).copied() else {
                continue;
            };
            let scale = gt.scale().abs().max_element();
            let center = (gt.transform_point(slot.center.as_dvec3()) - view.origin).as_vec3();
            // Limbs swing beyond the bind-pose bounds.
            let radius = slot.radius * scale * 1.35 + 0.25;
            let visible = view.frustum.intersects_sphere(center, radius);
            let casts = shadows && renderer.cast_shadows;
            if !visible && !casts {
                culled += 1;
                continue;
            }
            let detail = center.length() < renderer.detail_distance;
            let joint_base = self.joint_data.len() as u32;
            for j in 0..slot.joints as usize {
                self.joint_data
                    .push(pose.joints.get(j).copied().unwrap_or(identity));
            }
            let morph_base = self.weight_data.len() as u32;
            for m in 0..slot.morphs as usize {
                self.weight_data
                    .push(pose.morph_weights.get(m).copied().unwrap_or(0.0));
            }
            let flags = if detail && slot.morphs > 0 {
                JOB_MORPHS
            } else {
                0
            } | if detail { JOB_TENSION } else { 0 };
            let job = GpuJob {
                vertex_base: slot.vertex_base,
                vertex_count: slot.vertex_count,
                out_base,
                joint_base,
                morph_base,
                flags,
                _pad: [0; 2],
            };
            self.job_data.extend_from_slice(bytemuck::bytes_of(&job));
            self.job_data
                .resize(self.job_data.len().next_multiple_of(JOB_STRIDE as usize), 0);
            self.job_list.push((slot.vertex_count, detail));

            let material = self.material_data.len() as u32;
            self.material_data
                .push(GpuSkinMaterial::new(&renderer.material));
            let mut instance =
                MeshInstance::new(gt.matrix_relative_to(view.origin), material, 0.0, 1.0);
            instance.extra[1] = flags;
            // Shell 0 (the skin itself); `extra.w` says how many shells cover it.
            let shells = if visible {
                fur_shells(&renderer.material, center.length())
            } else {
                0
            };
            instance.extra[2] = 0;
            instance.extra[3] = shells;
            self.instance_data.push(instance);
            if shells > 0 {
                furred.push((self.items.len(), shells));
            }

            // Draws: one per submesh with its surface, or the whole mesh with the first surface
            // (the default surface for procedural characters).
            self.surface_ids.clear();
            if let Some(surfaces) = &mut self.surfaces {
                for m in &renderer.surfaces {
                    self.surface_ids.push(surfaces.surface(gpu, m, images));
                }
            }
            let part_base = self.parts.len() as u32;
            let submeshes = &self.submeshes
                [slot.submesh_base as usize..(slot.submesh_base + slot.submesh_count) as usize];
            let surface_of = |s: usize| self.surface_ids.get(s).copied().unwrap_or(0);
            let mut push = |first: u32, count: u32, surface: u32| {
                let set = self.surfaces.as_ref().and_then(|c| c.set(surface));
                self.parts.push(Part {
                    first,
                    count,
                    surface,
                    double_sided: set.is_some_and(|s| s.double_sided),
                    masked: set.is_some_and(|s| s.masked),
                });
            };
            if submeshes.is_empty() || self.surface_ids.is_empty() {
                push(slot.index_base, slot.index_count, surface_of(0));
            } else {
                for sm in submeshes {
                    push(sm.first, sm.count, surface_of(sm.surface as usize));
                }
            }
            self.items.push(Instance {
                mesh: index,
                out_base,
                visible,
                casts,
                detail,
                center,
                radius,
                part_base,
                part_count: self.parts.len() as u32 - part_base,
                shell_first: 0,
                shells: 0,
            });
            out_base += slot.vertex_count;
            if !visible {
                culled += 1;
            }
        }
        self.query = Some(query);
        for (i, shells) in furred {
            let first = self.instance_data.len() as u32;
            let base = self.instance_data[i];
            for k in 1..=shells {
                let mut shell = base;
                shell.extra[2] = k;
                self.instance_data.push(shell);
            }
            self.items[i].shell_first = first;
            self.items[i].shells = shells;
        }

        // Sun shadow cascades only. Characters stay out of the top-down sky occlusion map: they
        // don't roof over the ground, and a body there shades its own clothes from the sky,
        // wrinkle by wrinkle, as blotches.
        if shadows {
            let cascades = (view.cascades.count as usize).min(MAX_CASCADES);
            for c in 0..cascades {
                for (i, item) in self.items.iter().enumerate() {
                    if item.casts && view.cascades.sphere_in_cascade(c, item.center, item.radius) {
                        self.shadow[c].push(i as u32);
                    }
                }
            }
        }

        // Upload.
        let device = &gpu.device;
        let queue = &gpu.queue;
        if self.material_data.is_empty() {
            self.material_data
                .push(GpuSkinMaterial::new(&SkinMaterial::default()));
        }
        if self.joint_data.is_empty() {
            self.joint_data.push(identity);
        }
        if self.weight_data.is_empty() {
            self.weight_data.push(0.0);
        }
        if self.job_data.is_empty() {
            self.job_data.resize(JOB_STRIDE as usize, 0);
        }
        let mut regroup = self.compute_group.is_none();
        let needed = (out_base as u64).max(1) * SKINNED_SIZE;
        if self.output.as_ref().is_none_or(|o| o.capacity < needed) {
            self.output = Some(OutputBuffer::new(device, needed));
            regroup = true;
        }
        let write = |b: &mut Option<GrowBuffer>, data: &[u8]| {
            b.as_mut().is_some_and(|b| b.write(device, queue, data))
        };
        regroup |= write(&mut self.jobs, &self.job_data);
        regroup |= write(&mut self.joints, bytemuck::cast_slice(&self.joint_data));
        regroup |= write(
            &mut self.morph_weights,
            bytemuck::cast_slice(&self.weight_data),
        );
        write(
            &mut self.instances,
            bytemuck::cast_slice(&self.instance_data),
        );
        let materials_grown = write(
            &mut self.materials,
            bytemuck::cast_slice(&self.material_data),
        );
        if regroup {
            self.rebuild_compute_group(device);
        }
        if (materials_grown || self.material_group.is_none())
            && let (Some(p), Some(m)) = (&self.pipelines, &self.materials)
        {
            self.material_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("skin materials"),
                layout: &p.material_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: m.buffer.as_entire_binding(),
                }],
            }));
        }

        if let Some(surfaces) = &mut self.surfaces {
            surfaces.evict();
        }

        let mut stats = FeatureStats {
            culled,
            ..Default::default()
        };
        for item in self.items.iter().filter(|i| i.visible) {
            stats.instances += 1;
            for part in self.parts_of(item) {
                stats.draw_calls += 1;
                stats.triangles += (part.count / 3) as u64;
                if item.shells > 0 && !part.masked {
                    stats.draw_calls += 1;
                    stats.triangles += (part.count / 3) as u64 * item.shells as u64;
                }
            }
        }
        for list in &self.shadow {
            for &i in list {
                stats.draw_calls += self.items[i as usize].part_count;
            }
        }
        self.stats = stats;
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, _ctx: &FrameContext) {
        let (Some(p), Some(group)) = (&self.pipelines, &self.compute_group) else {
            return;
        };
        if self.items.is_empty() {
            return;
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("skin"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&p.skin);
            for (j, (count, _)) in self.job_list.iter().enumerate() {
                pass.set_bind_group(0, group, &[(j as u64 * JOB_STRIDE) as u32]);
                pass.dispatch_workgroups(count.div_ceil(WORKGROUP), 1, 1);
            }
        }
        if self.job_list.iter().any(|(_, detail)| *detail) {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("skin tension"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&p.tension);
            for (j, (count, detail)) in self.job_list.iter().enumerate() {
                if *detail {
                    pass.set_bind_group(0, group, &[(j as u64 * JOB_STRIDE) as u32]);
                    pass.dispatch_workgroups(count.div_ceil(WORKGROUP), 1, 1);
                }
            }
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        let (Some(p), Some(group), Some(surfaces)) =
            (&self.pipelines, &self.material_group, &self.surfaces)
        else {
            return;
        };
        if !self.items.iter().any(|i| i.visible) || !self.bind_shared(pass) {
            return;
        }
        pass.set_bind_group(1, group, &[]);
        // Single-sided surfaces first (back faces culled), then double-sided ones.
        for (double_sided, pipeline) in [(false, &p.draw), (true, &p.draw_double)] {
            let mut pipeline_set = false;
            let mut current = u32::MAX;
            for (n, item) in self.items.iter().enumerate() {
                if !item.visible {
                    continue;
                }
                let parts = self.parts_of(item);
                if !parts.iter().any(|p| p.double_sided == double_sided)
                    || !self.bind_vertices(pass, item)
                {
                    continue;
                }
                if !pipeline_set {
                    pass.set_pipeline(pipeline);
                    pipeline_set = true;
                }
                let n = n as u32;
                for part in parts.iter().filter(|p| p.double_sided == double_sided) {
                    if part.surface != current
                        && let Some(set) = surfaces.set(part.surface)
                    {
                        pass.set_bind_group(2, &set.group, &[]);
                        current = part.surface;
                    }
                    pass.draw_indexed(part.first..part.first + part.count, 0, n..n + 1);
                }
            }
        }
        // Fur shells over the opaque skin (hair cards and other cutouts grow none).
        let mut pipeline_set = false;
        let mut current = u32::MAX;
        for item in self.items.iter().filter(|i| i.visible && i.shells > 0) {
            if !self.bind_vertices(pass, item) {
                continue;
            }
            if !pipeline_set {
                pass.set_pipeline(&p.draw);
                pipeline_set = true;
            }
            let shells = item.shell_first..item.shell_first + item.shells;
            for part in self.parts_of(item).iter().filter(|p| !p.masked) {
                if part.surface != current
                    && let Some(set) = surfaces.set(part.surface)
                {
                    pass.set_bind_group(2, &set.group, &[]);
                    current = part.surface;
                }
                pass.draw_indexed(part.first..part.first + part.count, 0, shells.clone());
            }
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let (Some(p), Some(surfaces)) = (&self.pipelines, &self.surfaces) else {
            return;
        };
        let list = &self.shadow[cascade];
        if list.is_empty() || !self.bind_shared(pass) {
            return;
        }
        // Opaque surfaces: depth only; adjacent index ranges merge into one draw.
        pass.set_pipeline(&p.shadow);
        let mut any_masked = false;
        for &i in list {
            let item = &self.items[i as usize];
            let parts = self.parts_of(item);
            any_masked |= parts.iter().any(|p| p.masked);
            if parts.iter().all(|p| p.masked) || !self.bind_vertices(pass, item) {
                continue;
            }
            let mut run: Option<std::ops::Range<u32>> = None;
            for part in parts.iter().filter(|p| !p.masked) {
                match &mut run {
                    Some(r) if r.end == part.first => r.end += part.count,
                    _ => {
                        if let Some(r) = run.take() {
                            pass.draw_indexed(r, 0, i..i + 1);
                        }
                        run = Some(part.first..part.first + part.count);
                    }
                }
            }
            if let Some(r) = run {
                pass.draw_indexed(r, 0, i..i + 1);
            }
        }
        if !any_masked {
            return;
        }
        // Alpha-masked surfaces: cut out by base color alpha, like in the scene.
        pass.set_pipeline(&p.shadow_masked);
        let mut current = u32::MAX;
        for &i in list {
            let item = &self.items[i as usize];
            let parts = self.parts_of(item);
            if !parts.iter().any(|p| p.masked) || !self.bind_vertices(pass, item) {
                continue;
            }
            for part in parts.iter().filter(|p| p.masked) {
                if part.surface != current
                    && let Some(set) = surfaces.set(part.surface)
                {
                    pass.set_bind_group(1, &set.group, &[]);
                    current = part.surface;
                }
                pass.draw_indexed(part.first..part.first + part.count, 0, i..i + 1);
            }
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "meshes": self.slots.len(),
            "instances": self.items.len(),
            "detailed": self.items.iter().filter(|i| i.detail).count(),
            "surfaces": self.surfaces.as_ref().map_or(0, |c| c.sets.iter().flatten().count()),
            "images": self.surfaces.as_ref().map_or(0, SurfaceCache::image_count),
            "skinned_vertices": self.job_list.iter().map(|(c, _)| *c as u64).sum::<u64>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::features::{compose_shader, validate_wgsl};

    #[test]
    fn shaders_validate() {
        validate_wgsl("skin compute", &compose_shader(SKIN_COMPUTE_WGSL, false)).unwrap();
        validate_wgsl("skin draw", &compose_shader(SKIN_DRAW_WGSL, true)).unwrap();
        validate_wgsl("skin shadow", &compose_shader(SKIN_SHADOW_WGSL, false)).unwrap();
    }

    #[test]
    fn gpu_structs_match_the_shaders() {
        assert_eq!(std::mem::size_of::<GpuRest>(), 64);
        assert_eq!(std::mem::size_of::<GpuStatic>(), 80);
        assert_eq!(std::mem::size_of::<GpuMorph>(), 32);
        assert_eq!(std::mem::size_of::<GpuJob>(), 32);
        assert_eq!(std::mem::size_of::<GpuSkinMaterial>(), 112);
        assert!(std::mem::size_of::<GpuJob>() as u64 <= JOB_STRIDE);
        // `struct SkinSurface` (uniform layout: four 16-byte rows).
        assert_eq!(std::mem::size_of::<GpuSurface>(), 64);
        assert_eq!(std::mem::offset_of!(GpuSurface, base_color), 0);
        assert_eq!(std::mem::offset_of!(GpuSurface, params), 16);
        assert_eq!(std::mem::offset_of!(GpuSurface, extra), 32);
        assert_eq!(std::mem::offset_of!(GpuSurface, flags), 48);
    }

    #[test]
    fn fur_shells_thin_out_with_distance() {
        let fur = SkinMaterial {
            fur_length: 0.015,
            fur_shells: 16,
            fur_lod: 8.0,
            ..Default::default()
        };
        assert_eq!(fur_shells(&fur, 2.0), 16);
        assert_eq!(fur_shells(&fur, 8.0), 16);
        assert_eq!(fur_shells(&fur, 12.0), 8);
        assert_eq!(fur_shells(&fur, 16.0), 0);
        assert_eq!(
            fur_shells(&SkinMaterial::default(), 1.0),
            0,
            "no fur, no shells"
        );
        let gpu = GpuSkinMaterial::new(&fur);
        assert_eq!(gpu.fur[0], 0.015);
        assert_eq!(gpu.fur_tip[3], fur.fur_root);
    }

    #[test]
    fn surface_flags_and_factors_follow_the_material() {
        let plain = GpuSurface::new(&StandardMaterial::default());
        assert_eq!(plain.flags[0], SURFACE_STANDARD);
        assert!(!plain.masked() && !plain.double_sided());
        assert_eq!(plain.params[3], 0.0, "opaque surfaces have no cutout");

        let hair = StandardMaterial {
            base_color_texture: Some(Handle::from_index(1)),
            metallic_roughness_texture: Some(Handle::from_index(2)),
            double_sided: true,
            normal_scale: 0.5,
            ..StandardMaterial::default().with_alpha_mask(0.4)
        }
        .with_uv_scale(2.0, 3.0)
        .with_roughness(0.7);
        let g = GpuSurface::new(&hair);
        assert_eq!(
            g.flags[0],
            SURFACE_STANDARD
                | SURFACE_DOUBLE_SIDED
                | SURFACE_BASE_COLOR
                | SURFACE_METALLIC_ROUGHNESS
        );
        assert!(g.masked() && g.double_sided());
        assert_eq!(g.params, [0.7, 0.0, 0.5, 0.4]);
        assert_eq!(&g.extra[..2], &[2.0, 3.0]);
        // Blended surfaces have no transparent pass here: cut out at half alpha.
        let blend = GpuSurface::new(&StandardMaterial::default().with_alpha(0.3));
        assert_eq!(blend.params[3], 0.5);
        // The procedural default turns every texture and the standard factors off.
        assert_eq!(GpuSurface::PROCEDURAL.flags[0], 0);
        assert_eq!(GpuSurface::PROCEDURAL.extra[3], 0.5);
    }

    #[test]
    fn vertex_attributes_point_at_their_fields() {
        let offsets: Vec<(u32, u64)> = GpuStatic::ATTRIBUTES
            .iter()
            .map(|a| (a.shader_location, a.offset))
            .collect();
        assert_eq!(
            offsets,
            vec![
                (2, std::mem::offset_of!(GpuStatic, uv) as u64),
                (3, std::mem::offset_of!(GpuStatic, color) as u64),
                (11, std::mem::offset_of!(GpuStatic, pattern) as u64),
                (12, std::mem::offset_of!(GpuStatic, axis) as u64),
                (13, std::mem::offset_of!(GpuStatic, mask) as u64),
            ]
        );
    }
}
