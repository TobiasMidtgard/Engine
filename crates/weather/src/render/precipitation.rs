//! Rain streaks, snowflakes, splashes and lightning bolts (transparent phase). Particles are
//! generated in the vertex shader from the instance index, so heavy rain costs no CPU time.

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::{
    FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase, RenderSettings, ViewData, wgpu,
};
use engine_world::{PrecipitationKind, WeatherState};
use serde_json::json;

use super::PrecipitationSettings;
use crate::lightning::ActiveBolts;

/// Fall speed factors of the three drop sizes.
const RAIN_SPEEDS: [f32; 3] = [0.8, 1.0, 1.25];
const SNOW_SPEEDS: [f32; 3] = [0.7, 1.0, 1.35];
/// Luminance of a lightning channel at full brightness, cd/m².
const BOLT_LUMINANCE: f32 = 6.0e7;
const BOLT_COLOR: [f32; 3] = [0.85, 0.88, 1.0];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PrecipUniform {
    area: [f32; 4],
    wrap: [[f32; 4]; 3],
    velocity: [f32; 4],
    look: [f32; 4],
    splash: [f32; 4],
    bolt: [f32; 4],
}

struct Gpu {
    uniform: wgpu::Buffer,
    segments: GrowBuffer,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    rain: wgpu::RenderPipeline,
    snow: wgpu::RenderPipeline,
    splash: wgpu::RenderPipeline,
    bolt: wgpu::RenderPipeline,
}

/// The rain/snow/lightning [`RenderFeature`].
#[derive(Default)]
pub struct PrecipitationFeature {
    gpu: Option<Gpu>,
    time: f64,
    drops: u32,
    flakes: u32,
    splashes: u32,
    segments: u32,
}

fn bind_group(
    device: &wgpu::Device,
    g_layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    segments: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("precipitation"),
        layout: g_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: segments.as_entire_binding(),
            },
        ],
    })
}

/// `(offset − camera)` wrapped into `[0, box)` on each axis, in f64 so it never loses
/// precision far from the origin or after hours of falling.
fn wrapped(offset: DVec3, camera: DVec3, size: Vec3) -> [f32; 3] {
    let d = offset - camera;
    [
        d.x.rem_euclid(size.x as f64) as f32,
        d.y.rem_euclid(size.y as f64) as f32,
        d.z.rem_euclid(size.z as f64) as f32,
    ]
}

impl RenderFeature for PrecipitationFeature {
    fn name(&self) -> &str {
        "weather precipitation"
    }

