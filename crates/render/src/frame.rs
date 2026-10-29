//! Per-frame view data: the camera, sun, exposure and the GPU `Frame` uniform.
//!
//! Large worlds: the camera's f64 world position is the rendering origin. Everything sent to
//! the GPU is relative to it, so precision is the same next to the player whether they stand
//! at the spawn point or on a moon 400 000 km away.

use bytemuck::{Pod, Zeroable};
use engine_core::math::Frustum;
use engine_core::prelude::*;
use engine_world::{SurfaceLayers, WeatherState, WindField};

use crate::cloud_shadows::GpuCloudShadow;
use crate::components::{
    AmbientLight, Camera, DirectionalLight, Exposure, PointLight, Projection, SkyLightVolume,
};
use crate::settings::{Atmosphere, DebugView, FogSettings, RenderSettings};
use crate::shadows::{Cascades, FarShadowView, SkyOcclusionView};

/// GPU layout of `struct Frame` in `lib_types.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct FrameUniform {
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    pub view_proj: [[f32; 4]; 4],
    pub inv_view: [[f32; 4]; 4],
    pub inv_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub prev_view_proj: [[f32; 4]; 4],
    pub camera_world: [f32; 4],
    pub camera_world_frac: [f32; 4],
    pub viewport: [f32; 4],
    pub time: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_color: [f32; 4],
    pub sun_camera: [f32; 4],
    pub ambient: [f32; 4],
    pub fog: [f32; 4],
    pub fog_color: [f32; 4],
    pub wind: [f32; 4],
    pub weather: [f32; 4],
    pub planet: [f32; 4],
    pub atmosphere: [f32; 4],
    pub exposure: [f32; 4],
    pub shadow_cascades: [[[f32; 4]; 4]; 4],
    pub shadow_splits: [f32; 4],
    pub shadow_params: [f32; 4],
    pub counts: [u32; 4],
    pub ground: [f32; 4],
    pub sky_occlusion: [[f32; 4]; 4],
    pub sky_occlusion_params: [f32; 4],
    pub sky_volumes: [GpuSkyVolume; MAX_SKY_VOLUMES],
    /// `view_proj` without the TAA sub-pixel jitter (motion vectors use it).
    pub unjittered_view_proj: [[f32; 4]; 4],
    /// TAA jitter: NDC x, NDC y, pixels x, pixels y.
    pub jitter: [f32; 4],
    /// Camera origin minus the previous frame's origin (m), w = 1 when the previous frame is
    /// a valid history (no camera cut).
    pub camera_delta: [f32; 4],
    /// Ambient occlusion intensity (0 = off), contact shadow length (m, 0 = off), sun
    /// penumbra scale, ambient occlusion radius (m).
    pub effects: [f32; 4],
    /// Screen-space reflections this frame (1/0), the previous frame's pre-exposure (set by the
    /// renderer; 0 = no previous image), maximum perceptual roughness, screen-space global
    /// illumination strength (0 = off this frame).
    pub reflections: [f32; 4],
    /// Volumetric fog: range (m, 0 = off), 1 / depth scale (1/m), 1 / log2(1 + range / depth
    /// scale), depth slices (see `volumetrics::frame_params`).
    pub volumetrics: [f32; 4],
    /// Camera-relative → far shadow map clip space (see [`FarShadowView`]).
    pub far_shadow: [[f32; 4]; 4],
    /// Far shadow map: reach (m, 0 = off), texel size (m), clip depth per meter, array layer.
    pub far_shadow_params: [f32; 4],
    /// Sky: the flat cloud layer on (1/0, `RenderSettings::sky_cloud_layer`), night (0 day ..
    /// 1 night: stars and the night sky's floor; see [`sky_night`]), the stand-in moon's glow
    /// opposite a sun below the horizon (0..1), unused.
    pub sky: [f32; 4],
    /// The ground's weather map (`surface_weather`, set by the renderer): its minimum corner
    /// relative to the camera (x, z, m) and its size along x and z (m; 0 = no map).
    pub surface_map: [f32; 4],
    /// The open ground's weather in the map's encoding (wetness, standing water / 0.04 m, snow /
    /// 0.3 m, scorch): beyond the map, and for roofs over ground they keep dry.
    pub surface_open: [f32; 4],
    /// The cloud shadow map (see [`crate::cloud_shadows`]); all zero (off) until a feature asks
    /// for it, which the renderer fills in after `prepare`.
    pub cloud_shadow: GpuCloudShadow,
    /// The air ([`Atmosphere`], times its `density_scale`): Rayleigh scattering at the surface
    /// (1/m) RGB, w unused.
    pub air_rayleigh: [f32; 4],
    /// Mie scattering at the surface (1/m) RGB, less the forward peak that passes as
    /// unscattered light ([`Atmosphere::mie_forward_peak`]), w unused.
    pub air_mie: [f32; 4],
    /// Mie extinction (scattering + absorption) at the surface (1/m) RGB, less the forward
    /// peak, w unused.
    pub air_mie_extinction: [f32; 4],
    /// Ozone absorption at the layer's densest (1/m) RGB, w unused.
    pub air_ozone: [f32; 4],
    /// Mie anisotropy per channel RGB (of the whole lobe), w [`Atmosphere::mie_forward_peak`].
    pub air_mie_g: [f32; 4],
    /// The scene's place in the output image ([`crate::Viewport`]): its rectangle's top-left
    /// corner and size (x, y, width, height; pixels). The scene renders at `viewport.xy`
    /// pixels, smaller than the rectangle at a render scale under 1; an overlay over the scene
    /// finds the scene pixel under a fragment as `(position.xy - window.xy) * viewport.xy /
    /// window.zw`.
    pub window: [f32; 4],
    /// Probe GI (probe_gi/mod.rs): the grid's first probe relative to the camera, spacing (m).
    pub gi_volume: [f32; 4],
    /// Probes along x, y and z; indirect light's share where probes cover (0 = off).
    pub gi_size: [f32; 4],
    /// Storage slot of the grid's first probe along x, y and z (it scrolls); the probes' epoch
    /// (0 = off: probes written in another epoch are ignored).
    pub gi_grid: [f32; 4],
    /// A mirror's view ([`crate::PlanarReflection`]): the camera-relative plane (n, d) whose
    /// side `dot(n, rel) + d < 0` is cut away; zero for every other view.
    pub clip_plane: [f32; 4],
}

