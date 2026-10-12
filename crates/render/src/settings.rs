//! Tunable render settings. All of these are registered resources, so the AI bridge, console
//! and scripts can read and patch them by name (`resource.set RenderSettings {...}`).

use engine_core::prelude::*;

/// Tonemapping curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tonemapper {
    /// AgX with Blender's "Punchy" look: AgX's graceful highlights with more contrast and
    /// color, closer to a camera's picture (default).
    #[default]
    AgXPunchy,
    /// Filmic, hue-preserving; handles very bright colors gracefully. Flatter than `AgXPunchy`.
    AgX,
    /// Khronos PBR Neutral: keeps base colors as authored up to bright highlights (product
    /// shots, checking materials).
    Neutral,
    /// The classic ACES filmic look (more contrast, saturated highlights).
    Aces,
    Reinhard,
    /// No curve: clamps. For debugging.
    None,
}

/// How edges are smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AntiAliasing {
    /// Temporal anti-aliasing (default): the camera shifts by a fraction of a pixel every
    /// frame and the results are blended along motion vectors. Smooth edges, stable thin
    /// geometry (grass, wires), and it also cleans up the noise of ambient occlusion and soft
    /// shadows. Needs a few frames after a camera cut to converge.
    #[default]
    Taa,
    /// No anti-aliasing (sharpest, jagged edges).
    None,
}

/// What the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DebugView {
    #[default]
    None,
    /// World-space normals.
    Normals,
    /// Shadow cascades tinted.
    Cascades,
    /// How much open sky each surface sees (white = outdoors, black = cave or interior).
    SkyVisibility,
    /// Screen-space ambient occlusion (white = open, black = crevices and contact).
    AmbientOcclusion,
    /// Screen-space reflections: what each surface reflects from the screen, times how much of
    /// its reflection that covers (black where only the sky is reflected).
    Reflections,
    /// Screen-space global illumination: the light bounced onto each surface from nearby
    /// surfaces (before it is multiplied by the surface's color).
    IndirectLight,
}

/// Renderer-wide quality and look settings.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    /// Present with vsync.
    pub vsync: bool,
    /// Shadows are drawn up to this distance from the camera, meters.
    pub shadow_distance: f32,
    /// Shadow map resolution per cascade (applied at startup).
    pub shadow_map_size: u32,
    /// 0 = uniform cascade splits, 1 = logarithmic.
    pub shadow_split_lambda: f32,
    pub shadow_depth_bias: f32,
    /// Normal offset in shadow texels.
    pub shadow_normal_bias: f32,
    /// Smallest shadow filter radius in texels (the edge is never sharper than this).
    pub shadow_softness: f32,
    /// Scale of the sun's physical penumbra (1 = the real sun's size: shadows are crisp at
    /// contact and soften with distance from the caster; 2 = twice as soft).
    pub sun_penumbra_scale: f32,
    /// Screen-space contact shadows: each lit pixel marches this far (meters) toward the sun
    /// through the depth buffer, catching small shadows the shadow map is too coarse for
    /// (pebbles, grass, feet on the ground). 0 = off.
    pub contact_shadow_length: f32,
    /// Ground-truth ambient occlusion (GTAO) from the depth prepass: darkens ambient light in
    /// creases, under objects and where things touch.
    pub ambient_occlusion: bool,
    /// How far (meters) occluders are searched for around each pixel.
    pub ao_radius: f32,
    /// 0..2 strength of the ambient occlusion (1 = physical).
    pub ao_intensity: f32,
    /// Screen-space reflections: smooth and glossy surfaces reflect what is on screen (as of
    /// the previous frame), blending into the prefiltered sky where the reflection leaves the
    /// screen or is hidden.
    pub screen_space_reflections: bool,
    /// Surfaces rougher than this (perceptual roughness) reflect only the sky.
    pub ssr_max_roughness: f32,
    /// Screen-space global illumination: diffuse light bounced off nearby surfaces that are on
    /// screen (sunlit walls tint the ground, lamps and emissive surfaces light their
    /// surroundings). Builds up over a few frames.
    pub screen_space_gi: bool,
    /// Strength of the bounced light (1 = physical).
    pub gi_intensity: f32,
    pub anti_aliasing: AntiAliasing,
    /// 0..1 sharpening after temporal anti-aliasing (restores texture detail).
    pub sharpness: f32,
    pub tonemapper: Tonemapper,
    /// 0..1 mix of the blurred image (0.04 is subtle and physical).
    pub bloom_intensity: f32,
    pub saturation: f32,
    pub contrast: f32,
    pub vignette: f32,
    pub film_grain: f32,
    /// Warm (+) / cool (−) tint.
    pub white_balance: f32,
    /// Maximum point lights shaded per frame (nearest first).
    pub max_point_lights: u32,
    pub debug_view: DebugView,
    /// Re-render the sky environment map every N frames (1 = every frame).
    pub environment_interval: u32,
    /// Top-down sky occlusion map around the camera (caves and interiors lose sky light; rain
    /// stays off covered ground).
    pub sky_occlusion: bool,
    /// Width of the area it covers, meters (resolution is `sky_occlusion_size`²).
    pub sky_occlusion_extent: f32,
    pub sky_occlusion_size: u32,
    /// How many EV darker than "average gray" very dark scenes are shown (0 = a moonlit night
    /// is exposed like noon; 2.5 = night looks like night). Ramps in below EV100 ≈ 5.
    pub night_exposure_bias: f32,
    /// 0..1 loss of color in very dark areas (scotopic vision: moonlight looks blue-gray).
    pub night_desaturation: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            vsync: true,
            shadow_distance: 350.0,
            shadow_map_size: 2048,
            shadow_split_lambda: 0.8,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 1.5,
            shadow_softness: 1.0,
            sun_penumbra_scale: 1.0,
            contact_shadow_length: 0.4,
            ambient_occlusion: true,
            ao_radius: 1.5,
            ao_intensity: 1.0,
            screen_space_reflections: true,
            ssr_max_roughness: 0.6,
            screen_space_gi: true,
            gi_intensity: 1.0,
            anti_aliasing: AntiAliasing::Taa,
            sharpness: 0.3,
            tonemapper: Tonemapper::AgXPunchy,
            bloom_intensity: 0.04,
            saturation: 1.05,
            contrast: 1.0,
            vignette: 0.2,
            film_grain: 0.004,
            white_balance: 0.0,
            max_point_lights: 256,
            debug_view: DebugView::None,
            environment_interval: 1,
            sky_occlusion: true,
            sky_occlusion_extent: 256.0,
            sky_occlusion_size: 512,
            night_exposure_bias: 2.5,
            night_desaturation: 0.7,
        }
    }
}

