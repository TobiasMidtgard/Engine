//! Sky visibility at the camera from the occlusion map, computed once per frame on the GPU.
//!
//! The haze along a view ray is lit by the sky only when the camera or the far end is outdoors
//! (`fog_sky_visibility` in `lib_frame.wgsl`). The camera's value is the same for every pixel,
//! so one compute invocation writes it to a small buffer (frame binding 9) after the sky
//! occlusion map is drawn.
//!
//! Before it, the map's cover pyramid (frame binding 25, shaders/sky_cover.wgsl) is built from
//! the map: per block of texels the highest surface in and around it, so the per-pixel sky
//! visibility skips the rings of taps that would find no cover (a hierarchical skip like a
//! hierarchical-Z buffer's, Greene, Kass and Miller 1993). The image is the same.

use crate::features::GpuShared;

const COVER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// The cover pyramid of one occlusion map size and the bind groups that build it.
struct Cover {
    /// Level L: the highest surface in each 2^(L+1)-texel block of the map (`blocks`), then
    /// that over the block and its eight neighbors (`pyramid`, which the frame group binds).
    #[allow(dead_code)] // kept alive with their views
    textures: [wgpu::Texture; 2],
    pyramid: wgpu::TextureView,
    /// Per level: (halve, widen) bind groups and that level's size.
    steps: Vec<(wgpu::BindGroup, wgpu::BindGroup, u32)>,
}

pub(crate) struct SkyCamera {
    pipeline: wgpu::ComputePipeline,
    input_layout: wgpu::BindGroupLayout,
    input: Option<wgpu::BindGroup>,
    output: wgpu::BindGroup,
    /// `array<vec4<f32>>` with the camera's sky visibility in `[0].x`.
    pub buffer: wgpu::Buffer,
    first: wgpu::ComputePipeline,
    halve: wgpu::ComputePipeline,
    widen: wgpu::ComputePipeline,
    first_layout: wgpu::BindGroupLayout,
    level_layout: wgpu::BindGroupLayout,
    cover: Option<Cover>,
    /// Cover everywhere (depth 0): nothing is skipped until the pyramid exists.
    cover_stand_in: wgpu::TextureView,
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
                entry(25, cover_binding()),
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

        let storage = entry(
            2,
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: COVER_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
        );
        let first_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky cover first"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                storage,
            ],
        });
        let level_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky cover level"),
            entries: &[entry(1, cover_binding()), storage],
        });
        let cover_module =
            shared.shader("sky cover", include_str!("shaders/sky_cover.wgsl"), false);
        let cover_pipeline = |layout: &wgpu::BindGroupLayout, entry: &str| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sky cover"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &cover_module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let first = cover_pipeline(&first_layout, "cs_first");
        let halve = cover_pipeline(&level_layout, "cs_halve");
        let widen = cover_pipeline(&level_layout, "cs_widen");
        let stand_in = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("no sky cover pyramid"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COVER_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self {
            pipeline,
            input_layout,
            input: None,
            output,
            buffer,
            first,
            halve,
            widen,
            first_layout,
            level_layout,
            cover: None,
            // New textures start zeroed: depth 0, cover everywhere.
            cover_stand_in: stand_in.create_view(&Default::default()),
        }
    }

    /// The cover pyramid for frame binding 25.
    pub fn cover_view(&self) -> &wgpu::TextureView {
        self.cover
            .as_ref()
            .map_or(&self.cover_stand_in, |c| &c.pyramid)
    }

    /// (Re)create the cover pyramid for an occlusion map `size`² texels and bind its build to
    /// `occlusion`.
    fn set_cover(&mut self, shared: &GpuShared, occlusion: &wgpu::TextureView, size: u32) {
        let device = &shared.device;
        let mut levels = Vec::new();
        let mut n = size.div_ceil(2).max(1);
        loop {
            levels.push(n);
            if n == 1 {
                break;
            }
            n = n.div_ceil(2);
        }
        let texture = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: levels[0],
                    height: levels[0],
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COVER_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
        };
        let blocks = texture("sky cover blocks");
        let pyramid = texture("sky cover pyramid");
        let level = |t: &wgpu::Texture, l: usize| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: l as u32,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let group = |layout: &wgpu::BindGroupLayout,
                     binding: u32,
                     source: &wgpu::TextureView,
                     dest: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sky cover"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(dest),
                    },
                ],
            })
        };
        let steps = levels
            .iter()
            .enumerate()
            .map(|(l, &n)| {
                let block = level(&blocks, l);
                let halve = if l == 0 {
                    group(&self.first_layout, 0, occlusion, &block)
                } else {
                    group(&self.level_layout, 1, &level(&blocks, l - 1), &block)
                };
                let widen = group(&self.level_layout, 1, &block, &level(&pyramid, l));
                (halve, widen, n)
            })
            .collect();
        self.cover = Some(Cover {
            pyramid: pyramid.create_view(&Default::default()),
            textures: [blocks, pyramid],
            steps,
        });
    }

    /// (Re)bind the frame uniform and the occlusion map, `occlusion_size`² texels (after
    /// either is recreated). Rebuild the frame groups after: the cover pyramid is new.
    pub fn set_inputs(
        &mut self,
        shared: &GpuShared,
        frame: &wgpu::Buffer,
        occlusion: &wgpu::TextureView,
        occlusion_size: u32,
    ) {
        self.set_cover(shared, occlusion, occlusion_size);
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
                wgpu::BindGroupEntry {
                    binding: 25,
                    resource: wgpu::BindingResource::TextureView(self.cover_view()),
                },
            ],
        }));
    }

    /// Record the cover pyramid's build and the camera's dispatch. Call after the sky
    /// occlusion map is drawn.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(input) = &self.input else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("sky camera"),
            timestamp_writes: None,
        });
        if let Some(cover) = &self.cover {
            for (l, (halve, widen, n)) in cover.steps.iter().enumerate() {
                pass.set_pipeline(if l == 0 { &self.first } else { &self.halve });
                pass.set_bind_group(0, halve, &[]);
                pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), 1);
                pass.set_pipeline(&self.widen);
                pass.set_bind_group(0, widen, &[]);
                pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), 1);
            }
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, input, &[]);
        pass.set_bind_group(1, &self.output, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

/// Frame binding 25 (and the cover build's sources): the cover pyramid, read with
/// `textureLoad`.
pub(crate) fn cover_binding() -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: false },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}