/// Most [`SkyLightVolume`]s shaded per frame (nearest the camera first).
pub const MAX_SKY_VOLUMES: usize = 16;

/// GPU layout of `struct SkyVolume` in `lib_types.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct GpuSkyVolume {
    /// Rows of the camera-relative → box-local affine transform (no scale).
    pub to_local: [[f32; 4]; 3],
    /// Half extents xyz (m), fade distance (m).
    pub half_fade: [f32; 4],
    /// Sky light left inside, open face mask (as a float), and the opening's lowest and
    /// highest corner, each as two 16-bit centimeter counts offset by 327.68 m (bits).
    pub params: [f32; 4],
}

/// A point of a face's plane (meters) as two 16-bit centimeter counts offset by 327.68 m,
/// packed into an `f32`'s bits (`sky_volume_opening` in lib_frame.wgsl unpacks it).
fn pack_opening_corner(p: [f32; 2]) -> f32 {
    let q = |x: f32| ((x * 100.0).round() + 32768.0).clamp(0.0, 65535.0) as u32;
    f32::from_bits(q(p[0]) | q(p[1]) << 16)
}

impl GpuSkyVolume {
    fn new(volume: &SkyLightVolume, rel_center: Vec3, rotation: Quat) -> Self {
        let inv = Mat3::from_quat(rotation.inverse());
        let row = |i: usize| {
            let r = inv.row(i);
            [r.x, r.y, r.z, -r.dot(rel_center)]
        };
        let [hx, hy, hz] = volume.half_extents.map(|h| h.max(0.0));
        let mask = volume.open_faces.iter().fold(0, |m, f| m | f.bit());
        let opening = volume.opening.unwrap_or([[-400.0; 2], [400.0; 2]]);
        Self {
            to_local: [row(0), row(1), row(2)],
            half_fade: [hx, hy, hz, volume.fade.max(0.01)],
            params: [
                volume.sky_light.clamp(0.0, 1.0),
                mask as f32,
                pack_opening_corner(opening[0]),
                pack_opening_corner(opening[1]),
            ],
        }
    }
}

/// GPU layout of `struct PointLight` in `lib_types.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuPointLight {
    pub position_range: [f32; 4],
    pub color_radius: [f32; 4],
}

/// What the renderer remembers about the previous frame, for motion vectors and temporal
/// anti-aliasing.
#[derive(Debug, Clone, Copy)]
pub struct PreviousView {
    /// Unjittered view-projection of the previous frame (relative to its camera).
    pub view_proj: Mat4,
    /// The previous camera's world position.
    pub origin: DVec3,
    /// The previous camera's rotation.
    pub rotation: Quat,
}

/// Everything about the current view, in camera-relative coordinates.
#[derive(Debug, Clone)]
pub struct ViewData {
    pub camera: Option<Entity>,
    /// This is an [`crate::ExtraView`], not the main view. Features that keep history around
    /// the camera (temporal accumulation, streaming) key it by `camera`, or leave it as the
    /// main view left it.
    pub extra_view: bool,
    /// Camera world position: the rendering origin.
    pub origin: DVec3,
    pub rotation: Quat,
    /// Camera-relative view (rotation only).
    pub view: Mat4,
    /// Projection including the TAA sub-pixel jitter.
    pub proj: Mat4,
    /// `proj * view`: what geometry is rasterized with (jittered when TAA is on).
    pub view_proj: Mat4,
    /// `proj` without the jitter.
    pub unjittered_proj: Mat4,
    /// `view_proj` without the jitter (stable; culling and motion vectors use it).
    pub unjittered_view_proj: Mat4,
    /// Previous frame's unjittered view-projection (relative to the previous camera).
    pub prev_view_proj: Mat4,
    /// Previous frame's camera position (equals `origin` on the first frame).
    pub prev_origin: DVec3,
    /// TAA sub-pixel jitter this frame, pixels (zero without TAA).
    pub jitter: Vec2,
    /// Whether last frame's image can be reused (false on the first frame and after a camera
    /// cut).
    pub history_valid: bool,
    pub frustum: Frustum,
    pub near: f32,
    /// Vertical field of view in radians (0 for orthographic).
    pub fov_y: f32,
    pub orthographic: bool,
    /// The scene's size in pixels: its camera's [`crate::Viewport`] (or the whole window)
    /// times `RenderSettings::render_scale`.
    pub width: u32,
    pub height: u32,
    /// The output image's size in pixels (the window), which overlays such as the UI cover.
    pub window: UVec2,
    /// Where the scene sits in the window: its top-left corner, pixels.
    pub viewport_origin: UVec2,
    /// The scene's rectangle in the window, pixels (the scene's size before the render scale).
    pub viewport_size: UVec2,
    /// Direction toward the sun (unit), if there is a sun.
    pub sun_dir: Vec3,
    /// Sun color × illuminance (lux).
    pub sun_color: Vec3,
    pub sun_shadows: bool,
    pub cascades: Cascades,
    /// The coarse sun shadow map beyond the cascades (big casters only).
    pub far_shadow: FarShadowView,
    /// Top-down sky occlusion map around the camera.
    pub sky_occlusion: SkyOcclusionView,
    /// Multiply radiance by this before writing HDR (1 / max luminance).
    pub exposure: f32,
    pub ev100: f32,
    pub wind: Vec3,
    /// World seconds (wrapped for shader precision).
    pub time: f32,
    pub real_time: f32,
    pub delta: f32,
    pub frame_index: u64,
    pub point_lights: Vec<GpuPointLight>,
    /// Indoor volumes near the camera (at most [`MAX_SKY_VOLUMES`]).
    pub sky_volumes: Vec<GpuSkyVolume>,
    pub debug_view: DebugView,
    /// Angular radius of the sun disc, radians.
    pub sun_angular_radius: f32,
}

impl ViewData {
    /// Camera-relative position of a world point.
    pub fn relative(&self, world: DVec3) -> Vec3 {
        (world - self.origin).as_vec3()
    }

