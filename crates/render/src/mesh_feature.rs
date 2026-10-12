//! The standard mesh feature: draws every entity with a [`MeshRenderer`].
//!
//! Per frame it culls against the camera frustum and each shadow cascade, groups instances
//! by (pipeline, texture set, mesh), and draws each group with one instanced call. Material
//! factors live in one storage buffer indexed per instance (group 1), so thousands of objects
//! with different untextured materials still batch by mesh.
//!
//! Textures are bound per *texture set* (group 2: base color, normal, metallic/roughness,
//! emissive, occlusion + one sampler). Materials that use the same images share a set; every
//! untextured material uses the default set of 1×1 textures, so untextured scenes batch
//! exactly as before. Images are uploaded (with mipmaps) the first time a material uses them
//! and re-uploaded when their asset version changes (hot reload, `Assets::get_mut`).
//!
//! Shadows: opaque casters use a depth-only pipeline; alpha-masked casters use a pipeline with
//! a fragment stage that discards by base color alpha, so foliage cards cast leaf-shaped
//! shadows.
//!
//! Depth prepass: opaque batches are drawn a first time with `vs_prepass`/`fs_prepass`, writing
//! depth, normals, roughness and motion vectors. Motion comes from each entity's previous
//! transform (kept per entity), so moving objects stay sharp under temporal anti-aliasing.
//!
//! Mirrored instances (a transform with a negative determinant, e.g. `scale.x = -1`, common in
//! glTF files) flip their triangles' winding; they are drawn with clockwise-front pipelines so
//! back-face culling keeps the outside, as glTF requires.
//!
//! Dense opaque meshes (at least `RenderSettings::virtual_geometry_min_triangles`) are drawn
//! as clusters by [`ClusterRenderer`] once their cluster hierarchy is built, with the same
//! materials, texture sets and shading (see [`crate::virtual_geometry`]).

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_assets::Assets;
use engine_core::prelude::*;
use wgpu::util::DeviceExt;

use crate::components::{MeshRenderer, ResolvedMesh};
use crate::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase,
};
use crate::frame::ViewData;
use crate::material::{AlphaMode, GpuMaterial, StandardMaterial, TEXTURE_FLAG_BASE, TEXTURE_FLAGS};
use crate::mesh::{Mesh, Vertex};
use crate::settings::RenderSettings;
use crate::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};
use crate::texture::{Image, PixelFormat};
use crate::virtual_geometry::{ClusterInstance, ClusterRenderer};

/// Per-instance vertex data (locations 4–10 in `mesh.wgsl`, plus 11–13 for the previous
/// matrix in the mesh feature's own pipelines).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct MeshInstance {
    /// Camera-relative model matrix rows.
    pub m: [[f32; 4]; 3],
    /// Normal matrix columns.
    pub n: [[f32; 4]; 3],
    /// Material index, flags, mesh min y bits, mesh height bits.
    pub extra: [u32; 4],
    /// Last frame's model matrix rows, relative to last frame's camera (motion vectors).
    pub prev: [[f32; 4]; 3],
}

impl MeshInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
        4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
        7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
        10 => Uint32x4,
        11 => Float32x4, 12 => Float32x4, 13 => Float32x4,
    ];

    /// The instance layout other features share: model matrix, normal matrix and `extra` at
    /// locations 4–10. The previous matrix is in the buffer but not bound, which leaves
    /// locations 11 and up free for the feature's own vertex data.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES[..7],
        }
    }

    /// [`MeshInstance::layout`] plus the previous model matrix at locations 11–13, for
    /// pipelines that write motion vectors.
    pub fn layout_with_motion() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// Build from a camera-relative model matrix (not moving: last frame's matrix is the same
    /// world placement; see [`MeshInstance::with_previous`]).
    pub fn new(model: Mat4, material: u32, min_y: f32, height: f32) -> Self {
        let normal = Mat3::from_mat4(model).inverse().transpose();
        let rows = |m: Mat4| {
            [
                m.row(0).to_array(),
                m.row(1).to_array(),
                m.row(2).to_array(),
            ]
        };
        Self {
            m: rows(model),
            n: [
                normal.x_axis.extend(0.0).to_array(),
                normal.y_axis.extend(0.0).to_array(),
                normal.z_axis.extend(0.0).to_array(),
            ],
            extra: [material, 0, min_y.to_bits(), height.to_bits()],
            prev: rows(model),
        }
    }

    /// Set last frame's model matrix, relative to last frame's camera.
    pub fn with_previous(mut self, prev_model: Mat4) -> Self {
        self.prev = [
            prev_model.row(0).to_array(),
            prev_model.row(1).to_array(),
            prev_model.row(2).to_array(),
        ];
        self
    }
}