/// Height fog. Weather drives `density`; artists tune the rest.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FogSettings {
    /// Extinction per meter at `base_height` (0.0005 light haze, 0.02 thick fog).
    pub density: f32,
    /// How fast fog thins with height, per meter.
    pub height_falloff: f32,
    /// Altitude above the planet surface (world y when there is no planet) where `density`
    /// applies, meters.
    pub base_height: f32,
    /// Fog never hides more than this fraction of an object (keeps silhouettes).
    pub max_opacity: f32,
    /// Fog albedo (it is lit by the sky and the sun).
    pub color: [f32; 3],
    /// Forward scattering glow around the sun.
    pub sun_scattering: f32,
}

impl Default for FogSettings {
    fn default() -> Self {
        Self {
            density: 0.0004,
            height_falloff: 0.02,
            base_height: 0.0,
            max_opacity: 1.0,
            color: [0.9, 0.93, 1.0],
            sun_scattering: 0.6,
        }
    }
}

/// The planet whose atmosphere the sky, aerial perspective and sunlight use. Space travel
/// updates this to the nearest planet (or disables it in deep space).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Atmosphere {
    pub enabled: bool,
    /// Planet center in world coordinates, meters.
    pub planet_center: DVec3,
    pub planet_radius: f64,
    /// Height of the atmosphere above the surface, meters.
    pub thickness: f32,
    pub rayleigh_scale_height: f32,
    pub mie_scale_height: f32,
    /// Mie anisotropy (0.76 hazy .. 0.9 clear).
    pub mie_g: f32,
    /// Albedo (linear RGB) of the planet surface beyond the terrain: the ground seen from high
    /// up and toward the horizon, and the color of light bounced off the ground.
    pub ground_color: [f32; 3],
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self {
            enabled: true,
            planet_center: DVec3::new(0.0, -6_360_000.0, 0.0),
            planet_radius: 6_360_000.0,
            thickness: 100_000.0,
            rayleigh_scale_height: 8_000.0,
            mie_scale_height: 1_200.0,
            mie_g: 0.8,
            ground_color: [0.12, 0.15, 0.09],
        }
    }
}

impl Atmosphere {
    /// Fraction of sunlight (RGB) reaching camera-relative point `p` through the atmosphere,
    /// matching `sun_transmittance` in the shader library.
    pub fn sun_transmittance(&self, p_rel: DVec3, center_rel: DVec3, sun_dir: Vec3) -> Vec3 {
        if !self.enabled || self.planet_radius <= 0.0 {
            return Vec3::ONE;
        }
        const RAYLEIGH: Vec3 = Vec3::new(5.802e-6, 13.558e-6, 33.1e-6);
        const MIE_EXT: f32 = 4.40e-6;
        let sun = sun_dir.as_dvec3();
        let r_planet = self.planet_radius;
        let r_atmo = r_planet + self.thickness as f64;
        if let Some((t0, _)) = ray_sphere(p_rel, sun, center_rel, r_planet * 0.9995)
            && t0 > 0.0
        {
            return Vec3::ZERO;
        }
        let Some((_, t1)) = ray_sphere(p_rel, sun, center_rel, r_atmo) else {
            return Vec3::ONE;
        };
        let seg = t1.max(0.0) / 6.0;
        let (mut sr, mut sm) = (0.0f64, 0.0f64);
        for j in 0..6 {
            let q = p_rel + sun * ((j as f64 + 0.5) * seg);
            let h = ((q - center_rel).length() - r_planet).max(0.0);
            sr += (-h / self.rayleigh_scale_height as f64).exp() * seg;
            sm += (-h / self.mie_scale_height as f64).exp() * seg;
        }
        let tau = RAYLEIGH * sr as f32 + Vec3::splat(MIE_EXT * sm as f32);
        Vec3::new((-tau.x).exp(), (-tau.y).exp(), (-tau.z).exp())
    }
}

fn ray_sphere(origin: DVec3, dir: DVec3, center: DVec3, radius: f64) -> Option<(f64, f64)> {
    let oc = origin - center;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let h = b * b - c;
    if h < 0.0 {
        return None;
    }
    let s = h.sqrt();
    Some((-b - s, -b + s))
}
