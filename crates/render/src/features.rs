//! Extending the renderer with render features.
//!
//! A [`RenderFeature`] owns its GPU resources and hooks into fixed points of the frame:
//!
//! 1. `prepare` — read the world, cull, upload buffers (CPU side, `&mut World` access).
//! 2. `compute` — record compute or offscreen passes (particles, simulation, clouds).
//! 3. `draw_environment` — draw into the sky environment map (ambient light + reflections).
//! 4. `draw_shadow` — draw depth into one shadow cascade, or into the top-down sky occlusion
//!    map when the index is [`SKY_OCCLUSION_VIEW`](crate::shadows::SKY_OCCLUSION_VIEW).
//! 5. `draw` — draw into the HDR scene in a [`RenderPhase`] (opaque → sky → transparent).
//! 6. `overlay` — draw onto the final tonemapped image (UI, debug text, editor gizmos).
//!
//! Group 0 is bound by the renderer before `draw`/`draw_environment` (frame data, lights,
//! shadows, environment map; see `shaders/lib_frame.wgsl`) and before `draw_shadow` (the
//! cascade's [`ShadowView`](crate::shadows::ShadowViewUniform)). Features use groups 1+.
//!
//! Register a feature with [`RenderAppExt::add_render_feature`]. Water, terrain, particles,
//! volumetric clouds, foliage and UI are all built this way.

use bevy_ecs::prelude::*;
use engine_core::App;

use crate::frame::ViewData;
use crate::settings::RenderSettings;

/// Shared types and pure functions (no bindings).
pub const SHADER_LIB_TYPES: &str = include_str!("shaders/lib_types.wgsl");
/// Group 0 frame bindings, shadows, atmosphere, PBR shading and fog. Requires `SHADER_LIB_TYPES`.
pub const SHADER_LIB_FRAME: &str = include_str!("shaders/lib_frame.wgsl");

/// Which part of the scene pass a draw belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderPhase {
    /// Depth-tested, depth-writing geometry.
    Opaque,
    /// The sky, drawn where nothing opaque was drawn.
    Sky,
    /// Blended geometry and effects, drawn after the sky. Can read the scene color and depth
    /// copies when a feature asks for them with [`RenderFeature::needs_scene_copy`].
    Transparent,
}

/// Texture formats used by the renderer's targets.
#[derive(Debug, Clone, Copy)]
pub struct Formats {
    pub hdr: wgpu::TextureFormat,
    pub depth: wgpu::TextureFormat,
    pub ldr: wgpu::TextureFormat,
    pub shadow: wgpu::TextureFormat,
    pub env: wgpu::TextureFormat,
}

/// Device, queue, shared layouts and samplers. Everything a feature needs to build pipelines.
pub struct GpuShared {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub formats: Formats,
    /// Layout of group 0 in scene and environment passes.
    pub frame_layout: wgpu::BindGroupLayout,
    /// Layout of group 0 in shadow passes.
    pub shadow_view_layout: wgpu::BindGroupLayout,
    pub linear_clamp: wgpu::Sampler,
    pub linear_repeat: wgpu::Sampler,
    pub nearest_clamp: wgpu::Sampler,
    /// Adapter name, backend etc.
    pub adapter_info: wgpu::AdapterInfo,
    /// Optional features the device has (e.g. `DEPTH_CLIP_CONTROL`).
    pub features: wgpu::Features,
}

impl GpuShared {
    /// Compile WGSL composed as `lib_types` (+ `lib_frame` when `with_frame`) + `source`.
    pub fn shader(&self, label: &str, source: &str, with_frame: bool) -> wgpu::ShaderModule {
        let code = compose_shader(source, with_frame);
        self.device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(code.into()),
            })
    }

    /// Standard depth state for scene passes (reverse-Z: greater is closer).
    pub fn depth_state(&self, write: bool) -> wgpu::DepthStencilState {
        wgpu::DepthStencilState {
            format: self.formats.depth,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }
    }

    /// Depth state for shadow passes (standard Z, hardware slope bias).
    pub fn shadow_depth_state(&self) -> wgpu::DepthStencilState {
        wgpu::DepthStencilState {
            format: self.formats.shadow,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
        }
    }
}

/// Build the full WGSL source for a feature shader.
pub fn compose_shader(source: &str, with_frame: bool) -> String {
    let mut code =
        String::with_capacity(SHADER_LIB_TYPES.len() + SHADER_LIB_FRAME.len() + source.len() + 2);
    code.push_str(SHADER_LIB_TYPES);
    code.push('\n');
    if with_frame {
        code.push_str(SHADER_LIB_FRAME);
        code.push('\n');
    }
    code.push_str(source);
    code
}

