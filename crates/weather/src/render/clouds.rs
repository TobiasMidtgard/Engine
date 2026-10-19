//! Volumetric clouds. Each frame: march the cloud layer at reduced resolution (compute
//! phase), then composite over the sky (sky phase) and over geometry the clouds are in front
//! of (transparent phase). The environment map gets a cheaper march so ambient light and
//! reflections darken under overcast and catch sunset colors on the clouds.

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_render::{
    Atmosphere, CLOUD_SHADOW_FORMAT, CloudShadowRequest, FrameContext, GpuShared, RenderFeature,
    RenderPhase, RenderSettings, ViewData, wgpu,
};
use engine_world::{FarCloudLayer, WeatherState};
use serde_json::json;

use super::{CloudSettings, depth_always, transmittance_blend};
use crate::climate::{self, Climate};
use crate::cloudmap::CloudMap;
use crate::lightning::ActiveBolts;
use crate::presets::cloud_thickness_m;
use crate::sim::WeatherSim;
use crate::sky::{Sky, SkyLights};

const SHAPE_SIZE: u32 = 128;
const DETAIL_SIZE: u32 = 32;
const MARCH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const NOISE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Moon's angular radius, radians (0.26°).
const MOON_RADIUS: f32 = 0.0045;
const MOON_COLOR: [f32; 3] = [0.95, 0.93, 0.88];
const KEY_MOON_COLOR: [f32; 3] = [0.78, 0.84, 1.0];
/// Where falling rain starts to veil the view and where it does so fully, m: up close rain is
/// streaks, and even a downpour leaves a kilometer or more of visibility.
const RAIN_VEIL_M: [f32; 2] = [400.0, 2500.0];
/// The same for snow, which hides the distance a few hundred meters out: just past the flakes
/// drawn around the camera.
const SNOW_VEIL_M: [f32; 2] = [10.0, 40.0];

/// The march's `veil`: where falling rain or snow starts to veil the view and where fully (m),
/// and how much of it is snow (0..1; from the temperature as `kind_for` decides, sleet half).
fn precipitation_veil(temperature_c: f32) -> [f32; 4] {
    let snow = smoothstep(2.0, 0.5, temperature_c);
    let at = |i: usize| RAIN_VEIL_M[i] + (SNOW_VEIL_M[i] - RAIN_VEIL_M[i]) * snow;
    [at(0), at(1), snow, 0.0]
}

/// How much the weather already dimmed the light the renderer shades with (its brightest, the
/// sun or the moon) for thick cloud at the player ([`SkyLights::cloud_dimming`], with the
/// same floors). The cloud shadow map holds the sunlight the clouds let through relative to
/// that, so the ground under the deck isn't dimmed twice; gaps in it stay at the dimmed sun.
fn shadow_dimming(lights: &SkyLights) -> f32 {
    if !lights.in_charge {
        return 1.0;
    }
    let sun = lights.cloud_dimming.max(crate::sky::SUN_DIMMING_FLOOR);
    let moon = lights.cloud_dimming.max(crate::sky::MOON_DIMMING_FLOOR);
    if lights.sun_lux * sun >= lights.moon_lux * moon {
        sun
    } else {
        moon
    }
}