/// Number of texture slots per material (group 2 bindings 0..5; the sampler is binding 5).
pub const TEXTURE_SLOTS: usize = 5;
/// Slots holding color (decoded as sRGB when the image is sRGB): base color and emissive.
const COLOR_SLOT: [bool; TEXTURE_SLOTS] = [true, false, false, true, false];
/// Default 1×1 textures: white base color, flat normal, neutral metallic/roughness (factors
/// pass through), black emissive, white (unoccluded) AO.
const DEFAULT_TEXELS: [[u8; 4]; TEXTURE_SLOTS] = [
    [255, 255, 255, 255],
    [128, 128, 255, 255],
    [255, 255, 255, 255],
    [0, 0, 0, 255],
    [255, 255, 255, 255],
];
/// Texture sets and images unused for this many frames are released.
const EVICT_AFTER_FRAMES: u64 = 600;

struct GpuMesh {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    version: u64,
    center: Vec3,
    half_extents: Vec3,
    min_y: f32,
    height: f32,
    /// Last frame the instanced path drew it.
    last_used: u64,
}

struct GpuImage {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    version: u64,
    last_used: u64,
}

/// A group-2 bind group for one combination of images.
struct TextureSet {
    group: wgpu::BindGroup,
    key: [Option<u32>; TEXTURE_SLOTS],
    /// (image index, version, sRGB view) actually bound per slot; `None` = default texture
    /// (no image, or not loaded yet).
    bound: [Option<(u32, u64, bool)>; TEXTURE_SLOTS],
    last_used: u64,
}

impl TextureSet {
    /// Texture-present flag bits for the slots bound to real images.
    fn flags(&self) -> u32 {
        self.bound
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_some())
            .fold(0, |f, (i, _)| f | (TEXTURE_FLAG_BASE << i))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BatchKey {
    pipeline: u8,
    textures: u32,
    mesh: u32,
}

#[derive(Debug, Clone, Copy)]
struct Batch {
    key: BatchKey,
    first: u32,
    count: u32,
}

#[derive(Clone, Copy)]
struct Item {
    mesh: u32,
    pipeline: u8,
    textures: u32,
    masked: bool,
    visible: bool,
    casts: bool,
    center: Vec3,
    radius: f32,
    dist2: f32,
    instance: MeshInstance,
}

type MeshQuery = (
    Entity,
    &'static GlobalTransform,
    &'static MeshRenderer,
    &'static ResolvedMesh,
);

/// Draws [`MeshRenderer`] entities. Added by `RenderPlugin`.
#[derive(Default)]
pub struct MeshFeature {
    /// Indexed by `PIPE_*` bits: double-sided, blend, mirrored (clockwise front faces).
    pipelines: Vec<wgpu::RenderPipeline>,
    /// Depth prepass pipelines, indexed like `pipelines` (blend variants unused).
    prepass_pipelines: Vec<wgpu::RenderPipeline>,
    /// Each entity's world transform last frame, and the frame it was seen (motion vectors).
    previous: HashMap<Entity, (GlobalTransform, u64)>,
    /// Camera position last frame.
    prev_origin: Option<DVec3>,
    /// Depth-only shadow pipeline (opaque casters).
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    /// Alpha-tested shadow pipeline (masked casters).
    shadow_masked_pipeline: Option<wgpu::RenderPipeline>,
    material_layout: Option<wgpu::BindGroupLayout>,
    texture_layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    default_textures: Vec<(wgpu::Texture, wgpu::TextureView)>,
    materials: Option<GrowBuffer>,
    material_group: Option<wgpu::BindGroup>,
    instances: Option<GrowBuffer>,
    meshes: HashMap<u32, GpuMesh>,
    /// Uploaded images by (image index, sRGB view).
    images: HashMap<(u32, bool), GpuImage>,
    /// Images that could not be uploaded, by (image index, sRGB view) → asset version, so a
    /// bad image is reported once per version instead of every frame.
    failed_images: HashMap<(u32, bool), u64>,
    /// Texture sets by id; id 0 is the shared default (untextured) set.
    texture_sets: Vec<Option<TextureSet>>,
    texture_set_ids: HashMap<[Option<u32>; TEXTURE_SLOTS], u32>,
    free_set_ids: Vec<u32>,
    frame: u64,
    opaque: Vec<Batch>,
    transparent: Vec<Batch>,
    /// Per shadow cascade, then the sky occlusion map (index `SKY_OCCLUSION_VIEW`).
    shadow: [Vec<Batch>; MAX_CASCADES + 1],
    query: Option<QueryState<MeshQuery>>,
    items: Vec<Item>,
    keys: Vec<BatchKey>,
    casters: Vec<u32>,
    instance_data: Vec<MeshInstance>,
    material_data: Vec<GpuMaterial>,
    /// Material handle index → (material buffer slot, texture set id), rebuilt every frame.
    material_index: HashMap<u32, (u32, u32)>,
    /// Dense meshes drawn as clusters (virtual geometry).
    clusters: Option<ClusterRenderer>,
    /// Meshes drawn as clusters this frame.
    clustered: HashSet<u32>,
    stats: FeatureStats,
}

const PIPE_DOUBLE: u8 = 1;
const PIPE_BLEND: u8 = 2;
/// Clockwise front faces, for instances whose transform mirrors them.
const PIPE_MIRROR: u8 = 4;
const PIPELINE_VARIANTS: u8 = 8;
/// Shadow pipelines: depth only / alpha masked.
const SHADOW_DEPTH: u8 = 0;
const SHADOW_MASKED: u8 = 1;

impl MeshFeature {
    fn upload_mesh(&mut self, device: &wgpu::Device, index: u32, mesh: &Mesh, version: u64) {
        if mesh.indices.is_empty() || mesh.vertices.is_empty() {
            self.meshes.remove(&index);
            return;
        }
        let aabb = mesh.aabb();
        let vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.insert(
            index,
            GpuMesh {
                vertex,
                index: index_buffer,
                index_count: mesh.indices.len() as u32,
                version,
                center: aabb.center(),
                half_extents: aabb.half_extents(),
                min_y: aabb.min.y,
                height: (aabb.max.y - aabb.min.y).max(1e-3),
                last_used: self.frame,
            },
        );
    }

