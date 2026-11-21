//! The sun's trace (`trial_cut_more::RAY_SUN`, ray_sun.wgsl): one ray toward a point of the
//! sun's disc from each pixel of the prepass that wants one, a dispatch of the frame's one ray
//! pass (renderer.rs) against the scene's still structure. Its output, `sun_raw`, is made when
//! the switch turns on and dropped with this when it turns off; nothing reads it yet (the light
//! pass joins it in step 8 of the ray-traced lighting round).

use crate::features::GpuShared;
use crate::gpu_counts::GpuCounts;
use crate::renderer::layout_entry;
use crate::targets::{Targets, Tex};

/// `sun_raw` (r: the sun's visibility, g: the flags; ray_sun.wgsl). 4 bytes a pixel, 14.7 MB
/// at 2560x1440.
const RAW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// How far a ray of the trace looks for something between its pixel and the sun (meters,
/// `RayUniform::sun` y): what stands farther up-sun is left to the shadow maps.
pub(crate) const RAY_LENGTH: f32 = 64.0;

/// ray_sun.wgsl after the ray library, in group 1 as every pipeline of the ray pass has it.
fn shader_source() -> String {
    format!(
        "{}\n{}",
        super::shader_lib(1),
        include_str!("../shaders/ray_sun.wgsl")
    )
}

/// The trace's program, its output and the count of its rays.
pub(crate) struct SunTrace {
    pipeline: wgpu::ComputePipeline,
    /// Group 2: `sun_raw`, the counts, the still images.
    layout: wgpu::BindGroupLayout,
    /// What the rays found, at the view's size (made again when that changes).
    raw: Option<Tex>,
    /// Rays, rays stopped, "missed" and "false" (`render.ray.sun_*`), counted while
    /// `shade_skip::COUNT_MESH_RUNS` is set.
    counts: GpuCounts,
    /// A 1x1 stand-in for the still images, where the view has none.
    no_images: wgpu::TextureView,
    /// This frame's group 2 ([`begin`](Self::begin)), and whether the frame counts.
    group: Option<wgpu::BindGroup>,
    counting: bool,
}

impl SunTrace {
    /// `scene`: the ray scene's layout (group 1).
    pub(crate) fn new(shared: &GpuShared, scene: &wgpu::BindGroupLayout) -> Self {
        let device = &shared.device;
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray shadow trace"),
            entries: &[
                layout_entry(
                    0,
                    compute,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: RAW_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
                layout_entry(
                    1,
                    compute,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(
                    2,
                    compute,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
            ],
        });
        // The frame's group and the ray scene's as every pipeline of the ray pass has them (a
        // pipeline with another layout there would have wgpu walk the scene's instances again).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray shadow trace"),
            bind_group_layouts: &[Some(&shared.frame_layout), Some(scene), Some(&layout)],
            immediate_size: 0,
        });
        let module = shared.shader("ray shadow trace", &shader_source(), true);
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_sun_trace"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_sun_trace"),
            compilation_options: Default::default(),
            cache: None,
        });
        let no_images = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ray shadow trace still images (none)"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        Self {
            pipeline,
            layout,
            raw: None,
            counts: GpuCounts::new(device, "ray shadow trace counts"),
            no_images,
            group: None,
            counting: false,
        }
    }

    /// Before the frame's ray pass: this frame's group 2, with the counts that arrived taken in
    /// and this frame's cleared when `count` asks for them. `still_images`: the shadow maps'
    /// still images, one layer a map (`None`: the view has none).
    pub(crate) fn begin(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        still_images: Option<&wgpu::TextureView>,
        count: bool,
    ) {
        self.counts.take();
        if !count {
            self.counts.latest = None;
        }
        let size = (targets.width, targets.height);
        let raw = match &mut self.raw {
            Some(t) if (t.width, t.height) == size => t,
            slot => slot.insert(Tex::new(
                device,
                "ray shadow trace",
                size.0,
                size.1,
                RAW_FORMAT,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            )),
        };
        self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray shadow trace"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&raw.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.counts.counts.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        still_images.unwrap_or(&self.no_images),
                    ),
                },
            ],
        }));
        self.counting = count;
        if count {
            self.counts.clear(encoder);
        }
    }

    /// In the frame's ray pass (the frame's group and the scene's set): the trace, one
    /// invocation a pixel of the view.
    pub(crate) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>, targets: &Targets) {
        let Some(group) = &self.group else {
            return;
        };
        pass.set_bind_group(2, group, &[]);
        pass.set_pipeline(&self.pipeline);
        pass.dispatch_workgroups(targets.width.div_ceil(8), targets.height.div_ceil(8), 1);
    }

    /// After the frame's ray pass: the counts copied out, when the frame counted them.
    pub(crate) fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.group.take().is_some() {
            self.counts.copy(encoder, self.counting);
        }
    }

    /// The latest counts read back (rays, stopped, missed, false), while the frames count.
    pub(crate) fn counts(&self) -> Option<[u32; 4]> {
        self.counts.latest
    }
}

#[cfg(test)]
mod tests {
    /// ray_sun.wgsl composes with the ray library and the frame's, and passes validation.
    #[test]
    fn the_suns_trace_validates() {
        let code = crate::features::compose_shader(&super::shader_source(), true);
        crate::features::validate_wgsl("ray_sun", &code).unwrap();
    }
}
