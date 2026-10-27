//! The frame loop.
//!
//! ```text
//! prepare (features read the world) → compute → environment map (+ filters) → shadow cascades
//! → prepass → volumetric fog → AO, reflections, bounce light → scene pass [opaque → sky]
//! → (scene copies) → transparent pass → depth of field → TAA → motion blur → bloom
//! → tonemap to LDR → overlays (UI)
//! → screenshot readback → blit to window
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::window::RawWindowHandle;
use serde_json::json;

use crate::atmosphere::AtmosphereLuts;
use crate::cloud_shadows::{
    CLOUD_SHADOW_FORMAT, CloudShadowArea, CloudShadowMap, CloudShadowTarget,
};
use crate::effects::ScreenEffects;
use crate::environment::EnvironmentMaps;
use crate::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, PendingRenderFeatures, RenderFeature,
    RenderPhase,
};
use crate::frame::{
    ExposureState, GpuPointLight, PreviousView, ViewData, build_view, frame_uniform,
};
use crate::gpu::{self, SurfaceState};
use crate::settings::{AntiAliasing, RenderSettings, Tonemapper};
use crate::shadows::{FAR_SHADOW_LAYER, MAX_CASCADES, SKY_OCCLUSION_VIEW, ShadowViewUniform};
use crate::texture::Image;

/// Index of the far shadow map's view buffer and bind group (after the sky occlusion map's).
const FAR_SHADOW_VIEW: usize = SKY_OCCLUSION_VIEW + 1;
use crate::targets::{Targets, Tex};

/// Set when the renderer could not start (no GPU); rendering is skipped. With the environment
/// variable `ENGINE_REQUIRE_GPU=1` (as in CI) the renderer panics instead, so GPU tests can't
/// pass by skipping.
#[derive(Resource, Debug, Clone)]
pub struct RenderDisabled(pub String);

/// While present, frames run the simulation only: nothing is drawn and no renderer is started,
/// whether the renderer is inline or on its own thread ([`Pipelining`]). The way to skip
/// drawing for a while (a warm-up between pictures); taking the `Renderer` resource out of the
/// world does nothing while it is pipelined. `capture_now` still renders.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct RenderPaused;

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
    /// Virtual geometry clusters drawn by the camera (counted on the GPU a few frames ago).
    #[serde(default)]
    pub clusters: u32,
    pub point_lights: u32,
    pub ev100: f32,
    pub width: u32,
    pub height: u32,
    pub adapter: String,
    pub backend: String,
    pub features: Vec<String>,
    /// Images whose CPU pixels the renderer freed after uploading them, since startup
    /// (`RenderSettings::release_image_pixels`).
    #[serde(default)]
    pub images_released: u64,
    /// CPU bytes (level 0 and stored mips) those releases freed.
    #[serde(default)]
    pub image_bytes_released: u64,
    /// Image views block-compressed on the GPU since startup
    /// (`RenderSettings::texture_compression`).
    #[serde(default)]
    pub images_compressed: u64,
    /// GPU bytes those compressions saved.
    #[serde(default)]
    pub image_bytes_compressed_away: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PostUniform {
    params: [f32; 4],
    params2: [f32; 4],
    params3: [f32; 4],
    params4: [f32; 4],
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
    /// One per cascade, then the far shadow map's (`FAR_SHADOW_LAYER`).
    layer_views: Vec<wgpu::TextureView>,
    /// One per cascade, then one for the sky occlusion map (index `SKY_OCCLUSION_VIEW`) and one
    /// for the far shadow map (`FAR_SHADOW_VIEW`).
    view_buffers: Vec<wgpu::Buffer>,
    view_bind_groups: Vec<wgpu::BindGroup>,
    compare_sampler: wgpu::Sampler,
    size: u32,
    /// Top-down depth map of the highest surface around the camera (see `SkyOcclusionView`).
    occlusion: wgpu::TextureView,
    occlusion_size: u32,
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

/// The renderer. Lives in the world as a resource once the GPU is ready, except while it is
/// pipelined ([`RenderThread`]): reach it with [`with_renderer`], which works either way.
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
    /// Frame uniform as rasterized (TAA-jittered).
    frame_buffer: wgpu::Buffer,
    /// The same without the jitter, for overlays drawn on the final image.
    overlay_frame_buffer: wgpu::Buffer,
    lights: GrowBuffer,
    shadow: ShadowResources,
    sky_camera: crate::sky_occlusion::SkyCamera,
    /// `SurfaceLayers` around the camera for `surface_weather` (frame binding 21).
    surface_weather: crate::surface_weather::SurfaceWeatherMap,
    /// The cloud shadow map (frame binding 22) and its stand-in for compute passes.
    cloud_shadows: CloudShadowMap,
    /// Whether the stand-in has been cleared to 1 yet (textures start at 0).
    cloud_stand_in_ready: bool,
    environment: EnvironmentMaps,
    env_rendered: bool,
    /// 1x1 neutral stand-ins for the screen-space maps (no occlusion, nothing in the prepass),
    /// bound while the real ones are being drawn.
    screen_stand_ins: ScreenMaps,
    /// Scene passes.
    frame_group: wgpu::BindGroup,
    /// Features' `compute`: the scene group with a stand-in for the cloud shadow map, which a
    /// feature may be drawing.
    compute_group: wgpu::BindGroup,
    /// The prepass and the effects that write the screen-space maps.
    prepass_group: wgpu::BindGroup,
    /// Overlays: unjittered matrices.
    overlay_group: wgpu::BindGroup,
    post: PostResources,
    effects: ScreenEffects,
    atmosphere: AtmosphereLuts,
    exposure: ExposureState,
    meter: crate::exposure::ExposureMeter,
    volumetrics: crate::volumetrics::Volumetrics,
    /// Depth of field (before TAA) and motion blur (after it): camera components.
    lens: crate::lens::LensEffects,
    /// Skin's subsurface scattering (made the first time a feature asks for it).
    subsurface: Option<crate::subsurface::SubsurfacePass>,
    /// Frame timing and the `perf:` log line.
    perf: crate::perf::PerfLog,
    /// `render.record_cpu_ms`, averaged.
    thread_cpu: crate::perf::ThreadCpu,
    /// Which far shadow cascades keep last frame's image.
    cascade_cache: crate::shadows::CascadeCache,
    previous: Option<PreviousView>,
    /// How far the camera moved the frame before (camera cut detection).
    prev_step: f64,
    /// Anti-aliasing mode of the previous frame (a switch resets the history).
    prev_aa: AntiAliasing,
    /// Pre-exposure the previous frame's image was rendered with (screen-space reflections
    /// read it).
    prev_exposure: f32,
    frame_index: u64,
    last_view: Option<ViewData>,
    /// The last recorded frame's results, until `apply_return` hands them to the world.
    returned: Option<RenderReturn>,
    /// Each `ExtraView` camera's own resources and history, by camera.
    extra_views: HashMap<Entity, ExtraViewState>,
    /// Main-thread time this frame in `queue.submit` (extra views submit there), for
    /// `render.extract_wait_ms`.
    main_thread_submit_ms: f64,
}

/// Declares [`ViewState`] with the [`Renderer`] fields that belong to one view, and
/// [`Renderer::swap_view`] to swap them. A new field that differs per view goes here too.
macro_rules! per_view {
    ($($name:ident: $ty:ty,)*) => {
        /// One view's resources and history (Unreal's `FSceneViewState`): its targets, frame
        /// uniforms and bind groups, shadow maps, environment, fog, exposure and the previous
        /// frame's view. The renderer's own fields hold the main view's; an extra view's are
        /// swapped in while it renders.
        struct ViewState {
            $($name: $ty,)*
        }

        impl Renderer {
            fn swap_view(&mut self, view: &mut ViewState) {
                $(std::mem::swap(&mut self.$name, &mut view.$name);)*
            }
        }
    };
}

per_view! {
    targets: Targets,
    frame_buffer: wgpu::Buffer,
    overlay_frame_buffer: wgpu::Buffer,
    lights: GrowBuffer,
    shadow: ShadowResources,
    sky_camera: crate::sky_occlusion::SkyCamera,
    surface_weather: crate::surface_weather::SurfaceWeatherMap,
    cloud_shadows: CloudShadowMap,
    cloud_stand_in_ready: bool,
    environment: EnvironmentMaps,
    env_rendered: bool,
    screen_stand_ins: ScreenMaps,
    frame_group: wgpu::BindGroup,
    compute_group: wgpu::BindGroup,
    prepass_group: wgpu::BindGroup,
    overlay_group: wgpu::BindGroup,
    post: PostResources,
    effects: ScreenEffects,
    atmosphere: AtmosphereLuts,
    exposure: ExposureState,
    meter: crate::exposure::ExposureMeter,
    volumetrics: crate::volumetrics::Volumetrics,
    lens: crate::lens::LensEffects,
    subsurface: Option<crate::subsurface::SubsurfacePass>,
    perf: crate::perf::PerfLog,
    cascade_cache: crate::shadows::CascadeCache,
    previous: Option<PreviousView>,
    prev_step: f64,
    prev_aa: AntiAliasing,
    prev_exposure: f32,
    last_view: Option<ViewData>,
}

