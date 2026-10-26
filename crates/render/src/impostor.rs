//! Impostors for the mesh feature's instanced path: meshes whose levels of detail can't get
//! cheap (leaf-card trees, whose cards are separate quads the simplifier can't merge, and other
//! ragged props) are drawn far away as one quad each instead of hundreds of triangles.
//!
//! A mesh with a material is baked once, on the GPU, from 12×12 directions spread over the whole
//! sphere (octahedral layout) into one layer of an atlas array: color and coverage, and the
//! mesh-space normal and depth. The quad faces the camera; each pixel follows its view ray into
//! one of the four tiles around the view direction (picked stochastically, so temporal
//! anti-aliasing blends them), shades the baked surface like the mesh would be shaded, and
//! writes the surface's depth, so impostors sit in the ground and in each other as the meshes
//! do. In the shadow maps the quad faces the light. `impostor_common.wgsl` has the layout.
//!
//! Which meshes: those whose coarsest level still has [`MIN_TRIANGLES`], once an instance is
//! smaller on screen than `RenderSettings::impostor_pixels`; the most-drawn are baked first, one
//! per frame, up to `RenderSettings::impostor_atlas_layers`. A full atlas reuses the layer of the
//! mesh drawn as an impostor longest ago, once it has gone unseen for [`EVICT_AFTER_FRAMES`]
//! (least recently used, as virtual texture caches and Unreal's HLOD streaming do).
//!
//! Where textures are compressed (`RenderSettings::texture_compression`), the atlas is BC7, as
//! Unreal's impostor baker and Amplify Impostors ship theirs: each bake renders into an RGBA8
//! scratch pair and is compressed on the GPU into its layer (a quarter of the memory and of the
//! bandwidth the far forest's thousands of quads read). Otherwise it is RGBA8 and holds at most
//! [`MAX_RGBA8_LAYERS`].

use std::collections::HashMap;

use engine_core::prelude::*;

use block_compression::{BC7Settings, CompressionVariant};

use crate::features::GpuShared;
use crate::image_cache::copy_blocks;
use crate::mesh::Vertex;
use crate::mesh_feature::MeshInstance;

/// Directions per side of the octahedral grid (`IMPOSTOR_GRID` in the shaders).
pub(crate) const GRID: u32 = 12;
/// Texels per side of one direction's tile.
const TILE: u32 = 72;
const SIZE: u32 = GRID * TILE;
const TILES: u32 = GRID * GRID;
/// The atlas grows by this many layers at a time.
const GROW: u32 = 8;
/// Most layers of an uncompressed atlas (6 MB each; BC7 layers are 1.5 MB).
pub(crate) const MAX_RGBA8_LAYERS: u32 = 32;
/// Room in the bounding sphere buffer: the most layers any atlas has.
const MAX_LAYERS: u32 = 1024;
/// A full atlas reuses a layer whose mesh hasn't been drawn as an impostor for this many frames.
pub(crate) const EVICT_AFTER_FRAMES: u64 = 300;
/// Meshes whose coarsest level of detail keeps at least this many triangles get impostors.
pub(crate) const MIN_TRIANGLES: u32 = 256;
/// No impostor (`Item::impostor`).
pub(crate) const NONE: u32 = u32::MAX;

const ALBEDO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Bytes of one compressed layer of one texture (BC7: a byte a texel).
const BC7_LAYER_BYTES: u64 = SIZE as u64 * SIZE as u64;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// A mesh with a material: its mesh index and its material asset's index.
pub(crate) type Key = (u32, u64);

/// A mesh drawn small enough to want an impostor this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request {
    pub mesh: u32,
    pub material_slot: u32,
    pub textures: u32,
    /// Bounding sphere in mesh space.
    pub center: Vec3,
    pub radius: f32,
    /// Triangles its instances would draw (the most wanted bakes first).
    pub score: u64,
}

/// The bake recorded in this frame's `compute`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bake {
    pub layer: u32,
    pub mesh: u32,
    pub textures: u32,
}

struct Atlas {
    albedo: wgpu::Texture,
    normal: wgpu::Texture,
    layers: u32,
}

impl Atlas {
    fn new(device: &wgpu::Device, layers: u32, compressed: bool) -> Self {
        // Compressed layers are written by copies, not rendered to.
        let target = if compressed {
            wgpu::TextureUsages::empty()
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        };
        let (albedo, normal) = if compressed {
            (
                wgpu::TextureFormat::Bc7RgbaUnormSrgb,
                wgpu::TextureFormat::Bc7RgbaUnorm,
            )
        } else {
            (ALBEDO_FORMAT, NORMAL_FORMAT)
        };
        let texture = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: target
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        Self {
            albedo: texture("impostor albedo", albedo),
            normal: texture("impostor normal", normal),
            layers,
        }
    }