    /// Material buffer slot and texture set id of a material, added to this frame's materials
    /// on first use.
    fn material_slot(
        &mut self,
        gpu: &GpuShared,
        handle: u32,
        material: &StandardMaterial,
        images: Option<&Assets<Image>>,
    ) -> (u32, u32) {
        if let Some(v) = self.material_index.get(&handle) {
            return *v;
        }
        let textures = self.texture_set_for(gpu, material, images);
        let mut gpu_material = material.to_gpu();
        // Only claim the slots that have a real image bound (others may still load).
        let bound = self
            .texture_sets
            .get(textures as usize)
            .and_then(Option::as_ref)
            .map_or(0, TextureSet::flags);
        gpu_material.flags[0] = (gpu_material.flags[0] & !TEXTURE_FLAGS) | bound;
        let slot = self.material_data.len() as u32;
        self.material_data.push(gpu_material);
        self.material_index.insert(handle, (slot, textures));
        (slot, textures)
    }

    fn batch(keys: &[BatchKey], first: u32, out: &mut Vec<Batch>) {
        let mut i = 0;
        while i < keys.len() {
            let key = keys[i];
            let start = i;
            while i < keys.len() && keys[i] == key {
                i += 1;
            }
            out.push(Batch {
                key,
                first: first + start as u32,
                count: (i - start) as u32,
            });
        }
    }

    /// Make sure the image is on the GPU with the given color space; returns false if it
    /// cannot be uploaded (invalid data, too large).
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
        if self.failed_images.get(&(index, srgb)) == Some(&version) {
            return false;
        }
        match upload_image(gpu, image, srgb) {
            Some((texture, view)) => {
                self.failed_images.remove(&(index, srgb));
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
                self.failed_images.insert((index, srgb), version);
                false
            }
        }
    }

