//! Extending the renderer with render features.
//!
//! A [`RenderFeature`] owns its GPU resources and hooks into fixed points of the frame:
//!
//! 1. `prepare` — read the world, cull, upload buffers (CPU side, `&mut World` access).
//! 2. `compute` — record compute or offscreen passes (particles, simulation, clouds). A feature
//!    that asked for the cloud shadow map ([`RenderFeature::cloud_shadows`]) draws it here.
//! 3. `draw_environment` — draw into the sky environment map (ambient light + reflections).
//! 4. `draw_shadow` — draw depth into one shadow cascade, or into the top-down sky occlusion
//!    map when the index is [`SKY_OCCLUSION_VIEW`](crate::shadows::SKY_OCCLUSION_VIEW);
//!    `draw_far_shadow` — big casters (terrain, mountains) into the coarse far shadow map.
//! 5. `draw_prepass` — draw depth, normals and motion of opaque geometry. Ambient occlusion,
//!    contact shadows and temporal anti-aliasing are computed from it before the scene is
//!    shaded, so opaque features should implement it (geometry without it still renders, it
//!    just gets no ambient occlusion or contact shadows, and only camera motion for TAA).
//! 6. `draw` — draw into the HDR scene in a [`RenderPhase`] (opaque → sky → transparent).
//! 7. `overlay` — draw onto the final tonemapped image (UI, debug text, editor gizmos).
//!
//! Group 0 is bound by the renderer before `draw_prepass`/`draw`/`draw_environment` (frame
//! data, lights, shadows, environment map, ambient occlusion and the prepass maps; see
//! `shaders/lib_frame.wgsl`) and before `draw_shadow` (the cascade's
//! [`ShadowView`](crate::shadows::ShadowViewUniform)). Features use groups 1+.
//!
//! The camera is jittered by a fraction of a pixel every frame when temporal anti-aliasing is
//! on: use `frame.view_proj` for rasterizing (it includes the jitter) and, in the prepass,
//! `motion_clip`/`prev_motion_clip` for motion vectors. Overlays get an unjittered frame.
//!
//! Register a feature with [`RenderAppExt::add_render_feature`]. Water, terrain, particles,
//! volumetric clouds, foliage and UI are all built this way.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use engine_assets::{Assets, Handle};
use engine_core::App;
use engine_core::diagnostics::Diagnostics;

use crate::cloud_shadows::{CloudShadowRequest, CloudShadowTarget};
use crate::frame::ViewData;
use crate::image_cache::{ImageCache, ImageTexture};
use crate::material_library::{MaterialLibrary, MaterialMaps, MaterialPreset, MaterialTextures};
use crate::settings::RenderSettings;
use crate::texture::Image;

/// Shared types and pure functions (no bindings).
pub const SHADER_LIB_TYPES: &str = include_str!("shaders/lib_types.wgsl");
/// Group 0 frame bindings, shadows, PBR shading and fog. Requires `SHADER_LIB_TYPES`.
pub const SHADER_LIB_FRAME: &str = include_str!("shaders/lib_frame.wgsl");
/// The atmosphere: sky radiance, sunlight transmittance, aerial perspective. Requires
/// `SHADER_LIB_TYPES` and `SHADER_LIB_FRAME`.
pub const SHADER_LIB_ATMOSPHERE: &str = include_str!("shaders/lib_atmosphere.wgsl");

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
    /// Prepass normals (octahedral normal xy, roughness, written flag).
    pub normal: wgpu::TextureFormat,
    /// Prepass motion vectors (uv delta).
    pub motion: wgpu::TextureFormat,
    /// Ambient occlusion.
    pub ao: wgpu::TextureFormat,
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
    /// Generated material textures (presets), built on first use and shared by every feature.
    pub materials: MaterialLibrary,
    /// The renderer's uploaded [`Image`]s, shared by every feature (see
    /// [`GpuShared::image_texture`] and [`crate::image_cache`]).
    pub images: ImageCache,
    /// This frame's upload allowance, shared by every feature (see [`UploadBudget`]).
    pub uploads: UploadBudget,
    /// GPU work queued besides the frame's passes, for the slow-frame log (see
    /// [`crate::perf`]).
    pub(crate) side_work: crate::perf::SideWork,
    /// Command buffers recorded besides the frame (texture compression), submitted with the
    /// next frame's own commands, before them (see [`GpuShared::submit_with_frame`]).
    pub(crate) side_commands: std::sync::Mutex<SideCommands>,
    /// Compile pipelines ahead (`RenderSettings::pipeline_warmup` for this run): a feature
    /// whose pipelines wait for their first use builds them in `init` instead, which runs
    /// beside the other features' at start, rather than on the frame that first needs them.
    pub warm_pipelines: bool,
    /// A feature's own steps inside its `prepare` this frame (the meshes'), named in the
    /// slow-frame log line under the renderer's steps.
    pub(crate) prepare_steps: std::sync::Mutex<Vec<(&'static str, f32)>>,
    /// Mip chains drawn on the GPU for large images (`mipmaps`).
    pub(crate) mips: crate::mipmaps::MipGenerator,
}