    /// Distance from the camera along its view direction.
    pub fn view_depth(&self, rel: Vec3) -> f32 {
        -(self.view.transform_point3(rel)).z
    }

    /// Camera movement since the previous frame (current origin − previous origin), meters.
    pub fn camera_delta(&self) -> Vec3 {
        (self.origin - self.prev_origin).as_vec3()
    }

    /// Camera forward (−Z) direction.
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    /// Draw into the scene's rectangle of the output image: for overlays that draw over the
    /// scene (lines, markers) in its projection. Their fragments' positions are then window
    /// pixels: `(position.xy - frame.window.xy) * frame.viewport.xy / frame.window.zw` is the
    /// scene pixel, for reading scene-sized targets such as depth.
    pub fn set_scene_viewport(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_viewport(
            self.viewport_origin.x as f32,
            self.viewport_origin.y as f32,
            self.viewport_size.x as f32,
            self.viewport_size.y as f32,
            0.0,
            1.0,
        );
    }

    /// Screen-space size heuristic: projected radius in pixels of a sphere at distance `dist`.
    pub fn projected_radius_px(&self, radius: f32, dist: f32) -> f32 {
        if self.orthographic {
            radius * self.proj.y_axis.y * self.height as f32 * 0.5
        } else {
            radius / (dist.max(1e-3) * (self.fov_y * 0.5).tan()) * self.height as f32 * 0.5
        }
    }
}

/// Reverse-Z infinite perspective projection for wgpu (depth 1 at `near`, 0 at infinity).
pub fn perspective_infinite_reverse(fov_y: f32, aspect: f32, near: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    Mat4::from_cols(
        Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, near, 0.0),
    )
}

/// Reverse-Z infinite off-axis perspective projection through a window (Kooima's generalized
/// perspective projection): `tangents` = left, right, bottom, top edges as tangents of the view
/// direction (x / -z, y / -z), depth 1 at `near`, 0 at infinity.
pub fn off_axis_infinite_reverse(tangents: Vec4, near: f32) -> Mat4 {
    let (l, r, b, t) = (tangents.x, tangents.y, tangents.z, tangents.w);
    let (w, h) = ((r - l).max(1e-6), (t - b).max(1e-6));
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new((r + l) / w, (t + b) / h, 0.0, -1.0),
        Vec4::new(0.0, 0.0, near.max(1e-4), 0.0),
    )
}

/// Reverse-Z orthographic projection (depth 1 at `near`, 0 at `far`).
pub fn orthographic_reverse(width: f32, height: f32, near: f32, far: f32) -> Mat4 {
    let d = far - near;
    Mat4::from_cols(
        Vec4::new(2.0 / width, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / height, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / d, 0.0),
        Vec4::new(0.0, 0.0, far / d, 1.0),
    )
}

/// Standard-Z orthographic projection centered on the view axis (depth 0 at `near`).
pub fn orthographic_standard(half_extent: f32, near: f32, far: f32) -> Mat4 {
    let d = far - near;
    Mat4::from_cols(
        Vec4::new(1.0 / half_extent, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / half_extent, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -1.0 / d, 0.0),
        Vec4::new(0.0, 0.0, -near / d, 1.0),
    )
}

/// Exposure value (ISO 100) to the pre-exposure multiplier (1 / max luminance).
pub fn ev100_to_exposure(ev100: f32) -> f32 {
    1.0 / (1.2 * 2f32.powf(ev100))
}

/// Auto-exposure state carried between frames.
#[derive(Debug, Clone, Copy)]
pub struct ExposureState {
    pub ev100: f32,
    pub initialized: bool,
    /// EV100 measured from a recent frame's luminance histogram (GPU metering). `None` until the
    /// first reading arrives; the scene-luminance estimate is used meanwhile.
    pub measured_ev100: Option<f32>,
    /// Set on a camera cut: hold the exposure until the meter has measured the new view, then
    /// jump straight to it instead of adapting from the old view's exposure.
    pub snap: bool,
}

impl Default for ExposureState {
    fn default() -> Self {
        Self {
            ev100: 14.0,
            initialized: false,
            measured_ev100: None,
            snap: false,
        }
    }
}

/// How far above the light's gray (`estimate_scene_luminance`) the meter may read before auto
/// exposure takes a scene for a bright one (`RenderSettings::bright_scene_exposure`), in EV: a
/// view of sunlit grass and sky reads about 0.15 EV above it, a snowfield 2.4.
const BRIGHT_SCENE_START_EV: f32 = 0.4;

/// How far below the light's gray (`estimate_scene_luminance`) the meter starts and ends
/// fading in `RenderSettings::interior_exposure_bias`, in EV: shade under trees reads 1 to 2 EV
/// below it, the breach house at noon about 3.2 (lit by world GI) and the mushroom cave 7.9.
const INTERIOR_START_EV: f32 = 2.0;
const INTERIOR_FULL_EV: f32 = 4.5;

/// How many EV darker (positive) or brighter (negative) than average gray auto exposure shows
/// a scene, from its meter reading and the light's gray (`incident`, both EV100), the sun's
/// height `sun_y` and how much the camera faces it (0..1).
fn exposure_grade(
    settings: &RenderSettings,
    metered: f32,
    incident: f32,
    sun_y: f32,
    facing_sun: f32,
) -> f32 {
    let day = smoothstep(0.0, 0.1, sun_y);
    // Dark scenes stay darker than average gray, like the eye sees them.
    let night = smoothstep(5.0, -2.0, metered) * settings.night_exposure_bias;
    // Caves and interiors by day stay darker than the valley outside, the way games grade
    // them: by how far the meter reads below gray things in the daylight here.
    let interior = smoothstep(INTERIOR_START_EV, INTERIOR_FULL_EV, incident - metered)
        * settings.interior_exposure_bias.max(0.0)
        * day;
    // Bright scenes by day (snow, cloud tops) show brighter than average gray: by how far the
    // meter reads above gray things in the light here, past what an ordinary view of sky and
    // land reads above it. Not at night, when lamps outshine the light the estimate knows of.
    let bright = (metered - incident - BRIGHT_SCENE_START_EV)
        .clamp(0.0, settings.bright_scene_exposure.max(0.0))
        * (1.0 - facing_sun)
        * day;
    night.max(interior) - bright
}

