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

/// Share of the night fill that lights the clouds as moonlight instead (`moon_fill`).
const MOON_FILL_SHARE: f32 = 0.7;

/// On a moonlit night the ambient fill (the night floor that keeps the ground readable)
/// outshines the moon several times over. Spread evenly through the clouds it made a flat
/// blue-grey ceiling with no shape, and the clear breaks in it read as dark clouds. Most of it
/// reaches the clouds from the moon's side instead: the same light (an isotropic radiance L
/// scatters out as much as 4πL from one direction), so tops and edges toward the moon turn
/// silver and thick middles shade. Returns the illuminance to add to the moon's key light
/// (lux) and the share of the fill left (0..1).
fn moon_fill(moon_key: bool, moon_lux: f32, ambient_luminance: f32) -> (f32, f32) {
    if !moon_key {
        return (0.0, 1.0);
    }
    let share = MOON_FILL_SHARE * smoothstep(0.02, 0.15, moon_lux);
    (share * 4.0 * PI * ambient_luminance, 1.0 - share)
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
        Self::with_format(device, label, width, height, MARCH_FORMAT)
    }

    fn with_format(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
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
            format,
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

/// One image of the clouds: in-scattered light and transmittance, depths, clear sky.
struct CloudImage {
    color: Target,
    depth: Target,
    sky: Target,
}

impl CloudImage {
    fn new(device: &wgpu::Device, label: &str, (w, h): (u32, u32)) -> Self {
        Self {
            color: Target::new(device, &format!("{label} color"), w, h),
            depth: Target::new(device, &format!("{label} depth"), w, h),
            sky: Target::new(device, &format!("{label} clear sky"), w, h),
        }
    }

    fn attachments(&self) -> [Option<wgpu::RenderPassColorAttachment<'_>>; 3] {
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
        [
            attachment(&self.color.view),
            attachment(&self.depth.view),
            attachment(&self.sky.view),
        ]
    }
}

/// The uniform the march and the reconstruction share (see cloud_reproject.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct ReprojectUniform {
    offset: [f32; 4],
    size: [f32; 4],
    exposure: [f32; 4],
}

/// The pixel of each 2x2 block marched on each frame of four: diagonals first, so two frames
/// already cover the block evenly.
const BLOCK_ORDER: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)];
/// The same for 3x3 blocks: each next pixel far from the last ones.
const BLOCK_ORDER_3: [(f32, f32); 9] = [
    (0.0, 0.0),
    (2.0, 1.0),
    (1.0, 2.0),
    (1.0, 0.0),
    (0.0, 2.0),
    (2.0, 2.0),
    (0.0, 1.0),
    (2.0, 0.0),
    (1.0, 1.0),
];
/// And for 4x4 blocks: the order of a Bayer matrix, so any four frames in a row cover the
/// block evenly.
const BLOCK_ORDER_4: [(f32, f32); 16] = [
    (0.0, 0.0),
    (2.0, 2.0),
    (2.0, 0.0),
    (0.0, 2.0),
    (1.0, 1.0),
    (3.0, 3.0),
    (3.0, 1.0),
    (1.0, 3.0),
    (1.0, 0.0),
    (3.0, 2.0),
    (3.0, 0.0),
    (1.0, 2.0),
    (0.0, 1.0),
    (2.0, 3.0),
    (2.0, 1.0),
    (0.0, 3.0),
];

/// The largest block side (`CloudSettings::temporal_side`).
const MAX_BLOCK_SIDE: u32 = 4;

/// The block pixel marched on frame `frame` with blocks `side` pixels a side.
fn block_offset(side: u32, frame: u32) -> (f32, f32) {
    let order: &[(f32, f32)] = match side {
        2 => &BLOCK_ORDER,
        3 => &BLOCK_ORDER_3,
        4 => &BLOCK_ORDER_4,
        _ => return (0.0, 0.0),
    };
    order[frame as usize % order.len()]
}