impl ViewState {
    /// A view of `width` × `height` pixels (`surface`: the window's format, for the main view).
    fn new(
        shared: &GpuShared,
        settings: &RenderSettings,
        width: u32,
        height: u32,
        surface: Option<wgpu::TextureFormat>,
    ) -> Self {
        let device = &shared.device;
        let targets = Targets::new(
            device,
            &shared.formats,
            width,
            height,
            UVec2::new(width, height),
        );
        let frame_uniform_buffer = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<crate::frame::FrameUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_buffer = frame_uniform_buffer("frame uniform");
        let overlay_frame_buffer = frame_uniform_buffer("overlay frame uniform");
        let lights = GrowBuffer::new(
            device,
            "point lights",
            wgpu::BufferUsages::STORAGE,
            64 * std::mem::size_of::<GpuPointLight>() as u64,
        );
        let shadow = create_shadow_resources(
            shared,
            settings.shadow_map_size.clamp(256, 8192),
            settings.sky_occlusion_size.clamp(64, 4096),
        );
        let environment = EnvironmentMaps::new(shared);
        let mut sky_camera = crate::sky_occlusion::SkyCamera::new(shared);
        sky_camera.set_inputs(shared, &frame_buffer, &shadow.occlusion);
        let volumetrics = crate::volumetrics::Volumetrics::new(shared, &targets);
        let screen_stand_ins = ScreenMaps::stand_ins(shared, &volumetrics);
        let atmosphere = AtmosphereLuts::new(shared, &frame_buffer);
        let surface_weather = crate::surface_weather::SurfaceWeatherMap::new(shared);
        let cloud_shadows = CloudShadowMap::new(device);
        let groups = FrameGroups::new(
            shared,
            [&frame_buffer, &overlay_frame_buffer],
            &lights,
            &shadow,
            &environment,
            &sky_camera,
            &atmosphere,
            &surface_weather.view,
            &ScreenMaps::of(&targets, &volumetrics),
            &screen_stand_ins,
            &cloud_shadows,
        );
        let post = create_post(shared, &targets, surface);
        let effects = ScreenEffects::new(shared, &targets);
        let mut meter = crate::exposure::ExposureMeter::new(shared);
        meter.set_source(device, &targets.hdr.view);
        let perf = crate::perf::PerfLog::new(device, &shared.queue);
        Self {
            targets,
            frame_buffer,
            overlay_frame_buffer,
            lights,
            shadow,
            sky_camera,
            surface_weather,
            cloud_shadows,
            cloud_stand_in_ready: false,
            environment,
            env_rendered: false,
            screen_stand_ins,
            frame_group: groups.scene,
            compute_group: groups.compute,
            prepass_group: groups.prepass,
            overlay_group: groups.overlay,
            post,
            effects,
            atmosphere,
            exposure: ExposureState::default(),
            meter,
            volumetrics,
            lens: Default::default(),
            subsurface: None,
            perf,
            cascade_cache: Default::default(),
            previous: None,
            prev_step: 0.0,
            prev_aa: settings.anti_aliasing,
            prev_exposure: 0.0,
            last_view: None,
        }
    }
}

/// An [`crate::ExtraView`]'s state between frames.
struct ExtraViewState {
    view: ViewState,
    /// The image it renders into (canonical handle index) and the texture registered for it.
    image: Option<(u32, wgpu::Texture)>,
}

/// An extra view's picture, copied into the main view's output at `origin` (`ViewTarget::Window`).
struct Composite {
    texture: wgpu::Texture,
    origin: UVec2,
    size: UVec2,
}

/// What an extra view's frame is: its camera's picture size and settings.
struct ExtraJob {
    size: UVec2,
    settings: RenderSettings,
}

/// What [`Renderer::extract_frame`] takes from the world for [`Renderer::record_submit`]: the
/// frame's view, settings and uniforms (features keep their own extracted state).
struct FrameWork {
    started: Instant,
    cpu: crate::perf::ParkedSteps,
    extract_ms: f32,
    settings: RenderSettings,
    view: ViewData,
    uniform: crate::frame::FrameUniform,
    taa: bool,
    ssr: bool,
    gi: bool,
    lens_settings: (Option<crate::DepthOfField>, Option<crate::MotionBlur>),
    real_dt: f32,
    scene_origin: UVec2,
    viewport_size: UVec2,
    cloud_writer: Option<(usize, crate::cloud_shadows::CloudShadowRequest)>,
    cloud_area: Option<CloudShadowArea>,
    screenshots: Vec<PathBuf>,
    /// An extra view's frame: no window, screenshots or report of its own.
    extra: bool,
    /// Extra views' pictures to copy into this view's output, over the scene and under the
    /// overlays.
    composites: Vec<Composite>,
}

/// What a recorded frame hands back to the world, applied by [`Renderer::apply_return`] on the
/// main thread (a frame later when pipelined, Bevy's and Unreal's one-frame lag).
#[derive(Default)]
struct RenderReturn {
    stats: RenderStats,
    /// `render.*` metrics, merged into the world's `Diagnostics`.
    diagnostics: Diagnostics,
    screenshots: Vec<Result<ScreenshotInfo, String>>,
}

/// Views of the maps computed each frame that the frame group binds (bindings 10–12 and
/// 18–20): the screen-space maps and the volumetric fog.
struct ScreenMaps {
    ao: wgpu::TextureView,
    normal: wgpu::TextureView,
    depth: wgpu::TextureView,
    ssr: wgpu::TextureView,
    fog: wgpu::TextureView,
    gi: wgpu::TextureView,
}

impl ScreenMaps {
    fn of(targets: &Targets, volumetrics: &crate::volumetrics::Volumetrics) -> Self {
        Self {
            ao: targets.ao.view.clone(),
            normal: targets.prepass_normal.view.clone(),
            depth: targets.prepass_depth.view.clone(),
            ssr: targets.ssr.view.clone(),
            fog: volumetrics.view().clone(),
            gi: targets.gi.view.clone(),
        }
    }

