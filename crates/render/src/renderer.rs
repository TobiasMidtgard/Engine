//! The frame loop.
//!
//! ```text
//! prepare (features read the world) → compute → environment map → shadow cascades
//! → scene pass [opaque → sky] → (scene copies) → transparent pass
//! → bloom → tonemap to LDR → overlays (UI) → screenshot readback → blit to window
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_core::window::RawWindowHandle;
use serde_json::json;

use crate::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, PendingRenderFeatures, RenderFeature,
    RenderPhase,
};
use crate::frame::{ExposureState, GpuPointLight, ViewData, build_view, frame_uniform};
use crate::gpu::{self, SurfaceState};
use crate::settings::{RenderSettings, Tonemapper};
use crate::shadows::{MAX_CASCADES, ShadowViewUniform};
use crate::targets::{Targets, Tex};

const ENV_WIDTH: u32 = 256;
const ENV_HEIGHT: u32 = 128;

/// Set when the renderer could not start (no GPU); rendering is skipped.
#[derive(Resource, Debug, Clone)]
pub struct RenderDisabled(pub String);

/// A screenshot that was written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// Queue screenshots of the final image: `world.resource_mut::<Screenshots>().request("shot.png")`.
/// The PNG is written at the end of the next rendered frame.
#[derive(Resource, Default)]
pub struct Screenshots {
    pending: Vec<PathBuf>,
    /// Results of the most recent captures (newest last, at most 16 kept).
    pub done: Vec<Result<ScreenshotInfo, String>>,
    /// Captures finished since startup (keeps counting when `done` is trimmed).
    pub completed: u64,
}