/// The march's `far`: where the clouds hand over to a far cloud layer another module draws
/// (`FarCloudLayer`: inner and outer ground distance, m, and how much of it is drawn). Without
/// one the weight is 0 and every cloud is drawn.
fn far_handover(layer: Option<&FarCloudLayer>) -> [f32; 4] {
    layer
        .filter(|l| l.weight > 0.0 && l.outer_m.is_finite() && l.inner_m.is_finite())
        .map(|l| [l.inner_m, l.outer_m.max(l.inner_m), l.weight.min(1.0), 0.0])
        .unwrap_or([0.0; 4])
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct CloudUniform {
    layer: [f32; 4],
    map: [f32; 4],
    noise: [f32; 4],
    light: [f32; 4],
    light_dir: [f32; 4],
    background: [f32; 4],
    flash: [f32; 4],
    params: [f32; 4],
    detail: [f32; 4],
    veil: [f32; 4],
    shadow: [f32; 4],
    far: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct CelestialUniform {
    sun: [f32; 4],
    sun_color: [f32; 4],
    moon: [f32; 4],
    moon_color: [f32; 4],
    pole: [f32; 4],
    params: [f32; 4],
    veil: [f32; 4],
}

struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Target {
    fn new(device: &wgpu::Device, label: &str, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MARCH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            _texture: texture,
            view,
        }
    }
}

struct Targets {
    size: (u32, u32),
    color: Target,
    depth: Target,
    sky: Target,
}

struct Gpu {
    uniform: wgpu::Buffer,
    celestial: wgpu::Buffer,
    map: wgpu::Texture,
    group: wgpu::BindGroup,
    noise_group: wgpu::BindGroup,
    gen_shape: wgpu::ComputePipeline,
    gen_detail: wgpu::ComputePipeline,
    march: wgpu::RenderPipeline,
    env: wgpu::RenderPipeline,
    sky_replace: wgpu::RenderPipeline,
    sky_over: wgpu::RenderPipeline,
    geo: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
}

/// The volumetric cloud [`RenderFeature`].
#[derive(Default)]
pub struct CloudFeature {
    gpu: Option<Gpu>,
    targets: Option<Targets>,
    composite: Option<wgpu::BindGroup>,
    noise_ready: bool,
    map_revision: u64,
    active: bool,
    replace_sky: bool,
    top_m: f32,
    shadow_request: Option<CloudShadowRequest>,
}

fn noise_texture(device: &wgpu::Device, label: &str, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: NOISE_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(
    binding: u32,
    dim: wgpu::TextureViewDimension,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

fn float_tex(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    texture_entry(
        binding,
        dim,
        wgpu::TextureSampleType::Float { filterable: true },
    )
}

/// One fullscreen-triangle render pipeline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fullscreen_pipeline(
    gpu: &GpuShared,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    targets: &[Option<wgpu::ColorTargetState>],
    depth: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    gpu.device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: depth,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets,
            }),
            multiview_mask: None,
            cache: None,
        })
}

fn target(format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
    }
}

impl CloudFeature {
    fn ensure_targets(&mut self, device: &wgpu::Device, width: u32, height: u32, scale: f32) {
        let scale = scale.clamp(0.125, 1.0);
        let size = (
            ((width as f32 * scale).round() as u32).max(1),
            ((height as f32 * scale).round() as u32).max(1),
        );
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        self.targets = Some(Targets {
            size,
            color: Target::new(device, "clouds color", size.0, size.1),
            depth: Target::new(device, "clouds depth", size.0, size.1),
            sky: Target::new(device, "clouds clear sky", size.0, size.1),
        });
        self.composite = None;
    }
}

impl RenderFeature for CloudFeature {
    fn name(&self) -> &str {
        "weather clouds"
    }