/// See [`GpuShared::side_commands`].
#[derive(Default)]
pub(crate) struct SideCommands {
    pub commands: Vec<wgpu::CommandBuffer>,
    /// Run right after their submit (readbacks).
    pub after: Vec<Box<dyn FnOnce() + Send>>,
}

/// How many bytes the features may upload this frame (`RenderSettings::upload_mb_per_frame`
/// in play; while a `LoadingBudget` is present, as much as fits in its `budget_ms`), so a
/// burst of new meshes and textures spreads over frames instead of stalling one. After Unity's
/// Async Upload Pipeline (a time slice per frame) and Unreal's streaming, which spreads its
/// requests over frames.
///
/// A feature asks [`take`](Self::take) before uploading a new mesh or image: on `false` it
/// draws without it this frame and asks again next frame. Anything asked while budget is left
/// goes, however large (the overshoot comes off the next frames), so big items can't starve
/// behind small ones. New versions of what is already on the GPU (edited meshes, repainted
/// images) always go, so edits show at once. What was refused shows as
/// `render.pending_upload_mb`, each item counted once however often it is asked: zero once
/// everything asked for is on the GPU (a loading screen can wait for that).
#[derive(Debug, Default)]
pub struct UploadBudget {
    state: std::sync::Mutex<BudgetState>,
}

#[derive(Debug, Default)]
struct BudgetState {
    /// Bytes left this frame (`u64::MAX`: no limit).
    left: u64,
    /// What the last frame went over its budget by, taken off this one.
    debt: u64,
    /// A new item went this frame (the first always goes, so the queue always moves).
    took_new: bool,
    /// Bytes uploaded and refused this frame.
    uploaded: u64,
    refused: u64,
    /// Items refused this frame (each counts once in `refused`).
    refused_keys: std::collections::HashSet<u64>,
    /// Uploads stop at this time (while loading).
    deadline: Option<std::time::Instant>,
    /// Bytes of updates (new versions of what is on the GPU) this frame.
    updated: u64,
    /// What went over the current report window, by item: (uploads, bytes).
    window: HashMap<u64, (u32, u64)>,
    window_frames: u32,
    window_bytes: u64,
    window_started: Option<std::time::Instant>,
}

/// A window of frames that uploaded a lot in play (see [`UploadBudget::report`]).
#[derive(Debug, Clone, PartialEq)]
pub struct UploadReport {
    pub frames: u32,
    /// Bytes a frame, averaged over the window.
    pub bytes_per_frame: u64,
    /// The items that uploaded most: (key, uploads, bytes), most bytes first.
    pub top: Vec<(u64, u32, u64)>,
    /// Items uploaded in the window.
    pub items: usize,
}

/// A report window ends after this many frames or [`REPORT_SECS`], whichever comes first.
const REPORT_FRAMES: u32 = 120;
const REPORT_SECS: f32 = 5.0;
/// Windows that uploaded less than this a frame on average aren't reported.
const REPORT_BYTES_PER_FRAME: u64 = 1 << 20;

impl UploadBudget {
    fn state(&self) -> std::sync::MutexGuard<'_, BudgetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start a frame with `bytes` to spend (`u64::MAX`: no limit) and maybe a deadline.
    pub fn begin_frame(&self, bytes: u64, deadline: Option<std::time::Instant>) {
        let mut s = self.state();
        if bytes == u64::MAX {
            s.left = u64::MAX;
            s.debt = 0;
        } else {
            // At most a few frames' worth carries over.
            let debt = s.debt.min(bytes);
            s.left = bytes - debt;
            s.debt = (s.debt - debt).min(bytes.saturating_mul(3));
        }
        s.took_new = false;
        s.uploaded = 0;
        s.updated = 0;
        s.refused = 0;
        s.refused_keys.clear();
        s.deadline = deadline;
    }

