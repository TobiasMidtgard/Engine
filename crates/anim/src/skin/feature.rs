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
//! * **Images** are uploaded once per (image, color space) with their whole mip chain and
//!   re-uploaded when the asset's version changes. Every character that uses an image shares
//!   the upload.
//! * **Surface bind groups** are cached by what they are built from (canonical image handles
//!   and the factors), so characters with the same materials share them. A group is rebuilt
//!   only when an image it binds changes (loaded, edited, removed).
//! * Images and groups unused for `EVICT_AFTER_FRAMES` (600) frames are released.
//!
//! Single-sided and double-sided surfaces use two scene pipelines (back-face culling on/off;
//! double-sided ones shade the side the camera sees). Shadows are never culled, like the
//! standard mesh shadows; alpha-masked surfaces cast them through a pipeline that discards by
//! base color alpha.

use std::collections::HashMap;

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

use engine_render::{AlphaMode, Image, PixelFormat, StandardMaterial};

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
/// Surface bind groups and images unused for this many frames are released.
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
        }
    }
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

struct GpuImage {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    version: u64,
    last_used: u64,
}

/// One cached surface bind group.
struct SurfaceSet {
    group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
    key: SurfaceKey,
    /// (image index, version, sRGB view) bound per slot; `None` = the default texture (no
    /// image, not loaded yet, or not uploadable).
    bound: [Option<(u32, u64, bool)>; SURFACE_SLOTS],
    double_sided: bool,
    masked: bool,
    /// Frame its images were last checked (once per frame, however many characters use it).
    checked: u64,
}

/// GPU images and surface bind groups shared by every character (see the module docs).
struct SurfaceCache {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    defaults: Vec<(wgpu::Texture, wgpu::TextureView)>,
    /// By (canonical image index, sRGB view).
    images: HashMap<(u32, bool), GpuImage>,
    /// Images that could not be uploaded, by (image index, sRGB view) → asset version, so a
    /// broken image is reported once, not every frame.
    failed: HashMap<(u32, bool), u64>,
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
            .map(|(slot, texel)| {
                let srgb = SURFACE_COLOR_SLOT[slot];
                upload_image(gpu, &Image::solid(*texel, srgb), srgb).expect("1x1 default texture")
            })
            .collect();
        let mut cache = Self {
            layout,
            sampler,
            defaults,
            images: HashMap::new(),
            failed: HashMap::new(),
            sets: Vec::new(),
            ids: HashMap::new(),
            free: Vec::new(),
            frame: 0,
        };
        let key = SurfaceKey {
            textures: [None; SURFACE_SLOTS],
            factors: bytemuck::cast(GpuSurface::PROCEDURAL),
        };
        let default = cache.build(gpu, key, [None; SURFACE_SLOTS], GpuSurface::PROCEDURAL);
        cache.sets.push(Some(default));
        cache
    }

    /// Make sure the image is on the GPU with the given color space; false if it cannot be
    /// uploaded (invalid data, too large).
    fn ensure_image(
        &mut self,
        gpu: &GpuShared,
        index: u32,
        image: &Image,
        version: u64,
        srgb: bool,
    ) -> bool {
        let frame = self.frame;
        if let Some(existing) = self.images.get_mut(&(index, srgb))
            && existing.version == version
        {
            existing.last_used = frame;
            return true;
        }
        if self.failed.get(&(index, srgb)) == Some(&version) {
            return false;
        }
        match upload_image(gpu, image, srgb) {
            Some((texture, view)) => {
                self.failed.remove(&(index, srgb));
                self.images.insert(
                    (index, srgb),
                    GpuImage {
                        _texture: texture,
                        view,
                        version,
                        last_used: frame,
                    },
                );
                true
            }
            None => {
                self.images.remove(&(index, srgb));
                self.failed.insert((index, srgb), version);
                false
            }
        }
    }

    /// Surface id for a material, uploading its images and building (or rebuilding) its bind
    /// group when needed.
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
        // First use this frame: are the images uploaded and current?
        let mut bound = [None; SURFACE_SLOTS];
        for (slot, handle) in handles.iter().enumerate() {
            let (Some(handle), Some(images)) = (handle, images) else {
                continue;
            };
            let version = images.version(*handle);
            let Some(image) = images.get(*handle) else {
                continue;
            };
            let srgb = SURFACE_COLOR_SLOT[slot] && image.srgb && image.format == PixelFormat::Rgba8;
            if self.ensure_image(gpu, handle.index(), image, version, srgb) {
                bound[slot] = Some((handle.index(), version, srgb));
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
                let set = self.build(gpu, key, bound, uniform);
                self.sets[id as usize] = Some(set);
            }
        }
        id
    }

    fn build(
        &self,
        gpu: &GpuShared,
        key: SurfaceKey,
        bound: [Option<(u32, u64, bool)>; SURFACE_SLOTS],
        uniform: GpuSurface,
    ) -> SurfaceSet {
        let buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skin surface"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let views: [&wgpu::TextureView; SURFACE_SLOTS] = std::array::from_fn(|slot| {
            bound[slot]
                .and_then(|(index, _, srgb)| self.images.get(&(index, srgb)))
                .map(|i| &i.view)
                .unwrap_or(&self.defaults[slot].1)
        });
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

    /// Release surfaces and images nobody used for a while.
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
        self.images
            .retain(|_, image| image.last_used + EVICT_AFTER_FRAMES >= frame);
    }

    fn set(&self, id: u32) -> Option<&SurfaceSet> {
        self.sets
            .get(id as usize)
            .and_then(Option::as_ref)
            .or_else(|| self.sets.first().and_then(Option::as_ref))
    }
}