    fn layer_view(texture: &wgpu::Texture, layer: u32) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    }

    fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }
}

/// What a compressed atlas bakes into before compressing: an RGBA8 pair and the blocks.
struct Scratch {
    albedo: wgpu::Texture,
    normal: wgpu::Texture,
    blocks: wgpu::Buffer,
}

pub(crate) struct Impostors {
    /// Baked layers by mesh and material.
    layers: HashMap<Key, u32>,
    /// Each layer's mesh and material, and the frame it was last drawn.
    owners: Vec<(Key, u64)>,
    /// Frames scheduled so far (the clock of `owners`).
    frame: u64,
    /// The atlas is BC7 (decided with the first bake).
    compressed: Option<bool>,
    scratch: Option<Scratch>,
    requests: HashMap<Key, Request>,
    /// Bounding spheres per layer (`ImpostorInfo`).
    info: wgpu::Buffer,
    atlas: Option<Atlas>,
    /// The atlas before it grew, copied into the new one before the next bake.
    grown_from: Option<Atlas>,
    depth: Option<wgpu::TextureView>,
    bake_instances: wgpu::Buffer,
    pending: Option<Bake>,
    draw_layout: wgpu::BindGroupLayout,
    draw_group: Option<wgpu::BindGroup>,
    bake_group: wgpu::BindGroup,
    shader: wgpu::ShaderModule,
    shadow_shader: wgpu::ShaderModule,
    draw_pipeline_layout: wgpu::PipelineLayout,
    bake_pipeline_layout: wgpu::PipelineLayout,
    shadow_pipeline_layout: wgpu::PipelineLayout,
    main: Option<wgpu::RenderPipeline>,
    prepass: Option<wgpu::RenderPipeline>,
    bake: Option<wgpu::RenderPipeline>,
    shadow: Option<wgpu::RenderPipeline>,
    warned_full: bool,
}

