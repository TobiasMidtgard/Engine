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
use crate::shadows::{
    FAR_SHADOW_LAYER, FAR_SHADOW_MAP, MAX_CASCADES, SHADOW_MAPS, SKY_OCCLUSION_VIEW, ShadowTiles,
    ShadowViewUniform,
};
use crate::texture::Image;

/// Index of the far shadow map's view buffer and bind group (after the sky occlusion map's).
const FAR_SHADOW_VIEW: usize = FAR_SHADOW_MAP;
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
    /// Shadow map layers copied whole from their static images this frame (of the cascades,
    /// the far map and the sky occlusion map): an image that was drawn again, or a map with
    /// moving casters over more than a third of it.
    #[serde(default)]
    pub shadow_layers_copied: u32,
    /// Shadow map tiles (a 256th of a layer each, `shadows::ShadowTiles`) put back from the
    /// static images this frame: where moving casters were drawn the frame before.
    #[serde(default)]
    pub shadow_tiles_restored: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PostUniform {
    params: [f32; 4],
    params2: [f32; 4],
    params3: [f32; 4],
    params4: [f32; 4],
    /// The sun in the image (uv), its angular radius, the glare's strength (`sun_glare` in
    /// post.wgsl).
    sun: [f32; 4],
    /// Its illuminance at the camera and its uncovered disc's luminance, both pre-exposed.
    sun_light: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BloomUniform {
    params: [f32; 4],
}

struct ShadowResources {
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
    occlusion_texture: wgpu::Texture,
    occlusion_size: u32,
    /// The maps' static images (`shadows::ShadowCache`), made the first time they are used.
    statics: Option<StaticShadows>,
    /// The tiles of each map (by its number: cascades, `SKY_OCCLUSION_VIEW`, `FAR_SHADOW_MAP`)
    /// that hold something else than its static image: where moving casters were drawn the
    /// last time. The next frame puts those back before it draws its own.
    on_top: [ShadowTiles; SHADOW_MAPS],
    /// People's inset shadow maps (`ViewData::shadow_insets`, frame binding 27).
    insets: InsetShadows,
}

/// The inset shadow maps (`RenderSettings::shadow_insets`): one layer per person, each with
/// its view uniform. A 16² stand-in until a frame has people near the camera.
struct InsetShadows {
    /// Kept alive with its views.
    _texture: wgpu::Texture,
    array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
    view_buffers: Vec<wgpu::Buffer>,
    view_bind_groups: Vec<wgpu::BindGroup>,
    size: u32,
}

impl InsetShadows {
    fn new(shared: &GpuShared, size: u32) -> Self {
        let device = &shared.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("inset shadows"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: crate::shadows::MAX_SHADOW_INSETS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: shared.formats.shadow,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("inset shadows"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let view_buffers: Vec<wgpu::Buffer> = (0..crate::shadows::MAX_SHADOW_INSETS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("inset shadow view"),
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
                    label: Some("inset shadow view"),
                    layout: &shared.shadow_view_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: b.as_entire_binding(),
                    }],
                })
            })
            .collect();
        Self {
            layer_views: layer_views(&texture),
            _texture: texture,
            array_view,
            view_buffers,
            view_bind_groups,
            size,
        }
    }
}

/// The static images of the shadow maps: drawn when a map moves or what holds still in it
/// changes, copied into the maps every frame before the moving casters are drawn on top
/// (Unity HDRP's mixed cached shadows). Extra views draw everything straight into their maps
/// each frame and have none.
struct StaticShadows {
    /// The same layers as `ShadowResources::texture`.
    texture: wgpu::Texture,
    layers: Vec<wgpu::TextureView>,
    occlusion: (wgpu::Texture, wgpu::TextureView),
    occlusion_size: u32,
    /// Draws parts of an image into its map (`shadow_restore.wgsl`, [`restore_tiles`]).
    restore: wgpu::RenderPipeline,
    /// Each layer's image as `restore` reads it.
    restore_layers: Vec<wgpu::BindGroup>,
    /// The sky occlusion map's image as `restore` reads it.
    restore_occlusion: wgpu::BindGroup,
}

impl StaticShadows {
    fn new(shared: &GpuShared, size: u32, occlusion_size: u32) -> Self {
        let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow static images"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: FAR_SHADOW_LAYER as u32 + 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: shared.formats.shadow,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layers = layer_views(&texture);
        let occlusion =
            create_occlusion_target(shared, occlusion_size, wgpu::TextureUsages::COPY_SRC);
        let layout = restore_layout(shared);
        let pipeline_layout = shared.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow restore"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = shared.shader(
            "shadow_restore",
            include_str!("shaders/shadow_restore.wgsl"),
            false,
        );
        let restore = shared.render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow restore"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_restore"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            // The image's depth as it is, whatever the map holds there.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: shared.formats.shadow,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_restore"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            restore_layers: layers.iter().map(|v| restore_group(shared, v)).collect(),
            restore_occlusion: restore_group(shared, &occlusion.1),
            restore,
            texture,
            layers,
            occlusion,
            occlusion_size,
        }
    }
}

/// The layout of a static image as `shadow_restore.wgsl` reads it.
fn restore_layout(shared: &GpuShared) -> wgpu::BindGroupLayout {
    shared.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadow restore"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

fn restore_group(shared: &GpuShared, image: &wgpu::TextureView) -> wgpu::BindGroup {
    shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadow restore"),
        layout: &restore_layout(shared),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(image),
        }],
    })
}

/// More tiles than this to put back (of 256): the whole layer is copied, which costs less a
/// texel than drawing it.
const RESTORE_MOST_TILES: u32 = 96;

/// Put `tiles` of a `size`² map back as its static image (`image`) holds them, in a pass on
/// the map that loads it. Leaves the scissor on the whole map.
fn restore_tiles(
    pass: &mut wgpu::RenderPass<'_>,
    statics: &StaticShadows,
    image: &wgpu::BindGroup,
    tiles: ShadowTiles,
    size: u32,
) {
    pass.set_pipeline(&statics.restore);
    pass.set_bind_group(0, image, &[]);
    for [x, y, w, h] in tiles.rects(size) {
        pass.set_scissor_rect(x, y, w, h);
        pass.draw(0..3, 0..1);
    }
    pass.set_scissor_rect(0, 0, size, size);
}

/// The tiles of each shadow map the features' moving casters draw into this frame
/// (`RenderFeature::shadow_moving`), and each feature's number of them a map (those with any).
fn moving_tiles(
    features: &[Box<dyn RenderFeature>],
    ctx: &FrameContext,
) -> ([ShadowTiles; SHADOW_MAPS], Vec<(usize, [u16; SHADOW_MAPS])>) {
    let view = ctx.view;
    let cascades = (view.cascades.count as usize).min(MAX_CASCADES);
    let mut all = [ShadowTiles::NONE; SHADOW_MAPS];
    let mut by_feature = Vec::new();
    for (i, f) in features.iter().enumerate() {
        let mut counts = [0; SHADOW_MAPS];
        for (map, all) in all.iter_mut().enumerate() {
            let drawn = match map {
                SKY_OCCLUSION_VIEW => view.sky_occlusion.enabled,
                FAR_SHADOW_MAP => view.sun_shadows && view.far_shadow.enabled,
                c => view.sun_shadows && c < cascades,
            };
            if drawn {
                let tiles = f.shadow_moving(map, ctx);
                counts[map] = tiles.count() as u16;
                all.add(tiles);
            }
        }
        if counts.iter().any(|c| *c > 0) {
            by_feature.push((i, counts));
        }
    }
    (all, by_feature)
}

/// The tiles of `map` the casters joining its static image this frame are drawn into
/// (`RenderFeature::shadow_join_tiles` of the features with joins).
fn join_tiles(features: &[Box<dyn RenderFeature>], map: usize) -> ShadowTiles {
    let mut tiles = ShadowTiles::NONE;
    for f in features.iter().filter(|f| f.shadow_joins(map)) {
        tiles.add(f.shadow_join_tiles(map));
    }
    tiles
}

/// How a map gets back to its static image before a frame's moving casters are drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Restore {
    /// It holds nothing else.
    Nothing,
    /// These tiles are drawn from the image.
    Tiles(ShadowTiles),
    /// The whole layer is copied.
    Whole,
}

/// The frame's way of putting maps back (`trial_cut_more::SHADOW_TILES`, and the measurement
/// `shade_skip::SHADOW_COPIES`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum PutBack {
    /// Only the tiles that hold something else.
    Tiles,
    /// The whole image every frame (the frame as it was drawn until 11 Nov).
    Always,
    /// Never, but when the image changed: a measurement, moving casters leave trails.
    Never,
}

impl PutBack {
    fn of(settings: &RenderSettings) -> Self {
        if settings.debug_skip & crate::settings::shade_skip::SHADOW_COPIES != 0 {
            Self::Never
        } else if settings.trial_cuts_more & crate::settings::trial_cut_more::SHADOW_TILES == 0 {
            Self::Always
        } else {
            Self::Tiles
        }
    }

    /// What to put back of a map that holds `on_top` on its image, before casters are drawn
    /// into `moving`; `on_top` becomes what it holds after them. `redrawn`: the image was
    /// drawn again this frame; `joined`: the tiles casters that turned still were drawn into
    /// it in.
    fn plan(
        self,
        on_top: &mut ShadowTiles,
        moving: ShadowTiles,
        redrawn: bool,
        joined: ShadowTiles,
    ) -> Restore {
        let some = |tiles: ShadowTiles| {
            if tiles.is_empty() {
                Restore::Nothing
            } else if tiles.count() > RESTORE_MOST_TILES {
                Restore::Whole
            } else {
                Restore::Tiles(tiles)
            }
        };
        let restore = match self {
            _ if redrawn => Restore::Whole,
            Self::Always => Restore::Whole,
            Self::Never => some(joined),
            Self::Tiles => {
                // Where casters joined the image, the map holds it as it was.
                on_top.add(joined);
                some(*on_top)
            }
        };
        match restore {
            Restore::Whole => *on_top = moving,
            Restore::Tiles(tiles) if tiles == *on_top => *on_top = moving,
            // What it held is still there.
            _ => on_top.add(moving),
        }
        restore
    }
}

/// Layers copied whole and tiles drawn back in a frame's shadow maps (`RenderStats`).
#[derive(Debug, Clone, Copy, Default)]
struct PutBackCount {
    layers: u32,
    tiles: u32,
}

impl PutBackCount {
    fn count(&mut self, restore: Restore) {
        match restore {
            Restore::Whole => self.layers += 1,
            Restore::Tiles(tiles) => self.tiles += tiles.count(),
            Restore::Nothing => {}
        }
    }
}