impl Screenshots {
    pub fn request(&mut self, path: impl Into<PathBuf>) {
        self.pending.push(path.into());
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Per-frame renderer statistics (also mirrored into `Diagnostics` as `render.*`).
#[derive(Resource, Debug, Default, Clone, Serialize, Deserialize)]
pub struct RenderStats {
    pub cpu_ms: f32,
    pub draw_calls: u32,
    pub instances: u32,
    pub triangles: u64,
    pub culled: u32,
    pub point_lights: u32,
    pub ev100: f32,
    pub width: u32,
    pub height: u32,
    pub adapter: String,
    pub backend: String,
    pub features: Vec<String>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PostUniform {
    params: [f32; 4],
    params2: [f32; 4],
    params3: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BloomUniform {
    params: [f32; 4],
}

struct ShadowResources {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
    view_buffers: Vec<wgpu::Buffer>,
    view_bind_groups: Vec<wgpu::BindGroup>,
    compare_sampler: wgpu::Sampler,
    size: u32,
}

#[allow(dead_code)] // layouts and buffers are kept alive with their bind groups
struct PostResources {
    tonemap_pipeline: wgpu::RenderPipeline,
    tonemap_layout: wgpu::BindGroupLayout,
    tonemap_bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    bloom_layout: wgpu::BindGroupLayout,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    bloom_params: Vec<wgpu::Buffer>,
    bloom_down_groups: Vec<wgpu::BindGroup>,
    bloom_up_groups: Vec<wgpu::BindGroup>,
    blit_layout: wgpu::BindGroupLayout,
    blit_pipeline: Option<wgpu::RenderPipeline>,
    blit_bind_group: wgpu::BindGroup,
}

/// The renderer. Lives in the world as a resource once the GPU is ready.
#[derive(Resource)]
pub struct Renderer {
    #[allow(dead_code)]
    instance: wgpu::Instance,
    #[allow(dead_code)]
    adapter: wgpu::Adapter,
    pub shared: GpuShared,
    surface: Option<SurfaceState>,
    pub targets: Targets,
    features: Vec<Box<dyn RenderFeature>>,
    frame_buffer: wgpu::Buffer,
    lights: GrowBuffer,
    shadow: ShadowResources,
    env: [Tex; 2],
    env_current: usize,
    env_rendered: bool,
    frame_groups: [wgpu::BindGroup; 2],
    post: PostResources,
    exposure: ExposureState,
    meter: crate::exposure::ExposureMeter,
    prev_view_proj: Option<Mat4>,
    frame_index: u64,
    last_view: Option<ViewData>,
}

impl Renderer {
    /// Create the renderer for a window (or headless when `window` is `None`).
    pub fn new(
        window: Option<RawWindowHandle>,
        width: u32,
        height: u32,
        settings: &RenderSettings,
    ) -> Result<Self, String> {
        let (gpu, surface) = gpu::create(window.map(|w| w.0), width, height, settings.vsync)?;
        let device = gpu.device.clone();
        let formats = crate::features::Formats {
            hdr: wgpu::TextureFormat::Rgba16Float,
            depth: wgpu::TextureFormat::Depth32Float,
            ldr: wgpu::TextureFormat::Rgba8UnormSrgb,
            shadow: wgpu::TextureFormat::Depth32Float,
            env: wgpu::TextureFormat::Rgba16Float,
        };

        let sampler = |label: &str, address: wgpu::AddressMode, filter: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: match filter {
                    wgpu::FilterMode::Linear => wgpu::MipmapFilterMode::Linear,
                    wgpu::FilterMode::Nearest => wgpu::MipmapFilterMode::Nearest,
                },
                ..Default::default()
            })
        };
        let linear_clamp = sampler(
            "linear clamp",
            wgpu::AddressMode::ClampToEdge,
            wgpu::FilterMode::Linear,
        );
        let linear_repeat = sampler(
            "linear repeat",
            wgpu::AddressMode::Repeat,
            wgpu::FilterMode::Linear,
        );
        let nearest_clamp = sampler(
            "nearest clamp",
            wgpu::AddressMode::ClampToEdge,
            wgpu::FilterMode::Nearest,
        );

        use wgpu::ShaderStages as S;
        let all = S::VERTEX_FRAGMENT | S::COMPUTE;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame layout"),
            entries: &[
                layout_entry(
                    0,
                    all,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(
                    1,
                    all,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(
                    2,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    3,
                    all,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
                layout_entry(
                    4,
                    all,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                layout_entry(
                    5,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    6,
                    all,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let shadow_view_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shadow view layout"),
                entries: &[layout_entry(
                    0,
                    S::VERTEX_FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                )],
            });

        let shared = GpuShared {
            device: gpu.device.clone(),
            queue: gpu.queue.clone(),
            formats,
            frame_layout,
            shadow_view_layout,
            linear_clamp,
            linear_repeat,
            nearest_clamp,
            adapter_info: gpu.info.clone(),
            features: device.features(),
        };

        let targets = Targets::new(&device, &formats, width, height);
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniform"),
            size: std::mem::size_of::<crate::frame::FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights = GrowBuffer::new(
            &device,
            "point lights",
            wgpu::BufferUsages::STORAGE,
            64 * std::mem::size_of::<GpuPointLight>() as u64,
        );
        let shadow = create_shadow_resources(&shared, settings.shadow_map_size.clamp(256, 8192));
        let env = [
            Tex::new(
                &device,
                "environment 0",
                ENV_WIDTH,
                ENV_HEIGHT,
                formats.env,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            Tex::new(
                &device,
                "environment 1",
                ENV_WIDTH,
                ENV_HEIGHT,
                formats.env,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
        ];
        let frame_groups = [
            create_frame_group(&shared, &frame_buffer, &lights, &shadow, &env[0]),
            create_frame_group(&shared, &frame_buffer, &lights, &shadow, &env[1]),
        ];
        let post = create_post(&shared, &targets, surface.as_ref().map(|s| s.view_format));
        let mut meter = crate::exposure::ExposureMeter::new(&shared);
        meter.set_source(&device, &targets.hdr.view);

        Ok(Self {
            instance: gpu.instance,
            adapter: gpu.adapter,
            shared,
            surface,
            targets,
            features: Vec::new(),
            frame_buffer,
            lights,
            shadow,
            env,
            env_current: 0,
            env_rendered: false,
            frame_groups,
            post,
            exposure: ExposureState::default(),
            meter,
            prev_view_proj: None,
            frame_index: 0,
            last_view: None,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.shared.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.shared.queue
    }

    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    /// The view used for the most recent frame.
    pub fn last_view(&self) -> Option<&ViewData> {
        self.last_view.as_ref()
    }

    pub fn feature_names(&self) -> Vec<String> {
        self.features.iter().map(|f| f.name().to_string()).collect()
    }

    pub fn add_feature(&mut self, mut feature: Box<dyn RenderFeature>) {
        feature.init(&self.shared);
        feature.resize(&self.shared, self.targets.width, self.targets.height);
        self.features.push(feature);
        self.features.sort_by_key(|f| f.order());
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if let Some(s) = &mut self.surface {
            s.resize(&self.shared.device, width, height);
        }
        self.targets = Targets::new(&self.shared.device, &self.shared.formats, width, height);
        self.post = create_post(
            &self.shared,
            &self.targets,
            self.surface.as_ref().map(|s| s.view_format),
        );
        self.meter
            .set_source(&self.shared.device, &self.targets.hdr.view);
        for f in &mut self.features {
            f.resize(&self.shared, width, height);
        }
    }

    /// Render one frame from the world's current state.
    pub fn render(&mut self, world: &mut World) {
        let started = Instant::now();

        // Size & vsync.
        let (want_w, want_h) = world
            .get_resource::<Window>()
            .map(|w| (w.width.max(1), w.height.max(1)))
            .unwrap_or((self.targets.width, self.targets.height));
        if (want_w, want_h) != (self.targets.width, self.targets.height) {
            self.resize(want_w, want_h);
        }
        let settings = world.resource::<RenderSettings>().clone();
        if let Some(s) = &mut self.surface {
            s.set_vsync(&self.shared.device, settings.vsync);
        }

        // New features.
        if let Some(mut pending) = world.get_resource_mut::<PendingRenderFeatures>()
            && !pending.0.is_empty()
        {
            let new: Vec<_> = pending.0.drain(..).collect();
            for f in new {
                self.add_feature(f);
            }
        }

        // Metered exposure from earlier frames (non-blocking readback).
        self.meter.collect(&self.shared.device);
        self.exposure.measured_ev100 = self.meter.measured_ev100;

        // View & uniforms.
        let view = build_view(
            world,
            self.targets.width,
            self.targets.height,
            self.prev_view_proj,
            &mut self.exposure,
            self.frame_index,
        );
        let mut uniform = frame_uniform(world, &view);
        uniform.shadow_params[3] = self.shadow.size as f32;
        let queue = &self.shared.queue;
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        let light_bytes: &[u8] = if view.point_lights.is_empty() {
            &[0u8; 32]
        } else {
            bytemuck::cast_slice(&view.point_lights)
        };
        if self.lights.write(&self.shared.device, queue, light_bytes) {
            self.frame_groups = [
                create_frame_group(
                    &self.shared,
                    &self.frame_buffer,
                    &self.lights,
                    &self.shadow,
                    &self.env[0],
                ),
                create_frame_group(
                    &self.shared,
                    &self.frame_buffer,
                    &self.lights,
                    &self.shadow,
                    &self.env[1],
                ),
            ];
        }
        if view.sun_shadows {
            for i in 0..view.cascades.count as usize {
                let u = ShadowViewUniform {
                    view_proj: view.cascades.view_proj[i].to_cols_array_2d(),
                    wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                    time: [view.time, view.real_time, view.delta, 0.0],
                    camera_frac: uniform.camera_world_frac,
                };
                queue.write_buffer(&self.shadow.view_buffers[i], 0, bytemuck::bytes_of(&u));
            }
        }
        self.write_post_params(&settings, &view);

        // Prepare features.
        for f in &mut self.features {
            f.prepare(world, &self.shared, &view, &settings);
        }

        let device = &self.shared.device;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        let ctx = FrameContext {
            gpu: &self.shared,
            view: &view,
            settings: &settings,
            scene_color_copy: &self.targets.color_copy.view,
            scene_depth_copy: &self.targets.depth_copy.view,
            ldr: &self.targets.ldr.view,
        };

        for f in &mut self.features {
            f.compute(&mut encoder, &ctx);
        }

        // Environment map (double-buffered: draw into one while the other is bound).
        let interval = settings.environment_interval.max(1) as u64;
        if !self.env_rendered || self.frame_index.is_multiple_of(interval) {
            let target = 1 - self.env_current;
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("environment"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.env[target].view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.frame_groups[self.env_current], &[]);
                for f in &self.features {
                    f.draw_environment(&mut pass, &ctx);
                }
            }
            self.env_current = target;
            self.env_rendered = true;
        }
        let frame_group = &self.frame_groups[self.env_current];

        // Shadows.
        if view.sun_shadows {
            for i in 0..(view.cascades.count as usize).min(MAX_CASCADES) {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow cascade"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadow.layer_views[i],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.shadow.view_bind_groups[i], &[]);
                for f in &self.features {
                    f.draw_shadow(&mut pass, i, &ctx);
                }
            }
        }

        // Scene: opaque + sky.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.hdr.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, frame_group, &[]);
            for phase in [RenderPhase::Opaque, RenderPhase::Sky] {
                for f in &self.features {
                    f.draw(phase, &mut pass, &ctx);
                    pass.set_bind_group(0, frame_group, &[]);
                }
            }
        }

        // Copies for refraction / soft particles.
        if self.features.iter().any(|f| f.needs_scene_copy()) {
            let size = wgpu::Extent3d {
                width: self.targets.width,
                height: self.targets.height,
                depth_or_array_layers: 1,
            };
            encoder.copy_texture_to_texture(
                self.targets.hdr.texture.as_image_copy(),
                self.targets.color_copy.texture.as_image_copy(),
                size,
            );
            encoder.copy_texture_to_texture(
                self.targets.depth.texture.as_image_copy(),
                self.targets.depth_copy.texture.as_image_copy(),
                size,
            );
        }

        // Transparent.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transparent"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.hdr.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, frame_group, &[]);
            for f in &self.features {
                f.draw(RenderPhase::Transparent, &mut pass, &ctx);
                pass.set_bind_group(0, frame_group, &[]);
            }
        }

        // Exposure metering of the finished HDR image.
        self.meter.encode(
            &self.shared.queue,
            &mut encoder,
            self.targets.width,
            self.targets.height,
            view.exposure,
        );

        // Bloom.
        let levels = self.targets.bloom.len();
        for i in 0..levels {
            let mut pass = fullscreen_pass(
                &mut encoder,
                "bloom down",
                &self.targets.bloom[i].view,
                true,
            );
            pass.set_pipeline(&self.post.bloom_down);
            pass.set_bind_group(0, &self.post.bloom_down_groups[i], &[]);
            pass.draw(0..3, 0..1);
        }
        for i in (1..levels).rev() {
            let mut pass = fullscreen_pass(
                &mut encoder,
                "bloom up",
                &self.targets.bloom[i - 1].view,
                false,
            );
            pass.set_pipeline(&self.post.bloom_up);
            pass.set_bind_group(0, &self.post.bloom_up_groups[i], &[]);
            pass.draw(0..3, 0..1);
        }

        // Tonemap.
        {
            let mut pass = fullscreen_pass(&mut encoder, "tonemap", &self.targets.ldr.view, true);
            pass.set_pipeline(&self.post.tonemap_pipeline);
            pass.set_bind_group(0, &self.post.tonemap_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Overlays (UI).
        for f in &mut self.features {
            f.overlay(&mut encoder, &ctx);
        }

        // Screenshot readback.
        let pending: Vec<PathBuf> = world
            .get_resource_mut::<Screenshots>()
            .map(|mut s| std::mem::take(&mut s.pending))
            .unwrap_or_default();
        let readback = if pending.is_empty() {
            None
        } else {
            Some(copy_to_readback(device, &mut encoder, &self.targets.ldr))
        };

        // Present.
        let mut surface_texture = None;
        let mut reconfigure = false;
        if let Some(surface) = &self.surface {
            let acquired = match surface.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) => Some(t),
                wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                    reconfigure = true;
                    Some(t)
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    reconfigure = true;
                    None
                }
                _ => None,
            };
            if let (Some(t), Some(pipeline)) = (acquired, &self.post.blit_pipeline) {
                let view = t.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(surface.view_format),
                    ..Default::default()
                });
                {
                    let mut pass = fullscreen_pass(&mut encoder, "blit", &view, true);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &self.post.blit_bind_group, &[]);
                    pass.draw(0..3, 0..1);
                }
                surface_texture = Some(t);
            }
        }

        let stats = self.collect_stats(&view, started);
        self.shared.queue.submit([encoder.finish()]);
        self.meter.after_submit();
        if let Some(t) = surface_texture {
            self.shared.queue.present(t);
        }
        if reconfigure && let Some(s) = &mut self.surface {
            let (w, h) = (s.config.width, s.config.height);
            s.resize(&self.shared.device, w, h);
        }

        if let Some((buffer, padded_row)) = readback {
            let results = save_readback(
                &self.shared.device,
                &buffer,
                padded_row,
                self.targets.width,
                self.targets.height,
                &pending,
            );
            if let Some(mut s) = world.get_resource_mut::<Screenshots>() {
                s.completed += results.len() as u64;
                s.done.extend(results);
                let excess = s.done.len().saturating_sub(16);
                s.done.drain(..excess);
            }
        }

        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("render.cpu_ms", stats.cpu_ms as f64);
            d.set("render.draw_calls", stats.draw_calls as f64);
            d.set("render.instances", stats.instances as f64);
            d.set("render.triangles", stats.triangles as f64);
            d.set("render.ev100", stats.ev100 as f64);
            if let Some(ev) = self.meter.measured_ev100 {
                d.set("render.ev100_metered", ev as f64);
            }
        }
        world.insert_resource(stats);

        self.prev_view_proj = Some(view.view_proj);
        self.last_view = Some(view);
        self.frame_index += 1;
    }

    fn write_post_params(&self, settings: &RenderSettings, view: &ViewData) {
        let tonemapper = match settings.tonemapper {
            Tonemapper::AgX => 0.0,
            Tonemapper::Aces => 1.0,
            Tonemapper::Reinhard => 2.0,
            Tonemapper::None => 3.0,
        };
        let levels = self.targets.bloom.len().max(1) as f32;
        let u = PostUniform {
            params: [
                tonemapper,
                settings.saturation,
                settings.contrast,
                settings.vignette,
            ],
            params2: [
                settings.bloom_intensity.clamp(0.0, 1.0),
                settings.film_grain,
                view.real_time,
                1.0 / levels,
            ],
            params3: [
                0.0,
                settings.white_balance,
                view.exposure,
                settings.night_desaturation,
            ],
        };
        self.shared
            .queue
            .write_buffer(&self.post.params, 0, bytemuck::bytes_of(&u));
    }

    fn collect_stats(&self, view: &ViewData, started: Instant) -> RenderStats {
        let mut total = FeatureStats::default();
        for f in &self.features {
            let s = f.stats();
            total.draw_calls += s.draw_calls;
            total.instances += s.instances;
            total.triangles += s.triangles;
            total.culled += s.culled;
        }
        RenderStats {
            cpu_ms: started.elapsed().as_secs_f32() * 1000.0,
            draw_calls: total.draw_calls,
            instances: total.instances,
            triangles: total.triangles,
            culled: total.culled,
            point_lights: view.point_lights.len() as u32,
            ev100: view.ev100,
            width: self.targets.width,
            height: self.targets.height,
            adapter: self.shared.adapter_info.name.clone(),
            backend: format!("{:?}", self.shared.adapter_info.backend),
            features: self.feature_names(),
        }
    }

    /// JSON stats including each feature's description.
    pub fn describe(&self) -> serde_json::Value {
        let features: Vec<_> = self
            .features
            .iter()
            .map(|f| {
                let s = f.stats();
                json!({
                    "name": f.name(),
                    "draw_calls": s.draw_calls,
                    "instances": s.instances,
                    "triangles": s.triangles,
                    "culled": s.culled,
                    "state": f.describe(),
                })
            })
            .collect();
        json!({
            "adapter": self.shared.adapter_info.name,
            "backend": format!("{:?}", self.shared.adapter_info.backend),
            "size": [self.targets.width, self.targets.height],
            "windowed": self.surface.is_some(),
            "frame": self.frame_index,
            "ev100": self.last_view.as_ref().map(|v| v.ev100),
            "features": features,
        })
    }
}

fn layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn fullscreen_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    clear: bool,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

fn create_shadow_resources(shared: &GpuShared, size: u32) -> ShadowResources {
    let device = &shared.device;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shadow cascades"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: MAX_CASCADES as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: shared.formats.shadow,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("shadow array"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let layer_views = (0..MAX_CASCADES as u32)
        .map(|i| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("shadow layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: i,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let view_buffers: Vec<wgpu::Buffer> = (0..MAX_CASCADES)
        .map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow view"),
                size: std::mem::size_of::<ShadowViewUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
        .collect();
    let view_bind_groups = view_buffers
        .iter()
        .map(|b| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shadow view"),
                layout: &shared.shadow_view_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: b.as_entire_binding(),
                }],
            })
        })
        .collect();
    let compare_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow compare"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    ShadowResources {
        texture,
        array_view,
        layer_views,
        view_buffers,
        view_bind_groups,
        compare_sampler,
        size,
    }
}

