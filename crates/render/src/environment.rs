//! The sky environment map and its filtered versions, rebuilt whenever the sky is redrawn
//! (`RenderSettings::environment_interval`):
//!
//! - `source`: the equirectangular sky (256×128) that `RenderFeature::draw_environment` renders
//!   into, with a box-filtered mip chain;
//! - `specular`: the sky prefiltered for GGX reflections, one mip per perceptual roughness step
//!   (mip 0 sharp, the last fully rough), so a rough surface reflects a blurred sky
//!   (`sample_env_rough`);
//! - `irradiance` (32×16): the cosine-weighted sky radiance around every normal, the diffuse
//!   light from the sky (`sky_ambient`).
//!
//! Shaders read `specular`, `irradiance` and the sampler through frame bindings 5, 16 and 17
//! (lib_frame.wgsl). The filters are compute passes in `environment_filter.wgsl`.

use crate::features::GpuShared;
use crate::renderer::layout_entry;
use crate::targets::Tex;

pub(crate) const ENV_WIDTH: u32 = 256;
pub(crate) const ENV_HEIGHT: u32 = 128;
/// 256×128 down to 2×1.
const SOURCE_MIPS: u32 = 8;
/// Perceptual roughness 0, 0.2, … 1 (must match `SPECULAR_MIPS` in environment_filter.wgsl).
pub(crate) const SPECULAR_MIPS: u32 = 6;
const IRRADIANCE_SIZE: (u32, u32) = (32, 16);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(crate) struct EnvironmentMaps {
    /// Mip 0 of the source map: where the sky is drawn.
    pub target: wgpu::TextureView,
    /// All mips of the prefiltered map (frame binding 5).
    pub specular: wgpu::TextureView,
    /// Frame binding 16.
    pub irradiance: Tex,
    /// Wraps around the horizon, clamps at the poles, blends between mips (frame binding 17).
    pub sampler: wgpu::Sampler,
    downsample: wgpu::ComputePipeline,
    prefilter: wgpu::ComputePipeline,
    convolve: wgpu::ComputePipeline,
    /// One per source mip after the first.
    downsample_groups: Vec<(wgpu::BindGroup, (u32, u32))>,
    /// One per specular mip.
    prefilter_groups: Vec<(wgpu::BindGroup, (u32, u32))>,
    irradiance_group: wgpu::BindGroup,
}

impl EnvironmentMaps {
    pub fn new(shared: &GpuShared) -> Self {
        let device = &shared.device;
        debug_assert_eq!(shared.formats.env, FORMAT);
        let texture = |label: &str, mips: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: ENV_WIDTH,
                    height: ENV_HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage,
                view_formats: &[],
            })
        };
        use wgpu::TextureUsages as U;
        let source = texture(
            "environment",
            SOURCE_MIPS,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::STORAGE_BINDING,
        );
        let specular = texture(
            "environment specular",
            SPECULAR_MIPS,
            U::TEXTURE_BINDING | U::STORAGE_BINDING,
        );
        let irradiance = Tex::new(
            device,
            "environment irradiance",
            IRRADIANCE_SIZE.0,
            IRRADIANCE_SIZE.1,
            FORMAT,
            U::TEXTURE_BINDING | U::STORAGE_BINDING,
        );
        let mip = |t: &wgpu::Texture, level: u32| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("environment"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        use wgpu::ShaderStages as S;
        let input = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let output = wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("environment filter"),
            entries: &[
                layout_entry(0, S::COMPUTE, input),
                layout_entry(
                    1,
                    S::COMPUTE,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                layout_entry(2, S::COMPUTE, output),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("environment filter"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = shared.shader(
            "environment filter",
            include_str!("shaders/environment_filter.wgsl"),
            false,
        );
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let group = |input: &wgpu::TextureView, output: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("environment filter"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(output),
                    },
                ],
            })
        };
        let size = |level: u32| ((ENV_WIDTH >> level).max(1), (ENV_HEIGHT >> level).max(1));
        let source_all = source.create_view(&Default::default());
        let downsample_groups = (1..SOURCE_MIPS)
            .map(|level| {
                (
                    group(&mip(&source, level - 1), &mip(&source, level)),
                    size(level),
                )
            })
            .collect();
        let prefilter_groups = (0..SPECULAR_MIPS)
            .map(|level| (group(&source_all, &mip(&specular, level)), size(level)))
            .collect();
        let irradiance_group = group(&source_all, &irradiance.view);

        Self {
            target: mip(&source, 0),
            specular: specular.create_view(&Default::default()),
            irradiance,
            downsample: pipeline("cs_downsample"),
            prefilter: pipeline("cs_prefilter"),
            convolve: pipeline("cs_irradiance"),
            sampler,
            downsample_groups,
            prefilter_groups,
            irradiance_group,
        }
    }

    /// Record the filters after the sky was drawn into `target`.
    pub fn filter(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("environment filter"),
            timestamp_writes: None,
        });
        let mut run =
            |pipeline: &wgpu::ComputePipeline, group: &wgpu::BindGroup, size: (u32, u32)| {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
            };
        for (group, size) in &self.downsample_groups {
            run(&self.downsample, group, *size);
        }
        for (group, size) in &self.prefilter_groups {
            run(&self.prefilter, group, *size);
        }
        run(&self.convolve, &self.irradiance_group, IRRADIANCE_SIZE);
    }
}