/// One 2D view per layer of a shadow map array.
fn layer_views(texture: &wgpu::Texture) -> Vec<wgpu::TextureView> {
    (0..texture.depth_or_array_layers())
        .map(|i| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("shadow layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: i,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect()
}

/// Copy layer `layer` of `from` into the same layer of `to` (whole layers of the same size).
fn copy_layer(
    encoder: &mut wgpu::CommandEncoder,
    from: &wgpu::Texture,
    to: &wgpu::Texture,
    layer: u32,
) {
    let at = |texture| wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d {
            x: 0,
            y: 0,
            z: layer,
        },
        aspect: wgpu::TextureAspect::All,
    };
    encoder.copy_texture_to_texture(
        at(from),
        at(to),
        wgpu::Extent3d {
            width: to.width(),
            height: to.height(),
            depth_or_array_layers: 1,
        },
    );
}

/// `ShadowViewUniform::time.w` for a shadow map of `texel` meters: 1 where swaying leaves
/// are drawn at rest (`shadows::sway_moves`).
fn sway_rest(texel: f32) -> f32 {
    if crate::shadows::sway_moves(texel) {
        0.0
    } else {
        1.0
    }
}

/// What swaying casters in a shadow map of `texel` meters move by (the rest of its
/// `ShadowViewUniform` zero): the scene's plant wind and gust fronts, and its benders where
/// swaying leaves move (none where they rest in one pose, which a static image holds).
fn shadow_plants(uniform: &crate::frame::FrameUniform, texel: f32) -> ShadowViewUniform {
    let mut u = ShadowViewUniform::zeroed();
    if crate::shadows::sway_moves(texel) {
        u.bend[0] = uniform.shading[1];
        u.benders = uniform.benders;
    }
    u.plant_wind = uniform.plant_wind;
    u.gusts = uniform.gusts;
    u.gust_offset = uniform.gust_offset;
    u
}

/// A depth-only pass into `target`, cleared to 1 or loaded.
fn depth_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    clear: bool,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: target,
            depth_ops: Some(wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(1.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        ..Default::default()
    })
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
    /// Per feature (same order): whether it prepares shared this frame, what its shared prepare
    /// left for the world, and its time last frame (`crate::prepare`).
    prepare_shared: Vec<bool>,
    prepare_outs: Vec<crate::PrepareOut>,
    prepare_ms: Vec<f32>,
    /// The probe GI gather's time last frame (ms), to order the shared prepares.
    gi_ms: f32,
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
    /// When the environment map was last drawn: the frame, the sun's direction and the camera.
    env_rendered: Option<EnvDrawn>,
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
    /// The screen light pass: the prepass's depth and normals, stand-ins for the later maps.
    light_group: wgpu::BindGroup,
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
    /// Where each shadow map's static image was drawn, and which are drawn again.
    shadow_cache: crate::shadows::ShadowCache,
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
    /// Where the cloud shadow map was placed in the last extracted frame.
    last_cloud_area: Option<CloudShadowArea>,
    /// A mirror's view: the main view's maps it lights with (none for the main view).
    borrowed: Option<SharedLighting>,
    /// The last recorded frame's results, until `apply_return` hands them to the world.
    returned: Option<RenderReturn>,
    /// Each `ExtraView` camera's own resources and history, by camera.
    extra_views: HashMap<Entity, ExtraViewState>,
    /// The `render.extra_views_gpu.<pass>` and `render.extra_views_cpu.<step>` keys the extra
    /// views set last.
    extra_view_passes: std::collections::HashSet<String>,
    /// When the extra views' GPU passes were last logged.
    extra_view_logged: Option<Instant>,
    /// Main-thread time this frame in `queue.submit` (extra views submit there), for
    /// `render.extract_wait_ms`.
    main_thread_submit_ms: f64,
    /// Probe GI (`RenderSettings::probe_gi`): the main view's, its atlas bound by every view
    /// (frame binding 23).
    probe_gi: crate::probe_gi::ProbeGi,
    /// The main view's world reflections target holds a trace (clear it when they stop).
    world_reflection_written: bool,
    /// Shadow maps whose static image a feature asked to draw again (`shadow_static_changed`
    /// after the main view's `prepare_frame`), for the next frame.
    static_requests: [bool; SHADOW_MAPS],
    /// What the main view's last frame put back of its shadow maps.
    put_back: PutBackCount,
    /// And all its frames (wrapping).
    put_back_total: PutBackCount,
    /// The tiles of each shadow map each feature's moving casters drew into in the main view's
    /// last frame (features by index, those with any): `describe`.
    moving_tiles: Vec<(usize, [u16; SHADOW_MAPS])>,
    /// Finishes the frame's earlier encoders on worker threads (`RenderSettings::parallel_encoding`).
    finisher: crate::finish::Finisher,
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
    env_rendered: Option<EnvDrawn>,
    screen_stand_ins: ScreenMaps,
    frame_group: wgpu::BindGroup,
    compute_group: wgpu::BindGroup,
    prepass_group: wgpu::BindGroup,
    light_group: wgpu::BindGroup,
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
    shadow_cache: crate::shadows::ShadowCache,
    previous: Option<PreviousView>,
    prev_step: f64,
    prev_aa: AntiAliasing,
    prev_exposure: f32,
    last_view: Option<ViewData>,
    last_cloud_area: Option<CloudShadowArea>,
    borrowed: Option<SharedLighting>,
}

impl ViewState {
    /// A view of `width` × `height` pixels (`surface`: the window's format, for the main view).
    fn new(
        shared: &GpuShared,
        settings: &RenderSettings,
        width: u32,
        height: u32,
        surface: Option<wgpu::TextureFormat>,
        gi_atlas: &wgpu::TextureView,
    ) -> Self {
        Self::new_timed(shared, settings, width, height, surface, gi_atlas).0
    }

    /// [`new`](Self::new), with the milliseconds each part took (a new extra view's first
    /// frame: `render.extra_views_new.*`).
    fn new_timed(
        shared: &GpuShared,
        settings: &RenderSettings,
        width: u32,
        height: u32,
        surface: Option<wgpu::TextureFormat>,
        gi_atlas: &wgpu::TextureView,
    ) -> (Self, Vec<(&'static str, f32)>) {
        let mut laps = Laps::default();
        let device = &shared.device;
        let targets = Targets::new(
            device,
            &shared.formats,
            width,
            height,
            UVec2::new(width, height),
            UVec2::new(width, height),
        );
        laps.lap("targets");
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
        laps.lap("shadow maps");
        let environment = EnvironmentMaps::new(shared);
        laps.lap("environment");
        let mut sky_camera = crate::sky_occlusion::SkyCamera::new(shared);
        sky_camera.set_inputs(
            shared,
            &frame_buffer,
            &shadow.occlusion,
            shadow.occlusion_size,
        );
        laps.lap("sky occlusion");
        let volumetrics = crate::volumetrics::Volumetrics::new(shared, &targets);
        let screen_stand_ins = ScreenMaps::stand_ins(shared, &volumetrics);
        laps.lap("volumetrics");
        let atmosphere = AtmosphereLuts::new(shared, &frame_buffer);
        laps.lap("atmosphere");
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
            gi_atlas,
            None,
        );
        laps.lap("frame groups");
        let post = create_post(shared, &targets, surface);
        laps.lap("post");
        let effects = ScreenEffects::new(shared, &targets);
        laps.lap("screen effects");
        let mut meter = crate::exposure::ExposureMeter::new(shared);
        meter.set_source(device, &targets.hdr.view);
        laps.lap("exposure meter");
        let perf = crate::perf::PerfLog::new(device, &shared.queue);
        let view = Self {
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
            env_rendered: None,
            screen_stand_ins,
            frame_group: groups.scene,
            compute_group: groups.compute,
            prepass_group: groups.prepass,
            light_group: groups.light,
            overlay_group: groups.overlay,
            post,
            effects,
            atmosphere,
            exposure: ExposureState::default(),
            meter,
            volumetrics,
            lens: Default::default(),
            // Made with the other pipelines when they are warmed up, not on the frame skin first
            // shows (play's first frame, or an extra view's).
            subsurface: shared
                .warm_pipelines
                .then(|| crate::subsurface::SubsurfacePass::new(shared)),
            perf,
            shadow_cache: Default::default(),
            previous: None,
            prev_step: 0.0,
            prev_aa: settings.anti_aliasing,
            prev_exposure: 0.0,
            last_view: None,
            last_cloud_area: None,
            borrowed: None,
        };
        laps.lap("the rest");
        (view, laps.done)
    }
}

/// Milliseconds of consecutive parts of some work.
struct Laps {
    since: Instant,
    done: Vec<(&'static str, f32)>,
}

impl Default for Laps {
    fn default() -> Self {
        Self {
            since: Instant::now(),
            done: Vec::new(),
        }
    }
}

impl Laps {
    /// The part since the last lap is done.
    fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        self.done
            .push((name, (now - self.since).as_secs_f32() * 1000.0));
        self.since = now;
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
    /// A planar reflection's view: its picture is taken before bloom and tonemapping, which
    /// it skips.
    mirror: bool,
}

/// The main view's maps a mirror's view binds instead of its own
/// (`RenderSettings::mirror_shares_lighting`); compared to rebuild its bind groups on a change.
#[derive(Clone, PartialEq)]
struct SharedMaps {
    shadows: wgpu::TextureView,
    occlusion: wgpu::TextureView,
    occlusion_size: u32,
    env_specular: wgpu::TextureView,
    env_irradiance: wgpu::TextureView,
    surface: wgpu::TextureView,
    clouds: wgpu::TextureView,
}

/// What a mirror's view lights its scene with from the main view's last frame instead of
/// drawing its own (`RenderSettings::mirror_shares_lighting`): the sun's shadow maps, sky
/// occlusion, environment, cloud shadow and surface weather maps. All are placed around the
/// camera's position (cascades are squares centered on it, picked by distance), which a
/// mirror's camera shares but for the mirrored height, so the main view's maps hold what the
/// reflection shows; a point above the plane is never nearer the main camera than the mirrored
/// one, so its cascade (picked by distance from the mirrored eye) always holds it.
#[derive(Clone)]
struct SharedLighting {
    maps: SharedMaps,
    /// The main view's eye that frame: its maps' matrices are relative to it.
    origin: DVec3,
    sun_shadows: bool,
    cascades: crate::shadows::Cascades,
    far_shadow: crate::shadows::FarShadowView,
    sky_occlusion: crate::shadows::SkyOcclusionView,
    shadow_size: u32,
    /// The surface weather map's frame parameters at `origin`.
    surface: ([f32; 4], [f32; 4]),
    cloud_area: Option<CloudShadowArea>,
}

impl SharedLighting {
    /// Main-view-relative matrices made relative to `origin` (this view's eye) instead.
    fn place(&self, view: &mut ViewData) {
        let shift = Mat4::from_translation((view.origin - self.origin).as_vec3());
        view.sun_shadows = self.sun_shadows;
        view.cascades = self.cascades;
        for i in 0..MAX_CASCADES {
            view.cascades.view_proj[i] *= shift;
            view.cascades.light_view[i] *= shift;
        }
        view.far_shadow = self.far_shadow;
        view.far_shadow.view_proj *= shift;
        view.far_shadow.light_view *= shift;
        view.sky_occlusion = self.sky_occlusion;
        view.sky_occlusion.view_proj *= shift;
        view.sky_occlusion.view *= shift;
    }

    /// The surface weather map's frame parameters for a view at `origin`.
    fn surface_params(&self, origin: DVec3) -> ([f32; 4], [f32; 4]) {
        let (mut map, open) = self.surface;
        if map[2] > 0.0 {
            map[0] += (self.origin.x - origin.x) as f32;
            map[1] += (self.origin.z - origin.z) as f32;
        }
        (map, open)
    }
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
    /// A planar reflection's view (`ExtraJob::mirror`): no bloom or tonemapping.
    mirror: bool,
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
#[derive(Clone)]
struct ScreenMaps {
    ao: wgpu::TextureView,
    normal: wgpu::TextureView,
    depth: wgpu::TextureView,
    ssr: wgpu::TextureView,
    fog: wgpu::TextureView,
    gi: wgpu::TextureView,
    aerial: wgpu::TextureView,
    /// The screen light maps (lib_frame.wgsl `light_a`, `light_b`, `light_gi`, `light_fog`,
    /// `light_fog_through`).
    light: [wgpu::TextureView; 5],
    /// The lamps that can reach each tile (lib_frame.wgsl `lamp_tiles`).
    lamp_tiles: wgpu::TextureView,
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
            aerial: volumetrics.aerial.view().clone(),
            light: [
                targets.light_a.view.clone(),
                targets.light_b.view.clone(),
                targets.light_gi.view.clone(),
                targets.light_fog.view.clone(),
                targets.light_fog_through.view.clone(),
            ],
            lamp_tiles: targets.lamp_tiles.view.clone(),
        }
    }

