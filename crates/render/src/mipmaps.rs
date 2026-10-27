//! Mip chains made on the GPU: each level drawn from the one above it with a bilinear tap at
//! the center of every 2×2 texel block (the usual runtime downsample, as Unreal's and Unity's
//! runtime mip generation do it). Drawing through sRGB views filters in linear light, as the
//! CPU chain (`Image::mip_chain`) does, so colors don't darken. A 4096² image's chain took the
//! main thread hundreds of milliseconds on the CPU; here it is a few passes of GPU work queued
//! with the frame.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::features::GpuShared;

/// Images at least this many texels get their chain on the GPU (smaller ones are quick on the
/// CPU, and keep their exact CPU box filter).
pub(crate) const GPU_MIPS_MIN_TEXELS: u64 = 512 * 512;

/// The downsample pipelines by format, made on first use.
#[derive(Default)]
pub(crate) struct MipGenerator {
    inner: Mutex<Option<Inner>>,
}

struct Inner {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl MipGenerator {
    /// Whether a texture of `format` can take its chain here.
    pub fn supports(format: wgpu::TextureFormat) -> bool {
        matches!(
            format,
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Rgba16Float
        )
    }

    /// Make the pipelines of the usual formats now (with warm pipelines, at the renderer's
    /// start), not on the first large image.
    pub fn warm(&self, device: &wgpu::Device) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = guard.get_or_insert_with(|| Inner::new(device));
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba16Float,
        ] {
            inner.pipeline(device, format);
        }
    }

    /// Fill levels 1.. of `texture` (made with `RENDER_ATTACHMENT`) from level 0, viewed as
    /// `format` (its own format or an sRGB alias); the commands go with the next frame's submit,
    /// after the uploads queued before it.
    pub fn generate(&self, gpu: &GpuShared, texture: &wgpu::Texture, format: wgpu::TextureFormat) {
        let levels = texture.mip_level_count();
        if levels < 2 {
            return;
        }
        let device = &gpu.device;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = guard.get_or_insert_with(|| Inner::new(device));
        let pipeline = inner.pipeline(device, format);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mip chain"),
        });
        let view = |level: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mip level"),
                format: Some(format),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for level in 1..levels {
            let (source, target) = (view(level - 1), view(level));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip downsample"),
                layout: &inner.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&inner.sampler),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip downsample"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        drop(guard);
        gpu.submit_with_frame(encoder.finish(), None);
    }
}

impl Inner {
    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.pipeline_layout);
        self.pipelines
            .entry(format)
            .or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("mip downsample"),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(format.into())],
                    }),
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone()
    }

    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mip downsample"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip downsample"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mip downsample"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mip downsample"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            module,
            layout,
            pipeline_layout,
            sampler,
            pipelines: HashMap::new(),
        }
    }
}

const SHADER: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var bilinear: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // The center of the target texel is the shared corner of its 2×2 source block.
    return textureSampleLevel(source, bilinear, in.uv, 0.0);
}
"#;

#[cfg(test)]
mod tests {
    #[test]
    fn shader_validates() {
        crate::features::validate_wgsl("mip downsample", super::SHADER).unwrap();
    }
}