    /// Texture set id for a material (0 for untextured materials), creating or rebuilding the
    /// bind group when its images changed.
    fn texture_set_for(
        &mut self,
        gpu: &GpuShared,
        material: &StandardMaterial,
        images: Option<&Assets<Image>>,
    ) -> u32 {
        let mut handles = material.textures();
        if handles.iter().all(Option::is_none) {
            return 0;
        }
        // Two handles naming the same image (a forwarded reservation) share one upload.
        if let Some(images) = images {
            for h in handles.iter_mut().flatten() {
                *h = images.canonical(*h);
            }
        }
        let key = handles.map(|h| h.map(|h| h.index()));
        let mut bound: [Option<(u32, u64, bool)>; TEXTURE_SLOTS] = [None; TEXTURE_SLOTS];
        for (slot, handle) in handles.iter().enumerate() {
            let (Some(handle), Some(images)) = (handle, images) else {
                continue;
            };
            let version = images.version(*handle);
            let Some(image) = images.get(*handle) else {
                continue;
            };
            let srgb = COLOR_SLOT[slot] && image.srgb && image.format == PixelFormat::Rgba8;
            if self.ensure_image(gpu, handle.index(), image, version, srgb) {
                bound[slot] = Some((handle.index(), version, srgb));
            }
        }
        let id = match self.texture_set_ids.get(&key) {
            Some(id) => *id,
            None => {
                let id = self
                    .free_set_ids
                    .pop()
                    .unwrap_or(self.texture_sets.len() as u32);
                self.texture_set_ids.insert(key, id);
                id
            }
        };
        let needs_build = self
            .texture_sets
            .get(id as usize)
            .and_then(Option::as_ref)
            .is_none_or(|s| s.bound != bound);
        if needs_build {
            let views: [&wgpu::TextureView; TEXTURE_SLOTS] = std::array::from_fn(|slot| {
                bound[slot]
                    .and_then(|(index, _, srgb)| self.images.get(&(index, srgb)))
                    .map(|i| &i.view)
                    .unwrap_or(&self.default_textures[slot].1)
            });
            let (Some(layout), Some(sampler)) = (&self.texture_layout, &self.sampler) else {
                return 0;
            };
            let group = create_texture_group(&gpu.device, layout, sampler, views);
            if self.texture_sets.len() <= id as usize {
                self.texture_sets.resize_with(id as usize + 1, || None);
            }
            self.texture_sets[id as usize] = Some(TextureSet {
                group,
                key,
                bound,
                last_used: self.frame,
            });
        } else if let Some(Some(set)) = self.texture_sets.get_mut(id as usize) {
            set.last_used = self.frame;
        }
        id
    }

    /// Release texture sets and images nobody used for a while.
    fn evict(&mut self) {
        let frame = self.frame;
        for id in 1..self.texture_sets.len() {
            let stale = self.texture_sets[id]
                .as_ref()
                .is_some_and(|s| s.last_used + EVICT_AFTER_FRAMES < frame);
            if stale && let Some(set) = self.texture_sets[id].take() {
                self.texture_set_ids.remove(&set.key);
                self.free_set_ids.push(id as u32);
            }
        }
        self.images
            .retain(|_, image| image.last_used + EVICT_AFTER_FRAMES >= frame);
    }

    fn texture_group(&self, id: u32) -> Option<&wgpu::BindGroup> {
        self.texture_sets
            .get(id as usize)
            .and_then(Option::as_ref)
            .or_else(|| self.texture_sets.first().and_then(Option::as_ref))
            .map(|s| &s.group)
    }

    fn draw_batches<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        batches: &[Batch],
        pipeline_for: impl Fn(u8) -> Option<&'a wgpu::RenderPipeline>,
        bind_textures: impl Fn(u8) -> bool,
    ) {
        let mut current_pipeline = u8::MAX;
        let mut current_textures = u32::MAX;
        for b in batches {
            let Some(mesh) = self.meshes.get(&b.key.mesh) else {
                continue;
            };
            let Some(pipeline) = pipeline_for(b.key.pipeline) else {
                continue;
            };
            if b.key.pipeline != current_pipeline {
                pass.set_pipeline(pipeline);
                current_pipeline = b.key.pipeline;
                current_textures = u32::MAX;
            }
            if bind_textures(b.key.pipeline) && b.key.textures != current_textures {
                if let Some(group) = self.texture_group(b.key.textures) {
                    pass.set_bind_group(2, group, &[]);
                }
                current_textures = b.key.textures;
            }
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, b.first..b.first + b.count);
        }
    }
}

fn texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_texture_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    views: [&wgpu::TextureView; TEXTURE_SLOTS],
) -> wgpu::BindGroup {
    let mut entries: Vec<wgpu::BindGroupEntry> = views
        .iter()
        .enumerate()
        .map(|(i, view)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: wgpu::BindingResource::TextureView(view),
        })
        .collect();
    entries.push(wgpu::BindGroupEntry {
        binding: TEXTURE_SLOTS as u32,
        resource: wgpu::BindingResource::Sampler(sampler),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh textures"),
        layout,
        entries: &entries,
    })
}

/// Upload an image with its mip chain. `srgb` selects an sRGB view format for RGBA8 data
/// (mips are re-generated in the matching color space when the stored ones don't match).
/// Levels larger than the device allows are skipped.
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
        label: Some("material image"),
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