    fn order(&self) -> i32 {
        -90
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clouds"),
            size: std::mem::size_of::<CloudUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let celestial = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("celestial"),
            size: std::mem::size_of::<CelestialUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shape = noise_texture(device, "cloud shape noise", SHAPE_SIZE);
        let detail = noise_texture(device, "cloud detail noise", DETAIL_SIZE);
        let shape_view = shape.create_view(&Default::default());
        let detail_view = detail.create_view(&Default::default());
        let map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud weather map"),
            size: wgpu::Extent3d {
                width: crate::cloudmap::MAP_SIZE,
                height: crate::cloudmap::MAP_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let map_view = map.create_view(&Default::default());

        // Noise generation (compute, once).
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: NOISE_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        };
        let noise_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cloud noise"),
            entries: &[storage(0), storage(1)],
        });
        let noise_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cloud noise"),
            layout: &noise_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shape_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&detail_view),
                },
            ],
        });
        let noise_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cloud noise"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/cloud_noise.wgsl").into()),
        });
        let noise_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cloud noise"),
                bind_group_layouts: &[Some(&noise_layout)],
                immediate_size: 0,
            });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&noise_pipeline_layout),
                module: &noise_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let gen_shape = compute("gen_shape");
        let gen_detail = compute("gen_detail");

        // March + environment.
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds"),
            entries: &[
                uniform_entry(0),
                float_tex(1, wgpu::TextureViewDimension::D3),
                float_tex(2, wgpu::TextureViewDimension::D3),
                float_tex(3, wgpu::TextureViewDimension::D2),
            ],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clouds"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shape_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&detail_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&map_view),
                },
            ],
        });
        let shader = gpu.shader("clouds", include_str!("../shaders/clouds.wgsl"), true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clouds"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let march = fullscreen_pipeline(
            gpu,
            "clouds march",
            &pipeline_layout,
            &shader,
            "vs_full",
            "fs_march",
            &[
                Some(target(MARCH_FORMAT, None)),
                Some(target(MARCH_FORMAT, None)),
                Some(target(MARCH_FORMAT, None)),
            ],
            None,
        );
        let env = fullscreen_pipeline(
            gpu,
            "clouds environment",
            &pipeline_layout,
            &shader,
            "vs_env",
            "fs_env",
            &[Some(target(gpu.formats.env, None))],
            None,
        );
        let shadow = fullscreen_pipeline(
            gpu,
            "cloud shadows",
            &pipeline_layout,
            &shader,
            "vs_full",
            "fs_cloud_shadow",
            &[Some(target(CLOUD_SHADOW_FORMAT, None))],
            None,
        );

        // Composites.
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds composite"),
            entries: &[
                uniform_entry(0),
                float_tex(1, wgpu::TextureViewDimension::D2),
                float_tex(2, wgpu::TextureViewDimension::D2),
                float_tex(3, wgpu::TextureViewDimension::D2),
                texture_entry(
                    4,
                    wgpu::TextureViewDimension::D2,
                    wgpu::TextureSampleType::Depth,
                ),
            ],
        });
        let composite_shader = gpu.shader(
            "clouds composite",
            include_str!("../shaders/cloud_composite.wgsl"),
            true,
        );
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("clouds composite"),
                bind_group_layouts: &[Some(&gpu.frame_layout), Some(&composite_layout)],
                immediate_size: 0,
            });
        let sky_replace = fullscreen_pipeline(
            gpu,
            "clouds over sky (replace)",
            &composite_pipeline_layout,
            &composite_shader,
            "vs_sky",
            "fs_sky",
            &[Some(target(gpu.formats.hdr, None))],
            Some(gpu.depth_state(false)),
        );
        let sky_over = fullscreen_pipeline(
            gpu,
            "clouds over sky",
            &composite_pipeline_layout,
            &composite_shader,
            "vs_sky",
            "fs_sky_over",
            &[Some(target(gpu.formats.hdr, Some(transmittance_blend())))],
            Some(gpu.depth_state(false)),
        );
        let geo = fullscreen_pipeline(
            gpu,
            "clouds over geometry",
            &composite_pipeline_layout,
            &composite_shader,
            "vs_geo",
            "fs_geo",
            &[Some(target(gpu.formats.hdr, Some(transmittance_blend())))],
            Some(depth_always(gpu)),
        );

        self.gpu = Some(Gpu {
            uniform,
            celestial,
            map,
            group,
            noise_group,
            gen_shape,
            gen_detail,
            march,
            env,
            sky_replace,
            sky_over,
            geo,
            shadow,
            composite_layout,
        });
        self.noise_ready = false;
        self.map_revision = 0;
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        let settings = world
            .get_resource::<CloudSettings>()
            .copied()
            .unwrap_or_default();
        self.shadow_request = None;
        self.active = settings.enabled
            && !view.orthographic
            && self.gpu.is_some()
            && world.contains_resource::<WeatherSim>()
            && world.contains_resource::<CloudMap>();
        if !self.active {
            return;
        }
        self.replace_sky = settings.replace_sky;
        self.ensure_targets(
            &gpu.device,
            view.width,
            view.height,
            settings.resolution_scale,
        );
        let Some(g) = &self.gpu else { return };

        let map = world.resource::<CloudMap>();
        if map.revision != self.map_revision {
            self.map_revision = map.revision;
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &g.map,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&map.texels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(map.size * 4),
                    rows_per_image: Some(map.size),
                },
                wgpu::Extent3d {
                    width: map.size,
                    height: map.size,
                    depth_or_array_layers: 1,
                },
            );
            let max_type = map
                .texels
                .iter()
                .filter(|t| t[0] > 8)
                .map(|t| t[1])
                .max()
                .unwrap_or(0) as f32
                / 255.0;
            self.top_m = cloud_thickness_m(max_type.max(map.background.y));
        }

        let sim = world.resource::<WeatherSim>();
        let state = world.resource::<WeatherState>();
        let lights = world.resource::<SkyLights>();
        let sky = world.resource::<Sky>();
        let climate = world.resource::<Climate>();
        let origin = view.origin;
        let altitude = state.cloud_base_m.max(150.0);

        // Map placement: the pattern has drifted with the wind since the map was built.
        let span = map.size as f64 * map.cell_m as f64;
        let moved = sim
            .frame
            .dir_to_world((sim.cloud_drift - map.drift_at_build).as_vec2());
        let corner = map.origin + moved.as_dvec2() - DVec2::new(origin.x, origin.z);
        let wrap = |v: f64, period: f32| v.rem_euclid(period as f64) as f32;
        // The noise sticks to the drifting pattern (and stays put when the world shifts).
        let noise = sim.frame.to_pattern(DVec2::new(origin.x, origin.z)) - sim.cloud_drift;
        let (noise_x, noise_z) = (noise.x, noise.y);

        // Key light for the clouds: the sun by day, the moon by night (not dimmed by cloud).
        let (sun_dir, sun_lux, sun_radius) = if lights.in_charge {
            (lights.sun_dir, lights.sun_lux, 0.0047)
        } else {
            (
                view.sun_dir,
                view.sun_color.max_element(),
                view.sun_angular_radius.max(0.004),
            )
        };
        let (moon_dir, moon_lit) = if lights.in_charge {
            (lights.moon_dir, lights.moon_lit)
        } else {
            climate::moon(
                climate,
                state.time_of_day_h,
                state.day_of_year,
                sky.moon_phase,
            )
        };
        let moon_lux = if lights.in_charge {
            lights.moon_lux
        } else {
            0.0
        };
        let (key_dir, key_color) = if sun_lux >= moon_lux || !lights.in_charge {
            let c = if lights.in_charge {
                Vec3::splat(sun_lux)
            } else {
                view.sun_color
            };
            (sun_dir, c)
        } else {
            (moon_dir, Vec3::from(KEY_MOON_COLOR) * moon_lux)
        };
        let night = smoothstep(0.05, -0.25, sun_dir.y);
        let stars = smoothstep(-0.02, -0.25, sun_dir.y);
        let visibility = 1.0 - smoothstep(30_000.0, 120_000.0, view.origin.y as f32);
        let veil = precipitation_veil(state.temperature_c);
        let shadow_dimming = shadow_dimming(lights);
        self.shadow_request = world
            .get_resource::<Atmosphere>()
            .filter(|a| a.enabled && settings.cloud_shadows)
            .map(|a| {
                // The map lies in the world's y; the cloud base is an altitude.
                let camera_altitude = (origin - a.planet_center).length() - a.planet_radius;
                let base_height = origin.y + (altitude as f64 - camera_altitude);
                // Beyond the map: about the sunlight the region's cover lets through.
                let outside = (1.0 - 0.8 * sim.regional_cover.clamp(0.0, 1.0)) / shadow_dimming;
                CloudShadowRequest {
                    size: settings.cloud_shadow_size,
                    extent: settings.cloud_shadow_extent_m,
                    base_height,
                    top_height: base_height + self.top_m as f64,
                    outside: outside.min(1.0),
                }
            });

        let flash = world
            .get_resource::<ActiveBolts>()
            .and_then(|b| b.brightest())
            .filter(|(_, b)| *b > 0.01);
        let flash = flash
            .map(|(at, b)| {
                let rel = (at - origin).as_vec3();
                let intensity = world
                    .get_resource::<crate::lightning::Lightning>()
                    .map(|l| l.flash_lumens)
                    .unwrap_or(4e9)
                    / (4.0 * PI);
                [rel.x, rel.y, rel.z, intensity * b]
            })
            .unwrap_or([0.0; 4]);

        let u = CloudUniform {
            layer: [
                altitude,
                altitude + self.top_m,
                settings.extinction,
                settings.max_distance_m,
            ],
            map: [corner.x as f32, corner.y as f32, span as f32, 1.0],
            noise: [
                wrap(noise_x, settings.shape_scale_m),
                wrap(origin.y, settings.shape_scale_m),
                wrap(noise_z, settings.shape_scale_m),
                settings.shape_scale_m,
            ],
            light: [
                key_color.x,
                key_color.y,
                key_color.z,
                settings.steps.clamp(8, 256) as f32,
            ],
            light_dir: [
                key_dir.x,
                key_dir.y,
                key_dir.z,
                settings.light_steps.min(12) as f32,
            ],
            background: map.background.to_array(),
            flash,
            params: [
                visibility,
                settings.detail_scale_m,
                settings.environment_steps.clamp(4, 128) as f32,
                night,
            ],
            detail: [
                wrap(noise_x, settings.detail_scale_m),
                wrap(origin.y, settings.detail_scale_m),
                wrap(noise_z, settings.detail_scale_m),
                if settings.rain_shafts { 1.0 } else { 0.0 },
            ],
            veil,
            shadow: [
                shadow_dimming,
                settings.cloud_shadow_steps.clamp(4, 64) as f32,
                0.0,
                0.0,
            ],
            far: far_handover(world.get_resource::<FarCloudLayer>()),
        };
        gpu.queue
            .write_buffer(&g.uniform, 0, bytemuck::bytes_of(&u));

        // Rainbows need sunlight reaching the rain from behind the viewer: a sun below 42°
        // (the bow sinks under the horizon above that) that the clouds overhead don't hide.
        let sun_elevation = sun_dir.y.clamp(-1.0, 1.0).asin().to_degrees();
        let rainbow_lux = if settings.rain_shafts && lights.in_charge {
            sun_lux
                * smoothstep(0.4, 0.9, lights.cloud_dimming)
                * smoothstep(42.0, 32.0, sun_elevation)
                * smoothstep(0.0, 3.0, sun_elevation)
                * (1.0 - veil[2])
        } else {
            0.0
        };
        let moon_radius = MOON_RADIUS * settings.moon_scale.clamp(0.25, 8.0);
        let moon_solid_angle = PI * moon_radius * moon_radius;
        let moon_luminance = sky.moon_lux / moon_solid_angle * 0.55;
        let lat = climate.latitude_deg.to_radians();
        let sidereal = (state.time_of_day_h / 24.0 + state.day_of_year / climate.year_days) * TAU;
        let size = self.targets.as_ref().map(|t| t.size).unwrap_or((1, 1));
        let c = CelestialUniform {
            sun: [sun_dir.x, sun_dir.y, sun_dir.z, sun_radius],
            sun_color: [sun_lux, sun_lux, sun_lux, sim.frame.yaw as f32],
            moon: [moon_dir.x, moon_dir.y, moon_dir.z, moon_radius],
            moon_color: [
                MOON_COLOR[0] * moon_luminance,
                MOON_COLOR[1] * moon_luminance,
                MOON_COLOR[2] * moon_luminance,
                0.012 * (1.0 - moon_lit),
            ],
            pole: [0.0, lat.sin(), lat.cos(), sidereal],
            params: [
                stars * settings.star_brightness.max(0.0),
                rainbow_lux,
                1.0 / size.0 as f32,
                1.0 / size.1 as f32,
            ],
            veil,
        };
        gpu.queue
            .write_buffer(&g.celestial, 0, bytemuck::bytes_of(&c));
    }

    fn cloud_shadows(&self) -> Option<CloudShadowRequest> {
        self.shadow_request.filter(|_| self.active)
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if !self.active {
            return;
        }
        let (Some(g), Some(t)) = (&self.gpu, &self.targets) else {
            return;
        };
        if !self.noise_ready {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud noise"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &g.noise_group, &[]);
            pass.set_pipeline(&g.gen_shape);
            let n = SHAPE_SIZE.div_ceil(4);
            pass.dispatch_workgroups(n, n, n);
            pass.set_pipeline(&g.gen_detail);
            let n = DETAIL_SIZE.div_ceil(4);
            pass.dispatch_workgroups(n, n, n);
            self.noise_ready = true;
        }
        if let Some(target) = &ctx.cloud_shadows {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cloud shadows"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&g.shadow);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &g.group, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let attachment = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clouds march"),
                color_attachments: &[
                    attachment(&t.color.view),
                    attachment(&t.depth.view),
                    attachment(&t.sky.view),
                ],
                ..Default::default()
            });
            pass.set_pipeline(&g.march);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &g.group, &[]);
            pass.draw(0..3, 0..1);
        }
        let entries = [
            wgpu::BindGroupEntry {
                binding: 0,
                resource: g.celestial.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&t.color.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&t.depth.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&t.sky.view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
            },
        ];
        // Rebuilt every frame: the renderer's depth copy is recreated on resize.
        self.composite = Some(
            ctx.gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("clouds composite"),
                    layout: &g.composite_layout,
                    entries: &entries,
                }),
        );
    }

    fn draw_environment(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if !self.active || !self.noise_ready {
            return;
        }
        if let Some(g) = &self.gpu {
            pass.set_pipeline(&g.env);
            pass.set_bind_group(1, &g.group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if !self.active {
            return;
        }
        let (Some(g), Some(group)) = (&self.gpu, &self.composite) else {
            return;
        };
        let pipeline = match phase {
            RenderPhase::Sky if self.replace_sky => &g.sky_replace,
            RenderPhase::Sky => &g.sky_over,
            RenderPhase::Transparent => &g.geo,
            RenderPhase::Opaque => return,
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn needs_scene_copy(&self) -> bool {
        self.active
    }

    fn describe(&self) -> serde_json::Value {
        json!({
            "active": self.active,
            "march_size": self.targets.as_ref().map(|t| [t.size.0, t.size.1]),
            "layer_thickness_m": self.top_m,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clouds_hand_over_to_a_far_layer_only_while_one_is_drawn() {
        assert_eq!(far_handover(None), [0.0; 4]);
        let mut layer = FarCloudLayer {
            inner_m: 42_000.0,
            outer_m: 60_000.0,
            weight: 0.0,
        };
        assert_eq!(far_handover(Some(&layer)), [0.0; 4]);
        layer.weight = 1.5;
        assert_eq!(far_handover(Some(&layer)), [42_000.0, 60_000.0, 1.0, 0.0]);
    }

    #[test]
    fn cloud_shadows_are_relative_to_the_light_the_deck_already_dimmed() {
        let mut lights = SkyLights {
            in_charge: false,
            cloud_dimming: 0.2,
            ..Default::default()
        };
        // Lights the weather doesn't drive aren't dimmed by it.
        assert_eq!(shadow_dimming(&lights), 1.0);
        lights.in_charge = true;
        assert!((shadow_dimming(&lights) - 0.2).abs() < 1e-6);
        // A storm at noon: the sun keeps its tiny floor and stays the brighter light.
        lights.cloud_dimming = 0.0;
        assert_eq!(shadow_dimming(&lights), crate::sky::SUN_DIMMING_FLOOR);
        // At night the moon lights the land, with its own floor.
        lights.sun_lux = 0.0;
        lights.moon_lux = 0.3;
        assert_eq!(shadow_dimming(&lights), crate::sky::MOON_DIMMING_FLOOR);
    }

    #[test]
    fn snow_veils_the_view_from_just_past_the_flakes() {
        let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        assert!(close(precipitation_veil(10.0), [400.0, 2500.0, 0.0, 0.0]));
        assert!(close(precipitation_veil(-5.0), [10.0, 40.0, 1.0, 0.0]));
        // Sleet is half rain, half snow.
        let sleet = precipitation_veil(1.25);
        assert!((sleet[2] - 0.5).abs() < 1e-3);
        assert!(sleet[0] > 10.0 && sleet[0] < 400.0, "{sleet:?}");
    }
}
