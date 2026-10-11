//! The standard mesh feature: draws every entity with a [`MeshRenderer`].
//!
//! Per frame it culls against the camera frustum and each shadow cascade, groups instances
//! by mesh and pipeline, and draws each group with one instanced call. Materials live in one
//! storage buffer indexed per instance, so thousands of objects with different materials
//! still batch by mesh.

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_assets::Assets;
use engine_core::prelude::*;
use wgpu::util::DeviceExt;

use crate::components::{MeshRenderer, ResolvedMesh};
use crate::features::{FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase};
use crate::frame::ViewData;
use crate::material::{AlphaMode, GpuMaterial, StandardMaterial};
use crate::mesh::{Mesh, Vertex};
use crate::settings::RenderSettings;
use crate::shadows::MAX_CASCADES;

/// Per-instance vertex data (locations 4–10 in `mesh.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct MeshInstance {
    /// Camera-relative model matrix rows.
    pub m: [[f32; 4]; 3],
    /// Normal matrix columns.
    pub n: [[f32; 4]; 3],
    /// Material index, flags, mesh min y bits, mesh height bits.
    pub extra: [u32; 4],
}

impl MeshInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
        7 => Float32x4, 8 => Float32x4, 9 => Float32x4,
        10 => Uint32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// Build from a camera-relative model matrix.
    pub fn new(model: Mat4, material: u32, min_y: f32, height: f32) -> Self {
        let normal = Mat3::from_mat4(model).inverse().transpose();
        let r0 = model.row(0);
        let r1 = model.row(1);
        let r2 = model.row(2);
        Self {
            m: [r0.to_array(), r1.to_array(), r2.to_array()],
            n: [
                normal.x_axis.extend(0.0).to_array(),
                normal.y_axis.extend(0.0).to_array(),
                normal.z_axis.extend(0.0).to_array(),
            ],
            extra: [material, 0, min_y.to_bits(), height.to_bits()],
        }
    }
}

struct GpuMesh {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    version: u64,
    center: Vec3,
    half_extents: Vec3,
    min_y: f32,
    height: f32,
}

#[derive(Debug, Clone, Copy)]
struct Batch {
    mesh: u32,
    pipeline: u8,
    first: u32,
    count: u32,
}

#[derive(Clone, Copy)]
struct Item {
    mesh: u32,
    pipeline: u8,
    visible: bool,
    casts: bool,
    center: Vec3,
    radius: f32,
    dist2: f32,
    instance: MeshInstance,
}

type MeshQuery = (
    &'static GlobalTransform,
    &'static MeshRenderer,
    &'static ResolvedMesh,
);

/// Draws [`MeshRenderer`] entities. Added by `RenderPlugin`.
#[derive(Default)]
pub struct MeshFeature {
    /// [opaque, opaque double-sided, blend, blend double-sided]
    pipelines: Vec<wgpu::RenderPipeline>,
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    material_layout: Option<wgpu::BindGroupLayout>,
    materials: Option<GrowBuffer>,
    material_group: Option<wgpu::BindGroup>,
    instances: Option<GrowBuffer>,
    meshes: HashMap<u32, GpuMesh>,
    opaque: Vec<Batch>,
    transparent: Vec<Batch>,
    shadow: [Vec<Batch>; MAX_CASCADES],
    query: Option<QueryState<MeshQuery>>,
    items: Vec<Item>,
    instance_data: Vec<MeshInstance>,
    material_data: Vec<GpuMaterial>,
    material_index: HashMap<u32, u32>,
    stats: FeatureStats,
}