impl RenderFeature for MeshFeature {
    fn name(&self) -> &str {
        "meshes"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh materials"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let mut texture_entries: Vec<wgpu::BindGroupLayoutEntry> = (0..TEXTURE_SLOTS as u32)
            .map(texture_layout_entry)
            .collect();
        texture_entries.push(wgpu::BindGroupLayoutEntry {
            binding: TEXTURE_SLOTS as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh textures"),
            entries: &texture_entries,
        });
        // Trilinear, repeating, 16x anisotropic where supported (wgpu clamps to 1 otherwise).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        self.default_textures = DEFAULT_TEXELS
            .iter()
            .enumerate()
            .map(|(slot, texel)| {
                let image = Image::solid(*texel, COLOR_SLOT[slot]);
                upload_image(gpu, &image, COLOR_SLOT[slot]).expect("1x1 default texture")
            })
            .collect();

        let shader = gpu.shader("mesh", include_str!("shaders/mesh.wgsl"), true);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&material_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let buffers = [
            Some(Vertex::layout()),
            Some(MeshInstance::layout_with_motion()),
        ];
        self.pipelines = (0..PIPELINE_VARIANTS)
            .map(|variant| {
                let blend = variant & PIPE_BLEND != 0;
                let double = variant & PIPE_DOUBLE != 0;
                let mirror = variant & PIPE_MIRROR != 0;
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("mesh"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &buffers,
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: if double { None } else { Some(wgpu::Face::Back) },
                        front_face: if mirror {
                            wgpu::FrontFace::Cw
                        } else {
                            wgpu::FrontFace::Ccw
                        },
                        ..Default::default()
                    },
                    depth_stencil: Some(gpu.depth_state(!blend)),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: gpu.formats.hdr,
                            blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();

        let prepass_targets = gpu.prepass_targets();
        self.prepass_pipelines = (0..PIPELINE_VARIANTS)
            .map(|variant| {
                let double = variant & PIPE_DOUBLE != 0;
                let mirror = variant & PIPE_MIRROR != 0;
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("mesh prepass"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_prepass"),
                        compilation_options: Default::default(),
                        buffers: &buffers,
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: if double { None } else { Some(wgpu::Face::Back) },
                        front_face: if mirror {
                            wgpu::FrontFace::Cw
                        } else {
                            wgpu::FrontFace::Ccw
                        },
                        ..Default::default()
                    },
                    depth_stencil: Some(gpu.depth_state(true)),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_prepass"),
                        compilation_options: Default::default(),
                        targets: &prepass_targets,
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();

        let shadow_shader = gpu.shader(
            "mesh shadow",
            include_str!("shaders/mesh_shadow.wgsl"),
            false,
        );
        let shadow_primitive = wgpu::PrimitiveState {
            cull_mode: None,
            unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            ..Default::default()
        };
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        self.shadow_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("mesh shadow"),
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
            },
        ));
        let shadow_masked_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh shadow masked"),
            bind_group_layouts: &[
                Some(&gpu.shadow_view_layout),
                Some(&material_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        self.shadow_masked_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("mesh shadow masked"),
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
            },
        ));
        self.materials = Some(GrowBuffer::new(
            device,
            "mesh materials",
            wgpu::BufferUsages::STORAGE,
            64 * std::mem::size_of::<GpuMaterial>() as u64,
        ));
        self.instances = Some(GrowBuffer::new(
            device,
            "mesh instances",
            wgpu::BufferUsages::VERTEX,
            1024 * std::mem::size_of::<MeshInstance>() as u64,
        ));

        // Texture set 0: the shared default set for untextured materials.
        let default_views: [&wgpu::TextureView; TEXTURE_SLOTS] =
            std::array::from_fn(|slot| &self.default_textures[slot].1);
        let default_group = create_texture_group(device, &texture_layout, &sampler, default_views);
        self.texture_sets = vec![Some(TextureSet {
            group: default_group,
            key: [None; TEXTURE_SLOTS],
            bound: [None; TEXTURE_SLOTS],
            last_used: 0,
        })];
        self.texture_set_ids.clear();
        self.free_set_ids.clear();
        self.images.clear();
        self.failed_images.clear();
        self.clusters = Some(ClusterRenderer::new(gpu, &material_layout, &texture_layout));
        self.material_layout = Some(material_layout);
        self.texture_layout = Some(texture_layout);
        self.sampler = Some(sampler);
        self.material_group = None;
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
        if self.pipelines.is_empty() {
            return;
        }
        self.frame += 1;
        if let Some(clusters) = &mut self.clusters {
            clusters.begin_frame(gpu);
        }
        self.clustered.clear();
        let mut query = self
            .query
            .take()
            .unwrap_or_else(|| world.query::<MeshQuery>());
        let world_ref: &World = world;
        let Some(mesh_assets) = world_ref.get_resource::<Assets<Mesh>>() else {
            self.query = Some(query);
            return;
        };
        let material_assets = world_ref.get_resource::<Assets<StandardMaterial>>();
        let image_assets = world_ref.get_resource::<Assets<Image>>();
        let default_material = StandardMaterial::default();

        self.items.clear();
        self.material_data.clear();
        self.material_index.clear();
        let mut culled = 0u32;
        let shadows = view.sun_shadows || view.sky_occlusion.enabled;

        let frame = self.frame;
        let prev_origin = self.prev_origin.unwrap_or(view.origin);
        for (entity, gt, renderer, resolved) in query.iter(world_ref) {
            if !renderer.visible {
                continue;
            }
            // Key GPU caches by the slot that holds the value, so aliases share uploads.
            let mesh_handle = mesh_assets.canonical(resolved.mesh);
            let mesh_index = mesh_handle.index();
            let version = mesh_assets.version(mesh_handle);
            if version == 0 {
                continue;
            }
            let material_handle =
                material_assets.map_or(resolved.material, |m| m.canonical(resolved.material));
            let material = material_assets
                .and_then(|m| m.get(material_handle))
                .unwrap_or(&default_material);

            // Dense opaque meshes: clusters, culled and picked on the GPU, once built (until
            // then they are drawn below as usual).
            let min_triangles = settings.virtual_geometry_min_triangles as usize;
            if settings.virtual_geometry
                && renderer.virtual_geometry
                && material.alpha_mode == AlphaMode::Opaque
                && material.wind_sway <= 0.0
                && let Some(mesh) = mesh_assets.get(mesh_handle)
                && mesh.indices.len() / 3 >= min_triangles.max(1)
                && let Some(clusters) = &mut self.clusters
                && clusters.ready(mesh_index, version, mesh)
            {
                let (material_slot, textures) =
                    self.material_slot(gpu, material_handle.index(), material, image_assets);
                let prev_gt = match self.previous.insert(entity, (*gt, frame)) {
                    Some((prev, seen)) if seen + 1 == frame => prev,
                    _ => *gt,
                };
                if let Some(clusters) = &mut self.clusters {
                    clusters.push_instance(ClusterInstance {
                        mesh: mesh_index,
                        model: gt.matrix_relative_to(view.origin),
                        prev_model: prev_gt.matrix_relative_to(prev_origin),
                        material: material_slot,
                        textures,
                        double_sided: material.double_sided,
                        casts_shadows: shadows && renderer.cast_shadows,
                    });
                }
                self.clustered.insert(mesh_index);
                continue;
            }

            if self
                .meshes
                .get(&mesh_index)
                .is_none_or(|m| m.version != version)
                && let Some(mesh) = mesh_assets.get(mesh_handle)
            {
                self.upload_mesh(&gpu.device, mesh_index, mesh, version);
            }
            let Some(gpu_mesh) = self.meshes.get_mut(&mesh_index) else {
                continue;
            };
            gpu_mesh.last_used = frame;

            let scale = gt.scale().abs().max_element();
            let center_world = gt.transform_point(gpu_mesh.center.as_dvec3());
            let center = (center_world - view.origin).as_vec3();
            let radius = gpu_mesh.half_extents.length() * scale;
            let (min_y, height) = (gpu_mesh.min_y, gpu_mesh.height);
            let visible = view.frustum.intersects_sphere(center, radius);
            let blend = material.alpha_mode == AlphaMode::Blend;
            let masked = matches!(material.alpha_mode, AlphaMode::Mask(_));
            let casts = shadows && renderer.cast_shadows && !blend;
            if !visible && !casts {
                culled += 1;
                continue;
            }
            let (material_slot, textures) =
                self.material_slot(gpu, material_handle.index(), material, image_assets);
            let model = gt.matrix_relative_to(view.origin);
            let prev_gt = match self.previous.insert(entity, (*gt, frame)) {
                Some((prev, seen)) if seen + 1 == frame => prev,
                _ => *gt,
            };
            // A negative determinant mirrors the mesh and turns its front faces clockwise.
            let mirrored = Mat3::from_mat4(model).determinant() < 0.0;
            let pipeline = if blend { PIPE_BLEND } else { 0 }
                | if material.double_sided {
                    PIPE_DOUBLE
                } else {
                    0
                }
                | if mirrored { PIPE_MIRROR } else { 0 };
            self.items.push(Item {
                mesh: mesh_index,
                pipeline,
                textures,
                masked,
                visible,
                casts,
                center,
                radius,
                dist2: center.length_squared(),
                instance: MeshInstance::new(model, material_slot, min_y, height)
                    .with_previous(prev_gt.matrix_relative_to(prev_origin)),
            });
            if !visible {
                culled += 1;
            }
        }
        self.query = Some(query);
        self.prev_origin = Some(view.origin);
        if frame.is_multiple_of(60) {
            self.previous.retain(|_, (_, seen)| *seen + 1 >= frame);
        }
        // Meshes now drawn only as clusters don't need their instanced copy.
        if !self.clustered.is_empty() {
            let clustered = &self.clustered;
            self.meshes
                .retain(|m, g| g.last_used == frame || !clustered.contains(m));
        }
        if let Some(clusters) = &mut self.clusters {
            clusters.end_prepare(gpu, view, settings);
        }

        // Order: opaque (by pipeline, texture set, mesh), transparent (back to front).
        self.items.sort_unstable_by(|a, b| {
            let ab = a.pipeline & PIPE_BLEND;
            let bb = b.pipeline & PIPE_BLEND;
            ab.cmp(&bb).then_with(|| {
                if ab != 0 {
                    b.dist2.total_cmp(&a.dist2)
                } else {
                    (a.pipeline, a.textures, a.mesh).cmp(&(b.pipeline, b.textures, b.mesh))
                }
            })
        });

        self.instance_data.clear();
        self.opaque.clear();
        self.transparent.clear();
        for s in &mut self.shadow {
            s.clear();
        }
        self.keys.clear();

        // Opaque.
        for item in self
            .items
            .iter()
            .filter(|i| i.visible && i.pipeline & PIPE_BLEND == 0)
        {
            self.instance_data.push(item.instance);
            self.keys.push(BatchKey {
                pipeline: item.pipeline,
                textures: item.textures,
                mesh: item.mesh,
            });
        }
        Self::batch(&self.keys, 0, &mut self.opaque);
        // Transparent.
        let first = self.instance_data.len();
        self.keys.clear();
        for item in self
            .items
            .iter()
            .filter(|i| i.visible && i.pipeline & PIPE_BLEND != 0)
        {
            self.instance_data.push(item.instance);
            self.keys.push(BatchKey {
                pipeline: item.pipeline,
                textures: item.textures,
                mesh: item.mesh,
            });
        }
        Self::batch(&self.keys, first as u32, &mut self.transparent);
        // Shadows: opaque casters batch by mesh (depth only); masked casters by texture set
        // and mesh (alpha tested).
        if shadows {
            let shadow_key = |item: &Item| BatchKey {
                pipeline: if item.masked {
                    SHADOW_MASKED
                } else {
                    SHADOW_DEPTH
                },
                textures: if item.masked { item.textures } else { 0 },
                mesh: item.mesh,
            };
            self.casters.clear();
            self.casters.extend(
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.casts)
                    .map(|(n, _)| n as u32),
            );
            let items = &self.items;
            self.casters
                .sort_unstable_by_key(|&n| shadow_key(&items[n as usize]));
            let cascades = if view.sun_shadows {
                (view.cascades.count as usize).min(MAX_CASCADES)
            } else {
                0
            };
            for c in (0..cascades).chain(view.sky_occlusion.enabled.then_some(SKY_OCCLUSION_VIEW)) {
                let first = self.instance_data.len();
                self.keys.clear();
                for &n in &self.casters {
                    let item = &self.items[n as usize];
                    let inside = if c == SKY_OCCLUSION_VIEW {
                        view.sky_occlusion.contains_sphere(item.center, item.radius)
                    } else {
                        view.cascades.sphere_in_cascade(c, item.center, item.radius)
                    };
                    if inside {
                        self.instance_data.push(item.instance);
                        self.keys.push(shadow_key(item));
                    }
                }
                Self::batch(&self.keys, first as u32, &mut self.shadow[c]);
            }
        }

