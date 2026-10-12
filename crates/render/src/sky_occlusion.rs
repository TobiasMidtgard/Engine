//! Sky visibility at the camera from the occlusion map, computed once per frame on the GPU.
//!
//! The haze along a view ray is lit by the sky only when the camera or the far end is outdoors
//! (`fog_sky_visibility` in `lib_frame.wgsl`). The camera's value is the same for every pixel,
//! so one compute invocation writes it to a small buffer (frame binding 9) after the sky
//! occlusion map is drawn.

use crate::features::GpuShared;

pub(crate) struct SkyCamera {
    pipeline: wgpu::ComputePipeline,
    input_layout: wgpu::BindGroupLayout,
    input: Option<wgpu::BindGroup>,
    output: wgpu::BindGroup,
    /// `array<vec4<f32>>` with the camera's sky visibility in `[0].x`.
    pub buffer: wgpu::Buffer,
}

impl SkyCamera {
    pub fn new(shared: &GpuShared) -> Self {
        let device = &shared.device;
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let input_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky camera input"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    7,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    8,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                ),
            ],
        });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky camera output"),
            entries: &[entry(
                0,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky camera"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        shared
            .queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 0.0]));
        let output = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky camera output"),
            layout: &output_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky camera"),
            bind_group_layouts: &[Some(&input_layout), Some(&output_layout)],
            immediate_size: 0,
        });
        let module = shared.shader("sky camera", include_str!("shaders/sky_camera.wgsl"), true);
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sky camera"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("cs_sky_camera"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            input_layout,
            input: None,
            output,
            buffer,
        }
    }

    /// (Re)bind the frame uniform and the occlusion map (after either is recreated).
    pub fn set_inputs(
        &mut self,
        shared: &GpuShared,
        frame: &wgpu::Buffer,
        occlusion: &wgpu::TextureView,
    ) {
        self.input = Some(shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky camera input"),
            layout: &self.input_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(occlusion),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&shared.nearest_clamp),
                },
            ],
        }));
    }

    /// Record the dispatch. Call after the sky occlusion map is drawn.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(input) = &self.input else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("sky camera"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, input, &[]);
        pass.set_bind_group(1, &self.output, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}
