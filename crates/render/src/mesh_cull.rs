//! Occlusion culling of the camera's instanced meshes on the GPU (`shaders/mesh_cull.wgsl`),
//! two-phase as in Haar and Aaltonen, "GPU-Driven Rendering Pipelines" (SIGGRAPH 2015), the
//! scheme the virtual geometry clusters already use (Karis et al., "Nanite", SIGGRAPH 2021):
//!
//! 1. before the depth prepass, every opaque instance the CPU's frustum cull kept is tested
//!    against last frame's hierarchical depth (HZB); those that may be seen are copied into
//!    their batch's part of the culled instance buffer and drawn by the prepass;
//! 2. after it, the HZB is built from what it drew and the rest are tested against it; those
//!    that may be seen are appended and drawn by the late prepass;
//! 3. the scene pass draws both, through indirect arguments the GPU filled in.
//!
//! What last frame's depth wrongly hid is caught by the second test, so nothing that can be
//! seen is left out: the picture is the same, with fewer triangles behind hills, walls and
//! trees. The main camera only (extra views draw everything the frustum cull kept), with
//! multi-draw indirect; `RenderSettings::mesh_occlusion_culling` (`ENGINE_MESH_OCCLUSION`)
//! switches it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::{GpuShared, GrowBuffer};
use crate::frame::ViewData;

/// One instance to test: its camera-relative bounding sphere, its batch and its place in
/// the instance buffer.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct CullItem {
    pub sphere: [f32; 4],
    pub batch: u32,
    pub instance: u32,
    pub pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    early_view_proj: [[f32; 4]; 4],
    late_view_proj: [[f32; 4]; 4],
    early_offset: [f32; 4],
    hzb: [u32; 4],
    sizes: [u32; 4],
}

/// Indirect draw arguments of the three phases: the early prepass's, the late prepass's,
/// and the scene pass's (both).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Early = 0,
    Late = 1,
    Both = 2,
}

/// Hierarchical depth (`cluster_hzb.wgsl`): level 0 is half the depth buffer, each level half
/// the one before, down to 1 x 1; each texel the farthest depth below it.
struct Hzb {
    view: wgpu::TextureView,
    levels: Vec<wgpu::TextureView>,
    depth_size: [u32; 2],
    size: [u32; 2],
}

impl Hzb {
    fn new(device: &wgpu::Device, depth_size: [u32; 2]) -> Self {
        let size = depth_size.map(|s| (s / 2).max(1));
        let count = 32 - size[0].max(size[1]).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh hzb"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let levels = (0..count)
            .map(|level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        Self {
            view,
            levels,
            depth_size,
            size,
        }
    }
}

pub(crate) struct MeshCulling {
    early: wgpu::ComputePipeline,
    early_args: wgpu::ComputePipeline,
    late: wgpu::ComputePipeline,
    late_args: wgpu::ComputePipeline,
    hzb_from_depth: wgpu::ComputePipeline,
    hzb_reduce: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    items: GrowBuffer,
    culled: GrowBuffer,
    args: GrowBuffer,
    counts: GrowBuffer,
    drawn: GrowBuffer,
    hzb: Hzb,
    /// The camera, camera-relative view-projection and camera position of the frame the HZB
    /// holds, once built.
    built: Option<(Option<Entity>, Mat4, DVec3)>,
    /// The bind group, rebuilt when a buffer or the HZB is recreated.
    group: Option<wgpu::BindGroup>,
    /// This frame: the instance buffer it was made for, and what it culls.
    source: Option<wgpu::Buffer>,
    item_count: u32,
    batches: u32,
    /// This frame's camera and view-projection, for the HZB it builds.
    frame: Option<(Option<Entity>, Mat4, DVec3)>,
    active: bool,
    /// Read-backs of the drawn totals (instances, triangles), a few frames behind: the
    /// buffers, whether each map finished (`None`: free) and the instances tested that frame.
    readback: Vec<(wgpu::Buffer, Option<Arc<AtomicBool>>, u32)>,
    latest: CullStats,
}

/// What the culling did, a few frames ago: instances tested, and instances and triangles
/// drawn.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CullStats {
    pub tested: u32,
    pub instances: u32,
    pub triangles: u32,
}

const INSTANCE_WORDS: u32 = (std::mem::size_of::<crate::mesh_feature::MeshInstance>() / 16) as u32;
const ARGS_SIZE: u64 = 20;