    fn stand_ins(shared: &GpuShared, volumetrics: &crate::volumetrics::Volumetrics) -> Self {
        let device = &shared.device;
        let tex = |label: &str, format: wgpu::TextureFormat| {
            Tex::new(
                device,
                label,
                1,
                1,
                format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            )
        };
        let ao = tex("no ambient occlusion", shared.formats.ao);
        shared.queue.write_texture(
            ao.texture.as_image_copy(),
            &[255u8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        // New textures start zeroed: no normal written, depth 0 (nothing), no reflections, no
        // bounce light.
        let normal = tex("no prepass normals", shared.formats.normal);
        let depth = tex("no prepass depth", shared.formats.depth);
        let ssr = tex("no screen-space reflections", shared.formats.hdr);
        let gi = tex("no screen-space gi", shared.formats.hdr);
        Self {
            ao: ao.view,
            normal: normal.view,
            depth: depth.view,
            ssr: ssr.view,
            fog: volumetrics.stand_in().clone(),
            gi: gi.view,
        }
    }
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
            normal: wgpu::TextureFormat::Rgba16Float,
            motion: wgpu::TextureFormat::Rg16Float,
            ao: wgpu::TextureFormat::R8Unorm,
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
        let float_texture = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
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
                layout_entry(
                    7,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    8,
                    all,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                ),
                layout_entry(
                    9,
                    all,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(
                    10,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    11,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    12,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                // Atmosphere tables: transmittance, multiple scattering, sky view.
                layout_entry(13, all, float_texture),
                layout_entry(14, all, float_texture),
                layout_entry(15, all, float_texture),
                // Sky light: irradiance and the environment sampler (binding 5 is the
                // prefiltered environment map).
                layout_entry(16, all, float_texture),
                layout_entry(
                    17,
                    all,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                // Screen-space reflections (a screen-space map like 10-12).
                layout_entry(18, all, float_texture),
                // Volumetric fog: in-scattered light and transmittance per froxel slice.
                layout_entry(
                    19,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                ),
                // Screen-space global illumination.
                layout_entry(20, all, float_texture),
                // The ground's weather around the camera (`surface_weather`).
                layout_entry(21, all, float_texture),
                // Cloud shadows (cloud_shadows.rs). The 15th sampled texture in group 0.
                layout_entry(22, all, float_texture),
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
            materials: Default::default(),
            images: Default::default(),
            uploads: Default::default(),
            side_work: Default::default(),
            side_commands: Default::default(),
        };

        let view = ViewState::new(
            &shared,
            settings,
            width,
            height,
            surface.as_ref().map(|s| s.view_format),
        );

        Ok(Self {
            instance: gpu.instance,
            adapter: gpu.adapter,
            shared,
            surface,
            targets: view.targets,
            features: Vec::new(),
            frame_buffer: view.frame_buffer,
            overlay_frame_buffer: view.overlay_frame_buffer,
            lights: view.lights,
            shadow: view.shadow,
            sky_camera: view.sky_camera,
            surface_weather: view.surface_weather,
            cloud_shadows: view.cloud_shadows,
            cloud_stand_in_ready: view.cloud_stand_in_ready,
            environment: view.environment,
            env_rendered: view.env_rendered,
            screen_stand_ins: view.screen_stand_ins,
            atmosphere: view.atmosphere,
            frame_group: view.frame_group,
            compute_group: view.compute_group,
            prepass_group: view.prepass_group,
            overlay_group: view.overlay_group,
            post: view.post,
            effects: view.effects,
            exposure: view.exposure,
            meter: view.meter,
            volumetrics: view.volumetrics,
            lens: view.lens,
            subsurface: view.subsurface,
            perf: view.perf,
            thread_cpu: Default::default(),
            cascade_cache: view.cascade_cache,
            previous: view.previous,
            prev_step: view.prev_step,
            prev_aa: view.prev_aa,
            prev_exposure: view.prev_exposure,
            frame_index: 0,
            last_view: view.last_view,
            returned: None,
            extra_views: HashMap::new(),
            main_thread_submit_ms: 0.0,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.shared.device
    }

    /// Where the GPU allocator's memory goes, as the `--profile` log shows it (blocks by fill,
    /// the most free space and its tenants, the largest users by label); `None` where the
    /// backend doesn't report its allocations.
    pub fn gpu_memory_table(&self) -> Option<String> {
        let report = self.shared.device.generate_allocator_report()?;
        Some(crate::perf::memory_table(&report))
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

    /// Size the targets for a scene of `scene` pixels in a window of `window` pixels (`extra`:
    /// an extra view's, `window` its picture's size; the window and the features keep theirs).
    fn resize(&mut self, scene: UVec2, window: UVec2, extra: bool) {
        let (width, height) = (scene.x.max(1), scene.y.max(1));
        if !extra
            && let Some(s) = &mut self.surface
            && window != self.targets.window
        {
            s.resize(&self.shared.device, window.x.max(1), window.y.max(1));
        }
        self.targets = Targets::new(
            &self.shared.device,
            &self.shared.formats,
            width,
            height,
            window,
        );
        self.post = create_post(
            &self.shared,
            &self.targets,
            self.surface
                .as_ref()
                .filter(|_| !extra)
                .map(|s| s.view_format),
        );
        self.effects.resize(&self.shared, &self.targets);
        self.lens.resize();
        if let Some(s) = &mut self.subsurface {
            s.resize();
        }
        self.volumetrics.resize(&self.shared, &self.targets);
        self.rebuild_frame_groups();
        self.previous = None;
        self.meter
            .set_source(&self.shared.device, &self.targets.hdr.view);
        if !extra {
            for f in &mut self.features {
                f.resize(&self.shared, width, height);
            }
        }
    }

    fn rebuild_frame_groups(&mut self) {
        let groups = FrameGroups::new(
            &self.shared,
            [&self.frame_buffer, &self.overlay_frame_buffer],
            &self.lights,
            &self.shadow,
            &self.environment,
            &self.sky_camera,
            &self.atmosphere,
            &self.surface_weather.view,
            &ScreenMaps::of(&self.targets, &self.volumetrics),
            &self.screen_stand_ins,
            &self.cloud_shadows,
        );
        self.frame_group = groups.scene;
        self.compute_group = groups.compute;
        self.prepass_group = groups.prepass;
        self.overlay_group = groups.overlay;
    }

    /// Render a frame inline: extract, record and submit, then hand the results to the world.
    pub fn render(&mut self, world: &mut World) {
        let frame = self.extract(world);
        self.record_submit(frame);
        self.apply_return(world);
    }

    /// Everything of a frame that reads the world: the frame's start, the extra views
    /// (recorded and submitted here, before the main view), then the main view's extraction.
    fn extract(&mut self, world: &mut World) -> FrameWork {
        let started = Instant::now();
        self.begin_frame(world, started);
        let composites = self.render_extra_views(world);
        let mut frame = self.extract_frame(world, started, None);
        frame.composites = composites;
        frame
    }

    /// What every frame does once, before its views: pipeline warm-up, the image cache's and
    /// the upload budget's frame, new features.
    fn begin_frame(&mut self, world: &mut World, started: Instant) {
        let settings = crate::settings::effective(world);
        let headless = world
            .get_resource::<EngineConfig>()
            .is_none_or(|c| c.headless);
        // Every material preset's pipelines compile in the background from the first frame.
        if settings.pipeline_warmup.on(headless) {
            self.shared.materials.warm(&self.shared);
        }
        self.shared.images.begin_frame(
            self.frame_index,
            settings.release_image_pixels,
            (
                settings.texture_compression,
                settings.texture_compression_on_cpu,
                if world.contains_resource::<engine_world::LoadingBudget>() {
                    settings.texture_compression_loading_ms
                } else {
                    settings.texture_compression_ms
                },
            ),
        );
        self.shared.images.compress(&self.shared);
        // Uploads: a byte budget in play, a time budget behind a loading screen.
        let loading = world.get_resource::<engine_world::LoadingBudget>().copied();
        let budgeted = settings.upload_budget_headless
            || !world
                .get_resource::<EngineConfig>()
                .is_some_and(|c| c.headless);
        self.shared.uploads.begin_frame(
            match (loading, settings.upload_mb_per_frame) {
                (None, mb) if mb > 0.0 && budgeted => (mb as f64 * 1024.0 * 1024.0) as u64,
                _ => u64::MAX,
            },
            loading.map(|l| started + std::time::Duration::from_secs_f64(l.budget_ms / 1000.0)),
        );

        // New features.
        if let Some(mut pending) = world.get_resource_mut::<PendingRenderFeatures>()
            && !pending.0.is_empty()
        {
            let new: Vec<_> = std::mem::take(&mut pending.0);
            for f in new {
                self.add_feature(f);
            }
        }
    }

    /// Render every active [`crate::ExtraView`] camera, lowest `order` first: each with its own
    /// [`ViewState`] swapped in, extracted, recorded and submitted here (so the features'
    /// per-frame state is free again for the next view). Returns the pictures the main view
    /// shows in the window; images get their view's picture. Drops the state of cameras that
    /// are no extra views any more.
    fn render_extra_views(&mut self, world: &mut World) -> Vec<Composite> {
        self.main_thread_submit_ms = 0.0;
        let mut views: Vec<_> = world
            .query::<(
                Entity,
                &crate::Camera,
                &crate::ExtraView,
                Option<&crate::Viewport>,
            )>()
            .iter(world)
            .map(|(e, camera, extra, viewport)| {
                (
                    e,
                    camera.order,
                    camera.active,
                    extra.clone(),
                    viewport.copied(),
                )
            })
            .collect();
        let images = &self.shared.images;
        self.extra_views.retain(|e, state| {
            let keep = views.iter().any(|v| v.0 == *e);
            if !keep && let Some((index, _)) = &state.image {
                images.remove_target(*index);
            }
            keep
        });
        views.retain(|v| v.2);
        if views.is_empty() {
            return Vec::new();
        }
        views.sort_by_key(|v| v.1);
        let started = Instant::now();
        let window = world
            .get_resource::<Window>()
            .map(|w| UVec2::new(w.width.max(1), w.height.max(1)))
            .unwrap_or(self.targets.window);
        let mut composites = Vec::new();
        let mut gpu_ms = 0.0;
        for (camera, _, _, extra, viewport) in views {
            let (origin, size, image) = match &extra.target {
                crate::ViewTarget::Window => {
                    let (origin, size) = viewport.unwrap_or_default().within(window);
                    (Some(origin), size, None)
                }
                crate::ViewTarget::Image { image, size } => {
                    let image: engine_assets::Handle<Image> = *image;
                    let index = world
                        .get_resource::<Assets<Image>>()
                        .map_or(image.index(), |a| a.canonical(image).index());
                    (None, size.max(UVec2::ONE), Some(index))
                }
            };
            // The view's quality stands in for the game's while it renders, so everything that
            // reads the settings (`settings::effective`, features) sees this view's.
            let game_quality = extra
                .quality
                .map(|quality| swap_quality(world, Some(quality)));
            let settings = crate::settings::effective(world);
            let mut state = match self.extra_views.remove(&camera) {
                Some(state) => state,
                None => ExtraViewState {
                    view: ViewState::new(&self.shared, &settings, size.x, size.y, None),
                    image: None,
                },
            };
            // A new view's image shows its (still black) picture from the start: the scene
            // this view draws may show it too.
            if let Some(index) = image
                && state.image.is_none()
            {
                let texture = state.view.targets.ldr.texture.clone();
                self.shared.images.set_target(index, &texture);
                state.image = Some((index, texture));
            }
            self.swap_view(&mut state.view);
            world.insert_resource(crate::frame::RenderingExtraView(camera));
            let frame = self.extract_frame(world, started, Some(ExtraJob { size, settings }));
            world.remove_resource::<crate::frame::RenderingExtraView>();
            if let Some(quality) = game_quality {
                swap_quality(world, quality);
            }
            self.record_submit(frame);
            let texture = self.targets.ldr.texture.clone();
            self.swap_view(&mut state.view);
            gpu_ms += state.view.perf.gpu_ms.unwrap_or(0.0);

            let registered = state.image.as_ref().map(|(i, t)| (*i, t));
            if registered != image.map(|i| (i, &texture)) {
                if let Some((old, _)) = state.image.take() {
                    self.shared.images.remove_target(old);
                }
                if let Some(index) = image {
                    self.shared.images.set_target(index, &texture);
                    state.image = Some((index, texture.clone()));
                }
            }
            if let Some(origin) = origin {
                composites.push(Composite {
                    texture,
                    origin,
                    size,
                });
            }
            self.extra_views.insert(camera, state);
        }
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("render.extra_views", self.extra_views.len() as f64);
            d.set(
                "render.extra_views_ms",
                started.elapsed().as_secs_f64() * 1000.0,
            );
            d.set("render.extra_views_gpu_ms", gpu_ms as f64);
            // Main-thread time waiting in submits (the queue's lock is shared with the render
            // thread's submit and present); the frame's side work submits with the frame.
            d.set("render.extract_wait_ms", self.main_thread_submit_ms);
        }
        composites
    }

    /// The main-thread half of a view's frame: everything that reads the world, up to and
    /// including the features' `prepare`. `extra`: an extra view's (its view state swapped in).
    fn extract_frame(
        &mut self,
        world: &mut World,
        started: Instant,
        extra: Option<ExtraJob>,
    ) -> FrameWork {
        let mut cpu = crate::perf::CpuSteps::default();
        cpu.step("frame setup");

        // Size & vsync: the scene fills the active camera's viewport, the output the window
        // (an extra view's picture).
        let is_extra = extra.is_some();
        let (window, settings) = match extra {
            Some(job) => (job.size, job.settings),
            None => (
                world
                    .get_resource::<Window>()
                    .map(|w| UVec2::new(w.width.max(1), w.height.max(1)))
                    .unwrap_or(self.targets.window),
                crate::settings::effective(world),
            ),
        };
        let (scene_origin, viewport_size) = if is_extra {
            (UVec2::ZERO, window)
        } else {
            crate::frame::camera_viewport(world, window)
        };
        // At a render scale under 1 the scene renders smaller and the tonemap upscales it.
        let scene_size = (viewport_size.as_vec2() * settings.render_scale.clamp(0.25, 2.0))
            .round()
            .as_uvec2()
            .max(UVec2::ONE);
        if scene_size != UVec2::new(self.targets.width, self.targets.height)
            || window != self.targets.window
        {
            self.resize(scene_size, window, is_extra);
        }
        if !is_extra && let Some(s) = &mut self.surface {
            s.set_vsync(&self.shared.device, settings.vsync);
        }

        // Metered exposure from earlier frames (non-blocking readback).
        self.meter.collect(&self.shared.device);
        self.lens.collect();
        self.exposure.measured_ev100 = self.meter.measured_ev100;

        // View & uniforms.
        cpu.step("view");
        // A pixel-perfect camera renders without temporal anti-aliasing: no jitter to blur its
        // texels, nothing between them to smooth.
        let anti_aliasing = if crate::frame::active_camera_is_pixel_perfect(world) {
            AntiAliasing::None
        } else {
            settings.anti_aliasing
        };
        let taa = anti_aliasing == AntiAliasing::Taa;
        if anti_aliasing != self.prev_aa {
            self.effects.reset_history();
            self.prev_aa = anti_aliasing;
        }
        let jitter = if taa {
            crate::effects::jitter(self.frame_index)
        } else {
            Vec2::ZERO
        };
        let mut view = build_view(
            world,
            self.targets.width,
            self.targets.height,
            self.previous,
            jitter,
            &mut self.exposure,
            self.frame_index,
        );
        view.window = window;
        view.viewport_origin = scene_origin;
        view.viewport_size = viewport_size;
        if let Some(prev) = self.previous
            && crate::effects::is_camera_cut(
                prev.origin,
                prev.rotation,
                view.origin,
                view.rotation,
                self.prev_step,
            )
        {
            view.history_valid = false;
            self.meter.restart();
            self.exposure.snap = true;
        }
        self.cascade_cache.apply(
            &mut view.cascades,
            view.origin,
            self.frame_index,
            settings.shadow_cascade_reuse,
            !view.history_valid,
        );
        let mut uniform = frame_uniform(world, &view);
        uniform.shadow_params[3] = self.shadow.size as f32;
        let real_dt = world
            .get_resource::<Time>()
            .map_or(0.0, |t| t.real_delta_secs());
        // The camera's lens effects (off unless it has the components).
        let lens_settings = view.camera.map_or((None, None), |e| {
            (
                world.get::<crate::DepthOfField>(e).copied(),
                world.get::<crate::MotionBlur>(e).copied(),
            )
        });
        cpu.step("surface weather");
        let surface_resized = self.surface_weather.prepare(world, &self.shared, real_dt);
        (uniform.surface_map, uniform.surface_open) =
            self.surface_weather.frame_params(view.origin);
        // Screen-space reflections and bounce light need the previous frame's image of the same
        // view.
        cpu.step("uniforms and lights");
        let previous =
            self.effects.has_previous() && view.history_valid && self.prev_exposure > 0.0;
        let ssr = settings.screen_space_reflections && previous;
        let gi = uniform.reflections[3] > 0.0 && previous;
        uniform.reflections[0] = if ssr { 1.0 } else { 0.0 };
        uniform.reflections[1] = if previous { self.prev_exposure } else { 0.0 };
        if !gi {
            uniform.reflections[3] = 0.0;
        }
        let queue = &self.shared.queue;
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        let mut overlay_uniform = uniform;
        overlay_uniform.view_proj = uniform.unjittered_view_proj;
        overlay_uniform.proj = view.unjittered_proj.to_cols_array_2d();
        overlay_uniform.inv_proj = view.unjittered_proj.inverse().to_cols_array_2d();
        overlay_uniform.inv_view_proj = view.unjittered_view_proj.inverse().to_cols_array_2d();
        overlay_uniform.jitter = [0.0; 4];
        queue.write_buffer(
            &self.overlay_frame_buffer,
            0,
            bytemuck::bytes_of(&overlay_uniform),
        );
        let light_bytes: &[u8] = if view.point_lights.is_empty() {
            &[0u8; 32]
        } else {
            bytemuck::cast_slice(&view.point_lights)
        };
        let occlusion_resized =
            view.sky_occlusion.enabled && view.sky_occlusion.map_size != self.shadow.occlusion_size;
        if occlusion_resized {
            self.shadow.occlusion_size = view.sky_occlusion.map_size;
            self.shadow.occlusion =
                create_occlusion_target(&self.shared, self.shadow.occlusion_size);
            self.sky_camera
                .set_inputs(&self.shared, &self.frame_buffer, &self.shadow.occlusion);
        }
        let lights_moved = self.lights.write(&self.shared.device, queue, light_bytes);
        cpu.step("volumetrics prepare");
        let fog_moved =
            self.volumetrics
                .prepare(world, &self.shared, &view, &settings, uniform.volumetrics);
        cpu.step("shadow views");
        if lights_moved || occlusion_resized || fog_moved || surface_resized {
            self.rebuild_frame_groups();
        }
        let queue = &self.shared.queue;
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
        if view.sun_shadows && view.far_shadow.enabled {
            let u = ShadowViewUniform {
                view_proj: view.far_shadow.view_proj.to_cols_array_2d(),
                wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                time: [view.time, view.real_time, view.delta, 0.0],
                camera_frac: uniform.camera_world_frac,
            };
            queue.write_buffer(
                &self.shadow.view_buffers[FAR_SHADOW_VIEW],
                0,
                bytemuck::bytes_of(&u),
            );
        }
        if view.sky_occlusion.enabled {
            let u = ShadowViewUniform {
                view_proj: view.sky_occlusion.view_proj.to_cols_array_2d(),
                wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                time: [view.time, view.real_time, view.delta, 0.0],
                camera_frac: uniform.camera_world_frac,
            };
            queue.write_buffer(
                &self.shadow.view_buffers[SKY_OCCLUSION_VIEW],
                0,
                bytemuck::bytes_of(&u),
            );
        }
        self.write_post_params(&settings, &view, taa);

        // Prepare features.
        for f in &mut self.features {
            cpu.step(format!("prepare {}", f.name()));
            f.prepare(world, &self.shared, &view, &settings);
        }
        cpu.step("cloud shadows");

        // Cloud shadows: the first feature that asks draws the map in `compute`. The frame
        // uniform written above has them off; turn them on now that the area is known.
        let cloud_writer = if settings.cloud_shadows {
            self.features
                .iter()
                .enumerate()
                .find_map(|(i, f)| f.cloud_shadows().map(|r| (i, r)))
        } else {
            None
        };
        let cloud_area = cloud_writer
            .map(|(_, request)| CloudShadowArea::place(&request, view.origin, view.sun_dir));
        if let Some(area) = cloud_area {
            if self
                .cloud_shadows
                .ensure_size(&self.shared.device, area.size)
            {
                self.rebuild_frame_groups();
            }
            uniform.cloud_shadow = area.uniform(view.origin);
            let offset = std::mem::offset_of!(crate::frame::FrameUniform, cloud_shadow) as u64;
            let bytes = bytemuck::bytes_of(&uniform.cloud_shadow);
            self.shared
                .queue
                .write_buffer(&self.frame_buffer, offset, bytes);
            self.shared
                .queue
                .write_buffer(&self.overlay_frame_buffer, offset, bytes);
        }

        // Screenshots wanted from this frame (of the main view).
        let screenshots = if is_extra {
            Vec::new()
        } else {
            world
                .get_resource_mut::<Screenshots>()
                .map(|mut s| std::mem::take(&mut s.pending))
                .unwrap_or_default()
        };
        if !is_extra {
            self.shared
                .side_work
                .uploaded(self.shared.uploads.uploaded());
            if let Some(report) = self.shared.uploads.report() {
                log_uploads(world, &report);
            }
        }
        FrameWork {
            extra: is_extra,
            composites: Vec::new(),
            started,
            extract_ms: started.elapsed().as_secs_f32() * 1000.0,
            cpu: cpu.park(),
            settings,
            view,
            uniform,
            taa,
            ssr,
            gi,
            lens_settings,
            real_dt,
            scene_origin,
            viewport_size,
            cloud_writer,
            cloud_area,
            screenshots,
        }
    }

    /// The world-free half of a frame: record the passes, submit and present (on the render
    /// thread when pipelined). Results wait in `returned` for [`Self::apply_return`].
    fn record_submit(&mut self, frame: FrameWork) {
        let FrameWork {
            started,
            cpu,
            extract_ms,
            settings,
            view,
            uniform,
            taa,
            ssr,
            gi,
            lens_settings,
            real_dt,
            scene_origin,
            viewport_size,
            cloud_writer,
            cloud_area,
            screenshots,
            extra,
            composites,
        } = frame;
        let recording = Instant::now();
        // CPU time this thread spends, against the wall time: what is left is waiting
        // (for the GPU, the window, a lock, or a core).
        let thread_cpu = cpu_time::ThreadTime::try_now().ok();
        let mut cpu = crate::perf::CpuSteps::resume(cpu);
        let device = &self.shared.device;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        self.perf.begin(&mut cpu, &mut encoder, "atmosphere");
        let ctx = FrameContext {
            gpu: &self.shared,
            frame_group: &self.frame_group,
            view: &view,
            settings: &settings,
            scene_color_copy: &self.targets.color_copy.view,
            scene_depth_copy: &self.targets.depth_copy.view,
            ldr: &self.targets.ldr.view,
            cloud_shadows: None,
        };

        self.atmosphere.encode(&mut encoder, &uniform);
        // Cleared to 1 every frame, so nothing is shaded where (or when) nobody draws it.
        self.cloud_shadows
            .clear(&mut encoder, !self.cloud_stand_in_ready);
        self.cloud_stand_in_ready = true;
        let compute_ctx = FrameContext {
            frame_group: &self.compute_group,
            ..ctx
        };
        let cloud_target = cloud_area.map(|area| CloudShadowTarget {
            view: &self.cloud_shadows.view,
            format: CLOUD_SHADOW_FORMAT,
            area,
        });
        for (i, f) in self.features.iter_mut().enumerate() {
            let ctx = match cloud_writer {
                Some((writer, _)) if writer == i => FrameContext {
                    cloud_shadows: cloud_target,
                    ..compute_ctx
                },
                _ => compute_ctx,
            };
            self.perf
                .pass(&mut cpu, &mut encoder, format!("compute {}", f.name()));
            f.compute(&mut encoder, &ctx);
        }

        // Environment map: draw the sky, then filter it for reflections and diffuse light. The
        // passes drawing it read last frame's filtered maps.
        let interval = settings.environment_interval.max(1) as u64;
        if !self.env_rendered || self.frame_index.is_multiple_of(interval) {
            self.perf.pass(&mut cpu, &mut encoder, "environment");
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("environment"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.environment.target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.frame_group, &[]);
                for f in &self.features {
                    f.draw_environment(&mut pass, &ctx);
                }
            }
            self.environment.filter(&mut encoder);
            self.env_rendered = true;
        }
        let frame_group = &self.frame_group;

        // Shadows.
        if view.sun_shadows {
            self.perf.pass(&mut cpu, &mut encoder, "shadows");
            for i in 0..(view.cascades.count as usize).min(MAX_CASCADES) {
                // A reused cascade keeps last frame's image.
                if !view.cascades.drawn[i] {
                    continue;
                }
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
            // The far shadow map: big casters out to `far_shadow_distance`.
            if view.far_shadow.enabled {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("far shadow"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadow.layer_views[FAR_SHADOW_LAYER],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.shadow.view_bind_groups[FAR_SHADOW_VIEW], &[]);
                for f in &self.features {
                    f.draw_far_shadow(&mut pass, &ctx);
                }
            }
        }

        // Sky occlusion: the highest surface around the camera, seen from above. Cleared to
        // 1 (nothing) when disabled so stale data never darkens the sky light.
        self.perf.pass(&mut cpu, &mut encoder, "sky occlusion");
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky occlusion"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow.occlusion,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            if view.sky_occlusion.enabled {
                pass.set_bind_group(0, &self.shadow.view_bind_groups[SKY_OCCLUSION_VIEW], &[]);
                for f in &self.features {
                    f.draw_shadow(&mut pass, SKY_OCCLUSION_VIEW, &ctx);
                }
            }
        }
        self.sky_camera.encode(&mut encoder);

        // Depth prepass: depth, normals and motion of opaque geometry.
        self.perf.pass(&mut cpu, &mut encoder, "prepass");
        {
            let clear = wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prepass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.prepass_normal.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: clear,
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.motion.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: clear,
                    }),
                ],
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
            pass.set_bind_group(0, &self.prepass_group, &[]);
            for f in &self.features {
                f.draw_prepass(&mut pass, &ctx);
                pass.set_bind_group(0, &self.prepass_group, &[]);
            }
        }
        // Occlusion culling against that depth, then what it found visible after all.
        self.perf.pass(&mut cpu, &mut encoder, "occlusion culling");
        let mut late = false;
        for f in self.features.iter_mut() {
            late |= f.after_prepass(&mut encoder, &self.targets.depth.view, &ctx);
        }
        if late {
            let keep = wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prepass (late)"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.prepass_normal.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: keep,
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.motion.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: keep,
                    }),
                ],
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
            pass.set_bind_group(0, &self.prepass_group, &[]);
            for f in &self.features {
                f.draw_prepass_late(&mut pass, &ctx);
                pass.set_bind_group(0, &self.prepass_group, &[]);
            }
        }
        encoder.copy_texture_to_texture(
            self.targets.depth.texture.as_image_copy(),
            self.targets.prepass_depth.texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.targets.width,
                height: self.targets.height,
                depth_or_array_layers: 1,
            },
        );
        // Volumetric fog: needs the shadow cascades, the sky occlusion map, the sky light and
        // the prepass depth; shading reads it from frame binding 19 (a stand-in in the prepass
        // group).
        self.perf.pass(&mut cpu, &mut encoder, "volumetric fog");
        self.volumetrics.encode(&mut encoder, &self.prepass_group);
        if settings.ambient_occlusion {
            self.perf.pass(&mut cpu, &mut encoder, "ambient occlusion");
            self.effects
                .encode_ao(&mut encoder, &self.targets, &self.prepass_group);
        }
        if ssr {
            self.perf.pass(&mut cpu, &mut encoder, "reflections");
            self.effects
                .encode_ssr(&mut encoder, &self.targets, &self.prepass_group);
        }
        if gi {
            self.perf.pass(&mut cpu, &mut encoder, "bounce light");
            self.effects
                .encode_gi(&mut encoder, &self.targets, &self.prepass_group);
        } else {
            self.effects.skip_gi();
        }

        // Scene: opaque + sky (on the prepass depth). With the frame profiler on, each feature's
        // opaque draws get a pass of their own (`render.gpu.scene: <feature>`), then the sky.
        if engine_core::profile::enabled() {
            for (i, f) in self.features.iter().enumerate() {
                self.perf
                    .pass(&mut cpu, &mut encoder, format!("scene: {}", f.name()));
                let mut pass = scene_pass(&mut encoder, &self.targets, frame_group, i == 0);
                f.draw(RenderPhase::Opaque, &mut pass, &ctx);
            }
            self.perf.pass(&mut cpu, &mut encoder, "scene: sky phase");
            let mut pass = scene_pass(
                &mut encoder,
                &self.targets,
                frame_group,
                self.features.is_empty(),
            );
            for f in &self.features {
                f.draw(RenderPhase::Sky, &mut pass, &ctx);
                pass.set_bind_group(0, frame_group, &[]);
            }
        } else {
            self.perf.pass(&mut cpu, &mut encoder, "scene");
            let mut pass = scene_pass(&mut encoder, &self.targets, frame_group, true);
            for phase in [RenderPhase::Opaque, RenderPhase::Sky] {
                for f in &self.features {
                    f.draw(phase, &mut pass, &ctx);
                    pass.set_bind_group(0, frame_group, &[]);
                }
            }
        }

        // Skin: its diffuse light spreads under the surface (subsurface.rs).
        if settings.subsurface_width > 0.0
            && !view.orthographic
            && self.features.iter().any(|f| f.needs_subsurface())
        {
            self.perf
                .pass(&mut cpu, &mut encoder, "subsurface scattering");
            let sss = self
                .subsurface
                .get_or_insert_with(|| crate::subsurface::SubsurfacePass::new(&self.shared));
            {
                let mut pass =
                    sss.color_pass(&self.shared, &self.targets, &mut encoder, frame_group);
                for f in self.features.iter().filter(|f| f.needs_subsurface()) {
                    f.draw_subsurface_color(&mut pass, &ctx);
                    pass.set_bind_group(0, frame_group, &[]);
                }
            }
            sss.encode(
                &self.shared,
                &self.targets,
                &mut encoder,
                &view,
                settings.subsurface_width,
            );
        }

        // Copies for refraction / soft particles, and the color for TAA's reactive mask (what the
        // transparent phase changed).
        let scene_copy = self.features.iter().any(|f| f.needs_scene_copy());
        let reactive = if taa {
            settings.taa_reactive.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let size = wgpu::Extent3d {
            width: self.targets.width,
            height: self.targets.height,
            depth_or_array_layers: 1,
        };
        if scene_copy || reactive > 0.0 {
            self.perf.pass(&mut cpu, &mut encoder, "scene copies");
            encoder.copy_texture_to_texture(
                self.targets.hdr.texture.as_image_copy(),
                self.targets.color_copy.texture.as_image_copy(),
                size,
            );
        }
        if scene_copy {
            encoder.copy_texture_to_texture(
                self.targets.depth.texture.as_image_copy(),
                self.targets.depth_copy.texture.as_image_copy(),
                size,
            );
        }

        // Transparent.
        self.perf.pass(&mut cpu, &mut encoder, "transparent");
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
        if reactive > 0.0 {
            self.effects.encode_reactive(&mut encoder, &self.targets);
        }

        // Depth of field, on the scene before TAA (see depth_of_field.rs).
        self.perf.pass(&mut cpu, &mut encoder, "depth of field");
        self.lens.encode_depth_of_field(
            &self.shared,
            &self.targets,
            &mut encoder,
            &view,
            lens_settings.0.as_ref(),
            real_dt,
        );

        // Temporal anti-aliasing (writes the resolved image back into the HDR target). Either
        // way the finished image is kept for next frame's reflections and bounce light.
        self.perf.pass(&mut cpu, &mut encoder, "anti-aliasing");
        if taa {
            self.effects.encode_taa(
                &self.shared,
                &mut encoder,
                &self.targets,
                frame_group,
                view.history_valid,
                reactive,
            );
        } else if settings.screen_space_reflections || settings.screen_space_gi {
            self.effects.keep_frame(&mut encoder, &self.targets);
        }
        self.prev_exposure = view.exposure;

        // Exposure metering of the finished HDR image.
        self.perf.pass(&mut cpu, &mut encoder, "exposure meter");
        self.meter.encode(
            &self.shared.queue,
            &mut encoder,
            self.targets.width,
            self.targets.height,
            view.exposure,
        );

        // Motion blur: on the anti-aliased, pre-exposed HDR image, before bloom and tonemapping
        // (see motion_blur.rs).
        self.perf.pass(&mut cpu, &mut encoder, "motion blur");
        self.lens.encode_motion_blur(
            &self.shared,
            &self.targets,
            &mut encoder,
            &view,
            lens_settings.1.as_ref(),
            real_dt,
        );

        // Bloom.
        self.perf.pass(&mut cpu, &mut encoder, "bloom");
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
        self.perf.pass(&mut cpu, &mut encoder, "tonemap");
        {
            // Cleared to black around the scene's viewport.
            let mut pass = fullscreen_pass(&mut encoder, "tonemap", &self.targets.ldr.view, true);
            pass.set_viewport(
                scene_origin.x as f32,
                scene_origin.y as f32,
                viewport_size.x as f32,
                viewport_size.y as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(&self.post.tonemap_pipeline);
            pass.set_bind_group(0, &self.post.tonemap_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Extra views shown in the window (split screen, picture-in-picture): their pictures
        // over the scene, under the overlays.
        if !composites.is_empty() {
            self.perf.pass(&mut cpu, &mut encoder, "extra views");
        }
        for c in &composites {
            let size = c
                .size
                .min(self.targets.window.saturating_sub(c.origin))
                .min(UVec2::new(c.texture.width(), c.texture.height()));
            if size.x == 0 || size.y == 0 {
                continue;
            }
            encoder.copy_texture_to_texture(
                c.texture.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    texture: &self.targets.ldr.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: c.origin.x,
                        y: c.origin.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: size.x,
                    height: size.y,
                    depth_or_array_layers: 1,
                },
            );
        }

        // Overlays (UI), with unjittered matrices: the main view's only.
        let ctx = FrameContext {
            frame_group: &self.overlay_group,
            ..ctx
        };
        if !extra {
            for f in &mut self.features {
                self.perf
                    .pass(&mut cpu, &mut encoder, format!("overlay {}", f.name()));
                f.overlay(&mut encoder, &ctx);
            }
        }

        // Screenshot readback.
        self.perf.pass(&mut cpu, &mut encoder, "screenshot copy");
        let pending = screenshots;
        let readback = if pending.is_empty() {
            None
        } else {
            Some(copy_to_readback(device, &mut encoder, &self.targets.ldr))
        };

        let encode_ms = recording.elapsed().as_secs_f64() * 1000.0;
        // Present.
        let mut surface_texture = None;
        let mut reconfigure = false;
        let mut acquire_ms = 0.0;
        if !extra && let Some(surface) = &self.surface {
            cpu.step("acquire window image");
            let acquiring = Instant::now();
            let current = surface.surface.get_current_texture();
            acquire_ms = acquiring.elapsed().as_secs_f32() * 1000.0;
            let acquired = match current {
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
                self.perf.pass(&mut cpu, &mut encoder, "window blit");
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

        self.perf.end(&mut encoder);
        cpu.step("stats");
        let stats = self.collect_stats(&view, started);
        let mut back = RenderReturn::default();
        // wgpu validates and encodes every recorded pass into the backend's command buffer
        // here, not as the passes are recorded.
        cpu.step("finish commands");
        let finishing = Instant::now();
        let commands = encoder.finish();
        let finish_ms = finishing.elapsed().as_secs_f64() * 1000.0;
        cpu.step("submit");
        let submitting = Instant::now();
        let side = self.shared.take_side_commands();
        self.shared
            .queue
            .submit(side.commands.into_iter().chain([commands]));
        side.after.into_iter().for_each(|f| f());
        let side_work = if extra {
            Default::default()
        } else {
            self.shared.side_work.take()
        };
        let submit_ms = submitting.elapsed().as_secs_f64() * 1000.0;
        if extra {
            self.main_thread_submit_ms += submit_ms;
        }
        self.meter.after_submit();
        self.perf.after_submit();
        self.lens.after_submit();
        let mut present_ms = 0.0;
        if let Some(t) = surface_texture {
            cpu.step("present");
            let presenting = Instant::now();
            self.shared.queue.present(t);
            present_ms = presenting.elapsed().as_secs_f64() * 1000.0;
        }
        if reconfigure && let Some(s) = &mut self.surface {
            let (w, h) = (s.config.width, s.config.height);
            s.resize(&self.shared.device, w, h);
        }

        let mut screenshot_ms = 0.0;
        if let Some((buffer, padded_row)) = readback {
            cpu.step("screenshot save");
            let saving = Instant::now();
            let results = save_readback(
                &self.shared.device,
                &buffer,
                padded_row,
                self.targets.window.x,
                self.targets.window.y,
                &pending,
            );
            back.screenshots = results;
            screenshot_ms = saving.elapsed().as_secs_f64() * 1000.0;
        }

        if engine_core::profile::enabled() {
            self.perf.set_features(
                self.features
                    .iter()
                    .map(|f| (f.name().to_string(), f.stats())),
            );
        }
        self.perf.frame(
            &self.shared.device,
            &stats,
            acquire_ms,
            cpu,
            side_work,
            if extra { 0.0 } else { settings.perf_log_secs },
        );
        if extra {
            self.prev_step = (view.origin - view.prev_origin).length();
            self.previous = Some(PreviousView {
                view_proj: view.unjittered_view_proj,
                origin: view.origin,
                rotation: view.rotation,
            });
            self.last_view = Some(view);
            return;
        }
        {
            let d = &mut back.diagnostics;
            self.perf.publish(d);
            // Main thread: wait (for the frame in flight, set by render_system) and extract.
            // Render thread (or inline), all of it as record_total: record (recording the
            // passes), acquire, finish (wgpu encoding the recorded passes), submit, present,
            // screenshot (the GPU readback and the PNGs) and other (stats and bookkeeping, the
            // rest of the total); record_cpu is the CPU time it took, averaged over a second
            // (less than record_total = waiting).
            let total_ms = recording.elapsed().as_secs_f64() * 1000.0;
            let parts =
                encode_ms + acquire_ms as f64 + finish_ms + submit_ms + present_ms + screenshot_ms;
            d.set("render.extract_ms", extract_ms as f64);
            d.set("render.record_ms", encode_ms);
            d.set("render.finish_ms", finish_ms);
            d.set("render.submit_ms", submit_ms);
            d.set("render.present_ms", present_ms);
            d.set("render.screenshot_ms", screenshot_ms);
            d.set("render.record_other_ms", (total_ms - parts).max(0.0));
            d.set("render.record_total_ms", total_ms);
            if let Some(t) = thread_cpu.and_then(|t| t.try_elapsed().ok()) {
                self.thread_cpu.add(t);
                d.set("render.record_cpu_ms", self.thread_cpu.ms());
            }
            // wgpu's backend numbering: 0 none, 1 Vulkan, 2 Metal, 3 DX12, 4 OpenGL, 5 WebGPU.
            d.set(
                "render.backend",
                self.shared.adapter_info.backend as u8 as f64,
            );
            d.set("render.cpu_ms", stats.cpu_ms as f64);
            d.set("render.frame_ms", self.perf.frame_ms as f64);
            d.set("render.acquire_ms", self.perf.acquire_ms as f64);
            if let Some(ms) = self.perf.gpu_ms {
                d.set("render.gpu_ms", ms as f64);
            }
            if let Some(mb) = self.perf.gpu_memory_mb {
                d.set("render.gpu_memory_mb", mb as f64);
            }
            if let Some(mb) = self.perf.gpu_reserved_mb {
                d.set("render.gpu_reserved_mb", mb as f64);
            }
            d.set("render.draw_calls", stats.draw_calls as f64);
            d.set("render.instances", stats.instances as f64);
            d.set("render.triangles", stats.triangles as f64);
            d.set("render.clusters", stats.clusters as f64);
            d.set("render.ev100", stats.ev100 as f64);
            d.set(
                "render.surface_weather_uploads",
                self.surface_weather.uploads as f64,
            );
            let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
            d.set("render.upload_mb", mb(self.shared.uploads.uploaded()));
            d.set(
                "render.upload_updates_mb",
                mb(self.shared.uploads.updated()),
            );
            d.set(
                "render.pending_upload_mb",
                mb(self.shared.uploads.pending()),
            );
            if let Some(ev) = self.meter.measured_ev100 {
                d.set("render.ev100_metered", ev as f64);
            }
            if let Some(focus) = self.lens.focus_distance_m() {
                d.set("render.dof_focus_m", focus as f64);
                if let Some(lag) = self.lens.focus_lag_frames() {
                    d.set("render.dof_focus_lag_frames", lag as f64);
                }
            }
        }
        back.stats = stats;
        self.returned = Some(back);

        self.prev_step = (view.origin - view.prev_origin).length();
        self.previous = Some(PreviousView {
            view_proj: view.unjittered_view_proj,
            origin: view.origin,
            rotation: view.rotation,
        });
        self.last_view = Some(view);
        self.frame_index += 1;
    }

    /// Hand the last recorded frame's results to the world (main thread): screenshots,
    /// `RenderStats`, `render.*` diagnostics, and the CPU pixels of images that have been on the
    /// GPU unchanged for a while (when `release_image_pixels` is on).
    fn apply_return(&mut self, world: &mut World) {
        let Some(mut back) = self.returned.take() else {
            return;
        };
        if !back.screenshots.is_empty()
            && let Some(mut s) = world.get_resource_mut::<Screenshots>()
        {
            s.completed += back.screenshots.len() as u64;
            s.done.append(&mut back.screenshots);
            let excess = s.done.len().saturating_sub(16);
            s.done.drain(..excess);
        }
        // Drop images nobody draws any more, and release pixels.
        if let Some(mut images) = world.get_resource_mut::<Assets<Image>>()
            && self
                .shared
                .images
                .end_frame(images.bypass_change_detection())
                > 0
        {
            images.set_changed();
        }
        let image_stats = self.shared.images.stats();
        let stats = &mut back.stats;
        stats.images_released = image_stats.released;
        stats.image_bytes_released = image_stats.bytes_released;
        stats.images_compressed = image_stats.compressed;
        stats.image_bytes_compressed_away = image_stats.bytes_compressed_away;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            for (name, value) in &back.diagnostics.metrics {
                d.set(name, *value);
            }
            d.set("render.gpu_images", image_stats.gpu_images as f64);
            d.set("render.images_compressed", image_stats.compressed as f64);
            d.set(
                "render.image_mb_compressed_away",
                image_stats.bytes_compressed_away as f64 / (1024.0 * 1024.0),
            );
            d.set("render.image_uploads", image_stats.uploads as f64);
            let (warming, built_inline) = self.shared.materials.warming();
            d.set("render.materials.pipelines_warming", warming as f64);
            d.set(
                "render.materials.pipelines_built_inline",
                built_inline as f64,
            );
            d.set("render.images_released", image_stats.released as f64);
            d.set(
                "render.image_bytes_released",
                image_stats.bytes_released as f64,
            );
        }
        world.insert_resource(back.stats);
    }

    /// `taa`: temporal anti-aliasing runs this frame (its softness is sharpened back).
    fn write_post_params(&self, settings: &RenderSettings, view: &ViewData, taa: bool) {
        let tonemapper = match settings.tonemapper {
            Tonemapper::AgX => 0.0,
            Tonemapper::Aces => 1.0,
            Tonemapper::Reinhard => 2.0,
            Tonemapper::None => 3.0,
            Tonemapper::AgXPunchy => 4.0,
            Tonemapper::Neutral => 5.0,
        };
        let levels = self.targets.bloom.len().max(1) as f32;
        let upscaled = view.viewport_size != UVec2::new(view.width, view.height);
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
                // Temporal anti-aliasing's softness and an upscale's are sharpened back.
                if taa || upscaled {
                    settings.sharpness.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                settings.white_balance,
                view.exposure,
                settings.night_desaturation,
            ],
            params4: [if upscaled { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        };
        self.shared
            .queue
            .write_buffer(&self.post.params, 0, bytemuck::bytes_of(&u));
        // The first bloom downsample reads the scene and limits each pixel's brightness.
        if let Some(first) = self.post.bloom_params.first() {
            let src = &self.targets.hdr;
            let b = BloomUniform {
                params: [
                    1.0 / src.width as f32,
                    1.0 / src.height as f32,
                    1.0,
                    settings.bloom_limit.max(0.0),
                ],
            };
            self.shared
                .queue
                .write_buffer(first, 0, bytemuck::bytes_of(&b));
        }
    }

    fn collect_stats(&self, view: &ViewData, started: Instant) -> RenderStats {
        let mut total = FeatureStats::default();
        let mut clusters = 0;
        for f in &self.features {
            let s = f.stats();
            total.draw_calls += s.draw_calls;
            total.instances += s.instances;
            total.triangles += s.triangles;
            total.culled += s.culled;
            clusters += f.clusters_drawn();
        }
        RenderStats {
            cpu_ms: started.elapsed().as_secs_f32() * 1000.0,
            draw_calls: total.draw_calls,
            instances: total.instances,
            triangles: total.triangles,
            culled: total.culled,
            clusters,
            point_lights: view.point_lights.len() as u32,
            ev100: view.ev100,
            width: self.targets.width,
            height: self.targets.height,
            adapter: self.shared.adapter_info.name.clone(),
            backend: format!("{:?}", self.shared.adapter_info.backend),
            features: self.feature_names(),
            images_released: 0,
            image_bytes_released: 0,
            images_compressed: 0,
            image_bytes_compressed_away: 0,
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
                    "clusters": f.clusters_drawn(),
                    "state": f.describe(),
                })
            })
            .collect();
        let images = self.shared.images.stats();
        json!({
            "adapter": self.shared.adapter_info.name,
            "backend": format!("{:?}", self.shared.adapter_info.backend),
            "size": [self.targets.width, self.targets.height],
            "window": [self.targets.window.x, self.targets.window.y],
            "windowed": self.surface.is_some(),
            "frame": self.frame_index,
            "ev100": self.last_view.as_ref().map(|v| v.ev100),
            "features": features,
            "images": {
                "gpu": images.gpu_images,
                "uploads": images.uploads,
                "released_now": images.released_now,
                "released": images.released,
                "bytes_released": images.bytes_released,
            },
        })
    }
}