fn create_frame_group(
    shared: &GpuShared,
    frame: &wgpu::Buffer,
    lights: &GrowBuffer,
    shadow: &ShadowResources,
    env: &Tex,
) -> wgpu::BindGroup {
    shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame"),
        layout: &shared.frame_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&shadow.array_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&shadow.compare_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&env.view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&shared.linear_repeat),
            },
        ],
    })
}

#[allow(clippy::too_many_arguments)]
fn fullscreen_pipeline(
    shared: &GpuShared,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = shared
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
    shared
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
}

fn texture_sampler_layout(
    device: &wgpu::Device,
    label: &str,
    with_uniform: bool,
    with_second_texture: bool,
) -> wgpu::BindGroupLayout {
    use wgpu::ShaderStages as S;
    let mut entries = vec![
        layout_entry(
            0,
            S::FRAGMENT,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ),
        layout_entry(
            1,
            S::FRAGMENT,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        ),
    ];
    if with_uniform {
        entries.push(layout_entry(
            2,
            S::FRAGMENT,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        ));
    }
    if with_second_texture {
        entries.push(layout_entry(
            3,
            S::FRAGMENT,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

fn create_post(
    shared: &GpuShared,
    targets: &Targets,
    surface_format: Option<wgpu::TextureFormat>,
) -> PostResources {
    let device = &shared.device;
    let tonemap_layout = texture_sampler_layout(device, "tonemap layout", true, true);
    let bloom_layout = texture_sampler_layout(device, "bloom layout", true, false);
    let blit_layout = texture_sampler_layout(device, "blit layout", false, false);

    let post_module = shared.shader("post", include_str!("shaders/post.wgsl"), false);
    let bloom_module = shared.shader("bloom", include_str!("shaders/bloom.wgsl"), false);
    let blit_module = shared.shader("blit", include_str!("shaders/blit.wgsl"), false);

    let tonemap_pipeline = fullscreen_pipeline(
        shared,
        "tonemap",
        &post_module,
        "vs_fullscreen",
        "fs_tonemap",
        &tonemap_layout,
        shared.formats.ldr,
        None,
    );
    let bloom_down = fullscreen_pipeline(
        shared,
        "bloom down",
        &bloom_module,
        "vs_bloom",
        "fs_down",
        &bloom_layout,
        shared.formats.hdr,
        None,
    );
    let additive = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::REPLACE,
    };
    let bloom_up = fullscreen_pipeline(
        shared,
        "bloom up",
        &bloom_module,
        "vs_bloom",
        "fs_up",
        &bloom_layout,
        shared.formats.hdr,
        Some(additive),
    );
    let blit_pipeline = surface_format.map(|f| {
        fullscreen_pipeline(
            shared,
            "blit",
            &blit_module,
            "vs_blit",
            "fs_blit",
            &blit_layout,
            f,
            None,
        )
    });

    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("post params"),
        size: std::mem::size_of::<PostUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let levels = targets.bloom.len();
    let mut bloom_params = Vec::new();
    let mut bloom_down_groups = Vec::new();
    let mut bloom_up_groups = Vec::new();
    let make_group = |src: &wgpu::TextureView, buf: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom"),
            layout: &bloom_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buf.as_entire_binding(),
                },
            ],
        })
    };
    let uniform_buffer = |u: BloomUniform| {
        let b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bloom params"),
            size: std::mem::size_of::<BloomUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        shared.queue.write_buffer(&b, 0, bytemuck::bytes_of(&u));
        b
    };
    for i in 0..levels {
        let src = if i == 0 {
            &targets.hdr
        } else {
            &targets.bloom[i - 1]
        };
        let buf = uniform_buffer(BloomUniform {
            params: [
                1.0 / src.width as f32,
                1.0 / src.height as f32,
                if i == 0 { 1.0 } else { 0.0 },
                0.0,
            ],
        });
        bloom_down_groups.push(make_group(&src.view, &buf));
        bloom_params.push(buf);
    }
    for i in 0..levels {
        // Upsample from level i into level i-1 (entry 0 unused).
        let src = &targets.bloom[i];
        let buf = uniform_buffer(BloomUniform {
            params: [1.0 / src.width as f32, 1.0 / src.height as f32, 0.0, 1.0],
        });
        bloom_up_groups.push(make_group(&src.view, &buf));
        bloom_params.push(buf);
    }

    let tonemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tonemap"),
        layout: &tonemap_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&targets.hdr.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&targets.bloom[0].view),
            },
        ],
    });
    let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit"),
        layout: &blit_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&targets.ldr.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
            },
        ],
    });

    PostResources {
        tonemap_pipeline,
        tonemap_layout,
        tonemap_bind_group,
        params,
        bloom_layout,
        bloom_down,
        bloom_up,
        bloom_params,
        bloom_down_groups,
        bloom_up_groups,
        blit_layout,
        blit_pipeline,
        blit_bind_group,
    }
}