/// Upload an image with its mip chain (the same rules as the standard mesh feature). `srgb`
/// selects an sRGB format for RGBA8 data; mips are regenerated in the matching color space
/// when the stored ones don't match. Levels larger than the device allows are skipped.
fn upload_image(
    gpu: &GpuShared,
    image: &Image,
    srgb: bool,
) -> Option<(wgpu::Texture, wgpu::TextureView)> {
    if !image.is_valid() {
        tracing::warn!(
            "image data or mip chain does not match its size ({image:?}); not uploading it"
        );
        return None;
    }
    let format = match image.format {
        PixelFormat::Rgba8 if srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        PixelFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
        PixelFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
    };
    let generated;
    let mips: &[Vec<u8>] = if !image.mipmaps {
        &[]
    } else if !image.mips.is_empty() && (image.format != PixelFormat::Rgba8 || srgb == image.srgb) {
        &image.mips
    } else {
        generated = image.mip_chain(srgb);
        &generated
    };
    let mut levels: Vec<(u32, u32, &[u8])> = Vec::with_capacity(mips.len() + 1);
    levels.push((image.width, image.height, &image.data));
    for (i, m) in mips.iter().enumerate() {
        let (w, h) = image.level_size(i as u32 + 1);
        levels.push((w, h, m));
    }
    let max = gpu.device.limits().max_texture_dimension_2d;
    let skip = levels
        .iter()
        .take_while(|(w, h, _)| *w > max || *h > max)
        .count();
    let levels = &levels[skip.min(levels.len())..];
    let Some(&(width, height, _)) = levels.first() else {
        tracing::warn!(
            "image {}x{} is larger than the GPU allows ({max})",
            image.width,
            image.height
        );
        return None;
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skin surface image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bpp = image.format.bytes_per_pixel() as u32;
    for (level, (w, h, data)) in levels.iter().enumerate() {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bpp),
                rows_per_image: Some(*h),
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Some((texture, view))
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

impl SkinningFeature {
    /// Rebuild the shared static buffers from every mesh in `meshes` (canonical index → mesh).
    fn rebuild_statics(&mut self, device: &wgpu::Device, meshes: &[(u32, u64, &SkinnedMesh)]) {
        let mut rest = Vec::new();
        let mut attributes = Vec::new();
        let mut ranges: Vec<[u32; 4]> = Vec::new();
        let mut morphs = Vec::new();
        let mut neighbors: Vec<[u32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
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
                    pattern: v.pattern.extend(0.0).to_array(),
                    axis: v.axis.extend(0.0).to_array(),
                    mask: v.mask.to_array(),
                });
            }
            // Morphs grouped per vertex (CSR), so the skin pass gathers without atomics.
            let mut per_vertex: Vec<Vec<GpuMorph>> = vec![Vec::new(); mesh.vertices.len()];
            for (t, target) in mesh.morph_targets.iter().enumerate() {
                for d in &target.deltas {
                    if let Some(list) = per_vertex.get_mut(d.vertex as usize) {
                        list.push(GpuMorph {
                            position: d.position.extend(d.influence).to_array(),
                            normal: d.normal.extend(f32::from_bits(t as u32)).to_array(),
                        });
                    }
                }
            }
            let adjacency = mesh.neighbors();
            for (list, adj) in per_vertex.iter().zip(&adjacency) {
                ranges.push([
                    morphs.len() as u32,
                    list.len() as u32,
                    neighbors.len() as u32,
                    adj.len() as u32,
                ]);
                morphs.extend_from_slice(list);
                neighbors.extend(adj.iter().map(|(n, l)| [*n, l.to_bits()]));
            }
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
            self.instance_data.push(instance);

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
            });
            out_base += slot.vertex_count;
            if !visible {
                culled += 1;
            }
        }
        self.query = Some(query);

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
            "images": self.surfaces.as_ref().map_or(0, |c| c.images.len()),
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
        assert_eq!(std::mem::size_of::<GpuSkinMaterial>(), 80);
        assert!(std::mem::size_of::<GpuJob>() as u64 <= JOB_STRIDE);
        // `struct SkinSurface` (uniform layout: four 16-byte rows).
        assert_eq!(std::mem::size_of::<GpuSurface>(), 64);
        assert_eq!(std::mem::offset_of!(GpuSurface, base_color), 0);
        assert_eq!(std::mem::offset_of!(GpuSurface, params), 16);
        assert_eq!(std::mem::offset_of!(GpuSurface, extra), 32);
        assert_eq!(std::mem::offset_of!(GpuSurface, flags), 48);
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