impl Impostors {
    /// `mesh_source` is the mesh feature's shader source (with the virtual texture library);
    /// `material_layout` and `texture_layout` its group 1 and 2 layouts.
    pub(crate) fn new(
        gpu: &GpuShared,
        mesh_source: &str,
        material_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let device = &gpu.device;
        let common = include_str!("shaders/impostor_common.wgsl");
        let shader = gpu.shader(
            "impostor",
            &format!(
                "{mesh_source}\n{common}\n{}",
                include_str!("shaders/impostor.wgsl")
            ),
            true,
        );
        let shadow_shader = gpu.shader(
            "impostor shadow",
            &format!("{common}\n{}", include_str!("shaders/impostor_shadow.wgsl")),
            false,
        );
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let info_entry = wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("impostors"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                info_entry,
            ],
        });
        let bake_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("impostor bake"),
            entries: &[info_entry],
        });
        let info = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("impostor spheres"),
            size: MAX_LAYERS as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bake_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("impostor bake"),
            layout: &bake_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 3,
                resource: info.as_entire_binding(),
            }],
        });
        let bake_instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("impostor bake instances"),
            size: TILES as u64 * std::mem::size_of::<MeshInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline_layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let draw_pipeline_layout = pipeline_layout(
            "impostor",
            &[
                Some(&gpu.frame_layout),
                Some(material_layout),
                None,
                Some(&draw_layout),
            ],
        );
        let bake_pipeline_layout = pipeline_layout(
            "impostor bake",
            &[
                Some(&gpu.frame_layout),
                Some(material_layout),
                Some(texture_layout),
                Some(&bake_layout),
            ],
        );
        let shadow_pipeline_layout = pipeline_layout(
            "impostor shadow",
            &[
                Some(&gpu.shadow_view_layout),
                None,
                None,
                Some(&draw_layout),
            ],
        );
        Self {
            layers: HashMap::new(),
            owners: Vec::new(),
            frame: 0,
            compressed: None,
            scratch: None,
            requests: HashMap::new(),
            info,
            atlas: None,
            grown_from: None,
            depth: None,
            bake_instances,
            pending: None,
            draw_layout,
            draw_group: None,
            bake_group,
            shader,
            shadow_shader,
            draw_pipeline_layout,
            bake_pipeline_layout,
            shadow_pipeline_layout,
            main: None,
            prepass: None,
            bake: None,
            shadow: None,
            warned_full: false,
        }
    }

    /// The baked layer of a mesh with a material, which counts as drawn this frame.
    pub(crate) fn layer(&mut self, key: Key) -> Option<u32> {
        let layer = self.layers.get(&key).copied()?;
        if let Some(owner) = self.owners.get_mut(layer as usize) {
            owner.1 = self.frame;
        }
        Some(layer)
    }

    /// Ask for a bake (the scores of a frame's instances add up).
    pub(crate) fn request(&mut self, key: Key, request: Request) {
        self.requests
            .entry(key)
            .and_modify(|r| r.score += request.score)
            .or_insert(request);
    }

    /// Whether anything is baked (the draw pipelines are needed).
    pub(crate) fn any(&self) -> bool {
        !self.layers.is_empty()
    }

    /// Pick this frame's bake from the requests: the most wanted one, if the atlas has room or
    /// a layer that has gone unused. `max_layers` is `RenderSettings::impostor_atlas_layers`;
    /// `compress` asks for a BC7 atlas (applies to the first bake).
    pub(crate) fn schedule(
        &mut self,
        gpu: &GpuShared,
        (max_layers, compress): (u32, bool),
        min_y_height: impl Fn(u32) -> (f32, f32),
    ) {
        self.pending = None;
        self.frame += 1;
        let best = self
            .requests
            .drain()
            .max_by_key(|(key, r)| (r.score, std::cmp::Reverse(*key)));
        let Some((key, request)) = best else {
            return;
        };
        let device = &gpu.device;
        let compressed = *self.compressed.get_or_insert(compress);
        // The encoders build on a thread: bake once they are there.
        if compressed && gpu.images.with_compressor(gpu, |_| ()).is_none() {
            return;
        }
        let cap = if compressed {
            MAX_LAYERS
        } else {
            MAX_RGBA8_LAYERS
        };
        let max = max_layers
            .min(cap)
            .min(device.limits().max_texture_array_layers)
            .max(1);
        let Some(layer) = pick_layer(&self.owners, self.frame, max) else {
            if !self.warned_full {
                self.warned_full = true;
                tracing::info!(
                    "impostor atlas full ({max} meshes, all drawn in the last \
                     {EVICT_AFTER_FRAMES} frames); no more are baked for now"
                );
            }
            return;
        };
        match self.owners.get_mut(layer as usize) {
            Some(owner) => {
                // Reuse a layer nobody has drawn for a while.
                self.layers.remove(&owner.0);
                *owner = (key, self.frame);
            }
            None => self.owners.push((key, self.frame)),
        }
        if self.atlas.as_ref().is_none_or(|a| layer >= a.layers) {
            let layers = (layer + GROW).min(max);
            self.grown_from = self.atlas.replace(Atlas::new(device, layers, compressed));
            self.draw_group = None;
        }
        if compressed && self.scratch.is_none() {
            let texture = |label, format, view: &[wgpu::TextureFormat]| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: view,
                })
            };
            self.scratch = Some(Scratch {
                // The encoders read the bytes as they are: viewed as plain RGBA8.
                albedo: texture(
                    "impostor bake albedo",
                    ALBEDO_FORMAT,
                    &[wgpu::TextureFormat::Rgba8Unorm],
                ),
                normal: texture("impostor bake normal", NORMAL_FORMAT, &[]),
                blocks: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("impostor blocks"),
                    size: 2 * BC7_LAYER_BYTES,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
            });
        }
        if self.depth.is_none() {
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("impostor bake depth"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some(depth.create_view(&Default::default()));
        }
        // A little room around the sphere so filtering never reaches a neighboring tile.
        let sphere = [
            request.center.x,
            request.center.y,
            request.center.z,
            request.radius * 1.04,
        ];
        gpu.queue
            .write_buffer(&self.info, layer as u64 * 16, bytemuck::bytes_of(&sphere));
        let (min_y, height) = min_y_height(request.mesh);
        let mut instance = MeshInstance::new(Mat4::IDENTITY, request.material_slot, min_y, height);
        instance.extra[1] = layer;
        let instances = vec![instance; TILES as usize];
        gpu.queue
            .write_buffer(&self.bake_instances, 0, bytemuck::cast_slice(&instances));
        self.layers.insert(key, layer);
        self.warned_full = false;
        self.pending = Some(Bake {
            layer,
            mesh: request.mesh,
            textures: request.textures,
        });
    }

    /// This frame's bake, for the mesh feature to look up its buffers and textures.
    pub(crate) fn pending(&self) -> Option<Bake> {
        self.pending
    }

    /// Record this frame's bake (and, after the atlas grew, the copy of its old layers).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        gpu: &GpuShared,
        encoder: &mut wgpu::CommandEncoder,
        constants: &[(&str, f64)],
        frame_group: &wgpu::BindGroup,
        material_group: &wgpu::BindGroup,
        texture_group: &wgpu::BindGroup,
        vertex: &wgpu::Buffer,
        index: &wgpu::Buffer,
        index_count: u32,
    ) {
        let Some(bake) = self.pending.take() else {
            return;
        };
        let Some(atlas) = &self.atlas else {
            return;
        };
        if let Some(old) = self.grown_from.take() {
            for (from, to) in [(&old.albedo, &atlas.albedo), (&old.normal, &atlas.normal)] {
                encoder.copy_texture_to_texture(
                    from.as_image_copy(),
                    to.as_image_copy(),
                    wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: old.layers,
                    },
                );
            }
        }
        if self.bake.is_none() {
            self.bake = Some(self.bake_pipeline(gpu, constants));
        }
        let (Some(pipeline), Some(depth)) = (&self.bake, &self.depth) else {
            return;
        };
        let (albedo, normal) = match &self.scratch {
            Some(s) => (
                s.albedo.create_view(&Default::default()),
                s.normal.create_view(&Default::default()),
            ),
            None => (
                Atlas::layer_view(&atlas.albedo, bake.layer),
                Atlas::layer_view(&atlas.normal, bake.layer),
            ),
        };
        let target = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("impostor bake"),
            color_attachments: &[target(&albedo), target(&normal)],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(1, material_group, &[]);
        pass.set_bind_group(2, texture_group, &[]);
        pass.set_bind_group(3, &self.bake_group, &[]);
        pass.set_vertex_buffer(0, vertex.slice(..));
        pass.set_vertex_buffer(1, self.bake_instances.slice(..));
        pass.set_index_buffer(index.slice(..), wgpu::IndexFormat::Uint32);
        for tile in 0..TILES {
            let (x, y) = ((tile % GRID) * TILE, (tile / GRID) * TILE);
            pass.set_viewport(x as f32, y as f32, TILE as f32, TILE as f32, 0.0, 1.0);
            pass.draw_indexed(0..index_count, 0, tile..tile + 1);
        }
        drop(pass);
        // Compressed atlas: the scratch pair into BC7 blocks, then into the layer.
        if let Some(scratch) = &self.scratch {
            let albedo = scratch.albedo.create_view(&wgpu::TextureViewDescriptor {
                format: Some(wgpu::TextureFormat::Rgba8Unorm),
                ..Default::default()
            });
            let normal = scratch.normal.create_view(&Default::default());
            let variant = CompressionVariant::BC7(BC7Settings::alpha_basic());
            let blocks = &scratch.blocks;
            let compressed = gpu.images.with_compressor(gpu, |c| {
                c.add_compression_task(variant, &albedo, SIZE, SIZE, blocks, None, None);
                let second = Some(BC7_LAYER_BYTES as u32);
                c.add_compression_task(variant, &normal, SIZE, SIZE, blocks, None, second);
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("impostor compression"),
                    timestamp_writes: None,
                });
                c.compress(&mut pass);
            });
            if compressed.is_some() {
                let at = (0, bake.layer);
                for (texture, offset) in [(&atlas.albedo, 0), (&atlas.normal, BC7_LAYER_BYTES)] {
                    copy_blocks(encoder, blocks, offset, texture, at, (SIZE, SIZE), 16);
                }
            }
        }
    }

    /// Drops the pipelines built with other shader features; they are made again on use.
    pub(crate) fn clear_pipelines(&mut self) {
        self.main = None;
        self.prepass = None;
        self.bake = None;
    }

    /// Makes the scene, prepass and shadow pipelines and the atlas bind group, once something
    /// is baked.
    pub(crate) fn ensure_pipelines(&mut self, gpu: &GpuShared, constants: &[(&str, f64)]) {
        if !self.any() {
            return;
        }
        if self.main.is_none() {
            self.main = Some(self.draw_pipeline(gpu, constants, false));
        }
        if self.prepass.is_none() {
            self.prepass = Some(self.draw_pipeline(gpu, constants, true));
        }
        if self.shadow.is_none() {
            self.shadow = Some(self.shadow_pipeline(gpu));
        }
        if self.draw_group.is_none()
            && let Some(atlas) = &self.atlas
        {
            self.draw_group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("impostors"),
                layout: &self.draw_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&Atlas::array_view(
                            &atlas.albedo,
                        )),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&Atlas::array_view(
                            &atlas.normal,
                        )),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&gpu.linear_clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.info.as_entire_binding(),
                    },
                ],
            }));
        }
    }

    /// Draw `instances` (a range of the mesh feature's instance buffer, already bound at slot
    /// 1) in the scene (`prepass` false) or the depth prepass.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, range: (u32, u32), prepass: bool) {
        let pipeline = if prepass { &self.prepass } else { &self.main };
        if let (Some(pipeline), Some(group)) = (pipeline, &self.draw_group)
            && range.1 > 0
        {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(3, group, &[]);
            pass.draw(0..6, range.0..range.0 + range.1);
        }
    }

    /// Draw `instances` into a shadow map (its view bound at group 0, the instances at slot 1).
    pub(crate) fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, range: (u32, u32)) {
        if let (Some(pipeline), Some(group)) = (&self.shadow, &self.draw_group)
            && range.1 > 0
        {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(3, group, &[]);
            pass.draw(0..6, range.0..range.0 + range.1);
        }
    }

    fn draw_pipeline(
        &self,
        gpu: &GpuShared,
        constants: &[(&str, f64)],
        prepass: bool,
    ) -> wgpu::RenderPipeline {
        let compilation = wgpu::PipelineCompilationOptions {
            constants,
            ..Default::default()
        };
        let main_targets = [Some(wgpu::ColorTargetState {
            format: gpu.formats.hdr,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let prepass_targets = gpu.prepass_targets();
        let (label, fs, targets): (_, _, &[Option<wgpu::ColorTargetState>]) = if prepass {
            ("impostor prepass", "fs_impostor_prepass", &prepass_targets)
        } else {
            ("impostor", "fs_impostor", &main_targets)
        };
        gpu.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&self.draw_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_impostor"),
                    compilation_options: compilation.clone(),
                    buffers: &[None, Some(MeshInstance::layout_with_motion())],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(fs),
                    compilation_options: compilation,
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
    }

    fn bake_pipeline(&self, gpu: &GpuShared, constants: &[(&str, f64)]) -> wgpu::RenderPipeline {
        let compilation = wgpu::PipelineCompilationOptions {
            constants,
            ..Default::default()
        };
        let target = |format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        gpu.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("impostor bake"),
                layout: Some(&self.bake_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_impostor_bake"),
                    compilation_options: compilation.clone(),
                    buffers: &[
                        Some(Vertex::layout()),
                        Some(MeshInstance::layout_with_motion()),
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs_impostor_bake"),
                    compilation_options: compilation,
                    targets: &[target(ALBEDO_FORMAT), target(NORMAL_FORMAT)],
                }),
                multiview_mask: None,
                cache: None,
            })
    }

    fn shadow_pipeline(&self, gpu: &GpuShared) -> wgpu::RenderPipeline {
        gpu.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("impostor shadow"),
                layout: Some(&self.shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shadow_shader,
                    entry_point: Some("vs_impostor_shadow"),
                    compilation_options: Default::default(),
                    buffers: &[None, Some(MeshInstance::layout())],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
                    ..Default::default()
                },
                depth_stencil: Some(gpu.shadow_depth_state()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.shadow_shader,
                    entry_point: Some("fs_impostor_shadow"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            })
    }

    /// Meshes baked now.
    pub(crate) fn count(&self) -> usize {
        self.layers.len()
    }

    /// Whether the atlas is BC7.
    pub(crate) fn compressed(&self) -> bool {
        self.compressed == Some(true)
    }
}