/// Validate composed WGSL without a GPU (used by tests and hot-reload). Returns a readable
/// error with line context on failure.
pub fn validate_wgsl(label: &str, code: &str) -> Result<(), String> {
    use wgpu::naga;
    let module = naga::front::wgsl::parse_str(code)
        .map_err(|e| format!("{label}: {}", e.emit_to_string(code)))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("{label}: {}", e.emit_to_string(code)))?;
    Ok(())
}

/// Read-only per-frame state given to features while drawing.
pub struct FrameContext<'a> {
    pub gpu: &'a GpuShared,
    pub view: &'a ViewData,
    pub settings: &'a RenderSettings,
    /// Scene color copy (HDR, pre-exposed) taken after the sky; valid in the transparent phase.
    pub scene_color_copy: &'a wgpu::TextureView,
    /// Scene depth copy (reverse-Z) taken after the sky; valid in the transparent phase.
    pub scene_depth_copy: &'a wgpu::TextureView,
    /// The final LDR image (overlays draw here).
    pub ldr: &'a wgpu::TextureView,
}

/// Counters a feature reports for `render.stats`.
#[derive(Debug, Default, Clone, Copy)]
pub struct FeatureStats {
    pub draw_calls: u32,
    pub instances: u32,
    pub triangles: u64,
    pub culled: u32,
}

/// A pluggable piece of the renderer. See the module docs for the frame order.
#[allow(unused_variables)]
pub trait RenderFeature: Send + Sync + 'static {
    /// Unique name, shown in stats and logs.
    fn name(&self) -> &str;

    /// Draw order within a phase (lower first).
    fn order(&self) -> i32 {
        0
    }

    /// Create pipelines and persistent resources. Called once when the GPU is ready.
    fn init(&mut self, gpu: &GpuShared) {}

    /// The output size changed.
    fn resize(&mut self, gpu: &GpuShared, width: u32, height: u32) {}

    /// Gather and upload this frame's data.
    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
    }

    /// Record compute or offscreen work before the scene is drawn.
    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {}

    /// Draw into the environment map (equirectangular, radiance not pre-exposed).
    fn draw_environment(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Draw depth into shadow cascade `cascade` (cull with `ctx.view.cascades`), or into the
    /// sky occlusion map when `cascade == SKY_OCCLUSION_VIEW` (cull with
    /// `ctx.view.sky_occlusion.contains_sphere`). Anything that roofs over the ground (rock,
    /// buildings, canopies) should draw into both.
    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, ctx: &FrameContext) {}

    /// Draw into the HDR scene.
    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Whether the transparent phase needs `scene_color_copy`/`scene_depth_copy`.
    fn needs_scene_copy(&self) -> bool {
        false
    }

    /// Draw onto the tonemapped LDR image.
    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {}

    fn stats(&self) -> FeatureStats {
        FeatureStats::default()
    }

    /// Optional JSON describing the feature's state (for `render.stats`).
    fn describe(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// Features waiting for the GPU to be ready.
#[derive(Resource, Default)]
pub struct PendingRenderFeatures(pub Vec<Box<dyn RenderFeature>>);

/// Adds [`RenderAppExt::add_render_feature`] to [`App`].
pub trait RenderAppExt {
    fn add_render_feature(&mut self, feature: impl RenderFeature) -> &mut Self;
}

impl RenderAppExt for App {
    fn add_render_feature(&mut self, feature: impl RenderFeature) -> &mut Self {
        self.world
            .get_resource_or_init::<PendingRenderFeatures>()
            .0
            .push(Box::new(feature));
        self
    }
}

/// A GPU buffer that grows (to the next power of two) when data doesn't fit.
pub struct GrowBuffer {
    pub buffer: wgpu::Buffer,
    capacity: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl GrowBuffer {
    pub fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        capacity: u64,
    ) -> Self {
        let usage = usage | wgpu::BufferUsages::COPY_DST;
        let capacity = capacity.max(256).next_power_of_two();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity,
            usage,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            capacity,
            usage,
            label,
        }
    }

    /// Upload `data`; returns true when the buffer was recreated (rebuild bind groups).
    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> bool {
        let mut grown = false;
        if data.len() as u64 > self.capacity {
            *self = Self::new(device, self.label, self.usage, data.len() as u64);
            grown = true;
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buffer, 0, data);
        }
        grown
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }
}