/// The block side for a frame doing `share` of the work spread over frames
/// (`engine_render::FramePacing::share`), given the side in use: the side whose block takes as
/// long to march in full as a 2x2 block does at the pacing rate (2 / sqrt(share)), changed only
/// once that is well past the next side, so a frame rate hovering between two doesn't switch
/// back and forth.
fn paced_side(share: f32, current: u32) -> u32 {
    let ideal = 2.0 / share.clamp(0.25, 1.0).sqrt();
    let current = current.clamp(2, MAX_BLOCK_SIDE);
    if (ideal - current as f32).abs() <= 0.7 {
        return current;
    }
    (ideal.round() as u32).clamp(2, MAX_BLOCK_SIDE)
}

/// The cloud shadow map's world-anchored cache (see `fs_cloud_shadow_update`).
struct ShadowCache {
    size: u32,
    map: Target,
    uniform: wgpu::Buffer,
    update_group: wgpu::BindGroup,
    copy_group: wgpu::BindGroup,
    frames: u32,
    /// Rows to march this frame: all of them, or a quarter band.
    rows: (u32, u32),
    /// What the cache was last filled for: the map's corner (world texels), the world's
    /// turn, the sun.
    last: Option<([i64; 2], f32, Vec3)>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct ShadowCacheUniform {
    min_index: [i32; 4],
    params: [u32; 4],
}

impl ShadowCache {
    fn new(device: &wgpu::Device, g: &Gpu, size: u32) -> Self {
        let map = Target::with_format(
            device,
            "cloud shadow cache",
            size,
            size,
            CLOUD_SHADOW_FORMAT,
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cloud shadow cache"),
            size: std::mem::size_of::<ShadowCacheUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let update_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cloud shadow update"),
            layout: &g.shadow_update_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            }],
        });
        let copy_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cloud shadow copy"),
            layout: &g.shadow_copy_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&map.view),
                },
            ],
        });
        Self {
            size,
            map,
            uniform,
            update_group,
            copy_group,
            frames: 0,
            rows: (0, size),
            last: None,
        }
    }
}

/// One camera's cloud images.
struct Targets {
    camera: Option<Entity>,
    /// The cloud image's size.
    size: (u32, u32),
    /// Pixels per block side: 1 marches all of the image each frame, 2 a quarter, up to
    /// `MAX_BLOCK_SIDE` (a sixteenth).
    side: u32,
    march: CloudImage,
    /// The reconstructed image, this frame's and last frame's (`current` is this frame's).
    history: [CloudImage; 2],
    current: usize,
    /// Frames drawn, picking the block pixel to march.
    frames: u32,
    /// Whether `history` holds a frame yet.
    drawn: bool,
    /// The pre-exposure the latest history was drawn with.
    exposure: f32,
    uniform: wgpu::Buffer,
    march_group: wgpu::BindGroup,
    /// Reads the march and `history[1 - i]`, for drawing `history[i]`.
    reproject_groups: [wgpu::BindGroup; 2],
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        g: &Gpu,
        camera: Option<Entity>,
        size: (u32, u32),
        side: u32,
    ) -> Self {
        let msize = (size.0.div_ceil(side), size.1.div_ceil(side));
        let march = CloudImage::new(device, "clouds march", msize);
        let history = [
            CloudImage::new(device, "clouds", size),
            CloudImage::new(device, "clouds", size),
        ];
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("clouds reproject"),
            size: std::mem::size_of::<ReprojectUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let march_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clouds march slot"),
            layout: &g.slot_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let reproject_groups = Self::reproject_groups(device, g, &uniform, &march, &history);
        Self {
            camera,
            size,
            side,
            march,
            history,
            current: 0,
            frames: 0,
            drawn: false,
            exposure: 1.0,
            uniform,
            march_group,
            reproject_groups,
        }
    }

    /// The groups that read the march and `history[1 - i]`, for drawing `history[i]`.
    fn reproject_groups(
        device: &wgpu::Device,
        g: &Gpu,
        uniform: &wgpu::Buffer,
        march: &CloudImage,
        history: &[CloudImage; 2],
    ) -> [wgpu::BindGroup; 2] {
        fn tex(binding: u32, t: &Target) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(&t.view),
            }
        }
        let reproject_group = |prev: &CloudImage| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("clouds reproject"),
                layout: &g.reproject_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    tex(1, &march.color),
                    tex(2, &march.depth),
                    tex(3, &march.sky),
                    tex(4, &prev.color),
                    tex(5, &prev.depth),
                    tex(6, &prev.sky),
                ],
            })
        };
        [reproject_group(&history[1]), reproject_group(&history[0])]
    }

    /// March one pixel of blocks `side` a side from now on. The image so far stays: the next
    /// frames reproject it as before, only the fresh samples come more thinly or more thickly.
    fn set_side(&mut self, device: &wgpu::Device, g: &Gpu, side: u32) {
        self.side = side;
        self.march = CloudImage::new(device, "clouds march", self.march_size());
        self.reproject_groups =
            Self::reproject_groups(device, g, &self.uniform, &self.march, &self.history);
        self.frames = 0;
    }

    fn march_size(&self) -> (u32, u32) {
        (
            self.size.0.div_ceil(self.side),
            self.size.1.div_ceil(self.side),
        )
    }

    /// This frame's image (what the composites read).
    fn image(&self) -> &CloudImage {
        &self.history[self.current]
    }
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
    shadow_update: wgpu::RenderPipeline,
    shadow_copy: wgpu::RenderPipeline,
    shadow_update_layout: wgpu::BindGroupLayout,
    shadow_copy_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    slot_layout: wgpu::BindGroupLayout,
    reproject_layout: wgpu::BindGroupLayout,
    reproject: wgpu::RenderPipeline,
}