    /// Whether the item `key` (unique per mesh or image and use) may upload `bytes` this frame;
    /// `update`: a new version of something already on the GPU, which always goes. Counted
    /// when it may; recorded once as pending when not.
    pub fn take(&self, key: u64, bytes: u64, update: bool) -> bool {
        let mut s = self.state();
        if update {
            // Shares the frame's bandwidth, but never waits.
            if s.left != u64::MAX {
                s.left = s.left.saturating_sub(bytes);
            }
            s.uploaded += bytes;
            s.updated += bytes;
            s.record(key, bytes);
            return true;
        }
        let late = s.deadline.is_some_and(|d| std::time::Instant::now() >= d);
        if !s.took_new || (!late && s.left > 0) {
            if s.left != u64::MAX {
                let over = bytes.saturating_sub(s.left);
                s.left -= bytes - over;
                s.debt += over;
            }
            s.took_new = true;
            s.uploaded += bytes;
            s.record(key, bytes);
            true
        } else {
            if s.refused_keys.insert(key) {
                s.refused += bytes;
            }
            false
        }
    }

    /// Bytes uploaded through [`take`](Self::take) this frame.
    pub fn uploaded(&self) -> u64 {
        self.state().uploaded
    }

    /// Bytes refused this frame: waiting for a later one.
    pub fn pending(&self) -> u64 {
        self.state().refused
    }

    /// Bytes of new versions of items already on the GPU this frame (edited meshes, repainted
    /// images).
    pub fn updated(&self) -> u64 {
        self.state().updated
    }

    /// After a frame: once a window of frames is over ([`REPORT_FRAMES`] or [`REPORT_SECS`]),
    /// what it uploaded, when that was more than [`REPORT_BYTES_PER_FRAME`] a frame in play (no
    /// loading deadline). An item among the top uploading in most of the window's frames is
    /// re-uploaded every frame: its asset changes every frame.
    pub fn report(&self) -> Option<UploadReport> {
        let mut s = self.state();
        let now = std::time::Instant::now();
        let started = *s.window_started.get_or_insert(now);
        s.window_frames += 1;
        if s.window_frames < REPORT_FRAMES && (now - started).as_secs_f32() < REPORT_SECS {
            return None;
        }
        let frames = std::mem::take(&mut s.window_frames);
        let bytes = std::mem::take(&mut s.window_bytes);
        let window = std::mem::take(&mut s.window);
        s.window_started = Some(now);
        let bytes_per_frame = bytes / frames as u64;
        if s.deadline.is_some() || bytes_per_frame < REPORT_BYTES_PER_FRAME {
            return None;
        }
        let items = window.len();
        let mut top: Vec<(u64, u32, u64)> =
            window.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
        top.sort_by_key(|&(k, _, b)| (std::cmp::Reverse(b), k));
        top.truncate(6);
        Some(UploadReport {
            frames,
            bytes_per_frame,
            top,
            items,
        })
    }
}

impl BudgetState {
    fn record(&mut self, key: u64, bytes: u64) {
        let e = self.window.entry(key).or_insert((0, 0));
        e.0 += 1;
        e.1 += bytes;
        self.window_bytes += bytes;
    }
}

impl GpuShared {
    /// Submit `commands` with the next frame's, before them, instead of on their own: a
    /// `queue.submit` on the main thread waits for the render thread's submit and present.
    /// Writes they depend on (`queue.write_buffer`) go with the next submit anyway, so they
    /// stay ahead of them. `after` runs right after that submit.
    pub(crate) fn submit_with_frame(
        &self,
        commands: wgpu::CommandBuffer,
        after: Option<Box<dyn FnOnce() + Send>>,
    ) {
        let mut side = self.side_commands.lock().unwrap_or_else(|e| e.into_inner());
        side.commands.push(commands);
        side.after.extend(after);
    }

    /// The commands queued by [`submit_with_frame`](Self::submit_with_frame).
    pub(crate) fn take_side_commands(&self) -> SideCommands {
        std::mem::take(&mut *self.side_commands.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Compile WGSL composed as `lib_types` (+ `lib_frame` when `with_frame`) + `source`.
    pub fn shader(&self, label: &str, source: &str, with_frame: bool) -> wgpu::ShaderModule {
        let code = compose_shader(source, with_frame);
        self.device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(code.into()),
            })
    }

    /// The GPU-resident textures of a material preset at `size`² texels (a power of two,
    /// 64..4096), generating them the first time they are asked for. Feature shaders can bind
    /// the views like any texture (base color is an sRGB view).
    pub fn material_textures(&self, preset: MaterialPreset, size: u32) -> Arc<MaterialTextures> {
        self.materials.get(self, preset, size)
    }