/// Estimate the scene's average luminance from the light setup (cd/m²).
///
/// A physically motivated heuristic: 18% gray lit by direct sun (mix of horizontal and
/// vertical surfaces) and sky light. A GPU luminance histogram can replace it later without
/// changing anything else.
pub fn estimate_scene_luminance(sun_dir: Vec3, sun_camera: Vec3, ambient: f32) -> f32 {
    let sun_lux = luminance(sun_camera);
    let elevation = sun_dir.y;
    let direct = sun_lux * (0.5 * elevation.max(0.0) + 0.15 * smoothstep(-0.05, 0.1, elevation));
    let sky = sun_lux.max(1.0) * 0.15 * smoothstep(-0.2, 0.3, elevation)
        + 110_000.0 * 0.004 * smoothstep(-0.25, 0.0, elevation);
    0.18 * (direct + sky) / std::f32::consts::PI + ambient * 0.18
}

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// How much it is night for the sky (stars and the night sky's floor): 0 by day .. 1 at night,
/// and how much of the stand-in moon (a glow opposite the sun) shows.
///
/// Night is decided by the main light (the brightest directional light): `sun_dir` toward it,
/// `up` the local vertical, `sun_color` its color times illuminance (lux) and `sun_camera` what
/// reaches the camera through the air. A light brings day when it is up (night falls as it sinks
/// from 10° above the horizon to 4° below) and brighter than a moon: more than 0.5 to 2 lux after
/// the air. So a sun is day however dim (one under a storm deck at noon gives about 10 lux), a
/// moon (0.3 lux at most) that is the brightest light still shows the stars, and a sun at full
/// illuminance below the horizon gives night even without an atmosphere to dim it.
///
/// The stand-in moon belongs to a sun: it shows at night when the light is brighter than a moon.
/// When the light is moon-dim it is the moon itself (above the horizon, drawn as the light's
/// disc) or a scene that drives a real moon (one that has set, or a sun dimmed below it), so no
/// second, stand-in moon is drawn.
pub fn sky_night(sun_dir: Vec3, up: Vec3, sun_color: Vec3, sun_camera: Vec3) -> (f32, f32) {
    let above = (sun_dir.dot(up) * 4.0 + 0.3).clamp(0.0, 1.0);
    let daylight = above * smoothstep(MOON_LUX, SUN_LUX, luminance(sun_camera));
    let night = 1.0 - daylight;
    let stand_in_moon = night * smoothstep(MOON_LUX, SUN_LUX, luminance(sun_color));
    (night, stand_in_moon)
}

/// Main light illuminance (lux) up to which it is taken for a moon (a full moon gives 0.3) …
const MOON_LUX: f32 = 0.5;
/// … and from which on for a sun, however dim (a sun under a storm deck gives about 10).
const SUN_LUX: f32 = 2.0;

/// The extra view being rendered (see [`crate::ExtraView`]): [`active_camera`] returns its
/// camera meanwhile, so everything that asks for the camera sees this view's.
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct RenderingExtraView(pub Entity);

/// The camera that renders: the active [`Camera`] with the highest `order` that is no
/// [`crate::ExtraView`] (while an extra view renders, its camera).
pub fn active_camera(world: &mut World) -> Option<(Entity, Camera, GlobalTransform)> {
    if let Some(RenderingExtraView(e)) = world.get_resource::<RenderingExtraView>().copied() {
        let mut q = world.query::<(&Camera, &GlobalTransform)>();
        return q.get(world, e).ok().map(|(cam, gt)| (e, *cam, *gt));
    }
    let mut best: Option<(Entity, Camera, GlobalTransform)> = None;
    let mut q = world.query::<(Entity, &Camera, &GlobalTransform, Has<crate::ExtraView>)>();
    for (e, cam, gt, extra) in q.iter(world) {
        if !cam.active || extra {
            continue;
        }
        if best.as_ref().is_none_or(|(_, b, _)| cam.order > b.order) {
            best = Some((e, *cam, *gt));
        }
    }
    best
}

/// Where the active camera draws in a window of `window` pixels (top-left corner, size): its
/// [`crate::Viewport`], or the whole window.
pub fn camera_viewport(world: &mut World, window: UVec2) -> (UVec2, UVec2) {
    let viewport = active_camera(world)
        .and_then(|(e, _, _)| world.get::<crate::components::Viewport>(e).copied())
        .unwrap_or_default();
    viewport.within(window)
}

/// Whether the camera that renders is a pixel-perfect one ([`crate::PixelPerfect`] on an
/// orthographic camera): it renders without temporal anti-aliasing.
pub fn active_camera_is_pixel_perfect(world: &mut World) -> bool {
    active_camera(world)
        .is_some_and(|(e, _, _)| crate::sprites::camera_pixel_grid(world, e, 1).is_some())
}

