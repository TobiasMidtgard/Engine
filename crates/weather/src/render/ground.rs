//! Puddles, wet and snowy ground from [`SurfaceLayers`] (transparent phase, first): a
//! fullscreen pass over the scene that reads the simulated ground cover around the camera.

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_render::{
    FrameContext, GpuShared, RenderFeature, RenderPhase, RenderSettings, ViewData, wgpu,
};
use engine_world::{PrecipitationKind, SurfaceLayers, WeatherState};
use serde_json::json;

use super::GroundEffectSettings;
use super::clouds::fullscreen_pipeline;
use crate::surface::GroundWeather;

/// Snow depth stored as 1.0 in the cover texture, m.
const SNOW_SCALE_M: f32 = 0.3;
/// Seconds between cover uploads (the ground changes a few times per second at most).
const UPLOAD_INTERVAL_S: f32 = 0.25;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GroundUniform {
    map: [f32; 4],
    rain: [f32; 4],
    params: [f32; 4],
}

struct Gpu {
    uniform: wgpu::Buffer,
    cover: wgpu::Texture,
    cover_view: wgpu::TextureView,
    cover_size: (u32, u32),
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

/// The ground-cover [`RenderFeature`].
#[derive(Default)]
pub struct GroundFeature {
    gpu: Option<Gpu>,
    group: Option<wgpu::BindGroup>,
    active: bool,
    bytes: Vec<u8>,
    since_upload: f32,
    time: f64,
}

fn cover_texture(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ground cover"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

impl RenderFeature for GroundFeature {
    fn name(&self) -> &str {
        "weather ground"
    }

    fn order(&self) -> i32 {
        // First in the transparent phase: it rewrites ground pixels from the scene copy.
        -1000
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ground fx"),
            size: std::mem::size_of::<GroundUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tex = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground fx"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(1, float),
                tex(2, float),
                tex(3, wgpu::TextureSampleType::Depth),
            ],
        });
        let shader = gpu.shader("ground fx", include_str!("../shaders/ground.wgsl"), true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ground fx"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = fullscreen_pipeline(
            gpu,
            "ground fx",
            &pipeline_layout,
            &shader,
            "vs_ground",
            "fs_ground",
            &[Some(wgpu::ColorTargetState {
                format: gpu.formats.hdr,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            Some(super::depth_always(gpu)),
        );
        let (cover, cover_view) = cover_texture(device, 1, 1);
        self.gpu = Some(Gpu {
            uniform,
            cover,
            cover_view,
            cover_size: (1, 1),
            layout,
            pipeline,
        });
        self.since_upload = f32::INFINITY;
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        let settings = world
            .get_resource::<GroundEffectSettings>()
            .copied()
            .unwrap_or_default();
        self.time += view.delta.max(0.0) as f64;
        self.since_upload += view.delta.max(1.0 / 240.0);
        let (Some(surface), Some(g)) = (world.get_resource::<SurfaceLayers>(), &mut self.gpu)
        else {
            self.active = false;
            return;
        };
        if !settings.enabled || view.orthographic {
            self.active = false;
            return;
        }
        let max_puddle = world
            .get_resource::<GroundWeather>()
            .map(|p| p.max_puddle_m)
            .unwrap_or(0.04)
            .max(1e-3);
        let grid = &surface.grid;
        if self.since_upload >= UPLOAD_INTERVAL_S || self.bytes.is_empty() {
            self.since_upload = 0.0;
            let mut any = false;
            self.bytes.clear();
            self.bytes.reserve(grid.data.len() * 4);
            for c in &grid.data {
                let px = [
                    unorm(c.wetness),
                    unorm(c.puddle_m / max_puddle),
                    unorm(c.snow_m / SNOW_SCALE_M),
                    unorm(c.scorch),
                ];
                any |= px[0] > 2 || px[1] > 0 || px[2] > 0;
                self.bytes.extend_from_slice(&px);
            }
            self.active = any;
            if any {
                let size = (grid.width, grid.height);
                if g.cover_size != size {
                    let (t, v) = cover_texture(&gpu.device, size.0, size.1);
                    g.cover = t;
                    g.cover_view = v;
                    g.cover_size = size;
                }
                gpu.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &g.cover,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &self.bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size.0 * 4),
                        rows_per_image: Some(size.1),
                    },
                    wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        if !self.active {
            return;
        }
        let rain = world
            .get_resource::<WeatherState>()
            .filter(|s| {
                matches!(
                    s.precipitation_kind,
                    PrecipitationKind::Rain | PrecipitationKind::Sleet | PrecipitationKind::Hail
                )
            })
            .map(|s| 1.0 - (-s.precipitation_mm_h.max(0.0) / 6.0).exp())
            .unwrap_or(0.0);
        let corner = grid.origin - engine_core::math::DVec2::new(view.origin.x, view.origin.z);
        let u = GroundUniform {
            map: [
                corner.x as f32,
                corner.y as f32,
                grid.width as f32 * grid.cell,
                grid.width as f32,
            ],
            rain: [
                rain,
                (self.time % 1000.0) as f32,
                if settings.ripples { 1.0 } else { 0.0 },
                max_puddle,
            ],
            params: [
                settings.reflection_steps.min(96) as f32,
                SNOW_SCALE_M,
                0.0,
                0.0,
            ],
        };
        gpu.queue
            .write_buffer(&g.uniform, 0, bytemuck::bytes_of(&u));
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        self.group = None;
        if !self.active {
            return;
        }
        let Some(g) = &self.gpu else { return };
        self.group = Some(
            ctx.gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ground fx"),
                    layout: &g.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: g.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&g.cover_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_color_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                        },
                    ],
                }),
        );
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let (Some(g), Some(group)) = (&self.gpu, &self.group) else {
            return;
        };
        pass.set_pipeline(&g.pipeline);
        pass.set_bind_group(1, group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn needs_scene_copy(&self) -> bool {
        self.active
    }

    fn describe(&self) -> serde_json::Value {
        json!({ "active": self.active })
    }
}