    /// The renderer's own GPU copy of an image asset, ready to bind: uploaded with its mip chain
    /// the first time any feature asks for it and again when its asset gets new pixels, and
    /// shared by every feature that asks (one upload per image and color space). `color`: the
    /// slot holds color (base color, emissive), so an sRGB RGBA8 image is sampled through an sRGB
    /// view; data slots (normal, metallic/roughness, occlusion, masks) pass false. `None` while
    /// the image is still loading, or when it cannot be uploaded (invalid data, too large).
    ///
    /// Ask every frame the image is drawn (images nobody asks for in 600 frames leave the GPU),
    /// and rebuild a bind group that binds [`ImageTexture::view`] when [`ImageTexture::id`]
    /// changes. Use this rather than uploading `Image::data` yourself: with
    /// `RenderSettings::release_image_pixels` the renderer frees the CPU pixels of images
    /// uploaded here, and this keeps returning their texture.
    pub fn image_texture(
        &self,
        images: &Assets<Image>,
        handle: Handle<Image>,
        color: bool,
    ) -> Option<Arc<ImageTexture>> {
        self.images.get(self, images, handle, color)
    }

    /// Like [`image_texture`](Self::image_texture) for a slot that reads the image as `usage`,
    /// which decides how the renderer may block-compress it (`RenderSettings::texture_compression`):
    /// [`TextureUse::Red`] for one-channel slots, [`TextureUse::Exact`] for data that must stay
    /// exact.
    pub fn image_texture_for(
        &self,
        images: &Assets<Image>,
        handle: Handle<Image>,
        usage: crate::image_cache::TextureUse,
    ) -> Option<Arc<ImageTexture>> {
        self.images.get_for(self, images, handle, usage)
    }