/// Build the view for the active camera (a default camera at the origin when there is none).
///
/// `jitter_px` shifts the projection by a fraction of a pixel (temporal anti-aliasing; ignored
/// for a pixel-perfect camera); `previous` is the previous frame's view, for motion vectors.
pub fn build_view(
    world: &mut World,
    width: u32,
    height: u32,
    previous: Option<PreviousView>,
    jitter_px: Vec2,
    exposure_state: &mut ExposureState,
    frame_index: u64,
) -> ViewData {
    let settings = crate::settings::effective(world);

    let (camera_entity, camera, gt) = match active_camera(world) {
        Some((e, c, g)) => (Some(e), c, g),
        None => (None, Camera::default(), GlobalTransform::default()),
    };
    let rotation = gt.rotation();
    // A pixel-perfect camera (`PixelPerfect`): a whole number of pixels per art texel, the view
    // moved to whole texels (or pixels) in its plane, and no sub-pixel jitter.
    let pixel_grid =
        camera_entity.and_then(|e| crate::sprites::camera_pixel_grid(world, e, height));
    let origin = match &pixel_grid {
        Some(grid) => grid.snap_camera(gt.translation(), rotation, width, height),
        None => gt.translation(),
    };
    let jitter_px = if pixel_grid.is_some() {
        Vec2::ZERO
    } else {
        jitter_px
    };
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let view = Mat4::from_quat(rotation.inverse());
    let (proj, near, fov_y, orthographic) = match camera.projection {
        Projection::Perspective { fov_y_deg, near } => {
            let fov = fov_y_deg.clamp(1.0, 170.0).to_radians();
            (
                perspective_infinite_reverse(fov, aspect, near.max(1e-4)),
                near,
                fov,
                false,
            )
        }
        Projection::Orthographic {
            height: h,
            near,
            far,
        } => {
            let h = pixel_grid.map_or(h, |g| g.height);
            (
                orthographic_reverse(h * aspect, h, near, far),
                near,
                0.0,
                true,
            )
        }
    };
    // A portal's view looks through the portal's rectangle: an off-axis frustum through its
    // corners whose near plane lies in the exit's plane (portal.rs). Shadow cascades fit a
    // symmetric frustum around it.
    // A mirror's view does the same around the part of the screen the mirror covers, with its
    // near plane where the mirror's plane begins (planar_reflection.rs).
    let off_axis = camera_entity.and_then(|e| {
        world
            .get::<crate::portal::PortalCamera>(e)
            .map(|p| (p.tangents, p.near))
            .or_else(|| {
                world
                    .get::<crate::planar_reflection::MirrorCamera>(e)
                    .map(|m| (m.tangents, m.near))
            })
    });
    let (proj, near, fov_y, cascade_fov_aspect) = match off_axis {
        Some((tangents, off_near)) => {
            let t = tangents;
            let half = Vec2::new(t.x.abs().max(t.y.abs()), t.z.abs().max(t.w.abs()))
                .max(Vec2::splat(1e-4));
            (
                off_axis_infinite_reverse(t, off_near),
                off_near,
                2.0 * ((t.w - t.z) * 0.5).atan(),
                (2.0 * half.y.atan(), half.x / half.y),
            )
        }
        None => (proj, near, fov_y, (fov_y, aspect)),
    };
    let unjittered_proj = proj;
    let unjittered_view_proj = proj * view;
    // Sub-pixel jitter in NDC (pixel y points down, NDC y up).
    let jitter_ndc = Vec2::new(
        2.0 * jitter_px.x / width.max(1) as f32,
        -2.0 * jitter_px.y / height.max(1) as f32,
    );
    let proj = Mat4::from_translation(jitter_ndc.extend(0.0)) * proj;
    let view_proj = proj * view;

    // Sun: the brightest directional light.
    let mut sun: Option<(Vec3, Vec3, bool, f32)> = None;
    let mut q = world.query::<(&DirectionalLight, &GlobalTransform)>();
    for (light, gt) in q.iter(world) {
        let color = Vec3::from(light.color) * light.illuminance;
        if sun
            .as_ref()
            .is_none_or(|(_, c, _, _)| luminance(color) > luminance(*c))
        {
            let dir = -(gt.rotation() * Vec3::NEG_Z);
            sun = Some((
                dir.normalize_or(Vec3::Y),
                color,
                light.cast_shadows,
                (light.angular_size_deg * 0.5).to_radians(),
            ));
        }
    }
    let (sun_dir, sun_color, sun_shadows, sun_angular_radius) =
        sun.unwrap_or((Vec3::Y, Vec3::ZERO, false, 0.0046));

    let atmosphere = *world.resource::<Atmosphere>();
    let center_rel = atmosphere.planet_center - origin;
    let sun_camera = sun_color * atmosphere.sun_transmittance(DVec3::ZERO, center_rel, sun_dir);

    // Exposure.
    let ambient = world.resource::<AmbientLight>();
    let ambient_lum = luminance(Vec3::from(ambient.color)) * ambient.brightness;
    let time = world.resource::<Time>();
    let real_dt = time.real_delta_secs();
    let elapsed = time.elapsed_secs_f64();
    let real_elapsed = time.real_elapsed_secs_f64();
    let delta = time.delta_secs();
    let ev100 = match camera.exposure {
        Exposure::Manual { ev100 } => {
            exposure_state.snap = false;
            ev100
        }
        Exposure::Auto { .. } if exposure_state.snap && exposure_state.measured_ev100.is_none() => {
            exposure_state.ev100
        }
        Exposure::Auto {
            min_ev100,
            max_ev100,
            speed,
            compensation,
        } => {
            let metered = exposure_state.measured_ev100.unwrap_or_else(|| {
                let lum = estimate_scene_luminance(sun_dir, sun_camera, ambient_lum).max(1e-4);
                (lum * 100.0 / 12.5).log2()
            });
            let incident = (estimate_scene_luminance(sun_dir, sun_camera, ambient_lum).max(1e-4)
                * 100.0
                / 12.5)
                .log2();
            let facing_sun = smoothstep(0.3, 0.8, (rotation * Vec3::NEG_Z).dot(sun_dir));
            let grade = exposure_grade(&settings, metered, incident, sun_dir.y, facing_sun);
            let target = (metered + grade - compensation - crate::exposure::METER_BIAS_EV)
                .clamp(min_ev100, max_ev100);
            if !exposure_state.initialized || exposure_state.snap {
                exposure_state.ev100 = target;
                exposure_state.snap = false;
            } else {
                let k = 1.0 - (-speed.max(0.0) * real_dt).exp();
                exposure_state.ev100 += (target - exposure_state.ev100) * k;
            }
            exposure_state.ev100
        }
    };
    exposure_state.initialized = true;
    exposure_state.ev100 = ev100;

    let wind = world
        .get_resource::<WindField>()
        .map(|w| w.sample(origin))
        .or_else(|| world.get_resource::<WeatherState>().map(|w| w.wind))
        .unwrap_or(Vec3::ZERO);

    // Point lights: nearest first.
    let mut lights: Vec<(f32, GpuPointLight)> = Vec::new();
    let mut q = world.query::<(&PointLight, &GlobalTransform)>();
    let frustum = Frustum::from_view_proj(&unjittered_view_proj);
    for (light, gt) in q.iter(world) {
        let rel = (gt.translation() - origin).as_vec3();
        if !frustum.intersects_sphere(rel, light.range) {
            continue;
        }
        let intensity_cd = light.intensity_lm / (4.0 * std::f32::consts::PI);
        let color = Vec3::from(light.color) * intensity_cd;
        lights.push((
            rel.length_squared(),
            GpuPointLight {
                position_range: [rel.x, rel.y, rel.z, light.range],
                color_radius: [color.x, color.y, color.z, light.radius.max(0.01)],
            },
        ));
    }
    lights.sort_by(|a, b| a.0.total_cmp(&b.0));
    lights.truncate(settings.max_point_lights as usize);

    // Indoor volumes: nearest first (by distance from the camera to the box's bounding sphere).
    let mut volumes: Vec<(f32, GpuSkyVolume)> = Vec::new();
    let mut q = world.query::<(&SkyLightVolume, &GlobalTransform)>();
    for (volume, gt) in q.iter(world) {
        let rel = (gt.translation() - origin).as_vec3();
        let radius = Vec3::from(volume.half_extents).length();
        volumes.push((
            (rel.length() - radius).max(0.0),
            GpuSkyVolume::new(volume, rel, gt.rotation()),
        ));
    }
    volumes.sort_by(|a, b| a.0.total_cmp(&b.0));
    volumes.truncate(MAX_SKY_VOLUMES);

    let cascades = if sun_shadows && luminance(sun_color) > 0.0 {
        crate::shadows::compute_cascades(
            &settings,
            origin,
            rotation,
            near,
            cascade_fov_aspect.0,
            cascade_fov_aspect.1,
            orthographic,
            sun_dir,
            &unjittered_proj,
        )
    } else {
        Cascades::default()
    };
    let far_shadow = if cascades.count > 0 {
        FarShadowView::new(
            origin,
            sun_dir,
            cascades.map_size,
            settings.far_shadow_distance,
            cascades.splits[cascades.count as usize - 1],
        )
    } else {
        FarShadowView::default()
    };

    ViewData {
        camera: camera_entity,
        extra_view: world.contains_resource::<RenderingExtraView>(),
        origin,
        rotation,
        view,
        proj,
        view_proj,
        unjittered_proj,
        unjittered_view_proj,
        prev_view_proj: previous.map_or(unjittered_view_proj, |p| p.view_proj),
        prev_origin: previous.map_or(origin, |p| p.origin),
        jitter: jitter_px,
        history_valid: previous.is_some(),
        frustum,
        near,
        fov_y,
        orthographic,
        width,
        height,
        window: UVec2::new(width, height),
        viewport_origin: UVec2::ZERO,
        viewport_size: UVec2::new(width, height),
        sun_dir,
        sun_color,
        sun_shadows: sun_shadows && cascades.count > 0,
        cascades,
        far_shadow,
        sky_occlusion: SkyOcclusionView::new(
            origin,
            settings.sky_occlusion_size.clamp(64, 4096),
            settings.sky_occlusion_extent.max(8.0),
            800.0,
            settings.sky_occlusion,
        ),
        exposure: ev100_to_exposure(ev100),
        ev100,
        wind,
        time: (elapsed % 8192.0) as f32,
        real_time: (real_elapsed % 8192.0) as f32,
        delta,
        frame_index,
        point_lights: lights.into_iter().map(|(_, l)| l).collect(),
        sky_volumes: volumes.into_iter().map(|(_, v)| v).collect(),
        debug_view: settings.debug_view,
        sun_angular_radius,
    }
}