    fn order(&self) -> i32 {
        20
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("precipitation"),
            size: std::mem::size_of::<PrecipUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let segments = GrowBuffer::new(
            device,
            "lightning segments",
            wgpu::BufferUsages::STORAGE,
            4096,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("precipitation"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let group = bind_group(device, &layout, &uniform, &segments.buffer);
        let shader = gpu.shader(
            "precipitation",
            include_str!("../shaders/precipitation.wgsl"),
            true,
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("precipitation"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let over = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: over.alpha,
        };
        let pipeline = |label: &str, vs: &str, fs: &str, blend: wgpu::BlendState| {
            super::clouds::fullscreen_pipeline(
                gpu,
                label,
                &pipeline_layout,
                &shader,
                vs,
                fs,
                &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.hdr,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                Some(gpu.depth_state(false)),
            )
        };
        self.gpu = Some(Gpu {
            rain: pipeline("rain", "vs_rain", "fs_rain", over),
            snow: pipeline("snow", "vs_snow", "fs_snow", over),
            splash: pipeline("splashes", "vs_splash", "fs_splash", over),
            bolt: pipeline("lightning", "vs_bolt", "fs_bolt", additive),
            uniform,
            segments,
            layout,
            group,
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.drops = 0;
        self.flakes = 0;
        self.splashes = 0;
        self.segments = 0;
        let settings = world
            .get_resource::<PrecipitationSettings>()
            .copied()
            .unwrap_or_default();
        let Some(state) = world.get_resource::<WeatherState>() else {
            return;
        };
        if !settings.enabled || self.gpu.is_none() {
            return;
        }
        self.time += view.delta.max(0.0) as f64;
        let t = self.time;
        let mm = state.precipitation_mm_h.max(0.0);
        let (rain_share, snow_share) = match state.precipitation_kind {
            PrecipitationKind::None => (0.0, 0.0),
            PrecipitationKind::Rain | PrecipitationKind::Hail => (1.0, 0.0),
            PrecipitationKind::Snow => (0.0, 1.0),
            PrecipitationKind::Sleet => (0.6, 0.6),
        };
        let rain = (1.0 - (-mm / 8.0).exp()) * rain_share;
        let snow = (1.0 - (-mm / 1.5).exp()) * snow_share;
        let drops = (settings.max_drops as f32 * rain) as u32;
        let flakes = (settings.max_flakes as f32 * snow) as u32;
        let splashes = (settings.max_splashes as f32 * rain) as u32;

        // Rain falls faster than snow; both drift with the wind.
        let snowing = flakes > drops;
        let fall = if snowing { 1.1 } else { 6.0 + 3.0 * rain };
        let speeds = if snowing { SNOW_SPEEDS } else { RAIN_SPEEDS };
        let wind = state.wind;
        // Snow is drawn densely close to the eye (flakes far off are too small to see).
        let area = settings.area_m.clamp(8.0, 120.0) * if snowing { 0.45 } else { 1.0 };
        let size = Vec3::new(area, area * 0.6, area);
        let mut wrap = [[0.0f32; 4]; 3];
        for (k, s) in speeds.iter().enumerate() {
            let offset = DVec3::new(wind.x as f64, -(fall * s) as f64, wind.z as f64) * t;
            let w = wrapped(offset, view.origin, size);
            wrap[k] = [w[0], w[1], w[2], *s];
        }

        // Lightning channels, camera-relative.
        let mut segs: Vec<[f32; 4]> = Vec::new();
        if settings.bolts
            && let Some(bolts) = world.get_resource::<ActiveBolts>()
        {
            for bolt in bolts.bolts.iter().filter(|b| b.ground) {
                let b = bolt.brightness();
                if b < 0.01 {
                    continue;
                }
                for s in &bolt.segments {
                    let a = (s.a - view.origin).as_vec3();
                    let e = (s.b - view.origin).as_vec3();
                    segs.push([a.x, a.y, a.z, s.width]);
                    segs.push([e.x, e.y, e.z, s.brightness * b]);
                }
            }
        }
        let cloud_base = state.cloud_base_m.max(150.0);

        let u = PrecipUniform {
            area: [size.x, size.y, size.z, settings.shutter_s.max(0.0)],
            wrap,
            velocity: [wind.x, -fall, wind.z, settings.drop_width_m.max(0.0002)],
            look: [0.45, 1.0, 0.3, (t % 3600.0) as f32],
            splash: [9.0, 0.06, 0.02, 0.0],
            bolt: [
                BOLT_COLOR[0] * BOLT_LUMINANCE,
                BOLT_COLOR[1] * BOLT_LUMINANCE,
                BOLT_COLOR[2] * BOLT_LUMINANCE,
                cloud_base,
            ],
        };
        let Some(g) = &mut self.gpu else { return };
        gpu.queue
            .write_buffer(&g.uniform, 0, bytemuck::bytes_of(&u));
        if !segs.is_empty()
            && g.segments
                .write(&gpu.device, &gpu.queue, bytemuck::cast_slice(&segs))
        {
            g.group = bind_group(&gpu.device, &g.layout, &g.uniform, &g.segments.buffer);
        }
        self.drops = drops;
        self.flakes = flakes;
        self.splashes = splashes;
        self.segments = (segs.len() / 2) as u32;
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let Some(g) = &self.gpu else { return };
        pass.set_bind_group(1, &g.group, &[]);
        for (pipeline, count) in [
            (&g.bolt, self.segments),
            (&g.splash, self.splashes),
            (&g.snow, self.flakes),
            (&g.rain, self.drops),
        ] {
            if count > 0 {
                pass.set_pipeline(pipeline);
                pass.draw(0..6, 0..count);
            }
        }
    }

    fn stats(&self) -> engine_render::FeatureStats {
        let instances = self.drops + self.flakes + self.splashes + self.segments;
        engine_render::FeatureStats {
            draw_calls: [self.drops, self.flakes, self.splashes, self.segments]
                .iter()
                .filter(|c| **c > 0)
                .count() as u32,
            instances,
            triangles: instances as u64 * 2,
            culled: 0,
        }
    }

    fn describe(&self) -> serde_json::Value {
        json!({
            "drops": self.drops,
            "flakes": self.flakes,
            "splashes": self.splashes,
            "bolt_segments": self.segments,
        })
    }
}