/// The layer for the next bake: a new one while the atlas has fewer than `max`, else the one
/// drawn longest ago if that was at least [`EVICT_AFTER_FRAMES`] before `frame`.
fn pick_layer(owners: &[(Key, u64)], frame: u64, max: u32) -> Option<u32> {
    if (owners.len() as u32) < max {
        return Some(owners.len() as u32);
    }
    let (layer, (_, used)) = owners
        .iter()
        .take(max as usize)
        .enumerate()
        .min_by_key(|(_, (_, used))| *used)?;
    (used + EVICT_AFTER_FRAMES <= frame).then_some(layer as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_atlas_reuses_the_layer_drawn_longest_ago() {
        let key = |n| (n, 0u64);
        // Room: a new layer.
        assert_eq!(pick_layer(&[(key(0), 5)], 10, 2), Some(1));
        // Full, everything drawn lately: nothing.
        let owners = [(key(0), 900), (key(1), 950)];
        assert_eq!(pick_layer(&owners, 1000, 2), None);
        // Full, one unseen for long enough: that one.
        let owners = [(key(0), 900), (key(1), 600), (key(2), 650)];
        assert_eq!(pick_layer(&owners, 1000, 3), Some(1));
        // A smaller budget than the atlas has: only its first layers are candidates.
        assert_eq!(pick_layer(&owners, 1000, 1), None);
    }
}
