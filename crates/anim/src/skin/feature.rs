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
//! * `draw`/`draw_shadow` draw each instance from its slice of the shared output buffer.

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
use engine_render::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};
use engine_render::wgpu;
use wgpu::util::DeviceExt;

use super::{SkinMaterial, SkinPose, SkinnedMesh, SkinnedMeshRenderer};

/// Compute shader: morphs + linear blend skinning, and cloth tension.
pub const SKIN_COMPUTE_WGSL: &str = include_str!("shaders/skin_compute.wgsl");
/// Scene shader: PBR with tension wrinkles, compression bunching, veins and sheen.
pub const SKIN_DRAW_WGSL: &str = include_str!("shaders/skin_draw.wgsl");
/// Shadow depth shader.
pub const SKIN_SHADOW_WGSL: &str = include_str!("shaders/skin_shadow.wgsl");

const WORKGROUP: u32 = 64;
/// Bytes between jobs in the uniform buffer (the largest allowed dynamic offset alignment).
const JOB_STRIDE: u64 = 256;
const JOB_MORPHS: u32 = 1;
const JOB_TENSION: u32 = 2;

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

/// Where one mesh lives in the shared static buffers.
#[derive(Debug, Clone, Copy)]
struct MeshSlot {
    version: u64,
    vertex_base: u32,
    vertex_count: u32,
    index_base: u32,
    index_count: u32,
    joints: u32,
    morphs: u32,
    center: Vec3,
    radius: f32,
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
    draw: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
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
    query: Option<QueryState<SkinQuery>>,
    // Per frame.
    items: Vec<Instance>,
    job_data: Vec<u8>,
    job_list: Vec<(u32, bool)>,
    joint_data: Vec<[[f32; 4]; 3]>,
    weight_data: Vec<f32>,
    instance_data: Vec<MeshInstance>,
    material_data: Vec<GpuSkinMaterial>,
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
        let mut indices = Vec::new();
        self.slots.clear();
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
            indices.extend(mesh.indices.iter().copied().filter(|&i| i < count));
            let bounds = mesh.bounds();
            self.slots.insert(
                index,
                MeshSlot {
                    version,
                    vertex_base,
                    vertex_count: count,
                    index_base,
                    index_count: indices.len() as u32 - index_base,
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

    fn draw_instances(&self, pass: &mut wgpu::RenderPass<'_>, list: impl Iterator<Item = u32>) {
        let (Some(s), Some(out), Some(instances)) = (&self.statics, &self.output, &self.instances)
        else {
            return;
        };
        pass.set_vertex_buffer(2, instances.buffer.slice(..));
        pass.set_index_buffer(s.indices.slice(..), wgpu::IndexFormat::Uint32);
        for i in list {
            let item = &self.items[i as usize];
            let Some(slot) = self.slots.get(&item.mesh) else {
                continue;
            };
            if slot.index_count == 0 {
                continue;
            }
            let out_start = item.out_base as u64 * SKINNED_SIZE;
            let out_end = out_start + slot.vertex_count as u64 * SKINNED_SIZE;
            let attr_size = std::mem::size_of::<GpuStatic>() as u64;
            let attr_start = slot.vertex_base as u64 * attr_size;
            let attr_end = attr_start + slot.vertex_count as u64 * attr_size;
            pass.set_vertex_buffer(0, out.buffer.slice(out_start..out_end));
            pass.set_vertex_buffer(1, s.attributes.slice(attr_start..attr_end));
            pass.draw_indexed(
                slot.index_base..slot.index_base + slot.index_count,
                0,
                i..i + 1,
            );
        }
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

        let buffers = [
            Some(skinned_layout()),
            Some(GpuStatic::layout()),
            Some(MeshInstance::layout()),
        ];
        let draw_shader = gpu.shader("skin draw", SKIN_DRAW_WGSL, true);
        let draw_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skin draw"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let draw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skin draw"),
            layout: Some(&draw_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
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
        });
        let shadow_shader = gpu.shader("skin shadow", SKIN_SHADOW_WGSL, false);
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
            shadow,
            compute_layout,
            material_layout,
        });
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
        for s in &mut self.shadow {
            s.clear();
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

        let shadows = view.sun_shadows || view.sky_occlusion.enabled;
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
            self.items.push(Instance {
                mesh: index,
                out_base,
                visible,
                casts,
                detail,
                center,
                radius,
            });
            out_base += slot.vertex_count;
            if !visible {
                culled += 1;
            }
        }
        self.query = Some(query);

        if shadows {
            let cascades = if view.sun_shadows {
                (view.cascades.count as usize).min(MAX_CASCADES)
            } else {
                0
            };
            for c in (0..cascades).chain(view.sky_occlusion.enabled.then_some(SKY_OCCLUSION_VIEW)) {
                for (i, item) in self.items.iter().enumerate() {
                    if !item.casts {
                        continue;
                    }
                    let inside = if c == SKY_OCCLUSION_VIEW {
                        view.sky_occlusion.contains_sphere(item.center, item.radius)
                    } else {
                        view.cascades.sphere_in_cascade(c, item.center, item.radius)
                    };
                    if inside {
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

        let mut stats = FeatureStats {
            culled,
            ..Default::default()
        };
        for item in self.items.iter().filter(|i| i.visible) {
            stats.draw_calls += 1;
            stats.instances += 1;
            if let Some(s) = self.slots.get(&item.mesh) {
                stats.triangles += (s.index_count / 3) as u64;
            }
        }
        stats.draw_calls += self.shadow.iter().map(|s| s.len() as u32).sum::<u32>();
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
        let (Some(p), Some(group)) = (&self.pipelines, &self.material_group) else {
            return;
        };
        if !self.items.iter().any(|i| i.visible) {
            return;
        }
        pass.set_pipeline(&p.draw);
        pass.set_bind_group(1, group, &[]);
        let visible = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.visible)
            .map(|(n, _)| n as u32);
        self.draw_instances(pass, visible);
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let Some(p) = &self.pipelines else {
            return;
        };
        let list = &self.shadow[cascade];
        if list.is_empty() {
            return;
        }
        pass.set_pipeline(&p.shadow);
        self.draw_instances(pass, list.iter().copied());
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "meshes": self.slots.len(),
            "instances": self.items.len(),
            "detailed": self.items.iter().filter(|i| i.detail).count(),
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