pub(crate) fn layout_entry(
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

/// The scene pass over the HDR target and the prepass depth, cleared or continued.
fn scene_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    targets: &Targets,
    frame_group: &wgpu::BindGroup,
    clear: bool,
) -> wgpu::RenderPass<'e> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("scene"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &targets.hdr.view,
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
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &targets.depth.view,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        ..Default::default()
    });
    pass.set_bind_group(0, frame_group, &[]);
    pass
}

pub(crate) fn fullscreen_pass<'e>(
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

fn create_shadow_resources(shared: &GpuShared, size: u32, occlusion_size: u32) -> ShadowResources {
    let device = &shared.device;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shadow cascades"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: FAR_SHADOW_LAYER as u32 + 1,
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
    let layer_views = (0..=FAR_SHADOW_LAYER as u32)
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
    let view_buffers: Vec<wgpu::Buffer> = (0..=FAR_SHADOW_VIEW)
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
        occlusion: create_occlusion_target(shared, occlusion_size),
        occlusion_size,
    }
}

fn create_occlusion_target(shared: &GpuShared, size: u32) -> wgpu::TextureView {
    shared
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("sky occlusion"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: shared.formats.shadow,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// Group 0 bind groups for every kind of pass.
struct FrameGroups {
    /// Scene passes.
    scene: wgpu::BindGroup,
    /// Features' `compute`: a stand-in for the cloud shadow map.
    compute: wgpu::BindGroup,
    /// Prepass and screen-space effects: stand-ins for the maps they write.
    prepass: wgpu::BindGroup,
    /// Overlays: the unjittered frame uniform.
    overlay: wgpu::BindGroup,
}

impl FrameGroups {
    /// `frame` holds the jittered and the unjittered frame uniform buffers.
    #[allow(clippy::too_many_arguments)]
    fn new(
        shared: &GpuShared,
        frame: [&wgpu::Buffer; 2],
        lights: &GrowBuffer,
        shadow: &ShadowResources,
        env: &EnvironmentMaps,
        sky_camera: &crate::sky_occlusion::SkyCamera,
        atmosphere: &AtmosphereLuts,
        surface: &wgpu::TextureView,
        maps: &ScreenMaps,
        stand_ins: &ScreenMaps,
        clouds: &CloudShadowMap,
    ) -> Self {
        let group = |uniform: &wgpu::Buffer, maps: &ScreenMaps, clouds: &wgpu::TextureView| {
            create_frame_group(
                shared, uniform, lights, shadow, env, sky_camera, atmosphere, surface, maps, clouds,
            )
        };
        Self {
            scene: group(frame[0], maps, &clouds.view),
            compute: group(frame[0], maps, &clouds.stand_in),
            prepass: group(frame[0], stand_ins, &clouds.view),
            overlay: group(frame[1], maps, &clouds.view),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_frame_group(
    shared: &GpuShared,
    frame: &wgpu::Buffer,
    lights: &GrowBuffer,
    shadow: &ShadowResources,
    env: &EnvironmentMaps,
    sky_camera: &crate::sky_occlusion::SkyCamera,
    atmosphere: &AtmosphereLuts,
    surface: &wgpu::TextureView,
    maps: &ScreenMaps,
    clouds: &wgpu::TextureView,
) -> wgpu::BindGroup {
    fn view(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::TextureView(v)
    }
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
                resource: view(&shadow.array_view),
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
                resource: view(&env.specular),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&shared.linear_repeat),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: view(&shadow.occlusion),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(&shared.nearest_clamp),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: sky_camera.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: view(&maps.ao),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: view(&maps.normal),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: view(&maps.depth),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: view(&atmosphere.transmittance.view),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: view(&atmosphere.multiscatter.view),
            },
            wgpu::BindGroupEntry {
                binding: 15,
                resource: view(&atmosphere.sky_view.view),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: view(&env.irradiance.view),
            },
            wgpu::BindGroupEntry {
                binding: 17,
                resource: wgpu::BindingResource::Sampler(&env.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 18,
                resource: view(&maps.ssr),
            },
            wgpu::BindGroupEntry {
                binding: 19,
                resource: view(&maps.fog),
            },
            wgpu::BindGroupEntry {
                binding: 20,
                resource: view(&maps.gi),
            },
            wgpu::BindGroupEntry {
                binding: 21,
                resource: view(surface),
            },
            wgpu::BindGroupEntry {
                binding: 22,
                resource: view(clouds),
            },
        ],
    })
}

/// A pipeline that draws one fullscreen triangle (`fullscreen_vertex`) into `format`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fullscreen_pipeline(
    shared: &GpuShared,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    let pipeline_layout = shared
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &layouts,
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
        &[&tonemap_layout],
        shared.formats.ldr,
        None,
    );
    let bloom_down = fullscreen_pipeline(
        shared,
        "bloom down",
        &bloom_module,
        "vs_bloom",
        "fs_down",
        &[&bloom_layout],
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
        &[&bloom_layout],
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
            &[&blit_layout],
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
    if world.contains_resource::<Renderer>() || world.contains_resource::<RenderThread>() {
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
    let settings = crate::settings::effective(world);
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
            if std::env::var("ENGINE_REQUIRE_GPU").is_ok_and(|v| v == "1") {
                panic!("no GPU and ENGINE_REQUIRE_GPU=1: {e}");
            }
            tracing::error!("renderer disabled: {e}");
            world.insert_resource(RenderDisabled(e));
            false
        }
    }
}