/// The volumetric cloud [`RenderFeature`].
#[derive(Default)]
pub struct CloudFeature {
    gpu: Option<Gpu>,
    /// One set per camera, so a mirror or portal view doesn't recreate the main view's.
    views: Vec<Targets>,
    /// Index in `views` of the view being drawn.
    current: Option<usize>,
    composite: Option<wgpu::BindGroup>,
    noise_ready: bool,
    map_revision: u64,
    active: bool,
    replace_sky: bool,
    top_m: f32,
    shadow_request: Option<CloudShadowRequest>,
    shadow_cache: Option<ShadowCache>,
    /// The block side the frame rate asked for last (`paced_side`).
    paced_side: u32,
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
    /// The most cameras whose targets are kept; the least recently used goes first.
    const MAX_VIEWS: usize = 4;

    fn ensure_targets(&mut self, device: &wgpu::Device, view: &ViewData, scale: f32, side: u32) {
        let Some(g) = &self.gpu else { return };
        let scale = scale.clamp(0.125, 1.0);
        let size = (
            ((view.width as f32 * scale).round() as u32).max(1),
            ((view.height as f32 * scale).round() as u32).max(1),
        );
        let found = self.views.iter().position(|t| t.camera == view.camera);
        let targets = match found {
            Some(i) if self.views[i].size == size => {
                let mut targets = self.views.remove(i);
                if targets.side != side {
                    targets.set_side(device, g, side);
                }
                targets
            }
            found => {
                if let Some(i) = found {
                    self.views.remove(i);
                } else if self.views.len() >= Self::MAX_VIEWS {
                    self.views.remove(0);
                }
                Targets::new(device, g, view.camera, size, side)
            }
        };
        self.views.push(targets);
        self.current = Some(self.views.len() - 1);
        self.composite = None;
    }