impl MeshCulling {
    pub fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh culling"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, false),
                storage(4, false),
                storage(5, false),
                storage(6, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = gpu.shader(
            "mesh culling",
            include_str!("shaders/mesh_cull.wgsl"),
            false,
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh culling"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |module: &wgpu::ShaderModule, layout, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout,
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let hzb_shader = gpu.shader("mesh hzb", include_str!("shaders/cluster_hzb.wgsl"), false);
        let buffer = |label, usage| GrowBuffer::new(device, label, usage, 4096);
        let storage_usage = wgpu::BufferUsages::STORAGE;
        Self {
            early: pipeline(&shader, Some(&pipeline_layout), "cs_early"),
            early_args: pipeline(&shader, Some(&pipeline_layout), "cs_early_args"),
            late: pipeline(&shader, Some(&pipeline_layout), "cs_late"),
            late_args: pipeline(&shader, Some(&pipeline_layout), "cs_late_args"),
            hzb_from_depth: pipeline(&hzb_shader, None, "cs_vg_hzb_from_depth"),
            hzb_reduce: pipeline(&hzb_shader, None, "cs_vg_hzb_reduce"),
            layout,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh culling"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            items: buffer("mesh cull items", storage_usage),
            culled: buffer(
                "culled mesh instances",
                storage_usage | wgpu::BufferUsages::VERTEX,
            ),
            args: buffer(
                "culled mesh draw arguments",
                storage_usage | wgpu::BufferUsages::INDIRECT,
            ),
            counts: buffer(
                "culled mesh counts",
                storage_usage | wgpu::BufferUsages::COPY_SRC,
            ),
            drawn: buffer("culled mesh early", storage_usage),
            hzb: Hzb::new(device, [1, 1]),
            built: None,
            group: None,
            source: None,
            item_count: 0,
            batches: 0,
            frame: None,
            active: false,
            readback: (0..3)
                .map(|_| {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("mesh culling totals"),
                        size: 8,
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    (buffer, None, 0)
                })
                .collect(),
            latest: CullStats::default(),
        }
    }

    /// The latest totals read back (the GPU's work of a few frames ago).
    pub fn stats(&mut self) -> CullStats {
        for (buffer, pending, tested) in &mut self.readback {
            if pending
                .as_ref()
                .is_some_and(|done| done.load(Ordering::Acquire))
            {
                if let Ok(data) = buffer.slice(..).get_mapped_range() {
                    let totals: [u32; 2] = bytemuck::pod_read_unaligned(&data[..8]);
                    self.latest = CullStats {
                        tested: *tested,
                        instances: totals[0],
                        triangles: totals[1],
                    };
                }
                buffer.unmap();
                *pending = None;
            }
        }
        self.latest
    }

    /// Draw as the CPU culled until `prepare` says otherwise.
    pub fn deactivate(&mut self) {
        self.active = false;
    }

    /// The culled instances (vertex buffer 1 for the opaque batches) and the draw arguments
    /// of `phase` (the batches' `args` index into it, after `offset` entries).
    pub fn draws(&self, phase: Phase) -> Option<(&wgpu::Buffer, &wgpu::Buffer, u64)> {
        self.active.then(|| {
            (
                &self.culled.buffer,
                &self.args.buffer,
                phase as u64 * self.batches as u64 * ARGS_SIZE,
            )
        })
    }

    /// Set up this frame: `items` the opaque instances to test, `args` the opaque batches'
    /// draw arguments (in their `args` order, the instance counts as the CPU kept them),
    /// `source` the instance buffer and `instances` how many it holds. Off (draw as the CPU
    /// culled) when `on` is false.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        gpu: &GpuShared,
        view: &ViewData,
        on: bool,
        items: &[CullItem],
        args: &[[u32; 5]],
        source: &wgpu::Buffer,
        instances: usize,
    ) {
        self.active = on && !view.extra_view && !items.is_empty() && !args.is_empty();
        if !self.active {
            return;
        }
        let device = &gpu.device;
        let queue = &gpu.queue;
        let depth_size = [view.width.max(1), view.height.max(1)];
        let mut regroup = self.group.is_none()
            || self.source.as_ref() != Some(source)
            || self.hzb.depth_size != depth_size;
        if self.hzb.depth_size != depth_size {
            self.hzb = Hzb::new(device, depth_size);
            self.built = None;
        }
        if !view.history_valid {
            self.built = None;
        }
        regroup |= self.items.write(device, queue, bytemuck::cast_slice(items));
        // Early, late, both: each phase's arguments start as the CPU's with no instances.
        let mut all = Vec::with_capacity(args.len() * 3);
        for phase in 0..3 {
            all.extend(args.iter().map(|a| {
                let mut a = *a;
                if phase < 2 {
                    a[1] = 0;
                }
                a
            }));
        }
        regroup |= self.args.write(device, queue, bytemuck::cast_slice(&all));
        // Each batch's count, then the totals.
        regroup |= self
            .counts
            .write(device, queue, &vec![0u8; (args.len() + 2) * 4]);
        regroup |= self.drawn.reserve(device, (items.len() * 4) as u64);
        regroup |= self
            .culled
            .reserve(device, (instances * INSTANCE_WORDS as usize * 16) as u64);
        let early = self
            .built
            .filter(|(camera, ..)| *camera == view.camera)
            .map(|(_, view_proj, origin)| (view_proj, (view.origin - origin).as_vec3()));
        let params = Params {
            early_view_proj: early.map_or(Mat4::IDENTITY, |e| e.0).to_cols_array_2d(),
            late_view_proj: view.view_proj.to_cols_array_2d(),
            early_offset: early.map_or([0.0; 4], |(_, o)| [o.x, o.y, o.z, 1.0]),
            hzb: [
                self.hzb.size[0],
                self.hzb.size[1],
                self.hzb.levels.len() as u32,
                items.len() as u32,
            ],
            sizes: [
                depth_size[0],
                depth_size[1],
                args.len() as u32,
                INSTANCE_WORDS,
            ],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        if regroup {
            fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
                wgpu::BindGroupEntry {
                    binding,
                    resource: buffer.as_entire_binding(),
                }
            }
            self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mesh culling"),
                layout: &self.layout,
                entries: &[
                    entry(0, &self.params),
                    entry(1, &self.items.buffer),
                    entry(2, source),
                    entry(3, &self.culled.buffer),
                    entry(4, &self.args.buffer),
                    entry(5, &self.counts.buffer),
                    entry(6, &self.drawn.buffer),
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&self.hzb.view),
                    },
                ],
            }));
            self.source = Some(source.clone());
        }
        self.item_count = items.len() as u32;
        self.batches = args.len() as u32;
        self.frame = Some((view.camera, view.view_proj, view.origin));
    }

    /// The early phase: before the depth prepass.
    pub fn encode_early(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(group) = self.group.as_ref().filter(|_| self.active) else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("mesh culling: early"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, group, &[]);
        pass.set_pipeline(&self.early);
        pass.dispatch_workgroups(self.item_count.div_ceil(64), 1, 1);
        pass.set_pipeline(&self.early_args);
        pass.dispatch_workgroups(self.batches.div_ceil(64), 1, 1);
    }

    /// The late phase: after the depth prepass, from its depth. True when the late prepass
    /// should draw.
    pub fn encode_late(
        &mut self,
        gpu: &GpuShared,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
    ) -> bool {
        let Some(group) = self.group.as_ref().filter(|_| self.active) else {
            return false;
        };
        // The HZB: level 0 from the depth, then each level from the one before.
        let device = &gpu.device;
        let groups: Vec<_> = self
            .hzb
            .levels
            .iter()
            .enumerate()
            .map(|(level, out)| {
                let (pipeline, input) = if level == 0 {
                    (&self.hzb_from_depth, (1, depth))
                } else {
                    (&self.hzb_reduce, (2, &self.hzb.levels[level - 1]))
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("mesh hzb"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(out),
                        },
                        wgpu::BindGroupEntry {
                            binding: input.0,
                            resource: wgpu::BindingResource::TextureView(input.1),
                        },
                    ],
                });
                (pipeline, group)
            })
            .collect();
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mesh hzb"),
                timestamp_writes: None,
            });
            for (level, (pipeline, group)) in groups.iter().enumerate() {
                let size = self.hzb.size.map(|s| (s >> level).max(1));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
            }
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("mesh culling: late"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, group, &[]);
        pass.set_pipeline(&self.late);
        pass.dispatch_workgroups(self.item_count.div_ceil(64), 1, 1);
        pass.set_pipeline(&self.late_args);
        pass.dispatch_workgroups(self.batches.div_ceil(64), 1, 1);
        drop(pass);
        // The totals into a free read-back buffer, mapped once the GPU is done.
        if let Some(k) = self.readback.iter().position(|(_, p, _)| p.is_none()) {
            let (buffer, pending, tested) = &mut self.readback[k];
            encoder.copy_buffer_to_buffer(
                &self.counts.buffer,
                self.batches as u64 * 4,
                buffer,
                0,
                8,
            );
            let done = Arc::new(AtomicBool::new(false));
            let flag = done.clone();
            encoder.map_buffer_on_submit(buffer, wgpu::MapMode::Read, .., move |r| {
                flag.store(r.is_ok(), Ordering::Release);
            });
            *pending = Some(done);
            *tested = self.item_count;
        }
        // Next frame's early phase tests against this HZB.
        self.built = self.frame;
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::features::{compose_shader, validate_wgsl};

    #[test]
    fn culling_shader_is_valid_wgsl() {
        validate_wgsl(
            "mesh culling",
            &compose_shader(include_str!("shaders/mesh_cull.wgsl"), false),
        )
        .unwrap();
    }

    #[test]
    fn instance_is_whole_vec4s() {
        assert_eq!(size_of::<crate::mesh_feature::MeshInstance>() % 16, 0);
        assert_eq!(size_of::<super::CullItem>(), 32);
    }
}