/// The renderer on its own thread ([`Pipelining`]): absent from the world as a `Renderer`
/// resource while it is pipelined. Reach it with [`with_renderer`], which waits for the frame
/// in flight first.
#[derive(Resource)]
pub struct RenderThread(engine_core::tasks::Pipelined<Renderer>);

/// Put `quality` in the world as the game's `Scalability` (none: remove it) without marking it
/// changed, and return what was there.
fn swap_quality(
    world: &mut World,
    quality: Option<engine_world::Scalability>,
) -> Option<engine_world::Scalability> {
    let before = world.get_resource::<engine_world::Scalability>().copied();
    match (
        quality,
        world.get_resource_mut::<engine_world::Scalability>(),
    ) {
        (Some(q), Some(mut res)) => *res.bypass_change_detection() = q,
        (Some(q), None) => world.insert_resource(q),
        (None, Some(_)) => {
            world.remove_resource::<engine_world::Scalability>();
        }
        (None, None) => {}
    }
    before
}

/// The exclusive system that renders a frame.
///
/// Pipelined (`RenderSettings::pipelining`), it waits for the previous frame's recording to
/// finish (the one sync point), applies its results, extracts this frame from the world and
/// hands recording, submit and present to the render thread, so the next frame's simulation
/// overlaps them (pipelined-rendering.md, as Bevy's pipelined rendering and Unreal's
/// render thread). Otherwise it renders inline.
pub fn render_system(world: &mut World) {
    if world.contains_resource::<RenderPaused>() || !ensure_renderer(world) {
        return;
    }
    let headless = world.resource::<EngineConfig>().headless;
    let pipelined = world
        .get_resource::<RenderSettings>()
        .is_some_and(|s| s.pipelining.on(headless));
    if pipelined {
        let mut thread = match world.remove_resource::<RenderThread>() {
            Some(thread) => thread,
            None => {
                let renderer = world.remove_resource::<Renderer>().expect("ensured");
                RenderThread(engine_core::tasks::Pipelined::new("render", renderer))
            }
        };
        if world.remove_resource::<Renderer>().is_some() {
            // Someone took the renderer out before it moved to its thread and put it back since.
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "a Renderer resource came back while the renderer is pipelined; dropped it \
                     (pause with RenderPaused, reach the renderer with with_renderer)"
                )
            });
        }
        let waiting = Instant::now();
        let renderer = thread.0.get();
        let wait_ms = waiting.elapsed().as_secs_f64() * 1000.0;
        renderer.apply_return(world);
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            // The main thread held up by the render thread still recording the last frame.
            d.set("render.wait_ms", wait_ms);
        }
        let frame = renderer.extract(world);
        thread
            .0
            .dispatch(move |renderer| renderer.record_submit(frame));
        world.insert_resource(thread);
    } else {
        if let Some(thread) = world.remove_resource::<RenderThread>() {
            let mut renderer = thread.0.into_inner();
            renderer.apply_return(world);
            world.insert_resource(renderer);
        }
        world.resource_scope(|world, mut renderer: Mut<Renderer>| renderer.render(world));
    }
}