fn copy_to_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    tex: &Tex,
) -> (wgpu::Buffer, u32) {
    let unpadded = tex.width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: padded as u64 * tex.height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        tex.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(tex.height),
            },
        },
        wgpu::Extent3d {
            width: tex.width,
            height: tex.height,
            depth_or_array_layers: 1,
        },
    );
    (buffer, padded)
}

fn save_readback(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    padded_row: u32,
    width: u32,
    height: u32,
    paths: &[PathBuf],
) -> Vec<Result<ScreenshotInfo, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    if let Err(e) = device.poll(wgpu::PollType::wait_indefinitely()) {
        return paths
            .iter()
            .map(|_| Err(format!("GPU poll failed: {e}")))
            .collect();
    }
    match rx.recv() {
        Ok(Ok(())) => {}
        other => {
            return paths
                .iter()
                .map(|_| Err(format!("screenshot readback failed: {other:?}")))
                .collect();
        }
    }
    let data = match buffer.slice(..).get_mapped_range() {
        Ok(d) => d,
        Err(e) => {
            return paths
                .iter()
                .map(|_| Err(format!("screenshot map failed: {e:?}")))
                .collect();
        }
    };
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * padded_row) as usize;
        pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
    }
    drop(data);
    buffer.unmap();
    paths
        .iter()
        .map(|p| save_png(p, &pixels, width, height))
        .collect()
}