    fn targets(&self) -> Option<&Targets> {
        self.current.and_then(|i| self.views.get(i))
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
        let slot_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds march slot"),
            entries: &[uniform_entry(0)],
        });
        let march_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("clouds march"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout), Some(&slot_layout)],
            immediate_size: 0,
        });
        let march = fullscreen_pipeline(
            gpu,
            "clouds march",
            &march_layout,
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
        let shadow_update_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cloud shadow update"),
                entries: &[uniform_entry(1)],
            });
        let shadow_copy_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cloud shadow copy"),
                entries: &[
                    uniform_entry(1),
                    float_tex(2, wgpu::TextureViewDimension::D2),
                ],
            });
        let shadow_pipeline = |name: &str, fs: &str, cache: &wgpu::BindGroupLayout| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout), Some(cache)],
                immediate_size: 0,
            });
            fullscreen_pipeline(
                gpu,
                name,
                &layout,
                &shader,
                "vs_full",
                fs,
                &[Some(target(CLOUD_SHADOW_FORMAT, None))],
                None,
            )
        };
        let shadow_update = shadow_pipeline(
            "cloud shadow update",
            "fs_cloud_shadow_update",
            &shadow_update_layout,
        );
        let shadow_copy = shadow_pipeline(
            "cloud shadow copy",
            "fs_cloud_shadow_copy",
            &shadow_copy_layout,
        );

        // Temporal reconstruction.
        let reproject_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds reproject"),
            entries: &[
                uniform_entry(0),
                float_tex(1, wgpu::TextureViewDimension::D2),
                float_tex(2, wgpu::TextureViewDimension::D2),
                float_tex(3, wgpu::TextureViewDimension::D2),
                float_tex(4, wgpu::TextureViewDimension::D2),
                float_tex(5, wgpu::TextureViewDimension::D2),
                float_tex(6, wgpu::TextureViewDimension::D2),
            ],
        });
        let reproject_shader = gpu.shader(
            "clouds reproject",
            include_str!("../shaders/cloud_reproject.wgsl"),
            true,
        );
        let reproject_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("clouds reproject"),
                bind_group_layouts: &[Some(&gpu.frame_layout), Some(&reproject_layout)],
                immediate_size: 0,
            });
        let reproject = fullscreen_pipeline(
            gpu,
            "clouds reproject",
            &reproject_pipeline_layout,
            &reproject_shader,
            "vs_full",
            "fs_reproject",
            &[
                Some(target(MARCH_FORMAT, None)),
                Some(target(MARCH_FORMAT, None)),
                Some(target(MARCH_FORMAT, None)),
            ],
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
            shadow_update,
            shadow_copy,
            shadow_update_layout,
            shadow_copy_layout,
            composite_layout,
            slot_layout,
            reproject_layout,
            reproject,
        });
        self.views.clear();
        self.current = None;
        self.shadow_cache = None;
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
        // Reflections and insets are wavy or small: march them at half the scale.
        let scale = if view.extra_view {
            settings.resolution_scale * 0.5
        } else {
            settings.resolution_scale
        };
        let side = if !settings.temporal {
            1
        } else if settings.temporal_side >= 2 {
            settings.temporal_side.min(MAX_BLOCK_SIDE)
        } else {
            // The main view follows the frame rate; mirrors and insets take its side.
            if !view.extra_view {
                let share = world
                    .get_resource::<engine_render::FramePacing>()
                    .map_or(1.0, |p| p.share);
                self.paced_side = paced_side(share, self.paced_side);
            }
            self.paced_side
        };
        self.ensure_targets(&gpu.device, view, scale, side);
        if let Some(t) = self.current.and_then(|i| self.views.get_mut(i)) {
            t.current = 1 - t.current;
            let (ox, oy) = block_offset(t.side, t.frames);
            t.frames = t.frames.wrapping_add(1);
            let usable = t.drawn && view.history_valid;
            let ratio = view.exposure / t.exposure.max(1e-12);
            t.exposure = view.exposure;
            let m = t.march_size();
            let u = ReprojectUniform {
                offset: [ox, oy, t.side as f32, if usable { 1.0 } else { 0.0 }],
                size: [t.size.0 as f32, t.size.1 as f32, m.0 as f32, m.1 as f32],
                exposure: [ratio, 0.0, 0.0, 0.0],
            };
            gpu.queue
                .write_buffer(&t.uniform, 0, bytemuck::bytes_of(&u));
        }
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
        let moon_key = lights.in_charge && moon_lux > sun_lux;
        let ambient = world
            .get_resource::<engine_render::AmbientLight>()
            .map(|a| Vec3::from(a.color).dot(Vec3::new(0.2126, 0.7152, 0.0722)) * a.brightness)
            .unwrap_or(0.0);
        let (fill_lux, fill_left) = moon_fill(moon_key, moon_lux, ambient);
        let (key_dir, key_color) = if !moon_key {
            let c = if lights.in_charge {
                Vec3::splat(sun_lux)
            } else {
                view.sun_color
            };
            (sun_dir, c)
        } else {
            (moon_dir, Vec3::from(KEY_MOON_COLOR) * (moon_lux + fill_lux))
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
        if let (Some(request), Some(g)) = (&self.shadow_request, &self.gpu) {
            let size = request.size.clamp(
                engine_render::cloud_shadows::MIN_CLOUD_SHADOW_SIZE,
                engine_render::cloud_shadows::MAX_CLOUD_SHADOW_SIZE,
            );
            if self.shadow_cache.as_ref().is_none_or(|c| c.size != size) {
                self.shadow_cache = Some(ShadowCache::new(&gpu.device, g, size));
            }
            let cache = self.shadow_cache.as_mut().expect("just made");
            // The map's corner in world texels, as the renderer places it.
            let texel = request.extent.max(1.0) as f64 / size as f64;
            let half = request.extent.max(1.0) as f64 * 0.5;
            let corner = [
                ((origin.x - half) / texel).round() as i64,
                ((origin.z - half) / texel).round() as i64,
            ];
            let yaw = sim.frame.yaw as f32;
            // Refresh all of it when it would show: a jump (a world shift, a teleport), the
            // world turned, or the sun moved more than a few texels' worth of shadow.
            let full = cache.last.is_none_or(|(c, y, sun)| {
                let jump = (corner[0] - c[0]).abs().max((corner[1] - c[1]).abs());
                jump > size as i64 / 8 || y != yaw || sun.dot(view.sun_dir) < 0.99999
            });
            if full {
                cache.last = Some((corner, yaw, view.sun_dir));
            } else if let Some(last) = &mut cache.last {
                last.0 = corner;
            }
            let band = size.div_ceil(4);
            cache.rows = if full {
                (0, size)
            } else {
                let first = (cache.frames % 4) * band;
                (first, band.min(size - first))
            };
            cache.frames = cache.frames.wrapping_add(1);
            // Indices wrap in the shader; keep them in i32 (any world fits many times over).
            let wrap = |i: i64| i.rem_euclid(size as i64 * 1024) as i32;
            let u = ShadowCacheUniform {
                min_index: [wrap(corner[0]), wrap(corner[1]), 0, 0],
                params: [size, 0, 0, 0],
            };
            gpu.queue
                .write_buffer(&cache.uniform, 0, bytemuck::bytes_of(&u));
        }

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
                fill_left,
                (1.0 - fill_left) / MOON_FILL_SHARE,
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
        // The real moon's size when known (MoonSource), else the clock's; drawn bigger than in a
        // photo, as the eye sees it. No disc at all when the space module draws the body.
        let real = if lights.in_charge && lights.moon_radius > 0.0 {
            lights.moon_radius
        } else {
            MOON_RADIUS
        };
        let moon_radius = real * settings.moon_scale.clamp(0.25, 8.0);
        let moon_solid_angle = PI * moon_radius * moon_radius;
        let moon_luminance = sky.moon_lux / moon_solid_angle * 0.55;
        let moon_radius = if lights.in_charge && lights.moon_drawn_elsewhere {
            0.0
        } else {
            moon_radius
        };
        let lat = climate.latitude_deg.to_radians();
        let sidereal = (state.time_of_day_h / 24.0 + state.day_of_year / climate.year_days) * TAU;
        let size = self.targets().map(|t| t.size).unwrap_or((1, 1));
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
        let (Some(g), Some(t)) = (&self.gpu, self.current.and_then(|i| self.views.get(i))) else {
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
        if let (Some(target), Some(cache)) = (&ctx.cloud_shadows, &self.shadow_cache) {
            let pass = |encoder: &mut wgpu::CommandEncoder,
                        label: &str,
                        view: &wgpu::TextureView,
                        load: wgpu::LoadOp<wgpu::Color>,
                        pipeline: &wgpu::RenderPipeline,
                        group: &wgpu::BindGroup,
                        rows: Option<(u32, u32)>| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, ctx.frame_group, &[]);
                pass.set_bind_group(1, &g.group, &[]);
                pass.set_bind_group(2, group, &[]);
                if let Some((first, count)) = rows {
                    pass.set_scissor_rect(0, first, cache.size, count);
                }
                pass.draw(0..3, 0..1);
            };
            pass(
                encoder,
                "cloud shadow update",
                &cache.map.view,
                wgpu::LoadOp::Load,
                &g.shadow_update,
                &cache.update_group,
                Some(cache.rows),
            );
            pass(
                encoder,
                "cloud shadows",
                target.view,
                wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                &g.shadow_copy,
                &cache.copy_group,
                None,
            );
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clouds march"),
                color_attachments: &t.march.attachments(),
                ..Default::default()
            });
            pass.set_pipeline(&g.march);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &g.group, &[]);
            pass.set_bind_group(2, &t.march_group, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clouds reproject"),
                color_attachments: &t.image().attachments(),
                ..Default::default()
            });
            pass.set_pipeline(&g.reproject);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &t.reproject_groups[t.current], &[]);
            pass.draw(0..3, 0..1);
        }
        let image = t.image();
        let entries = [
            wgpu::BindGroupEntry {
                binding: 0,
                resource: g.celestial.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&image.color.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&image.depth.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&image.sky.view),
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
        if let Some(t) = self.current.and_then(|i| self.views.get_mut(i)) {
            t.drawn = true;
        }
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
            "march_size": self.targets().map(|t| [t.size.0, t.size.1]),
            "layer_thickness_m": self.top_m,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_order_marches_each_pixel_once_a_round() {
        for side in 2..=MAX_BLOCK_SIDE {
            let n = side * side;
            let mut seen = vec![false; n as usize];
            for frame in 0..n {
                let (x, y) = block_offset(side, frame);
                assert!(
                    x < side as f32 && y < side as f32,
                    "side {side} frame {frame}"
                );
                let i = (y as u32 * side + x as u32) as usize;
                assert!(!seen[i], "side {side}: pixel {x},{y} twice");
                seen[i] = true;
            }
            assert_eq!(block_offset(side, n), block_offset(side, 0));
        }
        assert_eq!(block_offset(1, 7), (0.0, 0.0));
    }

    #[test]
    fn four_frames_of_a_4x4_block_cover_each_of_its_quarters() {
        for start in [0u32, 4, 8, 12] {
            let mut quarters = [false; 4];
            for frame in start..start + 4 {
                let (x, y) = block_offset(4, frame);
                quarters[(y as usize / 2) * 2 + x as usize / 2] = true;
            }
            assert_eq!(quarters, [true; 4], "frames {start}..{}", start + 4);
        }
    }

    #[test]
    fn the_block_side_follows_the_frame_rate_and_holds_between_two() {
        // Pacing off, or at the pacing rate: a quarter a frame, as without pacing.
        assert_eq!(paced_side(1.0, 0), 2);
        assert_eq!(paced_side(1.0, 2), 2);
        // 60 a second paced for 30: a ninth; 120: a sixteenth.
        assert_eq!(paced_side(0.5, 2), 3);
        assert_eq!(paced_side(0.25, 3), 4);
        assert_eq!(paced_side(0.25, 2), 4);
        // Back down when the rate falls.
        assert_eq!(paced_side(1.0, 4), 2);
        assert_eq!(paced_side(0.5, 4), 3);
        // Hovering around a threshold keeps the side it has: 45 to 50 a second is between
        // a quarter and a ninth.
        for share in [0.6f32, 0.63, 0.66] {
            assert_eq!(paced_side(share, 2), 2, "{share}");
            assert_eq!(paced_side(share, 3), 3, "{share}");
        }
        // The whole image is marched again in at most 0.2 s at any rate from 30 a second up.
        let mut side = 2;
        for fps in 30..=240 {
            let share = (30.0 / fps as f32).clamp(0.25, 1.0);
            side = paced_side(share, side);
            let refresh_s = (side * side) as f32 / fps as f32;
            assert!(refresh_s <= 0.2, "{fps} fps: side {side}, {refresh_s} s");
        }
    }

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
    fn moonlit_clouds_take_most_of_the_night_fill_from_the_moon() {
        // By day, and on a moonless night, the clouds keep the whole fill.
        assert_eq!(moon_fill(false, 0.3, 0.07), (0.0, 1.0));
        assert_eq!(moon_fill(true, 0.0, 0.07), (0.0, 1.0));
        // Under a full moon most of it moves into the moon's light, energy kept: what leaves
        // the fill (radiance from all around) comes back as 4π times that from one side.
        let (lux, left) = moon_fill(true, 0.3, 0.07);
        assert!(left < 0.4, "{left}");
        assert!((lux - (1.0 - left) * 4.0 * PI * 0.07).abs() < 1e-5);
        assert!(lux > 0.3, "the moon becomes the clouds' main light: {lux}");
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
