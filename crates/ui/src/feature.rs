//! [`UiFeature`]: the render feature that draws the UI onto the final image.
//!
//! Per frame: upload changed atlas regions and the instance buffer, blur the scene for frosted
//! panels when any are visible (quarter resolution, two Gaussian passes), then draw every UI
//! primitive with one instanced draw call.

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderSettings, ViewData,
};

use crate::atlas::Atlas;
use crate::draw::{DrawList, MAX_PLANET_LOOKS, PlanetLook, UiAtlases, UiInstance, build_draw_list};

/// The UI shader (composed after `lib_types.wgsl`).
pub const SHADER: &str = include_str!("shaders/ui.wgsl");
/// Backdrop blur shader (composed after `lib_types.wgsl`).
pub const BLUR_SHADER: &str = include_str!("shaders/ui_blur.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Globals {
    viewport: [f32; 4],
    time: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BlurParams {
    texel_dir: [f32; 4],
}

struct AtlasTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

struct Blur {
    /// Quarter-resolution ping-pong targets.
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    size: (u32, u32),
    /// ldr → a, a → b (horizontal), b → a (vertical).
    groups: Option<[wgpu::BindGroup; 3]>,
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    down_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    textures_layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::BindGroupLayout,
    looks: wgpu::Buffer,
    blur_params: [wgpu::Buffer; 3],
    instances: GrowBuffer,
    mask: Option<AtlasTexture>,
    color: Option<AtlasTexture>,
    blur: Option<Blur>,
    textures_group: Option<wgpu::BindGroup>,
}

/// Draws the UI. Added by [`UiPlugin`](crate::UiPlugin).
#[derive(Default)]
pub struct UiFeature {
    gpu: Option<Gpu>,
    count: u32,
    uses_backdrop: bool,
    stats: FeatureStats,
    size: (u32, u32),
}

const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn atlas_texture(gpu: &GpuShared, atlas: &Atlas, label: &str) -> AtlasTexture {
    let format = if atlas.bpp == 1 {
        wgpu::TextureFormat::R8Unorm
    } else {
        wgpu::TextureFormat::Rgba8UnormSrgb
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    AtlasTexture {
        texture,
        view,
        size: (atlas.width, atlas.height),
    }
}

fn upload(gpu: &GpuShared, atlas: &mut Atlas, tex: &AtlasTexture) {
    let Some([x0, y0, x1, y1]) = atlas.dirty.take() else {
        return;
    };
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let bpp = atlas.bpp;
    let offset = (y0 * atlas.width + x0) as u64 * bpp as u64;
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex.texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &atlas.data,
        wgpu::TexelCopyBufferLayout {
            offset,
            bytes_per_row: Some(atlas.width * bpp),
            rows_per_image: Some(atlas.height),
        },
        wgpu::Extent3d {
            width: x1 - x0,
            height: y1 - y0,
            depth_or_array_layers: 1,
        },
    );
}

fn blur_target(gpu: &GpuShared, w: u32, h: u32, label: &str) -> wgpu::TextureView {
    gpu.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: BLUR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn fullscreen_pipeline(
    gpu: &GpuShared,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    fragment: &str,
) -> wgpu::RenderPipeline {
    gpu.device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(fragment),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: BLUR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
}

impl UiInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Uint32x4
    ];

    /// Instance-rate vertex buffer layout.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

impl Gpu {
    fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let shader = gpu.shader("ui", SHADER, false);
        let blur_shader = gpu.shader("ui blur", BLUR_SHADER, false);
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui globals"),
            entries: &[uniform(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            )],
        });
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui textures"),
            entries: &[
                texture(0),
                texture(1),
                texture(2),
                sampler(3),
                uniform(4, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui blur"),
            entries: &[
                texture(0),
                sampler(1),
                uniform(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui"),
            bind_group_layouts: &[Some(&globals_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui blur"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(UiInstance::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.ldr,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let down_pipeline =
            fullscreen_pipeline(gpu, &blur_pipeline_layout, &blur_shader, "fs_down");
        let blur_pipeline =
            fullscreen_pipeline(gpu, &blur_pipeline_layout, &blur_shader, "fs_blur");
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = buffer("ui globals", std::mem::size_of::<Globals>() as u64);
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let looks = buffer(
            "ui planet looks",
            (std::mem::size_of::<PlanetLook>() * MAX_PLANET_LOOKS) as u64,
        );
        let blur_params = [
            buffer("ui blur down", 16),
            buffer("ui blur h", 16),
            buffer("ui blur v", 16),
        ];
        Self {
            pipeline,
            down_pipeline,
            blur_pipeline,
            globals,
            globals_group,
            textures_layout,
            blur_layout,
            looks,
            blur_params,
            instances: GrowBuffer::new(
                device,
                "ui instances",
                wgpu::BufferUsages::VERTEX,
                4096 * std::mem::size_of::<UiInstance>() as u64,
            ),
            mask: None,
            color: None,
            blur: None,
            textures_group: None,
        }
    }

    fn ensure_blur(&mut self, gpu: &GpuShared, w: u32, h: u32) {
        let size = ((w / 4).max(1), (h / 4).max(1));
        if self.blur.as_ref().is_some_and(|b| b.size == size) {
            return;
        }
        self.blur = Some(Blur {
            a: blur_target(gpu, size.0, size.1, "ui blur a"),
            b: blur_target(gpu, size.0, size.1, "ui blur b"),
            size,
            groups: None,
        });
        self.textures_group = None;
        let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
        let (qw, qh) = (size.0 as f32, size.1 as f32);
        let params = [
            BlurParams {
                texel_dir: [1.0 / fw, 1.0 / fh, 0.0, 0.0],
            },
            BlurParams {
                texel_dir: [1.0 / qw, 1.0 / qh, 1.0, 0.0],
            },
            BlurParams {
                texel_dir: [1.0 / qw, 1.0 / qh, 0.0, 1.0],
            },
        ];
        for (buf, p) in self.blur_params.iter().zip(params) {
            gpu.queue.write_buffer(buf, 0, bytemuck::bytes_of(&p));
        }
    }

    fn textures_group(&mut self, gpu: &GpuShared) -> Option<&wgpu::BindGroup> {
        if self.textures_group.is_none() {
            let (mask, color, blur) = (
                self.mask.as_ref()?,
                self.color.as_ref()?,
                self.blur.as_ref()?,
            );
            self.textures_group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui textures"),
                layout: &self.textures_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&mask.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&color.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&blur.a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&gpu.linear_clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.looks.as_entire_binding(),
                    },
                ],
            }));
        }
        self.textures_group.as_ref()
    }
}