fn save_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<ScreenshotInfo, String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    image::save_buffer_with_format(
        path,
        rgba,
        width,
        height,
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    tracing::info!("screenshot saved to {}", path.display());
    Ok(ScreenshotInfo {
        path: path.display().to_string(),
        width,
        height,
    })
}

/// Create the renderer if possible. Returns whether a renderer exists.
pub fn ensure_renderer(world: &mut World) -> bool {
    if world.contains_resource::<Renderer>() {
        return true;
    }
    if world.contains_resource::<RenderDisabled>() {
        return false;
    }
    let headless = world.resource::<EngineConfig>().headless;
    let handle = world.get_resource::<RawWindowHandle>().cloned();
    if !headless && handle.is_none() {
        // The window doesn't exist yet.
        return false;
    }
    let (width, height) = world
        .get_resource::<Window>()
        .map(|w| (w.width, w.height))
        .unwrap_or((1280, 720));
    let settings = world.resource::<RenderSettings>().clone();
    match Renderer::new(
        if headless { None } else { handle },
        width,
        height,
        &settings,
    ) {
        Ok(mut renderer) => {
            if let Some(mut pending) = world.get_resource_mut::<PendingRenderFeatures>() {
                for f in pending.0.drain(..) {
                    renderer.add_feature(f);
                }
            }
            world.insert_resource(renderer);
            true
        }
        Err(e) => {
            tracing::error!("renderer disabled: {e}");
            world.insert_resource(RenderDisabled(e));
            false
        }
    }
}

/// The exclusive system that renders a frame.
pub fn render_system(world: &mut World) {
    if !ensure_renderer(world) {
        return;
    }
    world.resource_scope(|world, mut renderer: Mut<Renderer>| renderer.render(world));
}

/// Render immediately and capture the image (used by the `render.screenshot` API so tools get
/// the picture in the same call).
pub fn capture_now(world: &mut World, path: impl Into<PathBuf>) -> Result<ScreenshotInfo, String> {
    if !ensure_renderer(world) {
        return Err(world
            .get_resource::<RenderDisabled>()
            .map(|d| d.0.clone())
            .unwrap_or_else(|| "the renderer is not running yet (no window)".into()));
    }
    world.get_resource_or_init::<Screenshots>().request(path);
    let before = world.resource::<Screenshots>().completed;
    render_system(world);
    let mut s = world.resource_mut::<Screenshots>();
    if s.completed > before {
        s.done
            .last()
            .cloned()
            .unwrap_or_else(|| Err("no screenshot taken".into()))
    } else {
        s.pending.clear();
        Err("the frame was not rendered".into())
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<Renderer>();
}