        // Upload.
        let device = &gpu.device;
        let queue = &gpu.queue;
        if let Some(instances) = &mut self.instances {
            instances.write(device, queue, bytemuck::cast_slice(&self.instance_data));
        }
        if self.material_data.is_empty() {
            self.material_data.push(default_material.to_gpu());
        }
        let grown = self
            .materials
            .as_mut()
            .is_some_and(|m| m.write(device, queue, bytemuck::cast_slice(&self.material_data)));
        if (grown || self.material_group.is_none())
            && let (Some(layout), Some(materials)) = (&self.material_layout, &self.materials)
        {
            self.material_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh materials"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: materials.buffer.as_entire_binding(),
                }],
            }));
        }
        if self.frame.is_multiple_of(120) {
            self.evict();
        }

        // Stats.
        let mut stats = FeatureStats {
            culled,
            ..Default::default()
        };
        for b in self.opaque.iter().chain(self.transparent.iter()) {
            stats.draw_calls += 1;
            stats.instances += b.count;
            if let Some(m) = self.meshes.get(&b.key.mesh) {
                stats.triangles += (m.index_count / 3) as u64 * b.count as u64;
            }
        }
        stats.draw_calls += self.shadow.iter().map(|s| s.len() as u32).sum::<u32>();
        if let Some(clusters) = &self.clusters {
            let c = clusters.stats();
            stats.draw_calls += c.draw_calls;
            stats.instances += c.instances;
            stats.triangles += c.triangles;
        }
        self.stats = stats;
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, _ctx: &FrameContext) {
        if let Some(clusters) = &mut self.clusters {
            clusters.encode_culling(encoder);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        let batches = match phase {
            RenderPhase::Opaque => &self.opaque,
            RenderPhase::Transparent => &self.transparent,
            RenderPhase::Sky => return,
        };
        let (Some(group), Some(instances)) = (&self.material_group, &self.instances) else {
            return;
        };
        let clusters = self
            .clusters
            .as_ref()
            .filter(|c| phase == RenderPhase::Opaque && c.is_active());
        if batches.is_empty() && clusters.is_none() {
            return;
        }
        pass.set_bind_group(1, group, &[]);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        self.draw_batches(pass, batches, |p| self.pipelines.get(p as usize), |_| true);
        if let Some(clusters) = clusters {
            clusters.draw_scene(pass, |set| self.texture_group(set));
        }
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        let (Some(group), Some(instances)) = (&self.material_group, &self.instances) else {
            return;
        };
        let clusters = self.clusters.as_ref().filter(|c| c.is_active());
        if self.opaque.is_empty() && clusters.is_none() {
            return;
        }
        pass.set_bind_group(1, group, &[]);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        self.draw_batches(
            pass,
            &self.opaque,
            |p| self.prepass_pipelines.get(p as usize),
            |_| true,
        );
        if let Some(clusters) = clusters {
            clusters.draw_prepass(pass, |set| self.texture_group(set));
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let (Some(group), Some(instances)) = (&self.material_group, &self.instances) else {
            return;
        };
        let batches = &self.shadow[cascade];
        let clusters = self.clusters.as_ref().filter(|c| c.is_active());
        if batches.is_empty() && clusters.is_none() {
            return;
        }
        pass.set_bind_group(1, group, &[]);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        let pipeline_for = |p: u8| match p {
            SHADOW_MASKED => self.shadow_masked_pipeline.as_ref(),
            _ => self.shadow_pipeline.as_ref(),
        };
        self.draw_batches(pass, batches, pipeline_for, |p| p == SHADOW_MASKED);
        if let Some(clusters) = clusters {
            clusters.draw_shadow(pass, cascade);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn clusters_drawn(&self) -> u32 {
        self.clusters.as_ref().map_or(0, |c| c.stats().clusters)
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "gpu_meshes": self.meshes.len(),
            "gpu_images": self.images.len(),
            "texture_sets": self.texture_sets.iter().filter(|s| s.is_some()).count(),
            "materials": self.material_data.len(),
            "opaque_batches": self.opaque.len(),
            "transparent_batches": self.transparent.len(),
            "shadow_batches": self.shadow.iter().map(|s| s.len()).collect::<Vec<_>>(),
            "virtual_geometry": self.clusters.as_ref().map(ClusterRenderer::describe),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Other crates bind `MeshInstance::layout()` next to their own vertex data at locations 11
    /// and up (the skinning feature does), so the shared layout must stay at 4–10.
    #[test]
    fn the_shared_instance_layout_leaves_locations_above_10_free() {
        let shared = MeshInstance::layout();
        assert!(
            shared
                .attributes
                .iter()
                .all(|a| (4..=10).contains(&a.shader_location))
        );
        let motion = MeshInstance::layout_with_motion();
        assert_eq!(motion.array_stride, shared.array_stride);
        assert!(motion.attributes.iter().any(|a| a.shader_location == 13));
    }
}