const PIPE_BLEND: u8 = 2;
const PIPE_DOUBLE: u8 = 1;

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
            },
        );
    }

    fn batch(items: &[MeshInstance], keys: &[(u32, u8)], first: u32, out: &mut Vec<Batch>) {
        debug_assert_eq!(items.len(), keys.len());
        let mut i = 0;
        while i < keys.len() {
            let (mesh, pipeline) = keys[i];
            let start = i;
            while i < keys.len() && keys[i] == (mesh, pipeline) {
                i += 1;
            }
            out.push(Batch {
                mesh,
                pipeline,
                first: first + start as u32,
                count: (i - start) as u32,
            });
        }
    }
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
        let shader = gpu.shader("mesh", include_str!("shaders/mesh.wgsl"), true);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let buffers = [Some(Vertex::layout()), Some(MeshInstance::layout())];
        self.pipelines = (0..4u8)
            .map(|variant| {
                let blend = variant & PIPE_BLEND != 0;
                let double = variant & PIPE_DOUBLE != 0;
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
                        front_face: wgpu::FrontFace::Ccw,
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

        let shadow_shader = gpu.shader("mesh shadow", include_str!("shaders/mesh_shadow.wgsl"), false);
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        self.shadow_pipeline = Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh shadow"),
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
        }));
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
        self.material_layout = Some(material_layout);
        self.material_group = None;
    }

    fn prepare(&mut self, world: &mut World, gpu: &GpuShared, view: &ViewData, _settings: &RenderSettings) {
        if self.pipelines.is_empty() {
            return;
        }
        let mut query = self.query.take().unwrap_or_else(|| world.query::<MeshQuery>());
        let world_ref: &World = world;
        let Some(mesh_assets) = world_ref.get_resource::<Assets<Mesh>>() else {
            self.query = Some(query);
            return;
        };
        let material_assets = world_ref.get_resource::<Assets<StandardMaterial>>();
        let default_material = StandardMaterial::default();

        self.items.clear();
        self.material_data.clear();
        self.material_index.clear();
        let mut culled = 0u32;
        let shadows = view.sun_shadows;

        for (gt, renderer, resolved) in query.iter(world_ref) {
            if !renderer.visible {
                continue;
            }
            let mesh_index = resolved.mesh.index();
            let version = mesh_assets.version(resolved.mesh);
            if version == 0 {
                continue;
            }
            if self.meshes.get(&mesh_index).is_none_or(|m| m.version != version)
                && let Some(mesh) = mesh_assets.get(resolved.mesh)
            {
                self.upload_mesh(&gpu.device, mesh_index, mesh, version);
            }
            let Some(gpu_mesh) = self.meshes.get(&mesh_index) else {
                continue;
            };
            let material = material_assets
                .and_then(|m| m.get(resolved.material))
                .unwrap_or(&default_material);

            let scale = gt.scale().abs().max_element() as f32;
            let center_world = gt.transform_point(gpu_mesh.center.as_dvec3());
            let center = (center_world - view.origin).as_vec3();
            let radius = gpu_mesh.half_extents.length() * scale;
            let visible = view.frustum.intersects_sphere(center, radius);
            let blend = material.alpha_mode == AlphaMode::Blend;
            let casts = shadows && renderer.cast_shadows && !blend;
            if !visible && !casts {
                culled += 1;
                continue;
            }
            let next = self.material_data.len() as u32;
            let material_slot = *self
                .material_index
                .entry(resolved.material.index())
                .or_insert_with(|| {
                    self.material_data.push(material.to_gpu());
                    next
                });
            let model = gt.matrix_relative_to(view.origin);
            let pipeline = if blend { PIPE_BLEND } else { 0 } | if material.double_sided { PIPE_DOUBLE } else { 0 };
            self.items.push(Item {
                mesh: mesh_index,
                pipeline,
                visible,
                casts,
                center,
                radius,
                dist2: center.length_squared(),
                instance: MeshInstance::new(model, material_slot, gpu_mesh.min_y, gpu_mesh.height),
            });
            if !visible {
                culled += 1;
            }
        }
        self.query = Some(query);

        // Order: opaque (by pipeline, mesh), transparent (back to front), shadow casters per cascade.
        self.items.sort_unstable_by(|a, b| {
            let ab = a.pipeline & PIPE_BLEND;
            let bb = b.pipeline & PIPE_BLEND;
            ab.cmp(&bb).then_with(|| {
                if ab != 0 {
                    b.dist2.total_cmp(&a.dist2)
                } else {
                    (a.pipeline, a.mesh).cmp(&(b.pipeline, b.mesh))
                }
            })
        });

        self.instance_data.clear();
        self.opaque.clear();
        self.transparent.clear();
        for s in &mut self.shadow {
            s.clear();
        }
        let mut keys: Vec<(u32, u8)> = Vec::with_capacity(self.items.len());

        // Opaque.
        for item in self.items.iter().filter(|i| i.visible && i.pipeline & PIPE_BLEND == 0) {
            self.instance_data.push(item.instance);
            keys.push((item.mesh, item.pipeline));
        }
        Self::batch(&self.instance_data, &keys, 0, &mut self.opaque);
        // Transparent.
        let first = self.instance_data.len();
        keys.clear();
        for item in self.items.iter().filter(|i| i.visible && i.pipeline & PIPE_BLEND != 0) {
            self.instance_data.push(item.instance);
            keys.push((item.mesh, item.pipeline));
        }
        Self::batch(&self.instance_data[first..], &keys, first as u32, &mut self.transparent);
        // Shadows: casters are grouped by mesh for each cascade.
        if shadows {
            let mut casters: Vec<&Item> = self.items.iter().filter(|i| i.casts).collect();
            casters.sort_unstable_by_key(|i| i.mesh);
            for c in 0..view.cascades.count as usize {
                let first = self.instance_data.len();
                keys.clear();
                for item in &casters {
                    if view.cascades.sphere_in_cascade(c, item.center, item.radius) {
                        self.instance_data.push(item.instance);
                        keys.push((item.mesh, 0));
                    }
                }
                Self::batch(&self.instance_data[first..], &keys, first as u32, &mut self.shadow[c]);
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

        // Stats.
        let mut stats = FeatureStats {
            culled,
            ..Default::default()
        };
        for b in self.opaque.iter().chain(self.transparent.iter()) {
            stats.draw_calls += 1;
            stats.instances += b.count;
            if let Some(m) = self.meshes.get(&b.mesh) {
                stats.triangles += (m.index_count / 3) as u64 * b.count as u64;
            }
        }
        stats.draw_calls += self.shadow.iter().map(|s| s.len() as u32).sum::<u32>();
        self.stats = stats;
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
        if batches.is_empty() {
            return;
        }
        pass.set_bind_group(1, group, &[]);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        let mut current = u8::MAX;
        for b in batches {
            let Some(mesh) = self.meshes.get(&b.mesh) else {
                continue;
            };
            if b.pipeline != current {
                pass.set_pipeline(&self.pipelines[b.pipeline as usize]);
                current = b.pipeline;
            }
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, b.first..b.first + b.count);
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let (Some(pipeline), Some(group), Some(instances)) =
            (&self.shadow_pipeline, &self.material_group, &self.instances)
        else {
            return;
        };
        let batches = &self.shadow[cascade];
        if batches.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, group, &[]);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        for b in batches {
            let Some(mesh) = self.meshes.get(&b.mesh) else {
                continue;
            };
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, b.first..b.first + b.count);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "gpu_meshes": self.meshes.len(),
            "materials": self.material_data.len(),
            "opaque_batches": self.opaque.len(),
            "transparent_batches": self.transparent.len(),
            "shadow_batches": self.shadow.iter().map(|s| s.len()).collect::<Vec<_>>(),
        })
    }
}