/// Run `f` with the renderer, inline or pipelined; when pipelined, first wait for the frame in
/// flight and apply its results (Unreal's `FlushRenderingCommands`). `None` without a renderer.
pub fn with_renderer<R>(
    world: &mut World,
    f: impl FnOnce(&mut Renderer, &mut World) -> R,
) -> Option<R> {
    if let Some(mut thread) = world.remove_resource::<RenderThread>() {
        let renderer = thread.0.get();
        renderer.apply_return(world);
        let out = f(renderer, world);
        world.insert_resource(thread);
        return Some(out);
    }
    let mut renderer = world.remove_resource::<Renderer>()?;
    let out = f(&mut renderer, world);
    world.insert_resource(renderer);
    Some(out)
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
    // Inline even when pipelined, so the picture is this call's.
    with_renderer(world, |renderer, world| renderer.render(world));
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

/// Log a window of frames that uploaded a lot in play, naming what uploaded most: a mesh by
/// the components of an entity drawing it (which module owns it), an image by its size.
fn log_uploads(world: &mut World, report: &crate::features::UploadReport) {
    let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    let mut parts = Vec::new();
    for &(key, uploads, bytes) in &report.top {
        let what = if key & crate::mesh_feature::MESH_UPLOAD_KEY != 0 {
            let index = (key & !crate::mesh_feature::MESH_UPLOAD_KEY) as u32;
            let mut users = world.query::<(Entity, &crate::ResolvedMesh)>();
            let user = users
                .iter(world)
                .find(|(_, r)| r.mesh.index() == index)
                .map(|(e, _)| e);
            let components = user
                .and_then(|e| world.inspect_entity(e).ok())
                .map(|c| {
                    c.map(|(_, info)| {
                        engine_core::registry::short_type_name(&info.name().to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
                })
                .unwrap_or_else(|| "no entity draws it".into());
            format!("mesh {index} [{components}]")
        } else {
            let index = (key / crate::image_cache::VIEW_SLOTS as u64) as u32;
            let size = world
                .get_resource::<engine_assets::Assets<crate::Image>>()
                .and_then(|a| a.get(engine_assets::Handle::from_index(index)))
                .map_or_else(|| "gone".into(), |i| format!("{}x{}", i.width, i.height));
            format!("image {index} ({size})")
        };
        parts.push(format!(
            "{what}: {uploads} uploads, {:.2} MB a frame",
            mb(bytes) / report.frames as f64
        ));
    }
    tracing::info!(
        "uploads in play: {:.1} MB a frame over {} frames, {} items; most: {}",
        mb(report.bytes_per_frame),
        report.frames,
        report.items,
        parts.join("; ")
    );
}