    /// For the screen light pass: the prepass's depth and normals and the fog's tables (all
    /// finished by then), and stand-ins for every map drawn after it, its own among them.
    fn for_light(&self, stand_ins: &Self) -> Self {
        Self {
            normal: self.normal.clone(),
            depth: self.depth.clone(),
            fog: self.fog.clone(),
            aerial: self.aerial.clone(),
            ..stand_ins.clone()
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
        // One texel says "no map" to `screen_light`, whatever it holds.
        let light = [
            tex("no screen light a", crate::targets::LIGHT_FORMAT).view,
            tex("no screen light b", crate::targets::LIGHT_FORMAT).view,
            tex("no screen light gi", shared.formats.hdr).view,
            tex("no screen light fog", shared.formats.hdr).view,
            tex(
                "no screen light through the fog",
                crate::targets::FOG_THROUGH_FORMAT,
            )
            .view,
        ];
        // One texel says "no tiles" to the lamp loop.
        let lamp_tiles = Tex::new_layers(
            device,
            "no lamp tiles",
            1,
            1,
            1,
            crate::targets::LAMP_TILE_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .view;
        Self {
            ao: ao.view,
            normal: normal.view,
            depth: depth.view,
            ssr: ssr.view,
            fog: volumetrics.stand_in().clone(),
            gi: gi.view,
            aerial: volumetrics.aerial.stand_in().clone(),
            light,
            lamp_tiles,
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
        let started = Instant::now();
        let headless = window.is_none();
        let (gpu, surface) = {
            let _span = tracing::info_span!("render: create the GPU device").entered();
            gpu::create(window.map(|w| w.0), width, height, settings.vsync)?
        };
        let device_ms = started.elapsed().as_secs_f64() * 1000.0;
        let device = gpu.device.clone();
        let formats = crate::features::Formats {
            hdr: wgpu::TextureFormat::Rgba16Float,
            depth: wgpu::TextureFormat::Depth32Float,
            ldr: wgpu::TextureFormat::Rgba8UnormSrgb,
            shadow: shadow_depth_format(settings),
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
                // Probe GI's irradiance and distance atlas (probe_gi). The 16th.
                layout_entry(
                    23,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                // The aerial perspective table (aerial.rs). The 17th.
                layout_entry(
                    24,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                ),
                // The sky occlusion map's cover pyramid (sky_occlusion.rs). The 18th.
                layout_entry(25, all, crate::sky_occlusion::cover_binding()),
                // The canopy map (canopy.rs). The 19th.
                layout_entry(26, all, float_texture),
                // People's inset shadow maps (shadows.rs `ShadowInset`). The 20th.
                layout_entry(
                    27,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                // The screen light maps (effects.rs `encode_light`). The 21st to 23rd.
                layout_entry(28, all, float_texture),
                layout_entry(29, all, float_texture),
                layout_entry(30, all, float_texture),
                layout_entry(32, all, float_texture),
                layout_entry(33, all, float_texture),
                // The lamps that can reach each tile (effects.rs `encode_lamp_tiles`). The 24th.
                layout_entry(
                    31,
                    all,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
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
            materials: Default::default(),
            images: Default::default(),
            uploads: Default::default(),
            side_work: Default::default(),
            side_commands: Default::default(),
            warm_pipelines: settings.pipeline_warmup.on(headless),
            prepare_steps: Default::default(),
            mips: Default::default(),
            cache: Default::default(),
            canopy: crate::canopy::CanopyMap::new(&device),
        };

        if shared.warm_pipelines {
            shared.mips.warm(&shared.device);
        }
        let probe_gi = crate::probe_gi::ProbeGi::new(&shared.device);
        let viewing = Instant::now();
        let view = {
            let _span = tracing::info_span!("render: the view's targets and pipelines").entered();
            ViewState::new(
                &shared,
                settings,
                width,
                height,
                surface.as_ref().map(|s| s.view_format),
                probe_gi.atlas_view(),
            )
        };
        tracing::info!(
            "renderer: GPU device in {device_ms:.0} ms, the view's targets and pipelines in {:.0} ms",
            viewing.elapsed().as_secs_f64() * 1000.0
        );

        Ok(Self {
            instance: gpu.instance,
            adapter: gpu.adapter,
            shared,
            surface,
            targets: view.targets,
            features: Vec::new(),
            prepare_shared: Vec::new(),
            prepare_outs: Vec::new(),
            prepare_ms: Vec::new(),
            gi_ms: 0.0,
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
            light_group: view.light_group,
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
            shadow_cache: view.shadow_cache,
            previous: view.previous,
            prev_step: view.prev_step,
            prev_aa: view.prev_aa,
            prev_exposure: view.prev_exposure,
            frame_index: 0,
            last_view: view.last_view,
            last_cloud_area: view.last_cloud_area,
            borrowed: view.borrowed,
            returned: None,
            extra_views: HashMap::new(),
            extra_view_passes: Default::default(),
            extra_view_logged: None,
            main_thread_submit_ms: 0.0,
            probe_gi,
            static_requests: [false; SHADOW_MAPS],
            put_back: PutBackCount::default(),
            put_back_total: PutBackCount::default(),
            moving_tiles: Vec::new(),
            finisher: crate::finish::Finisher::new(crate::finish::THREADS),
            world_reflection_written: false,
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

    pub fn add_feature(&mut self, feature: Box<dyn RenderFeature>) {
        self.add_features(vec![feature]);
    }

    /// Add features, starting them on threads of their own: their `init` compiles their
    /// shaders and pipelines, which the driver does on the calling thread, so a game's dozens
    /// of features compile side by side instead of one after another (the first start after a
    /// shader change compiles everything). Returns each one's name and start time (ms).
    pub fn add_features(&mut self, features: Vec<Box<dyn RenderFeature>>) -> Vec<(String, f64)> {
        let (shared, width, height) = (&self.shared, self.targets.width, self.targets.height);
        let start = |mut feature: Box<dyn RenderFeature>| {
            let started = Instant::now();
            feature.init(shared);
            feature.resize(shared, width, height);
            (feature, started.elapsed().as_secs_f64() * 1000.0)
        };
        // The renderer's own start has every core: no frame is drawn until it's done. Features
        // added later (a zone streaming in) lease threads from those every module's background
        // work shares, held until every feature has started, so they don't starve the frame.
        let first = self.features.is_empty();
        let lease = (!first).then(|| engine_core::tasks::background_lease(features.len()));
        let threads = lease
            .as_ref()
            .map_or_else(
                || std::thread::available_parallelism().map_or(1, |n| n.get()),
                |l| l.threads(),
            )
            .min(features.len());
        let started: Vec<(Box<dyn RenderFeature>, f64)> = if threads <= 1 {
            features.into_iter().map(start).collect()
        } else {
            let queue = std::sync::Mutex::new(features.into_iter().enumerate());
            let mut done: Vec<_> = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        scope.spawn(|| {
                            let mut out = Vec::new();
                            loop {
                                // Taken first: a guard in the `while let` would be held
                                // through the start.
                                let next = queue.lock().unwrap_or_else(|e| e.into_inner()).next();
                                let Some((i, f)) = next else { break };
                                out.push((i, start(f)));
                            }
                            out
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                    .collect()
            });
            // In the order given, as when they started one by one.
            done.sort_by_key(|(i, _)| *i);
            done.into_iter().map(|(_, f)| f).collect()
        };
        let mut times = Vec::with_capacity(started.len());
        for (feature, ms) in started {
            times.push((feature.name().to_string(), ms));
            self.features.push(feature);
        }
        self.features.sort_by_key(|f| f.order());
        // Times by index no longer match: they come back after a frame.
        self.prepare_ms.clear();
        self.prepare_ms.resize(self.features.len(), 0.0);
        self.prepare_outs
            .resize_with(self.features.len(), Default::default);
        times
    }

    /// Size the targets for a scene of `scene` pixels in a window of `window` pixels (`extra`:
    /// an extra view's, `window` its picture's size; the window and the features keep theirs).
    fn resize(&mut self, scene: UVec2, output: UVec2, window: UVec2, extra: bool) {
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
            output,
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

    /// The main view's last frame's lighting, for a mirror's view to borrow (`SharedLighting`);
    /// `None` before the main view's first frame. Call with the main view's state in place.
    fn lighting_to_share(&self) -> Option<SharedLighting> {
        let last = self.last_view.as_ref()?;
        Some(SharedLighting {
            maps: SharedMaps {
                shadows: self.shadow.array_view.clone(),
                occlusion: self.shadow.occlusion.clone(),
                occlusion_size: self.shadow.occlusion_size,
                env_specular: self.environment.specular.clone(),
                env_irradiance: self.environment.irradiance.view.clone(),
                surface: self.surface_weather.view.clone(),
                clouds: self.cloud_shadows.view.clone(),
            },
            origin: last.origin,
            sun_shadows: last.sun_shadows,
            cascades: last.cascades,
            far_shadow: last.far_shadow,
            sky_occlusion: last.sky_occlusion,
            shadow_size: self.shadow.size,
            surface: self.surface_weather.frame_params(last.origin),
            cloud_area: self.last_cloud_area,
        })
    }

    /// Set what the extra view in place borrows from the main view. Its own shadow maps shrink
    /// to a stand-in while it borrows (they are not drawn) and come back at full size when it
    /// stops; its bind groups follow the maps.
    fn set_borrowed(&mut self, lighting: Option<SharedLighting>, settings: &RenderSettings) {
        let before = self.borrowed.as_ref().map(|b| b.maps.clone());
        let was = self.borrowed.is_some();
        self.borrowed = lighting;
        if self.borrowed.as_ref().map(|b| &b.maps) == before.as_ref() {
            return;
        }
        match (was, self.borrowed.is_some()) {
            (false, true) => self.shadow = create_shadow_resources(&self.shared, 256, 64),
            (true, false) => {
                self.shadow = create_shadow_resources(
                    &self.shared,
                    settings.shadow_map_size.clamp(256, 8192),
                    settings.sky_occlusion_size.clamp(64, 4096),
                )
            }
            _ => {}
        }
        let (occlusion, occlusion_size) = self
            .borrowed
            .as_ref()
            .map_or((&self.shadow.occlusion, self.shadow.occlusion_size), |b| {
                (&b.maps.occlusion, b.maps.occlusion_size)
            });
        self.sky_camera
            .set_inputs(&self.shared, &self.frame_buffer, occlusion, occlusion_size);
        self.rebuild_frame_groups();
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
            self.probe_gi.atlas_view(),
            self.borrowed.as_ref().map(|b| &b.maps),
        );
        self.frame_group = groups.scene;
        self.compute_group = groups.compute;
        self.prepass_group = groups.prepass;
        self.light_group = groups.light;
        self.overlay_group = groups.overlay;
    }

    /// After a camera cut auto exposure holds until the meter has read the new view, a frame
    /// or two later, sooner or later with the machine's speed. A screenshot waits for that
    /// reading when it is on its way, so it shows the new view's exposure on any machine.
    fn settle_exposure(&mut self) {
        if self.exposure.snap && self.meter.measured_ev100.is_none() && self.meter.in_flight() {
            let _ = self.shared.device.poll(wgpu::PollType::wait_indefinitely());
            self.meter.collect(&self.shared.device);
        }
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
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            // Main-thread time waiting in submits (the queue's lock is shared with the render
            // thread's submit and present): the extra views'; the frame's side work submits
            // with the frame. Every frame, 0 without extra views.
            d.set("render.extract_wait_ms", self.main_thread_submit_ms);
        }
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
        // `ENGINE_UPLOAD_BUDGET=on`: a headless profile run uploads as a windowed game does.
        static BUDGET_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let budget_env = *BUDGET_ENV.get_or_init(|| {
            std::env::var("ENGINE_UPLOAD_BUDGET")
                .is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "on" | "1" | "always"))
        });
        let budgeted = settings.upload_budget_headless
            || budget_env
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
            let _span = tracing::info_span!("render: start the features").entered();
            self.add_features(new);
        }
    }

    /// Render every active [`crate::ExtraView`] camera, lowest `order` first: each with its own
    /// [`ViewState`] swapped in, extracted, recorded and submitted here (so the features'
    /// per-frame state is free again for the next view). Returns the pictures the main view
    /// shows in the window; images get their view's picture. Drops the state of cameras that
    /// are no extra views any more.
    fn render_extra_views(&mut self, world: &mut World) -> Vec<Composite> {
        self.main_thread_submit_ms = 0.0;
        crate::portal::place_portal_views(world);
        crate::planar_reflection::place_reflection_views(world);
        let mut views: Vec<_> = world
            .query::<(
                Entity,
                &crate::Camera,
                &crate::ExtraView,
                Option<&crate::Viewport>,
                Option<&crate::portal::PortalCamera>,
                Option<&crate::planar_reflection::MirrorCamera>,
            )>()
            .iter(world)
            .map(|(e, camera, extra, viewport, portal, mirror)| {
                (
                    e,
                    camera.order,
                    camera.active,
                    extra.clone(),
                    viewport.copied(),
                    portal.copied(),
                    mirror.copied(),
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
            // None this frame: say so, not the last extra view's numbers.
            if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
                for key in [
                    "render.extra_views",
                    "render.extra_views_ms",
                    "render.extra_views_gpu_ms",
                    "render.extra_views_shared_lighting",
                ] {
                    if d.metrics.contains_key(key) {
                        d.set(key, 0.0);
                    }
                }
                for gone in self.extra_view_passes.drain() {
                    d.set(&gone, 0.0);
                }
            }
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
        let mut shared_lighting = 0;
        let mut passes: HashMap<String, f32> = HashMap::new();
        // The main view's state is in place until the first swap.
        let main_lighting = self.lighting_to_share();
        for (camera, _, _, extra, viewport, portal, mirror) in views {
            let off_screen = portal.map(|p| p.size).or(mirror.map(|m| m.size));
            let (origin, size, image) = match (&extra.target, off_screen) {
                // A portal's or a mirror's view: the portal feature draws its picture, features
                // sample a mirror's.
                (_, Some(size)) => (None, size.max(UVec2::ONE), None),
                (crate::ViewTarget::Window, None) => {
                    let (origin, size) = viewport.unwrap_or_default().within(window);
                    (Some(origin), size, None)
                }
                (crate::ViewTarget::Image { image, size }, None) => {
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
            let settings = if mirror.is_some() {
                settings.for_reflection()
            } else {
                settings
            };
            let mut state = match self.extra_views.remove(&camera) {
                Some(state) => state,
                None => {
                    let made = self.shared.cache.made();
                    let (view, laps) = ViewState::new_timed(
                        &self.shared,
                        &settings,
                        size.x,
                        size.y,
                        None,
                        self.probe_gi.atlas_view(),
                    );
                    let total: f32 = laps.iter().map(|l| l.1).sum();
                    // Shader modules, layouts and pipelines the main view didn't have yet.
                    let compiled = self.shared.cache.made() - made;
                    tracing::info!(
                        "renderer: a new extra view's state in {total:.1} ms, {compiled} new \
                         shaders, layouts and pipelines ({})",
                        laps.iter()
                            .map(|(name, ms)| format!("{name} {ms:.1}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
                        d.set("render.extra_views_new_ms", total as f64);
                        d.set("render.extra_views_new_compiled", compiled as f64);
                        for (name, ms) in laps {
                            d.set(&format!("render.extra_views_new.{name}"), ms as f64);
                        }
                    }
                    ExtraViewState { view, image: None }
                }
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
            let lighting = main_lighting
                .clone()
                .filter(|_| mirror.is_some() && settings.mirror_shares_lighting);
            shared_lighting += lighting.is_some() as u32;
            self.set_borrowed(lighting, &settings);
            world.insert_resource(crate::frame::RenderingExtraView(camera));
            let frame = self.extract_frame(
                world,
                started,
                Some(ExtraJob {
                    size,
                    settings,
                    mirror: mirror.is_some(),
                }),
            );
            world.remove_resource::<crate::frame::RenderingExtraView>();
            if let Some(quality) = game_quality {
                swap_quality(world, quality);
            }
            let exposure = frame.view.exposure;
            let mirror_view = (
                frame.view.unjittered_view_proj,
                frame.view.origin,
                frame.view.exposure,
            );
            self.record_submit(frame);
            if let Some(mirror) = mirror {
                let mut encoder =
                    self.shared
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("planar reflection"),
                        });
                crate::planar_reflection::keep_picture(
                    world,
                    &self.shared,
                    &mut encoder,
                    &mirror,
                    camera,
                    &self.targets.hdr.texture,
                    &self.targets.depth.view,
                    mirror_view,
                );
                self.shared.queue.submit([encoder.finish()]);
            }
            if let Some(portal) = portal {
                world
                    .get_resource_or_init::<crate::portal::PortalPictures>()
                    .0
                    .insert(
                        portal.portal,
                        crate::portal::PortalPicture {
                            camera,
                            color: self.targets.hdr.view.clone(),
                            depth: self.targets.depth.view.clone(),
                            tangents: portal.tangents,
                            near: portal.near,
                            exposure,
                        },
                    );
            }
            let texture = self.targets.ldr.texture.clone();
            self.swap_view(&mut state.view);
            gpu_ms += state.view.perf.gpu_ms.unwrap_or(0.0);
            for (name, ms) in state.view.perf.gpu_passes().0 {
                *passes
                    .entry(format!("render.extra_views_gpu.{name}"))
                    .or_insert(0.0) += *ms;
            }
            // With the parts features time inside their steps ("meshes: gather" inside
            // "prepare meshes").
            let inner = state.view.perf.inner_steps().iter();
            for (name, ms) in state
                .view
                .perf
                .cpu_steps()
                .iter()
                .map(|(n, ms)| (n.as_ref(), ms))
                .chain(inner.map(|(n, ms)| (*n, ms)))
            {
                *passes
                    .entry(format!("render.extra_views_cpu.{name}"))
                    .or_insert(0.0) += *ms;
            }

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
        {
            // Each GPU pass and CPU step of the extra views together
            // (`render.extra_views_gpu.<pass>`, `render.extra_views_cpu.<step>`), to see what a
            // mirror's or a portal's picture spends its time on.
            let every = world
                .get_resource::<RenderSettings>()
                .map_or(0.0, |s| s.perf_log_secs);
            if every > 0.0
                && self
                    .extra_view_logged
                    .is_none_or(|t| t.elapsed().as_secs_f32() >= every)
            {
                self.extra_view_logged = Some(Instant::now());
                let rows = |prefix: &str| {
                    let mut rows: Vec<_> = passes
                        .iter()
                        .filter(|r| r.0.starts_with(prefix) && *r.1 >= 0.05)
                        .collect();
                    rows.sort_by(|a, b| b.1.total_cmp(a.1));
                    rows.iter()
                        .map(|(k, ms)| format!("{} {ms:.2}", k.trim_start_matches(prefix)))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                tracing::info!(
                    "extra views: {} ({} sharing the main view's lighting), CPU {:.2} ms: {}; \
                     GPU {gpu_ms:.2} ms: {}",
                    self.extra_views.len(),
                    shared_lighting,
                    started.elapsed().as_secs_f64() * 1000.0,
                    rows("render.extra_views_cpu."),
                    rows("render.extra_views_gpu."),
                );
            }
            let Some(mut d) = world.get_resource_mut::<Diagnostics>() else {
                return composites;
            };
            let mut now = std::collections::HashSet::new();
            for (key, ms) in passes {
                d.set(&key, ms as f64);
                now.insert(key);
            }
            for gone in self.extra_view_passes.difference(&now) {
                d.set(gone, 0.0);
            }
            self.extra_view_passes = now;
            d.set("render.extra_views", self.extra_views.len() as f64);
            d.set(
                "render.extra_views_ms",
                started.elapsed().as_secs_f64() * 1000.0,
            );
            d.set("render.extra_views_gpu_ms", gpu_ms as f64);
            // Mirrors lit with the main view's maps (`RenderSettings::mirror_shares_lighting`).
            d.set("render.extra_views_shared_lighting", shared_lighting as f64);
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
        let mirror = extra.as_ref().is_some_and(|job| job.mirror);
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
        // A pixel-perfect camera renders without temporal anti-aliasing: no jitter to blur its
        // texels, nothing between them to smooth.
        let anti_aliasing = if crate::frame::active_camera_is_pixel_perfect(world) {
            AntiAliasing::None
        } else {
            settings.anti_aliasing
        };
        let taa = anti_aliasing == AntiAliasing::Taa;
        // At a render scale under 1 the scene renders smaller. Temporal anti-aliasing then
        // rebuilds it at the viewport's size from the jittered frames (temporal upscaling);
        // without it, or under motion blur (which works on the scene's image), the tonemap
        // stretches it.
        let scene_size = (viewport_size.as_vec2() * settings.render_scale.clamp(0.25, 2.0))
            .round()
            .as_uvec2()
            .max(UVec2::ONE);
        let upscale = taa
            && !is_extra
            && settings.temporal_upscaling
            && scene_size.cmplt(viewport_size).any()
            && scene_size.cmple(viewport_size).all()
            && !crate::frame::active_camera(world)
                .is_some_and(|(e, _, _)| world.get::<crate::MotionBlur>(e).is_some());
        let output = if upscale { viewport_size } else { scene_size };
        if scene_size != UVec2::new(self.targets.width, self.targets.height)
            || output != self.targets.output
            || window != self.targets.window
        {
            self.resize(scene_size, output, window, is_extra);
        }
        // The screen light maps exist only while a cut that fills them is on.
        let screen_light = settings.trial_cuts
            & (crate::settings::trial_cut::SCREEN_LIGHT | crate::settings::trial_cut::SCREEN_FOG)
            != 0;
        if screen_light != self.targets.screen_light {
            self.targets
                .set_screen_light(&self.shared.device, &self.shared.formats, screen_light);
            self.rebuild_frame_groups();
        }
        if !is_extra && let Some(s) = &mut self.surface {
            s.set_vsync(&self.shared.device, settings.vsync);
        }

        // Metered exposure from earlier frames (non-blocking readback).
        self.meter.collect(&self.shared.device);
        self.lens.collect();
        self.exposure.measured_ev100 = self.meter.measured_ev100;
        self.exposure.measured_highlights_ev100 = self.meter.measured_highlights_ev100;

        // View & uniforms.
        cpu.step("view");
        if anti_aliasing != self.prev_aa {
            self.effects.reset_history();
            self.prev_aa = anti_aliasing;
        }
        let jitter = if taa {
            let ratio = self.targets.output.x as f32 / self.targets.width as f32;
            crate::effects::jitter_upscaled(self.frame_index, ratio)
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
        // A mirror borrowing the main view's maps takes their placement too.
        let borrowed = if is_extra {
            self.borrowed.clone()
        } else {
            None
        };
        if let Some(b) = &borrowed {
            b.place(&mut view);
        }
        // Inset shadows are the main view's (its maps made at their size once people are near).
        if is_extra {
            view.shadow_insets.clear();
        }
        let inset_size = settings.shadow_inset_size.clamp(256, 4096);
        let insets_resized =
            !view.shadow_insets.is_empty() && self.shadow.insets.size != inset_size;
        if insets_resized {
            self.shadow.insets = InsetShadows::new(&self.shared, inset_size);
        }
        // Extra views draw every shadow caster straight into their maps each frame.
        let requests = if is_extra {
            [false; SHADOW_MAPS]
        } else {
            std::mem::take(&mut self.static_requests)
        };
        let new_statics = !is_extra && self.shadow.statics.is_none();
        if new_statics {
            self.shadow.statics = Some(StaticShadows::new(
                &self.shared,
                self.shadow.size,
                self.shadow.occlusion_size,
            ));
        }
        if borrowed.is_none() {
            self.shadow_cache.hold = crate::shadows::static_holds(
                settings.shadow_refresh_secs,
                world
                    .get_resource::<crate::FramePacing>()
                    .map_or(0.0, |p| p.frame_secs()),
            );
            self.shadow_cache.apply(
                &mut view.cascades,
                &mut view.sky_occlusion,
                &mut view.far_shadow,
                view.origin,
                new_statics
                    || is_extra
                    || settings.shadow_cascade_reuse == 0
                    || !view.history_valid,
                requests,
            );
        }
        let mut uniform = frame_uniform(world, &view);
        uniform.sky[3] = crate::frame::texture_mip_bias(taa, settings.render_scale);
        uniform.shadow_params[3] = borrowed
            .as_ref()
            .map_or(self.shadow.size, |b| b.shadow_size) as f32;
        uniform.distant_shadows[3] = self.shadow.insets.size as f32;
        if borrowed.is_some() {
            // The main view draws these maps: no feature draws casters for this view.
            view.sun_shadows = false;
            view.far_shadow.enabled = false;
            view.sky_occlusion.enabled = false;
        }
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
        // The ground's weather map: a mirror borrows the main view's; the main view uploads
        // its own beside the shared prepares below.
        let mut surface_resized = false;
        if let Some(b) = &borrowed {
            (uniform.surface_map, uniform.surface_open) = b.surface_params(view.origin);
        }
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
        // The features that only read the world prepare side by side, with the probe GI's
        // gather (`crate::prepare`); the others prepare after the frame's uniforms, as before.
        cpu.step("prepare serial");
        if !is_extra && let Some(ms) = self.perf.fresh_gpu_pass("probe gi") {
            self.probe_gi.measured(ms);
        }
        if !is_extra {
            self.probe_gi.ensure_query(world);
        }
        self.prepare_shared.clear();
        self.prepare_shared
            .extend(self.features.iter().map(|f| f.prepares_shared()));
        self.prepare_outs
            .resize_with(self.features.len(), Default::default);
        self.prepare_ms.resize(self.features.len(), 0.0);
        for (f, _) in self
            .features
            .iter_mut()
            .zip(&self.prepare_shared)
            .filter(|(_, on)| **on)
        {
            f.prepare_serial(world, &self.shared, &view, &settings);
        }
        cpu.step("prepare side by side");
        let gi_done = crate::prepare::run(
            &mut self.features,
            &self.prepare_shared,
            &mut self.prepare_outs,
            &mut self.prepare_ms,
            (!is_extra).then_some(crate::prepare::GiJob {
                gi: &mut self.probe_gi,
                uniform: &mut uniform,
                last_ms: self.gi_ms,
            }),
            borrowed.is_none().then_some(crate::prepare::SurfaceJob {
                map: &mut self.surface_weather,
                real_dt,
                resized: &mut surface_resized,
            }),
            world,
            &self.shared,
            &view,
            &settings,
        );
        cpu.step("prepare results");
        if borrowed.is_none() {
            (uniform.surface_map, uniform.surface_open) =
                self.surface_weather.frame_params(view.origin);
        }
        let gi_atlas_moved = gi_done.atlas_moved;
        self.apply_shared_prepares(world, &mut cpu, (!is_extra).then_some(gi_done.ms));
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
        let lights_and_capsules: Vec<GpuPointLight>;
        let light_bytes: &[u8] = if view.point_lights.is_empty() && view.capsules.is_empty() {
            &[0u8; 32]
        } else if view.capsules.is_empty() {
            bytemuck::cast_slice(&view.point_lights)
        } else {
            lights_and_capsules = [view.point_lights.as_slice(), &view.capsules].concat();
            bytemuck::cast_slice(&lights_and_capsules)
        };
        let occlusion_resized =
            view.sky_occlusion.enabled && view.sky_occlusion.map_size != self.shadow.occlusion_size;
        if occlusion_resized {
            self.shadow.occlusion_size = view.sky_occlusion.map_size;
            self.shadow.on_top[SKY_OCCLUSION_VIEW] = ShadowTiles::ALL;
            (self.shadow.occlusion_texture, self.shadow.occlusion) = create_occlusion_target(
                &self.shared,
                self.shadow.occlusion_size,
                wgpu::TextureUsages::COPY_DST,
            );
            self.sky_camera.set_inputs(
                &self.shared,
                &self.frame_buffer,
                &self.shadow.occlusion,
                self.shadow.occlusion_size,
            );
        }
        let lights_moved = self.lights.write(&self.shared.device, queue, light_bytes);
        cpu.step("volumetrics prepare");
        let mut fog_moved =
            self.volumetrics
                .prepare(world, &self.shared, &view, &settings, uniform.volumetrics);
        fog_moved |= self
            .volumetrics
            .aerial
            .prepare(&self.shared, &view, uniform.aerial);
        cpu.step("shadow views");
        if lights_moved
            || occlusion_resized
            || fog_moved
            || surface_resized
            || gi_atlas_moved
            || insets_resized
        {
            self.rebuild_frame_groups();
        }
        let queue = &self.shared.queue;
        if view.sun_shadows {
            for i in 0..view.cascades.count as usize {
                let texel =
                    2.0 * view.cascades.half_extent[i] / view.cascades.map_size.max(1) as f32;
                let u = ShadowViewUniform {
                    view_proj: view.cascades.view_proj[i].to_cols_array_2d(),
                    wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                    time: [view.time, view.real_time, view.delta, sway_rest(texel)],
                    camera_frac: uniform.camera_world_frac,
                    ..shadow_plants(&uniform, texel)
                };
                queue.write_buffer(&self.shadow.view_buffers[i], 0, bytemuck::bytes_of(&u));
            }
        }
        if view.sun_shadows && view.far_shadow.enabled {
            let u = ShadowViewUniform {
                view_proj: view.far_shadow.view_proj.to_cols_array_2d(),
                wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                time: [
                    view.time,
                    view.real_time,
                    view.delta,
                    sway_rest(view.far_shadow.texel()),
                ],
                camera_frac: uniform.camera_world_frac,
                ..shadow_plants(&uniform, view.far_shadow.texel())
            };
            queue.write_buffer(
                &self.shadow.view_buffers[FAR_SHADOW_VIEW],
                0,
                bytemuck::bytes_of(&u),
            );
        }
        for (i, inset) in view.shadow_insets.iter().enumerate() {
            let u = ShadowViewUniform {
                view_proj: inset.view_proj.to_cols_array_2d(),
                wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                time: [view.time, view.real_time, view.delta, 0.0],
                camera_frac: uniform.camera_world_frac,
                ..shadow_plants(&uniform, inset.texel(self.shadow.insets.size))
            };
            queue.write_buffer(
                &self.shadow.insets.view_buffers[i],
                0,
                bytemuck::bytes_of(&u),
            );
        }
        if view.sky_occlusion.enabled {
            let texel = view.sky_occlusion.extent / view.sky_occlusion.map_size.max(1) as f32;
            let u = ShadowViewUniform {
                view_proj: view.sky_occlusion.view_proj.to_cols_array_2d(),
                wind: [view.wind.x, view.wind.y, view.wind.z, 0.0],
                time: [view.time, view.real_time, view.delta, sway_rest(texel)],
                camera_frac: uniform.camera_world_frac,
                ..shadow_plants(&uniform, texel)
            };
            queue.write_buffer(
                &self.shadow.view_buffers[SKY_OCCLUSION_VIEW],
                0,
                bytemuck::bytes_of(&u),
            );
        }
        let sun_camera = Vec3::from_slice(&uniform.sun_camera[..3]);
        self.write_post_params(&settings, &view, taa, sun_camera);

        // Prepare the features that don't prepare shared.
        for (f, _) in self
            .features
            .iter_mut()
            .zip(&self.prepare_shared)
            .filter(|(_, shared)| !**shared)
        {
            cpu.step(format!("prepare {}", f.name()));
            f.prepare(world, &self.shared, &view, &settings);
            cpu.inner(std::mem::take(
                &mut *self
                    .shared
                    .prepare_steps
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()),
            ));
        }
        cpu.step("cloud shadows");

        // Cloud shadows: the first feature that asks draws the map in `compute`. The frame
        // uniform written above has them off; turn them on now that the area is known.
        let cloud_writer = if settings.cloud_shadows && borrowed.is_none() {
            self.features
                .iter()
                .enumerate()
                .find_map(|(i, f)| f.cloud_shadows().map(|r| (i, r)))
        } else {
            None
        };
        let cloud_area = cloud_writer
            .map(|(_, request)| CloudShadowArea::place(&request, view.origin, view.sun_dir));
        if !is_extra {
            self.last_cloud_area = cloud_area;
        }
        // A mirror borrowing the main view's maps shows its cloud shadows where they lie.
        let shown_area = cloud_area.or(borrowed.as_ref().and_then(|b| b.cloud_area));
        if let Some(area) = shown_area {
            if cloud_area.is_some()
                && self
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
            mirror,
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

    /// What the shared prepares left (`crate::prepare`), applied in feature order: their
    /// `PrepareOut`s, and each one's time as `render.cpu.prepare <name>` (`render.cpu.probe gi`
    /// for the probe GI's gather, `gi_ms`) and as an inner step of "prepare side by side",
    /// followed by the steps it timed itself. `render.prepare_shared_ms` is their sum: against
    /// the wall time of "prepare side by side", what running them side by side saved.
    fn apply_shared_prepares(
        &mut self,
        world: &mut World,
        cpu: &mut crate::perf::CpuSteps,
        gi_ms: Option<f32>,
    ) {
        let mut inner = Vec::new();
        let mut metrics = Vec::new();
        let mut sum = 0.0;
        if let Some(ms) = gi_ms {
            self.gi_ms = ms;
            inner.push(("probe gi", ms));
            metrics.push(("render.cpu.probe gi".to_string(), ms));
            sum += ms;
        }
        for ((f, out), _) in self
            .features
            .iter()
            .zip(&mut self.prepare_outs)
            .zip(&self.prepare_shared)
            .filter(|(_, shared)| **shared)
        {
            let name = format!("prepare {}", f.name());
            inner.push((crate::prepare::static_label(&name), out.ms));
            metrics.push((format!("render.cpu.{name}"), out.ms));
            sum += out.ms;
            inner.extend(out.apply(world));
        }
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            for (key, ms) in metrics {
                d.set(&key, ms as f64);
            }
            d.set("render.prepare_shared_ms", sum as f64);
        }
        cpu.inner(inner);
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
            mirror,
            composites,
        } = frame;
        let recording = Instant::now();
        // CPU time this thread spends, against the wall time: what is left is waiting
        // (for the GPU, the window, a lock, or a core).
        let thread_cpu = cpu_time::ThreadTime::try_now().ok();
        let mut cpu = crate::perf::CpuSteps::resume(cpu);
        // The features' world-free preparation.
        cpu.step("prepare on the render thread");
        let mut feature_diagnostics = Diagnostics::default();
        for f in &mut self.features {
            f.prepare_frame(&self.shared, &view, &settings, &mut feature_diagnostics);
        }
        // The canopy map as the meshes left it this frame (other views read the main view's).
        {
            let canopy = self
                .shared
                .canopy
                .uniform(view.origin, settings.canopy_strength);
            let offset = std::mem::offset_of!(crate::frame::FrameUniform, canopy) as u64;
            let bytes = bytemuck::bytes_of(&canopy);
            self.shared
                .queue
                .write_buffer(&self.frame_buffer, offset, bytes);
            self.shared
                .queue
                .write_buffer(&self.overlay_frame_buffer, offset, bytes);
        }
        if !extra {
            for f in &self.features {
                for (map, asked) in self.static_requests.iter_mut().enumerate() {
                    *asked |= f.shadow_static_changed(map);
                }
            }
        }
        cpu.inner(std::mem::take(
            &mut *self
                .shared
                .prepare_steps
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        ));
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
            let perf = &mut self.perf;
            f.compute_parts(&mut encoder, &ctx, &mut |encoder, name| {
                perf.pass(&mut cpu, encoder, name)
            });
        }

        // Environment map: draw the sky, then filter it for reflections and diffuse light. The
        // passes drawing it read last frame's filtered maps. Every `environment_interval`
        // frames, and at once after a cut, when the sun turns or the camera travels far.
        let env_now = EnvDrawn {
            frame: self.frame_index,
            sun_dir: view.sun_dir,
            origin: view.origin,
        };
        // A mirror borrowing the main view's maps lights with its environment.
        let env_due = self.borrowed.is_none()
            && self.env_rendered.is_none_or(|last| {
                last.due(&env_now, settings.environment_interval, view.history_valid)
            });
        if env_due {
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
            self.perf.pass(&mut cpu, &mut encoder, "environment filter");
            self.environment.filter(&mut encoder);
            self.env_rendered = Some(env_now);
        }
        let frame_group = &self.frame_group;
        // The frame so far is finished on a worker while the rest is recorded (`finish.rs`).
        let parallel = settings.parallel_encoding;
        let mut part = crate::finish::PARTS[0];
        if parallel {
            self.finisher
                .hand_off(device, &mut encoder, &mut part, "shadows");
        }

        // Shadows: each map holds its static image (drawn again only when due, `ShadowCache`)
        // with what moves drawn on top; the tiles the last frame's moving casters drew into
        // are put back from the image first. Extra views draw everything straight in.
        let mut copied = PutBackCount::default();
        let moving = if extra {
            Default::default()
        } else {
            let (moving, by_feature) = moving_tiles(&self.features, &ctx);
            self.moving_tiles = by_feature;
            moving
        };
        if view.sun_shadows {
            // Under the profiler the moving casters are drawn feature by feature, each with
            // its own row ("shadows: grass"), after every map's static image is in.
            let split = engine_core::profile::enabled() && !extra && self.shadow.statics.is_some();
            let whole = if split {
                "shadows: still images"
            } else {
                "shadows"
            };
            self.perf.pass(&mut cpu, &mut encoder, whole);
            let size = self.shadow.size;
            let occlusion_size = self.shadow.occlusion_size;
            // The map size changed (the cache redraws every map then).
            if !extra
                && self
                    .shadow
                    .statics
                    .as_ref()
                    .is_some_and(|s| s.texture.width() != size)
            {
                self.shadow.statics = Some(StaticShadows::new(&self.shared, size, occlusion_size));
            }
            let count = (view.cascades.count as usize).min(MAX_CASCADES);
            let maps: Vec<(usize, usize, bool)> = (0..count)
                .map(|i| (i, i, view.cascades.static_redraw[i]))
                .chain(view.far_shadow.enabled.then_some((
                    FAR_SHADOW_MAP,
                    FAR_SHADOW_LAYER,
                    view.far_shadow.static_redraw,
                )))
                .collect();
            let shadow = &self.shadow;
            let statics = shadow.statics.as_ref().filter(|_| !extra);
            let put_back = PutBack::of(&settings);
            let mut on_top = shadow.on_top;
            for &(map, layer, redraw) in &maps {
                let group = &shadow.view_bind_groups[map];
                let live = &shadow.layer_views[layer];
                let draw_moving = |pass: &mut wgpu::RenderPass<'_>| {
                    pass.set_bind_group(0, group, &[]);
                    for f in &self.features {
                        if map == FAR_SHADOW_MAP {
                            f.draw_far_shadow(pass, &ctx);
                        } else {
                            f.draw_shadow(pass, map, &ctx);
                        }
                    }
                };
                let draw_still = |pass: &mut wgpu::RenderPass<'_>| {
                    pass.set_bind_group(0, group, &[]);
                    for f in &self.features {
                        f.draw_shadow_static(pass, map, &ctx);
                    }
                };
                let Some(statics) = statics else {
                    let mut pass = depth_pass(&mut encoder, "shadow map", live, true);
                    draw_still(&mut pass);
                    draw_moving(&mut pass);
                    continue;
                };
                let joins = !redraw && self.features.iter().any(|f| f.shadow_joins(map));
                if redraw {
                    let mut pass = depth_pass(
                        &mut encoder,
                        "shadow static image",
                        &statics.layers[layer],
                        true,
                    );
                    draw_still(&mut pass);
                } else if joins {
                    // Casters that turned still go into the image on top of what it holds.
                    let mut pass = depth_pass(
                        &mut encoder,
                        "shadow static join",
                        &statics.layers[layer],
                        false,
                    );
                    pass.set_bind_group(0, group, &[]);
                    for f in &self.features {
                        f.draw_shadow_join(&mut pass, map, &ctx);
                    }
                }
                // The map is its image again where the last frame's casters were, then this
                // frame's are drawn: nothing is touched where nothing moves.
                let moving = moving[map];
                let joined = if joins {
                    join_tiles(&self.features, map)
                } else {
                    ShadowTiles::NONE
                };
                let restore = put_back.plan(&mut on_top[map], moving, redraw, joined);
                copied.count(restore);
                if restore == Restore::Whole {
                    copy_layer(
                        &mut encoder,
                        &statics.texture,
                        &shadow.texture,
                        layer as u32,
                    );
                }
                let tiles = match restore {
                    Restore::Tiles(tiles) => Some(tiles),
                    _ => None,
                };
                if tiles.is_some() || (!split && !moving.is_empty()) {
                    let mut pass = depth_pass(&mut encoder, "shadow map", live, false);
                    if let Some(tiles) = tiles {
                        restore_tiles(
                            &mut pass,
                            statics,
                            &statics.restore_layers[layer],
                            tiles,
                            size,
                        );
                    }
                    if !split {
                        draw_moving(&mut pass);
                    }
                }
            }
            if split {
                for f in &self.features {
                    self.perf
                        .pass(&mut cpu, &mut encoder, format!("shadows: {}", f.name()));
                    for &(map, layer, _) in &maps {
                        if f.shadow_moving(map, &ctx).is_empty() {
                            continue;
                        }
                        let live = &shadow.layer_views[layer];
                        let mut pass = depth_pass(&mut encoder, "shadow map", live, false);
                        pass.set_bind_group(0, &shadow.view_bind_groups[map], &[]);
                        if map == FAR_SHADOW_MAP {
                            f.draw_far_shadow(&mut pass, &ctx);
                        } else {
                            f.draw_shadow(&mut pass, map, &ctx);
                        }
                    }
                }
                self.perf
                    .pass(&mut cpu, &mut encoder, "shadows: people's own maps");
            }
            // People's inset shadow maps: only what the features that draw people put in.
            let insets = &self.shadow.insets;
            for i in 0..view.shadow_insets.len() {
                let mut pass =
                    depth_pass(&mut encoder, "inset shadow", &insets.layer_views[i], true);
                pass.set_bind_group(0, &insets.view_bind_groups[i], &[]);
                for f in &self.features {
                    f.draw_inset_shadow(&mut pass, i, &ctx);
                }
            }
            let kept = statics.is_some();
            if kept {
                self.shadow.on_top = on_top;
            }
        }

        // Sky occlusion: the highest surface around the camera, seen from above. Cleared to
        // 1 (nothing) when disabled so stale data never darkens the sky light.
        self.perf.pass(&mut cpu, &mut encoder, "sky occlusion");
        if view.sky_occlusion.enabled {
            let shadow = &mut self.shadow;
            if !extra
                && shadow
                    .statics
                    .as_ref()
                    .is_some_and(|s| s.occlusion_size != shadow.occlusion_size)
            {
                let statics = shadow.statics.as_mut().expect("checked");
                statics.occlusion = create_occlusion_target(
                    &self.shared,
                    shadow.occlusion_size,
                    wgpu::TextureUsages::COPY_SRC,
                );
                statics.restore_occlusion = restore_group(&self.shared, &statics.occlusion.1);
                statics.occlusion_size = shadow.occlusion_size;
            }
            let shadow = &self.shadow;
            let group = &shadow.view_bind_groups[SKY_OCCLUSION_VIEW];
            let statics = shadow.statics.as_ref().filter(|_| !extra);
            let mut occlusion_on_top = None;
            match statics {
                // Without static images (an extra view, or no sun): everything, straight in.
                None => {
                    let mut pass =
                        depth_pass(&mut encoder, "sky occlusion", &shadow.occlusion, true);
                    pass.set_bind_group(0, group, &[]);
                    for f in &self.features {
                        f.draw_shadow_static(&mut pass, SKY_OCCLUSION_VIEW, &ctx);
                        f.draw_shadow(&mut pass, SKY_OCCLUSION_VIEW, &ctx);
                    }
                }
                Some(statics) => {
                    let redraw = view.sky_occlusion.static_redraw;
                    let joins = !redraw
                        && self
                            .features
                            .iter()
                            .any(|f| f.shadow_joins(SKY_OCCLUSION_VIEW));
                    if redraw {
                        let mut pass = depth_pass(
                            &mut encoder,
                            "sky occlusion static image",
                            &statics.occlusion.1,
                            true,
                        );
                        pass.set_bind_group(0, group, &[]);
                        for f in &self.features {
                            f.draw_shadow_static(&mut pass, SKY_OCCLUSION_VIEW, &ctx);
                        }
                    } else if joins {
                        let mut pass = depth_pass(
                            &mut encoder,
                            "sky occlusion static join",
                            &statics.occlusion.1,
                            false,
                        );
                        pass.set_bind_group(0, group, &[]);
                        for f in &self.features {
                            f.draw_shadow_join(&mut pass, SKY_OCCLUSION_VIEW, &ctx);
                        }
                    }
                    let moving = moving[SKY_OCCLUSION_VIEW];
                    let mut on_top = shadow.on_top[SKY_OCCLUSION_VIEW];
                    let joined = if joins {
                        join_tiles(&self.features, SKY_OCCLUSION_VIEW)
                    } else {
                        ShadowTiles::NONE
                    };
                    let restore = PutBack::of(&settings).plan(&mut on_top, moving, redraw, joined);
                    copied.count(restore);
                    if restore == Restore::Whole {
                        copy_layer(
                            &mut encoder,
                            &statics.occlusion.0,
                            &shadow.occlusion_texture,
                            0,
                        );
                    }
                    let tiles = match restore {
                        Restore::Tiles(tiles) => Some(tiles),
                        _ => None,
                    };
                    if tiles.is_some() || !moving.is_empty() {
                        let mut pass =
                            depth_pass(&mut encoder, "sky occlusion", &shadow.occlusion, false);
                        if let Some(tiles) = tiles {
                            restore_tiles(
                                &mut pass,
                                statics,
                                &statics.restore_occlusion,
                                tiles,
                                shadow.occlusion_size,
                            );
                        }
                        pass.set_bind_group(0, group, &[]);
                        for f in &self.features {
                            f.draw_shadow(&mut pass, SKY_OCCLUSION_VIEW, &ctx);
                        }
                    }
                    occlusion_on_top = Some(on_top);
                }
            }
            if let Some(on_top) = occlusion_on_top {
                self.shadow.on_top[SKY_OCCLUSION_VIEW] = on_top;
            }
        } else if self.borrowed.is_none() {
            // Cleared to 1 (nothing) when disabled so stale data never darkens the sky light.
            depth_pass(&mut encoder, "sky occlusion", &self.shadow.occlusion, true);
            self.shadow.on_top[SKY_OCCLUSION_VIEW] = ShadowTiles::ALL;
        }
        if !extra {
            self.put_back = copied;
            self.put_back_total.layers = self.put_back_total.layers.wrapping_add(copied.layers);
            self.put_back_total.tiles = self.put_back_total.tiles.wrapping_add(copied.tiles);
        }
        if parallel {
            self.finisher
                .hand_off(device, &mut encoder, &mut part, "prepass");
        }
        self.sky_camera.encode(&mut encoder);
        if !extra {
            self.perf.pass(&mut cpu, &mut encoder, "probe gi");
            self.probe_gi.encode(&mut encoder, frame_group);
        }

        // Depth prepass: depth, normals and motion of opaque geometry. With the frame profiler
        // on, each feature's prepass draws get a pass of their own (`render.gpu.prepass:
        // <feature>`), as the scene's do.
        if engine_core::profile::enabled() {
            for (i, f) in self.features.iter().enumerate() {
                self.perf
                    .pass(&mut cpu, &mut encoder, format!("prepass: {}", f.name()));
                let mut pass =
                    prepass_pass(&mut encoder, &self.targets, &self.prepass_group, i == 0);
                f.draw_prepass(&mut pass, &ctx);
            }
            if self.features.is_empty() {
                self.perf.pass(&mut cpu, &mut encoder, "prepass");
                drop(prepass_pass(
                    &mut encoder,
                    &self.targets,
                    &self.prepass_group,
                    true,
                ));
            }
        } else {
            self.perf.pass(&mut cpu, &mut encoder, "prepass");
            let mut pass = prepass_pass(&mut encoder, &self.targets, &self.prepass_group, true);
            for f in &self.features {
                f.draw_prepass(&mut pass, &ctx);
                pass.set_bind_group(0, &self.prepass_group, &[]);
            }
        }
        // Prepasses of the features' own (the meshes' stored material, mesh_once.rs).
        let own_targets = crate::features::PrepassTargets {
            normal: &self.targets.prepass_normal.view,
            motion: &self.targets.motion.view,
            depth: &self.targets.depth.view,
            size: (self.targets.width, self.targets.height),
            group: &self.prepass_group,
        };
        for f in self.features.iter_mut() {
            if engine_core::profile::enabled() && f.has_prepass_own() {
                self.perf.pass(
                    &mut cpu,
                    &mut encoder,
                    format!("prepass: {} stored", f.name()),
                );
            }
            f.prepass_own(&mut encoder, &own_targets, false, &ctx);
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
            drop(pass);
            for f in self.features.iter_mut() {
                f.prepass_own(&mut encoder, &own_targets, true, &ctx);
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
        if parallel {
            self.finisher
                .hand_off(device, &mut encoder, &mut part, "lighting");
        }
        // Volumetric fog: needs the shadow cascades, the sky occlusion map, the sky light and
        // the prepass depth; shading reads it from frame binding 19 (a stand-in in the prepass
        // group).
        self.perf.pass(&mut cpu, &mut encoder, "volumetric fog");
        self.volumetrics.encode(&mut encoder, &self.prepass_group);
        // The haze and atmosphere in front of distant surfaces: needs the far shadow map and
        // the cloud shadows; shading reads it from frame binding 24 (a stand-in in the prepass
        // group).
        self.perf.pass(&mut cpu, &mut encoder, "aerial perspective");
        self.volumetrics
            .aerial
            .encode(&mut encoder, &self.prepass_group);
        if settings.ambient_occlusion {
            self.perf.pass(&mut cpu, &mut encoder, "ambient occlusion");
            self.effects
                .encode_ao(&mut encoder, &self.targets, &self.prepass_group);
        }
        // The sun, the sky's cover and the probes' light for every pixel the prepass drew,
        // once a pixel (shading loads them: lib_frame.wgsl `screen_light`).
        if self.targets.screen_light {
            self.perf.pass(&mut cpu, &mut encoder, "screen light");
            self.effects
                .encode_light(&mut encoder, &self.targets, &self.light_group);
        }
        // Which lamps can reach each tile of the screen, from the prepass's depth (shading
        // goes through those instead of every lamp: lib_frame.wgsl `lamp_mask`).
        if settings.trial_cuts & crate::settings::trial_cut::TILED_LAMPS != 0 {
            self.perf.pass(&mut cpu, &mut encoder, "lamp tiles");
            self.effects
                .encode_lamp_tiles(&mut encoder, &self.targets, &self.light_group);
        }
        if ssr {
            // What screen-space reflections can't see, traced against probe GI's scene.
            if !extra {
                self.perf.pass(&mut cpu, &mut encoder, "world reflections");
                if settings.world_reflections
                    && self.probe_gi.encode_reflections(
                        &self.shared.device,
                        &mut encoder,
                        frame_group,
                        &self.targets,
                        settings.trial_cuts & crate::settings::trial_cut::REFLECTIONS_ALTERNATE
                            != 0,
                    )
                {
                    self.world_reflection_written = true;
                } else if self.world_reflection_written {
                    self.world_reflection_written = false;
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("clear world reflections"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.targets.world_reflection.view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                }
            }
            self.perf.pass(&mut cpu, &mut encoder, "reflections");
            self.effects
                .encode_ssr(&mut encoder, &self.targets, &self.prepass_group);
        }
        if gi {
            self.perf.pass(&mut cpu, &mut encoder, "bounce light");
            self.effects.encode_gi(
                &self.shared,
                &mut encoder,
                &self.targets,
                &self.prepass_group,
                settings.trial_cuts & crate::settings::trial_cut::BOUNCE_HALF != 0,
            );
        } else {
            self.effects.skip_gi();
        }

        // Work that reads the prepass and lighting maps (the strand renderer's hair).
        self.perf.pass(&mut cpu, &mut encoder, "before scene");
        for f in self.features.iter_mut() {
            f.before_scene(&mut encoder, &self.targets.motion.view, &ctx);
        }

        if parallel {
            self.finisher
                .hand_off(device, &mut encoder, &mut part, "scene");
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

        if parallel {
            self.finisher
                .hand_off(device, &mut encoder, &mut part, "transparent");
        }
        // Transparent. With the frame profiler on, each feature's see-through draws get a pass
        // of their own (`render.gpu.transparent: <feature>`), as the scene's do.
        if engine_core::profile::enabled() {
            for f in &self.features {
                self.perf
                    .pass(&mut cpu, &mut encoder, format!("transparent: {}", f.name()));
                let mut pass = transparent_pass(&mut encoder, &self.targets, frame_group);
                f.draw(RenderPhase::Transparent, &mut pass, &ctx);
            }
        } else {
            self.perf.pass(&mut cpu, &mut encoder, "transparent");
            let mut pass = transparent_pass(&mut encoder, &self.targets, frame_group);
            for f in &self.features {
                f.draw(RenderPhase::Transparent, &mut pass, &ctx);
                pass.set_bind_group(0, frame_group, &[]);
            }
        }
        if reactive > 0.0 {
            self.effects.encode_reactive(&mut encoder, &self.targets);
        }

        if parallel {
            self.finisher
                .hand_off(device, &mut encoder, &mut part, "post");
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

        // Bloom (not in a planar reflection's view: its picture is the HDR image above).
        self.perf.pass(&mut cpu, &mut encoder, "bloom");
        let levels = if mirror { 0 } else { self.targets.bloom.len() };
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

        // Tonemap (not in a planar reflection's view either).
        self.perf.pass(&mut cpu, &mut encoder, "tonemap");
        if !mirror {
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

        // Overlays (UI), with unjittered matrices: the main view's, and in extra views those
        // that belong to the scene (`RenderFeature::overlay_in_extra_views`).
        let ctx = FrameContext {
            frame_group: &self.overlay_group,
            ..ctx
        };
        for f in &mut self.features {
            if extra && !f.overlay_in_extra_views() {
                continue;
            }
            self.perf
                .pass(&mut cpu, &mut encoder, format!("overlay {}", f.name()));
            f.overlay(&mut encoder, &ctx);
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
        let mut back = RenderReturn {
            diagnostics: feature_diagnostics,
            ..Default::default()
        };
        // wgpu validates and encodes every recorded pass into the backend's command buffer
        // here, not as the passes are recorded.
        cpu.step("finish commands");
        let finishing = Instant::now();
        let commands = encoder.finish();
        let finish_ms = finishing.elapsed().as_secs_f64() * 1000.0;
        cpu.step("wait for the finishing threads");
        let mut buffers = self.finisher.collect();
        buffers.push(commands);
        cpu.step("submit");
        let submitting = Instant::now();
        let side = self.shared.take_side_commands();
        self.shared
            .queue
            .submit(side.commands.into_iter().chain(buffers));
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
            if !extra {
                self.probe_gi.publish(d);
            }
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
            // Bits of depth the shadow maps were made with (`shadow_depth_bits` at startup).
            d.set(
                "render.shadow_depth_bits",
                match self.shared.formats.shadow {
                    wgpu::TextureFormat::Depth16Unorm => 16.0,
                    _ => 32.0,
                },
            );
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
            d.set(
                "render.shadow_layers_copied",
                stats.shadow_layers_copied as f64,
            );
            d.set(
                "render.shadow_tiles_restored",
                stats.shadow_tiles_restored as f64,
            );
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
            if let Some(ev) = self.meter.measured_highlights_ev100 {
                d.set("render.ev100_highlights", ev as f64);
            }
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
        // Frames, not views: every view of a frame shares its number, so with a mirror the
        // main view still steps through the whole jitter sequence and the environment map
        // keeps its interval.
        if !extra {
            self.frame_index += 1;
        }
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
            // Images uploaded already compressed from the disk cache, the GPU memory that
            // spared at the peak, and images stored there for the next run.
            d.set("render.image_cache_hits", image_stats.cache_hits as f64);
            d.set(
                "render.image_cache_saved_mb",
                image_stats.cache_bytes_saved as f64 / (1024.0 * 1024.0),
            );
            d.set("render.image_cache_stores", image_stats.cache_stores as f64);
            self.perf.note_for_census(&d);
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

    /// `taa`: temporal anti-aliasing runs this frame (its softness is sharpened back);
    /// `sun_camera`: the sun's color times illuminance reaching the camera (lux).
    fn write_post_params(
        &self,
        settings: &RenderSettings,
        view: &ViewData,
        taa: bool,
        sun_camera: Vec3,
    ) {
        let tonemapper = match settings.tonemapper {
            Tonemapper::AgX => 0.0,
            Tonemapper::Aces => 1.0,
            Tonemapper::Reinhard => 2.0,
            Tonemapper::None => 3.0,
            Tonemapper::AgXPunchy => 4.0,
            Tonemapper::Neutral => 5.0,
        };
        let levels = self.targets.bloom.len().max(1) as f32;
        // Stretched by the tonemap (temporal upscaling already rebuilt the viewport's size).
        let post = self.targets.post();
        let upscaled = view.viewport_size != UVec2::new(post.width, post.height);
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
            params4: [
                if upscaled { 1.0 } else { 0.0 },
                view.viewport_size.x.max(1) as f32 / view.viewport_size.y.max(1) as f32,
                (view.fov_y * 0.5).tan(),
                0.0,
            ],
            sun: [0.0; 4],
            sun_light: [0.0; 4],
        };
        let u = sun_glare_params(u, settings, view, sun_camera);
        self.shared
            .queue
            .write_buffer(&self.post.params, 0, bytemuck::bytes_of(&u));
        // The first bloom downsample reads the scene and limits each pixel's brightness.
        if let Some(first) = self.post.bloom_params.first() {
            let src = self.targets.post();
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
            shadow_layers_copied: self.put_back.layers,
            shadow_tiles_restored: self.put_back.tiles,
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
        // What the shadow maps hold on top of their static images, in tiles of 256 a map (the
        // cascades, the sky occlusion map, the far map), and whose moving casters put it there.
        let moving: serde_json::Map<String, serde_json::Value> = self
            .moving_tiles
            .iter()
            .filter_map(|(i, counts)| {
                Some((self.features.get(*i)?.name().to_string(), json!(counts)))
            })
            .collect();
        json!({
            "volumetric_fog": self.volumetrics.describe(),
            "shadow_maps": {
                "tiles_on_top": self.shadow.on_top.map(|t| t.count()),
                "moving_tiles": moving,
                "layers_copied": self.put_back.layers,
                "tiles_restored": self.put_back.tiles,
                "layers_copied_total": self.put_back_total.layers,
                "tiles_restored_total": self.put_back_total.tiles,
            },
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

/// The depth prepass over the normal and motion targets and the depth, cleared or continued.
fn prepass_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    targets: &Targets,
    prepass_group: &wgpu::BindGroup,
    clear: bool,
) -> wgpu::RenderPass<'e> {
    let color = wgpu::Operations {
        load: if clear {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        } else {
            wgpu::LoadOp::Load
        },
        store: wgpu::StoreOp::Store,
    };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("prepass"),
        color_attachments: &[
            Some(wgpu::RenderPassColorAttachment {
                view: &targets.prepass_normal.view,
                depth_slice: None,
                resolve_target: None,
                ops: color,
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &targets.motion.view,
                depth_slice: None,
                resolve_target: None,
                ops: color,
            }),
        ],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &targets.depth.view,
            depth_ops: Some(wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(0.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        ..Default::default()
    });
    pass.set_bind_group(0, prepass_group, &[]);
    pass
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

/// The see-through pass: the scene pass's targets, continued.
fn transparent_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    targets: &Targets,
    frame_group: &wgpu::BindGroup,
) -> wgpu::RenderPass<'e> {
    scene_pass(encoder, targets, frame_group, false)
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

/// The depth format of the sun's shadow maps, their static images and people's insets
/// (`RenderSettings::shadow_depth_bits`).
fn shadow_depth_format(settings: &RenderSettings) -> wgpu::TextureFormat {
    if settings.shadow_depth_bits == 16 {
        wgpu::TextureFormat::Depth16Unorm
    } else {
        wgpu::TextureFormat::Depth32Float
    }
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("shadow array"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let layer_views = layer_views(&texture);
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
    let occlusion = create_occlusion_target(shared, occlusion_size, wgpu::TextureUsages::COPY_DST);
    ShadowResources {
        insets: InsetShadows::new(shared, 16),
        texture,
        array_view,
        layer_views,
        view_buffers,
        view_bind_groups,
        compare_sampler,
        size,
        occlusion: occlusion.1,
        occlusion_texture: occlusion.0,
        occlusion_size,
        statics: None,
        on_top: [ShadowTiles::ALL; SHADOW_MAPS],
    }
}

/// The sky occlusion map's depth texture (`extra` usage: copies into or out of it).
fn create_occlusion_target(
    shared: &GpuShared,
    size: u32,
    extra: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | extra,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Group 0 bind groups for every kind of pass.
struct FrameGroups {
    /// Scene passes.
    scene: wgpu::BindGroup,
    /// Features' `compute`: a stand-in for the cloud shadow map.
    compute: wgpu::BindGroup,
    /// Prepass and screen-space effects: stand-ins for the maps they write.
    prepass: wgpu::BindGroup,
    /// The screen light pass: the prepass's depth and normals, stand-ins for the rest.
    light: wgpu::BindGroup,
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
        gi_atlas: &wgpu::TextureView,
        borrowed: Option<&SharedMaps>,
    ) -> Self {
        let lighting = match borrowed {
            Some(b) => LightingMaps {
                shadows: &b.shadows,
                occlusion: &b.occlusion,
                env_specular: &b.env_specular,
                env_irradiance: &b.env_irradiance,
                surface: &b.surface,
            },
            None => LightingMaps {
                shadows: &shadow.array_view,
                occlusion: &shadow.occlusion,
                env_specular: &env.specular,
                env_irradiance: &env.irradiance.view,
                surface,
            },
        };
        let shown_clouds = borrowed.map_or(&clouds.view, |b| &b.clouds);
        let group = |uniform: &wgpu::Buffer, maps: &ScreenMaps, clouds: &wgpu::TextureView| {
            create_frame_group(
                shared, uniform, lights, shadow, env, &lighting, sky_camera, atmosphere, maps,
                clouds, gi_atlas,
            )
        };
        Self {
            scene: group(frame[0], maps, shown_clouds),
            compute: group(frame[0], maps, &clouds.stand_in),
            prepass: group(frame[0], stand_ins, shown_clouds),
            light: group(frame[0], &maps.for_light(stand_ins), shown_clouds),
            overlay: group(frame[1], maps, shown_clouds),
        }
    }
}

/// The lighting maps a view's frame group binds: its own, or the main view's (`SharedMaps`).
struct LightingMaps<'a> {
    shadows: &'a wgpu::TextureView,
    occlusion: &'a wgpu::TextureView,
    env_specular: &'a wgpu::TextureView,
    env_irradiance: &'a wgpu::TextureView,
    surface: &'a wgpu::TextureView,
}

#[allow(clippy::too_many_arguments)]
fn create_frame_group(
    shared: &GpuShared,
    frame: &wgpu::Buffer,
    lights: &GrowBuffer,
    shadow: &ShadowResources,
    env: &EnvironmentMaps,
    lighting: &LightingMaps<'_>,
    sky_camera: &crate::sky_occlusion::SkyCamera,
    atmosphere: &AtmosphereLuts,
    maps: &ScreenMaps,
    clouds: &wgpu::TextureView,
    gi_atlas: &wgpu::TextureView,
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
                resource: view(lighting.shadows),
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
                resource: view(lighting.env_specular),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&shared.linear_repeat),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: view(lighting.occlusion),
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
                resource: view(lighting.env_irradiance),
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
                resource: view(lighting.surface),
            },
            wgpu::BindGroupEntry {
                binding: 22,
                resource: view(clouds),
            },
            wgpu::BindGroupEntry {
                binding: 23,
                resource: view(gi_atlas),
            },
            wgpu::BindGroupEntry {
                binding: 24,
                resource: view(&maps.aerial),
            },
            wgpu::BindGroupEntry {
                binding: 25,
                resource: view(sky_camera.cover_view()),
            },
            wgpu::BindGroupEntry {
                binding: 26,
                resource: view(shared.canopy.view()),
            },
            wgpu::BindGroupEntry {
                binding: 27,
                resource: view(&shadow.insets.array_view),
            },
            wgpu::BindGroupEntry {
                binding: 28,
                resource: view(&maps.light[0]),
            },
            wgpu::BindGroupEntry {
                binding: 29,
                resource: view(&maps.light[1]),
            },
            wgpu::BindGroupEntry {
                binding: 30,
                resource: view(&maps.light[2]),
            },
            wgpu::BindGroupEntry {
                binding: 31,
                resource: view(&maps.lamp_tiles),
            },
            wgpu::BindGroupEntry {
                binding: 32,
                resource: view(&maps.light[3]),
            },
            wgpu::BindGroupEntry {
                binding: 33,
                resource: view(&maps.light[4]),
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
    let pipeline_layout = shared.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    shared.render_pipeline(&wgpu::RenderPipelineDescriptor {
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

/// The sun's glare in the tonemap (`sun_glare` in post.wgsl): where the sun is in the image and
/// how bright, when it is in front of a perspective camera and `RenderSettings::sun_glare` is on.
fn sun_glare_params(
    mut u: PostUniform,
    settings: &RenderSettings,
    view: &ViewData,
    sun_camera: Vec3,
) -> PostUniform {
    let radius = view.sun_angular_radius;
    if settings.sun_glare <= 0.0 || view.orthographic || view.fov_y <= 0.0 || radius <= 0.0 {
        return u;
    }
    // A direction projects like a point at infinity.
    let clip = view.unjittered_view_proj * view.sun_dir.extend(0.0);
    if clip.w <= 1e-4 {
        return u;
    }
    let ndc = clip.truncate().truncate() / clip.w;
    let uv = [ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5];
    if !(-0.1..=1.1).contains(&uv[0]) || !(-0.1..=1.1).contains(&uv[1]) {
        return u;
    }
    let light = sun_camera * view.exposure;
    let solid_angle = std::f32::consts::PI * radius * radius;
    let lum = light.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    // The sky clamps what it writes (`fs_sky`): the uncovered disc reads no more than that.
    let disc = (lum / solid_angle).min(50_000.0);
    u.sun = [uv[0], uv[1], radius, settings.sun_glare];
    u.sun_light = [light.x, light.y, light.z, disc];
    u
}

fn texture_sampler_layout(
    shared: &GpuShared,
    label: &str,
    with_uniform: bool,
    with_second_texture: bool,
) -> wgpu::BindGroupLayout {
    use wgpu::ShaderStages as S;
    let mut entries = vec![
        layout_entry(
            0,
            S::VERTEX_FRAGMENT,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ),
        layout_entry(
            1,
            S::VERTEX_FRAGMENT,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        ),
    ];
    if with_uniform {
        entries.push(layout_entry(
            2,
            S::VERTEX_FRAGMENT,
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
            S::VERTEX_FRAGMENT,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ));
    }
    shared.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    let tonemap_layout = texture_sampler_layout(shared, "tonemap layout", true, true);
    let bloom_layout = texture_sampler_layout(shared, "bloom layout", true, false);
    let blit_layout = texture_sampler_layout(shared, "blit layout", false, false);

    let post_module = shared.shader("post", include_str!("shaders/post.wgsl"), false);
    let bloom_module = shared.shader("bloom", include_str!("shaders/bloom.wgsl"), false);
    let blit_module = shared.shader("blit", include_str!("shaders/blit.wgsl"), false);

    let tonemap_pipeline = fullscreen_pipeline(
        shared,
        "tonemap",
        &post_module,
        "vs_tonemap",
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
            targets.post()
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
                resource: wgpu::BindingResource::TextureView(&targets.post().view),
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
    let _span = tracing::info_span!("render: start the renderer").entered();
    let started = Instant::now();
    match Renderer::new(
        if headless { None } else { handle },
        width,
        height,
        &settings,
    ) {
        Ok(mut renderer) => {
            // Each feature's start (its pipelines compile here), slowest first in the log.
            let adding = Instant::now();
            let mut features = match world.get_resource_mut::<PendingRenderFeatures>() {
                Some(mut pending) => {
                    let _span = tracing::info_span!("render: start the features").entered();
                    renderer.add_features(std::mem::take(&mut pending.0))
                }
                None => Vec::new(),
            };
            let adding_ms = adding.elapsed().as_secs_f64() * 1000.0;
            features.sort_by(|a, b| b.1.total_cmp(&a.1));
            let list: Vec<String> = features
                .iter()
                .take(12)
                .map(|(name, ms)| format!("{name} {ms:.0}"))
                .collect();
            tracing::info!(
                "renderer started in {:.0} ms; {} features in {adding_ms:.0} ms on threads \
                 ({:.0} ms of work; slowest, ms: {})",
                started.elapsed().as_secs_f64() * 1000.0,
                features.len(),
                features.iter().map(|f| f.1).sum::<f64>(),
                list.join(", ")
            );
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
/// When the environment map was drawn (`RenderSettings::environment_interval`).
#[derive(Clone, Copy, Debug)]
struct EnvDrawn {
    frame: u64,
    sun_dir: Vec3,
    origin: DVec3,
}

/// The sun may turn this far (radians, 0.2°) before the environment map is drawn again early:
/// a day-night cycle of a few minutes turns it about that much a frame.
const ENV_SUN_TURN: f32 = 0.0035;
/// The camera may travel this far (m) before it is drawn again early (the sky's color changes
/// with altitude; the map holds no nearby geometry).
const ENV_TRAVEL_M: f64 = 50.0;

impl EnvDrawn {
    /// Whether the map drawn at `self` is due again at `now`.
    fn due(&self, now: &EnvDrawn, interval: u32, history_valid: bool) -> bool {
        !history_valid
            || now.frame >= self.frame + interval.max(1) as u64
            || now.frame < self.frame
            || self.sun_dir.dot(now.sun_dir) < ENV_SUN_TURN.cos()
            || self.origin.distance(now.origin) > ENV_TRAVEL_M
    }
}

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

/// Keep [`crate::FramePacing`] for the frame about to be drawn (`RenderSettings::pacing_fps`).
fn pace_frame(world: &mut World) {
    let pacing_fps = world
        .get_resource::<RenderSettings>()
        .map_or(0.0, |s| s.pacing_fps);
    let dt = world
        .get_resource::<Time>()
        .map_or(0.0, |t| t.real_delta_secs());
    let mut pacing = world.get_resource_or_insert_with(crate::FramePacing::default);
    let before = *pacing;
    let mut now = before;
    now.advance(dt, pacing_fps);
    if now != before {
        *pacing = now;
    }
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
    pace_frame(world);
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
    with_renderer(world, |renderer, world| {
        renderer.settle_exposure();
        renderer.render(world)
    });
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

#[cfg(test)]
mod env_tests {
    use super::*;

    #[test]
    fn the_environment_map_is_drawn_every_interval_and_early_when_the_sky_changes() {
        let at = |frame: u64, sun: Vec3, origin: DVec3| EnvDrawn {
            frame,
            sun_dir: sun.normalize(),
            origin,
        };
        let sun = Vec3::new(0.3, 0.8, 0.2);
        let last = at(100, sun, DVec3::ZERO);
        // Within the interval, nothing changed: not yet.
        assert!(!last.due(&at(103, sun, DVec3::new(10.0, 0.0, 0.0)), 4, true));
        assert!(last.due(&at(104, sun, DVec3::ZERO), 4, true));
        // A cut, a turned sun, a long trip: at once.
        assert!(last.due(&at(101, sun, DVec3::ZERO), 4, false));
        let turned = Quat::from_rotation_y(0.01) * sun;
        assert!(last.due(&at(101, turned, DVec3::ZERO), 4, true));
        assert!(last.due(&at(101, sun, DVec3::new(0.0, 60.0, 0.0)), 4, true));
        // Interval 1: every frame.
        assert!(last.due(&at(101, sun, DVec3::ZERO), 1, true));
    }
}