fn mat(m: Mat4) -> [[f32; 4]; 4] {
    m.to_cols_array_2d()
}

/// Fill the GPU frame uniform.
pub fn frame_uniform(world: &World, v: &ViewData) -> FrameUniform {
    let settings = &crate::settings::effective(world);
    let atmosphere = world.resource::<Atmosphere>();
    let fog = world.resource::<FogSettings>();
    let ambient = world.resource::<AmbientLight>();

    let weather = world.get_resource::<WeatherState>();
    let surface = world
        .get_resource::<SurfaceLayers>()
        .map(|s| s.sample(v.origin));
    let wetness = surface.map(|s| s.wetness).unwrap_or(0.0);
    let snow = surface.map(|s| s.snow_m).unwrap_or(0.0);
    let cloud_cover = weather.map(|w| w.cloud_cover).unwrap_or(0.25);
    let precipitation = weather.map(|w| w.precipitation_mm_h).unwrap_or(0.0);
    let fog_density = fog.density + weather.map(|w| w.fog * 0.02).unwrap_or(0.0);

    let center_rel = atmosphere.planet_center - v.origin;
    let altitude = (-center_rel).length() - atmosphere.planet_radius;
    let sun_camera = v.sun_color * atmosphere.sun_transmittance(DVec3::ZERO, center_rel, v.sun_dir);
    let up = if atmosphere.enabled {
        (-center_rel).normalize_or(DVec3::Y).as_vec3()
    } else {
        Vec3::Y
    };
    let (night, stand_in_moon) = sky_night(v.sun_dir, up, v.sun_color, sun_camera);
    let frac = DVec3::new(
        v.origin.x.rem_euclid(4096.0),
        v.origin.y.rem_euclid(4096.0),
        v.origin.z.rem_euclid(4096.0),
    );
    let ambient_rgb = Vec3::from(ambient.color) * ambient.brightness;
    let air = atmosphere.optics();
    let rgb0 = |v: Vec3| [v.x, v.y, v.z, 0.0];
    let mut cascades = [[[0.0; 4]; 4]; 4];
    for (i, c) in v.cascades.view_proj.iter().enumerate() {
        cascades[i] = mat(*c);
    }

    FrameUniform {
        view: mat(v.view),
        proj: mat(v.proj),
        view_proj: mat(v.view_proj),
        inv_view: mat(v.view.inverse()),
        inv_proj: mat(v.proj.inverse()),
        inv_view_proj: mat(v.view_proj.inverse()),
        prev_view_proj: mat(v.prev_view_proj),
        camera_world: [
            v.origin.x as f32,
            v.origin.y as f32,
            v.origin.z as f32,
            altitude as f32,
        ],
        camera_world_frac: [frac.x as f32, frac.y as f32, frac.z as f32, 0.0],
        viewport: [
            v.width as f32,
            v.height as f32,
            1.0 / v.width.max(1) as f32,
            1.0 / v.height.max(1) as f32,
        ],
        time: [v.time, v.real_time, v.delta, (v.frame_index % 65536) as f32],
        sun_dir: [v.sun_dir.x, v.sun_dir.y, v.sun_dir.z, v.sun_angular_radius],
        sun_color: [
            v.sun_color.x,
            v.sun_color.y,
            v.sun_color.z,
            if v.sun_shadows { 1.0 } else { 0.0 },
        ],
        sun_camera: [sun_camera.x, sun_camera.y, sun_camera.z, 0.0],
        ambient: [
            ambient_rgb.x,
            ambient_rgb.y,
            ambient_rgb.z,
            luminance(Vec3::from(atmosphere.ground_color)),
        ],
        fog: [
            fog_density,
            fog.height_falloff,
            fog.base_height,
            fog.max_opacity,
        ],
        fog_color: [
            fog.color[0].max(0.0),
            fog.color[1].max(0.0),
            fog.color[2].max(0.0),
            fog.sun_scattering.clamp(-0.95, 0.95),
        ],
        wind: [v.wind.x, v.wind.y, v.wind.z, 0.0],
        weather: [
            wetness.clamp(0.0, 1.0),
            snow.max(0.0),
            cloud_cover,
            precipitation,
        ],
        planet: if atmosphere.enabled {
            [
                center_rel.x as f32,
                center_rel.y as f32,
                center_rel.z as f32,
                atmosphere.planet_radius as f32,
            ]
        } else {
            [0.0; 4]
        },
        atmosphere: [
            atmosphere.thickness,
            atmosphere.rayleigh_scale_height,
            atmosphere.mie_scale_height,
            atmosphere.mie_g,
        ],
        exposure: [
            v.exposure,
            v.ev100,
            v.near,
            if v.orthographic { 1.0 } else { 0.0 },
        ],
        shadow_cascades: cascades,
        shadow_splits: v.cascades.splits,
        shadow_params: [
            settings.shadow_depth_bias,
            settings.shadow_normal_bias,
            settings.shadow_softness,
            v.cascades.map_size as f32,
        ],
        counts: [
            v.point_lights.len() as u32,
            v.cascades.count,
            match v.debug_view {
                DebugView::None => 0,
                DebugView::Normals => 1,
                DebugView::Cascades => 2,
                DebugView::SkyVisibility => 3,
                DebugView::AmbientOcclusion => 4,
                DebugView::Reflections => 5,
                DebugView::IndirectLight => 6,
                DebugView::Clusters => 7,
                DebugView::ClusterLevels => 8,
                DebugView::VirtualTextures => 9,
            },
            v.sky_volumes.len() as u32,
        ],
        ground: [
            atmosphere.ground_color[0],
            atmosphere.ground_color[1],
            atmosphere.ground_color[2],
            // Light bounced off the ground fades out between 20 and 200 km up.
            1.0 - smoothstep(20_000.0, 200_000.0, altitude as f32),
        ],
        sky_occlusion: mat(v.sky_occlusion.view_proj),
        sky_occlusion_params: v.sky_occlusion.params(),
        sky_volumes: std::array::from_fn(|i| v.sky_volumes.get(i).copied().unwrap_or_default()),
        unjittered_view_proj: mat(v.unjittered_view_proj),
        jitter: [
            2.0 * v.jitter.x / v.width.max(1) as f32,
            -2.0 * v.jitter.y / v.height.max(1) as f32,
            v.jitter.x,
            v.jitter.y,
        ],
        camera_delta: {
            let d = v.camera_delta();
            [d.x, d.y, d.z, if v.history_valid { 1.0 } else { 0.0 }]
        },
        effects: [
            if settings.ambient_occlusion {
                settings.ao_intensity.clamp(0.0, 2.0)
            } else {
                0.0
            },
            settings.contact_shadow_length.max(0.0),
            settings.sun_penumbra_scale.max(0.0),
            settings.ao_radius.max(0.05),
        ],
        reflections: [
            if settings.screen_space_reflections {
                1.0
            } else {
                0.0
            },
            0.0,
            settings.ssr_max_roughness.clamp(0.0, 1.0),
            if settings.screen_space_gi {
                settings.gi_intensity.max(0.0)
            } else {
                0.0
            },
        ],
        volumetrics: crate::volumetrics::frame_params(settings, v),
        far_shadow: mat(v.far_shadow.view_proj),
        far_shadow_params: v.far_shadow.params(),
        sky: [
            if settings.sky_cloud_layer { 1.0 } else { 0.0 },
            night,
            stand_in_moon,
            0.0,
        ],
        surface_map: [0.0; 4],
        surface_open: [0.0; 4],
        cloud_shadow: GpuCloudShadow::default(),
        air_rayleigh: rgb0(air.rayleigh),
        air_mie: rgb0(air.mie_scattering),
        air_mie_extinction: rgb0(air.mie_extinction),
        air_ozone: rgb0(air.ozone),
        air_mie_g: [air.mie_g.x, air.mie_g.y, air.mie_g.z, air.mie_forward_peak],
        window: [
            v.viewport_origin.x as f32,
            v.viewport_origin.y as f32,
            v.viewport_size.x as f32,
            v.viewport_size.y as f32,
        ],
        // Filled by the renderer's probe GI when it is on.
        gi_volume: [0.0, 0.0, 0.0, 1.0],
        gi_size: [1.0, 1.0, 1.0, 0.0],
        gi_grid: [0.0; 4],
        clip_plane: v
            .camera
            .and_then(|c| world.get::<crate::planar_reflection::MirrorCamera>(c))
            .map_or([0.0; 4], |m| m.clip_plane(v.origin)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opening_packs_to_the_centimeter() {
        let unpack = |f: f32| {
            let b = f.to_bits();
            [(b & 0xffff) as f32, (b >> 16) as f32].map(|c| c * 0.01 - 327.68)
        };
        for p in [[-5.0, 0.0], [5.0, 6.5], [-327.68, 327.67], [0.004, -0.004]] {
            let q = unpack(pack_opening_corner(p));
            assert!(
                (q[0] - p[0]).abs() < 0.006 && (q[1] - p[1]).abs() < 0.006,
                "{p:?} -> {q:?}"
            );
        }
        // Beyond the range: clamped to its ends.
        let q = unpack(pack_opening_corner([-400.0, 400.0]));
        assert!(
            (q[0] + 327.68).abs() < 1e-3 && (q[1] - 327.67).abs() < 1e-3,
            "{q:?}"
        );
    }

    #[test]
    fn caves_and_interiors_by_day_show_darker_than_the_valley() {
        let settings = RenderSettings::default();
        let noon = 15.2;
        let grade = |metered: f32, sun_y: f32| exposure_grade(&settings, metered, noon, sun_y, 0.0);
        // The valley in the sun, and shade under trees: as metered.
        assert_eq!(grade(15.3, 0.9), 0.0);
        assert_eq!(grade(13.5, 0.9), 0.0);
        // The breach house and the mushroom cave at noon.
        let house = grade(12.0, 0.9);
        assert!(house > 0.4 && house < 1.0, "{house}");
        assert_eq!(grade(7.4, 0.9), settings.interior_exposure_bias);
        // Not by night, and never on top of the night bias.
        assert_eq!(grade(7.4, -0.3), 0.0);
        assert_eq!(grade(-2.5, 0.9), settings.night_exposure_bias);
        let off = RenderSettings {
            interior_exposure_bias: 0.0,
            ..RenderSettings::default()
        };
        assert_eq!(exposure_grade(&off, 7.4, noon, 0.9, 0.0), 0.0);
    }

    #[test]
    fn frame_uniform_matches_wgsl_size() {
        // 7 mat4 + 15 vec4 + 4 mat4 + 3 vec4 + ground, sky occlusion, volumes, then the
        // screen-space block (mat4 + 4 vec4), volumetric fog (vec4), far shadow (mat4 + vec4),
        // sky (vec4), surface weather (2 vec4), cloud shadows (mat4 + 2 vec4), the air (5 vec4),
        // the window (vec4), probe GI (3 vec4), the clip plane (vec4).
        assert_eq!(
            std::mem::size_of::<FrameUniform>(),
            7 * 64
                + 15 * 16
                + 4 * 64
                + 3 * 16
                + 16
                + 64
                + 16
                + MAX_SKY_VOLUMES * 80
                + 64
                + 5 * 16
                + 64
                + 16
                + 16
                + 2 * 16
                + 64
                + 2 * 16
                + 5 * 16
                + 16
                + 3 * 16
                + 16
        );
        assert_eq!(std::mem::size_of::<GpuSkyVolume>(), 80);
        assert_eq!(std::mem::size_of::<GpuPointLight>(), 32);
    }

    #[test]
    fn night_follows_the_main_lights_illuminance() {
        let up = Vec3::Y;
        let high = Vec3::new(0.0, 0.8, -0.6);
        let low = Vec3::new(0.0, -0.3, -0.95).normalize();
        let sun = Vec3::splat(110_000.0);
        let moon = Vec3::splat(0.3);
        // A sun up: day, no stand-in moon.
        assert_eq!(sky_night(high, up, sun, sun * 0.8), (0.0, 0.0));
        // A moon up and the brightest light: night, and its own disc is the moon.
        assert_eq!(sky_night(high, up, moon, moon * 0.8), (1.0, 0.0));
        // A sun dimmed to 12 lux by a storm deck at noon, 60° up: still day.
        let up60 = Vec3::new(0.0, 0.866, -0.5);
        let storm = Vec3::splat(12.0);
        assert_eq!(sky_night(up60, up, storm, storm * 0.85), (0.0, 0.0));
        // A sun at full illuminance below the horizon: night with the stand-in moon, whether
        // the air dims it to nothing or there is no atmosphere at all.
        assert_eq!(sky_night(low, up, sun, Vec3::ZERO), (1.0, 1.0));
        assert_eq!(sky_night(low, up, sun, sun), (1.0, 1.0));
        // A moon below the horizon: still night, no stand-in.
        assert_eq!(sky_night(low, up, moon, moon), (1.0, 0.0));
        // On a planet, up is away from its center, not world y.
        let side = Vec3::X;
        assert_eq!(sky_night(Vec3::X, side, sun, sun), (0.0, 0.0));
        assert_eq!(sky_night(Vec3::NEG_X, side, sun, sun).0, 1.0);
        // Dusk: a sun on the horizon is half way.
        let horizon = sky_night(Vec3::new(0.0, 0.0, -1.0), up, sun, sun * 0.01);
        assert!((0.6..0.8).contains(&horizon.0), "{horizon:?}");
    }

    #[test]
    fn reverse_z_perspective() {
        let p = perspective_infinite_reverse(1.0, 1.0, 0.1);
        let near = p * Vec4::new(0.0, 0.0, -0.1, 1.0);
        assert!((near.z / near.w - 1.0).abs() < 1e-5);
        let far = p * Vec4::new(0.0, 0.0, -1.0e9, 1.0);
        assert!(far.z / far.w < 1e-6);
    }

    #[test]
    fn reverse_z_orthographic() {
        let p = orthographic_reverse(10.0, 10.0, 0.0, 100.0);
        let near = p * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let far = p * Vec4::new(0.0, 0.0, -100.0, 1.0);
        assert!((near.z - 1.0).abs() < 1e-5);
        assert!(far.z.abs() < 1e-5);
    }

    #[test]
    fn daylight_exposure_is_sunny_sixteen() {
        let sun = Vec3::new(0.3, 0.9, 0.2).normalize();
        let lum = estimate_scene_luminance(sun, Vec3::splat(100_000.0), 0.0);
        let ev = (lum * 100.0 / 12.5).log2();
        assert!((14.0..16.5).contains(&ev), "ev {ev}");
        let night =
            estimate_scene_luminance(Vec3::new(0.0, -0.5, 0.8).normalize(), Vec3::ZERO, 0.05);
        let ev_night = (night * 100.0 / 12.5).log2();
        assert!(ev_night < 0.0, "ev {ev_night}");
    }
}