impl RenderFeature for UiFeature {
    fn name(&self) -> &str {
        "ui"
    }

    /// After debug lines (1000): game UI stays on top.
    fn order(&self) -> i32 {
        2000
    }

    fn init(&mut self, gpu: &GpuShared) {
        self.gpu = Some(Gpu::new(gpu));
    }

    fn resize(&mut self, gpu: &GpuShared, width: u32, height: u32) {
        self.size = (width, height);
        if let Some(g) = &mut self.gpu {
            g.ensure_blur(gpu, width, height);
            if let Some(b) = &mut g.blur {
                b.groups = None;
            }
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.count = 0;
        self.stats = FeatureStats::default();
        let Some(g) = &mut self.gpu else {
            return;
        };
        if !world.contains_resource::<crate::layout::UiDrawOrder>() {
            return;
        }
        build_draw_list(world);
        let (w, h) = (view.width.max(1), view.height.max(1));
        if self.size != (w, h) {
            self.size = (w, h);
            g.ensure_blur(gpu, w, h);
            if let Some(b) = &mut g.blur {
                b.groups = None;
            }
        }
        g.ensure_blur(gpu, w, h);

        // Atlases.
        let mut atlases = world.resource_mut::<UiAtlases>();
        for (is_mask, slot) in [(true, &mut g.mask), (false, &mut g.color)] {
            let atlas = if is_mask {
                &mut atlases.mask
            } else {
                &mut atlases.color
            };
            if slot.as_ref().map(|t| t.size) != Some((atlas.width, atlas.height)) || atlas.resized {
                *slot = Some(atlas_texture(
                    gpu,
                    atlas,
                    if is_mask {
                        "ui glyph atlas"
                    } else {
                        "ui color atlas"
                    },
                ));
                atlas.resized = false;
                atlas.dirty = Some([0, 0, atlas.width, atlas.height]);
                g.textures_group = None;
            }
            if let Some(tex) = slot.as_ref() {
                upload(gpu, atlas, tex);
            }
        }

        let list = world.resource::<DrawList>();
        self.count = list.instances.len() as u32;
        self.uses_backdrop = list.uses_backdrop;
        let globals = Globals {
            viewport: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
            time: [view.real_time, 0.0, 0.0, 0.0],
        };
        gpu.queue
            .write_buffer(&g.globals, 0, bytemuck::bytes_of(&globals));
        if !list.looks.is_empty() {
            gpu.queue
                .write_buffer(&g.looks, 0, bytemuck::cast_slice(&list.looks));
        }
        if self.count > 0 {
            let grown = g.instances.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&list.instances),
            );
            let _ = grown;
        }
        self.stats.draw_calls = (self.count > 0) as u32;
        self.stats.instances = self.count;
        let glyphs = list.glyphs;
        if let Some(mut d) = world.get_resource_mut::<engine_core::diagnostics::Diagnostics>() {
            d.set("ui.instances", self.count as f64);
            d.set("ui.glyphs", glyphs as f64);
        }
    }

    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if self.count == 0 {
            return;
        }
        let Some(g) = &mut self.gpu else {
            return;
        };
        let gpu = ctx.gpu;
        if self.uses_backdrop
            && let Some(blur) = &mut g.blur
        {
            if blur.groups.is_none() {
                let group = |src: &wgpu::TextureView, params: &wgpu::Buffer| {
                    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("ui blur"),
                        layout: &g.blur_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(src),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&gpu.linear_clamp),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: params.as_entire_binding(),
                            },
                        ],
                    })
                };
                blur.groups = Some([
                    group(ctx.ldr, &g.blur_params[0]),
                    group(&blur.a, &g.blur_params[1]),
                    group(&blur.b, &g.blur_params[2]),
                ]);
            }
            let groups = blur.groups.as_ref().expect("blur groups");
            let passes: [(&wgpu::TextureView, &wgpu::RenderPipeline, &wgpu::BindGroup); 5] = [
                (&blur.a, &g.down_pipeline, &groups[0]),
                (&blur.b, &g.blur_pipeline, &groups[1]),
                (&blur.a, &g.blur_pipeline, &groups[2]),
                (&blur.b, &g.blur_pipeline, &groups[1]),
                (&blur.a, &g.blur_pipeline, &groups[2]),
            ];
            for (target, pipeline, group) in passes {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui backdrop blur"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        let count = self.count;
        let Some(textures) = g.textures_group(gpu).cloned() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.ldr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&g.pipeline);
        pass.set_bind_group(0, &g.globals_group, &[]);
        pass.set_bind_group(1, &textures, &[]);
        pass.set_vertex_buffer(0, g.instances.buffer.slice(..));
        pass.draw(0..6, 0..count);
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "instances": self.count,
            "backdrop_blur": self.uses_backdrop,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_validate() {
        let code = engine_render::compose_shader(SHADER, false);
        engine_render::validate_wgsl("ui", &code).unwrap();
        let code = engine_render::compose_shader(BLUR_SHADER, false);
        engine_render::validate_wgsl("ui blur", &code).unwrap();
    }

    #[test]
    fn instance_layout_matches_struct() {
        assert_eq!(std::mem::size_of::<UiInstance>(), 160);
        assert_eq!(UiInstance::layout().array_stride, 160);
        assert_eq!(std::mem::size_of::<PlanetLook>(), 176);
    }
}