    /// Copy a preset's maps back to the CPU (mip 0), generating them first if needed. Blocks
    /// until the GPU is done. For tests and exporting.
    pub fn read_material_maps(
        &self,
        preset: MaterialPreset,
        size: u32,
    ) -> Result<MaterialMaps, String> {
        self.materials.read_back(self, preset, size)
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

    /// Color targets of the depth prepass, in order: normals and motion vectors. A prepass
    /// pipeline uses these with `depth_state(true)` and returns `PrepassOutput` from its
    /// fragment shader (see `prepass_output` in `lib_frame.wgsl`).
    pub fn prepass_targets(&self) -> [Option<wgpu::ColorTargetState>; 2] {
        [self.formats.normal, self.formats.motion].map(|format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
    }

    /// The target of `RenderFeature::draw_subsurface_color`: sRGB color, no blending. Such a
    /// pipeline uses `depth_state(false)` (the pass tests against the opaque scene's depth and
    /// can't write it).
    pub fn subsurface_color_target(&self) -> wgpu::ColorTargetState {
        wgpu::ColorTargetState {
            format: crate::subsurface::COLOR_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
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
    let mut code = String::with_capacity(
        SHADER_LIB_TYPES.len()
            + SHADER_LIB_FRAME.len()
            + SHADER_LIB_ATMOSPHERE.len()
            + source.len()
            + 3,
    );
    code.push_str(SHADER_LIB_TYPES);
    code.push('\n');
    if with_frame {
        code.push_str(SHADER_LIB_FRAME);
        code.push('\n');
        code.push_str(SHADER_LIB_ATMOSPHERE);
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
#[derive(Clone, Copy)]
pub struct FrameContext<'a> {
    pub gpu: &'a GpuShared,
    /// Group 0 frame bindings (`lib_frame.wgsl`) for passes a feature records itself, such as
    /// overlays. During `compute` and `draw_environment` it still holds last frame's
    /// environment map; during `overlay` its matrices are unjittered.
    pub frame_group: &'a wgpu::BindGroup,
    pub view: &'a ViewData,
    pub settings: &'a RenderSettings,
    /// Scene color copy (HDR, pre-exposed) taken after the sky; valid in the transparent phase.
    pub scene_color_copy: &'a wgpu::TextureView,
    /// Scene depth copy (reverse-Z) taken after the sky; valid in the transparent phase.
    pub scene_depth_copy: &'a wgpu::TextureView,
    /// The final LDR image (overlays draw here).
    pub ldr: &'a wgpu::TextureView,
    /// The cloud shadow map to draw into: set only in [`RenderFeature::compute`], and only for
    /// the feature whose [`RenderFeature::cloud_shadows`] asked for it this frame (see
    /// [`crate::cloud_shadows`]). It has been cleared to 1 (no shadow). During `compute`,
    /// `frame_group` binds a white stand-in in its place (frame binding 22), so the pass that
    /// draws the map can bind `frame_group` too.
    pub cloud_shadows: Option<CloudShadowTarget<'a>>,
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

    /// The part of the frame's preparation that reads no world: cull, sort, build draw lists
    /// and write buffers from what `prepare` copied out of the world. Runs after every
    /// feature's `prepare` and before `compute`, on the render thread when the renderer is
    /// pipelined, so the simulation of the next frame overlaps it (pipelined-rendering.md, as
    /// Unreal's scene proxies and Bevy's prepare after extract). `diagnostics` reach the
    /// world's `Diagnostics` with the frame's other `render.*` metrics.
    fn prepare_frame(
        &mut self,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        diagnostics: &mut Diagnostics,
    ) {
    }

    /// Record compute or offscreen work before the scene is drawn.
    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {}

    /// The cloud shadow map this feature draws this frame, if any (the weather module's clouds;
    /// see [`crate::cloud_shadows`]). Called every frame after `prepare`. When it returns a
    /// request, `compute` gets the map, placed around the camera, in `ctx.cloud_shadows`, and
    /// everything the sun lights is shaded by it. The first feature (in `order`) that asks
    /// gets it. Nothing asks by default: the map stays 1 and costs nothing.
    fn cloud_shadows(&self) -> Option<CloudShadowRequest> {
        None
    }

    /// Draw into the environment map (equirectangular, radiance not pre-exposed).
    fn draw_environment(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Draw depth into shadow cascade `cascade` (cull with `ctx.view.cascades`), or into the
    /// sky occlusion map when `cascade == SKY_OCCLUSION_VIEW` (cull with
    /// `ctx.view.sky_occlusion.contains_sphere`). Anything that roofs over the ground (rock,
    /// buildings, canopies) should draw into both.
    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, ctx: &FrameContext) {}

    /// Draw depth into the far shadow map: one coarse map (texels of several meters) around the
    /// camera that gives sun shadows beyond the cascades, out to
    /// `RenderSettings::far_shadow_distance`. Draw only what is big enough to show at that
    /// resolution, culled with `ctx.view.far_shadow.casts(center, radius)`: terrain, mountains,
    /// cliffs, large buildings. Same pipeline and group 0 layout as `draw_shadow`. Features that
    /// don't implement it simply cast no shadows past the cascades.
    fn draw_far_shadow(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Draw the depth prepass: opaque geometry with depth writes, returning `PrepassOutput`
    /// (world normal, roughness, motion) into [`GpuShared::prepass_targets`]. Use the same
    /// vertex math as the opaque draw and mark the clip position `@invariant`, so the opaque
    /// pass's depth test (`GreaterEqual`) passes exactly where the prepass drew. Alpha-masked
    /// geometry discards the same pixels as in the opaque pass.
    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// The depth prepass is recorded and `depth` (reverse-Z) holds what it drew. Record work
    /// that tests against it (virtual geometry's occlusion culling) and return true to draw
    /// more into the prepass with [`draw_prepass_late`](Self::draw_prepass_late).
    fn after_prepass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::TextureView,
        ctx: &FrameContext,
    ) -> bool {
        false
    }

    /// Draw into the depth prepass again (same targets and group 0, contents kept) when some
    /// feature's `after_prepass` returned true: what turned out visible only then.
    fn draw_prepass_late(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Draw into the HDR scene.
    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Whether the transparent phase needs `scene_color_copy`/`scene_depth_copy`.
    fn needs_scene_copy(&self) -> bool {
        false
    }

    /// Whether this frame draws skin that scatters light under it: pixels its prepass marks
    /// with `prepass_output_subsurface`, whose scene pass leaves specular light in alpha. The
    /// subsurface pass runs after the opaque scene while any feature says so (see
    /// `subsurface.rs`).
    fn needs_subsurface(&self) -> bool {
        false
    }

    /// Post-scatter color (Jimenez et al. 2015, HDRP's "post-scatter" texturing): the color
    /// the subsurface pass multiplies into its pixels' diffuse light after the blur, so
    /// freckles and pores stay sharp and fair skin keeps its tint. Called right before the
    /// blur, only while `needs_subsurface()` and the pass runs (`subsurface_width` above 0, a
    /// perspective view); group 0 is the frame group, as in the scene pass. Draw with
    /// `GpuShared::subsurface_color_target()` and `depth_state(false)`, returning the color
    /// (rgb, alpha 1) where the scene pass lit the skin without it (albedo 1, or the square root
    /// of the albedo here and in the lighting, HDRP's pre-and-post mode). The color applies to
    /// marked pixels only (scatter strength above 0); pixels nobody draws keep 1.
    fn draw_subsurface_color(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {}

    /// Draw onto the tonemapped LDR image.
    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {}

    /// Whether `overlay` also draws in extra views (`ExtraView` cameras, such as an editor's
    /// split scene views), with that view's context (its LDR image, viewport, scene depth copy
    /// and unjittered frame group). Off by default: most overlays are the game's window-wide
    /// UI, drawn once over the main view. Overlays that belong to the scene, such as debug
    /// lines and selection outlines, turn it on.
    fn overlay_in_extra_views(&self) -> bool {
        false
    }

    fn stats(&self) -> FeatureStats {
        FeatureStats::default()
    }

    /// Virtual geometry clusters the camera drew (their triangles are in `stats().triangles`).
    /// Kept out of `FeatureStats` so features in other crates can keep building it field by
    /// field.
    fn clusters_drawn(&self) -> u32 {
        0
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

    /// Make room for `size` bytes (contents are lost when it grows); returns true when the
    /// buffer was recreated (rebuild bind groups). For writes through
    /// `Queue::write_buffer_with`, straight into staging memory.
    pub fn reserve(&mut self, device: &wgpu::Device, size: u64) -> bool {
        let grown = size > self.capacity;
        if grown {
            *self = Self::new(device, self.label, self.usage, size);
        }
        grown
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }
}

#[cfg(test)]
mod upload_budget_tests {
    use super::UploadBudget;

    const MB: u64 = 1 << 20;

    #[test]
    fn big_items_are_not_starved_by_small_ones() {
        // 16 MB a frame; every frame a small image asks first, then a 64 MB texture.
        let b = UploadBudget::default();
        let mut big_went = None;
        for frame in 0..8 {
            b.begin_frame(16 * MB, None);
            assert!(b.take(1000 + frame, MB, false));
            if b.take(7, 64 * MB, false) {
                big_went = Some(frame);
                break;
            }
        }
        assert_eq!(big_went, Some(0), "the big texture waited");
        // Its overshoot comes off the next frames, but the first new upload still goes.
        b.begin_frame(16 * MB, None);
        assert!(b.take(1, MB, false));
        assert!(!b.take(2, MB, false));
    }

    #[test]
    fn pending_counts_each_item_once_and_edits_always_go() {
        let b = UploadBudget::default();
        b.begin_frame(MB, None);
        assert!(b.take(1, 2 * MB, false));
        // The same image asked by three materials: counted once.
        for _ in 0..3 {
            assert!(!b.take(2, 4 * MB, false));
        }
        assert_eq!(b.pending(), 4 * MB);
        // A new version of a resident mesh goes even with the budget spent.
        assert!(b.take(3, 8 * MB, true));
        assert_eq!(b.uploaded(), 10 * MB);
    }

    #[test]
    fn a_window_of_heavy_uploads_names_what_uploads_every_frame() {
        let b = UploadBudget::default();
        let mut report = None;
        for frame in 0..super::REPORT_FRAMES as u64 {
            b.begin_frame(16 * MB, None);
            // An edited mesh every frame, and some new images now and then.
            assert!(b.take(7, 2 * MB, true));
            if frame % 10 == 0 {
                assert!(b.take(100 + frame, MB, false));
            }
            report = b.report();
            assert_eq!(b.updated(), 2 * MB);
        }
        let report = report.expect("a report after the window");
        assert_eq!(report.frames, super::REPORT_FRAMES);
        assert_eq!(report.items, 13);
        assert_eq!(
            report.top[0],
            (
                7,
                super::REPORT_FRAMES,
                2 * MB * super::REPORT_FRAMES as u64
            )
        );
        assert!(report.top.len() <= 6);
        // Light windows, and loading ones, aren't reported.
        for _ in 0..super::REPORT_FRAMES {
            b.begin_frame(16 * MB, None);
            b.take(8, MB / 4, true);
            assert_eq!(b.report(), None);
        }
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        for _ in 0..super::REPORT_FRAMES {
            b.begin_frame(16 * MB, Some(later));
            b.take(9, 4 * MB, true);
            assert_eq!(b.report(), None);
        }
    }
}
