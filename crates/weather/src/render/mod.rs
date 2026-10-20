//! What the weather looks like: volumetric clouds and rain curtains, the sun, moon and stars,
//! falling rain and snow with splashes, lightning bolts, and puddles that reflect the world.
//! All of it plugs into the renderer as [`RenderFeature`](engine_render::RenderFeature)s.

mod clouds;
mod ground;
mod precipitation;

use engine_core::prelude::*;
use engine_render::{RenderAppExt, wgpu};
use serde::{Deserialize, Serialize};

pub use clouds::CloudFeature;
pub use ground::GroundFeature;
pub use precipitation::PrecipitationFeature;

/// Volumetric cloud quality and look.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudSettings {
    pub enabled: bool,
    /// March resolution relative to the screen (0.25–1).
    pub resolution_scale: f32,
    /// Samples along each view ray through the cloud layer.
    pub steps: u32,
    /// Samples toward the sun or moon from each cloud sample (self-shadowing).
    pub light_steps: u32,
    /// Samples per ray in the environment map (ambient light and reflections).
    pub environment_steps: u32,
    /// Extinction at full density, 1/m (higher = darker, more solid clouds).
    pub extinction: f32,
    /// Clouds fade out by this distance, m.
    pub max_distance_m: f32,
    /// Size of the billowy shape noise tile, m.
    pub shape_scale_m: f32,
    /// Size of the wispy detail noise tile, m.
    pub detail_scale_m: f32,
    /// Draw the clear sky behind the clouds too, covering the sky's own flat cloud layer.
    pub replace_sky: bool,
    /// Grey curtains of rain under distant rain clouds.
    pub rain_shafts: bool,
    /// Star and Milky Way brightness relative to the real sky (the eye sees more stars at
    /// night than a linear exposure shows).
    pub star_brightness: f32,
    /// Moon disc size relative to the real one (the moon looks bigger to the eye than in a
    /// photo of the same view).
    pub moon_scale: f32,
    /// Clouds cast shadows on the ground and into the fog (the renderer's cloud shadow map).
    pub cloud_shadows: bool,
    /// Texels per side of the cloud shadow map.
    pub cloud_shadow_size: u32,
    /// Side of the square the cloud shadow map covers around the camera, m.
    pub cloud_shadow_extent_m: f32,
    /// Samples toward the sun through the cloud layer per shadow texel.
    pub cloud_shadow_steps: u32,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            resolution_scale: 0.5,
            steps: 72,
            light_steps: 5,
            environment_steps: 24,
            extinction: 0.08,
            max_distance_m: 120_000.0,
            shape_scale_m: 4_000.0,
            detail_scale_m: 700.0,
            replace_sky: true,
            rain_shafts: true,
            star_brightness: 4.0,
            moon_scale: 2.0,
            cloud_shadows: true,
            cloud_shadow_size: 512,
            cloud_shadow_extent_m: 48_000.0,
            cloud_shadow_steps: 16,
        }
    }
}

/// Falling rain and snow, splashes and lightning bolts.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrecipitationSettings {
    pub enabled: bool,
    /// Rain streaks in the heaviest rain.
    pub max_drops: u32,
    /// Snowflakes in the heaviest snow.
    pub max_flakes: u32,
    /// Splash rings on the ground in the heaviest rain.
    pub max_splashes: u32,
    /// Width and depth of the box of falling drops around the camera, m.
    pub area_m: f32,
    /// Raindrop diameter, m.
    pub drop_width_m: f32,
    /// Streak length as camera shutter time, s.
    pub shutter_s: f32,
    /// Draw lightning bolts.
    pub bolts: bool,
}

impl Default for PrecipitationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_drops: 100_000,
            max_flakes: 120_000,
            max_splashes: 3_000,
            area_m: 30.0,
            drop_width_m: 0.0025,
            shutter_s: 1.0 / 30.0,
            bolts: true,
        }
    }
}

/// Puddles, wet and snowy ground drawn from the simulated ground cover.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundEffectSettings {
    pub enabled: bool,
    /// Screen-space reflection steps in puddles (0 = reflect only the sky).
    pub reflection_steps: u32,
    /// Raindrop rings on puddles.
    pub ripples: bool,
}

impl Default for GroundEffectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reflection_steps: 32,
            ripples: true,
        }
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<CloudSettings>()
        .init_resource::<PrecipitationSettings>()
        .init_resource::<GroundEffectSettings>()
        .register_resource::<CloudSettings>(
            "Volumetric clouds: enabled, resolution_scale, steps, light_steps, \
             environment_steps, extinction (1/m), max_distance_m, shape_scale_m, \
             detail_scale_m, replace_sky, rain_shafts.",
        )
        .register_resource::<PrecipitationSettings>(
            "Falling rain and snow: enabled, max_drops, max_flakes, max_splashes, area_m, \
             drop_width_m, shutter_s (streak length), bolts (draw lightning).",
        )
        .register_resource::<GroundEffectSettings>(
            "Puddles and wet/snowy ground: enabled, reflection_steps, ripples.",
        )
        .add_render_feature(CloudFeature::default())
        .add_render_feature(GroundFeature::default())
        .add_render_feature(PrecipitationFeature::default())
        .add_systems(Update, sky_cloud_layer_system);
}

/// While the volumetric clouds are on, the renderer's own flat cloud layer is off: where the
/// clouds blend over the sky (`replace_sky` off) it showed as a second deck under them, and
/// elsewhere it was drawn only to be painted over. It comes back when the clouds are switched
/// off, unless something else had turned it off.
fn sky_cloud_layer_system(
    clouds: Res<CloudSettings>,
    settings: Option<ResMut<engine_render::RenderSettings>>,
    mut turned_off: Local<bool>,
) {
    let Some(mut settings) = settings else {
        return;
    };
    if clouds.enabled {
        if settings.sky_cloud_layer {
            settings.sky_cloud_layer = false;
            *turned_off = true;
        }
    } else if *turned_off {
        settings.sky_cloud_layer = true;
        *turned_off = false;
    }
}

/// Standard alpha blending for premultiplied color where alpha is how much of the background
/// still shows (transmittance).
pub(crate) fn transmittance_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

/// Depth state that never rejects and never writes (fullscreen passes over everything).
pub(crate) fn depth_always(gpu: &engine_render::GpuShared) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: gpu.formats.depth,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn shaders_validate() {
        for (name, src, frame) in [
            (
                "cloud_noise",
                include_str!("../shaders/cloud_noise.wgsl"),
                false,
            ),
            ("clouds", include_str!("../shaders/clouds.wgsl"), true),
            (
                "cloud_composite",
                include_str!("../shaders/cloud_composite.wgsl"),
                true,
            ),
            (
                "precipitation",
                include_str!("../shaders/precipitation.wgsl"),
                true,
            ),
            ("ground", include_str!("../shaders/ground.wgsl"), true),
        ] {
            let code = if name == "cloud_noise" {
                src.to_string()
            } else {
                compose_shader(src, frame)
            };
            if let Err(e) = validate_wgsl(name, &code) {
                panic!("{e}");
            }
        }
    }
}
