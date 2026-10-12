//! The water renderer: lakes and oceans on a camera-centered polar grid, flowing regions,
//! puddles and the underwater view. The light model is in `water.wgsl`.
//!
//! Lakes and oceans are drawn on one grid of rings around the camera whose spacing grows with
//! distance (fine near the camera, coarse at the horizon), so the cost is the same for a pond
//! and an ocean. The vertex shader displaces it with the body's Gerstner waves and the
//! interactive ripples; the fragment shader recomputes the waves per pixel down to what the
//! pixel can show. Everything draws in the transparent phase from the scene copies (refraction
//! and screen-space reflections).

use std::f64::consts::TAU;

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_render::features::{FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase};
use engine_render::texture::f32_to_f16;
use engine_render::{RenderSettings, ViewData};
use engine_world::{Ground, SurfaceLayers, WeatherState};

use crate::body::{RainShower, WaterBed, WaterExtent, WaterOptics};
use crate::state::{BodyState, SharedWater, WaterState};
use crate::waves::MAX_WAVES;

/// The water shader (composed after the renderer's `lib_types` and `lib_frame`).
pub const SHADER: &str = include_str!("water.wgsl");

/// Bodies drawn at once (the nearest ones).
pub const MAX_BODIES: usize = 8;
const MAX_SHOWERS: usize = 4;
/// Grid points around each ring.
const SEGMENTS: u32 = 256;
/// Most rings a body can use (≈ 60 km at the finest spacing).
const MAX_RINGS: u32 = 420;
/// Puddle window, cells per side (of the ground-surface grid).
const PUDDLE_N: u32 = 128;
/// How far an ocean is drawn, meters.
const OCEAN_REACH: f64 = 30_000.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuWave {
    a: [f32; 4],
    b: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuBody {
    origin: [f32; 4],
    extent: [f32; 4],
    rot: [f32; 4],
    bed: [f32; 4],
    absorb: [f32; 4],
    scatter: [f32; 4],
    look: [f32; 4],
    grid: [f32; 4],
    grid2: [f32; 4],
    wind: [f32; 4],
    waves: [GpuWave; MAX_WAVES],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    ripple: [f32; 4],
    rain: [f32; 4],
    showers: [[f32; 4]; MAX_SHOWERS],
    puddle: [f32; 4],
    puddle2: [f32; 4],
    counts: [u32; 4],
    under: [f32; 4],
    under2: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WaterUniform {
    g: Globals,
    bodies: [GpuBody; MAX_BODIES],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FlowUniform {
    origin: [f32; 4],
    dims: [f32; 4],
    absorb: [f32; 4],
    scatter: [f32; 4],
    look: [f32; 4],
}

fn optics_vectors(o: &WaterOptics) -> ([f32; 4], [f32; 4]) {
    (
        [
            o.absorption[0],
            o.absorption[1],
            o.absorption[2],
            o.scattering,
        ],
        [
            o.scatter_color[0],
            o.scatter_color[1],
            o.scatter_color[2],
            o.roughness,
        ],
    )
}

/// One body's draw: which bands of the grid to draw.
#[derive(Debug, Clone, Copy)]
struct BodyDraw {
    first_band: u32,
    last_band: u32,
}

/// GPU resources of one flowing region.
struct FlowGpu {
    entity: Entity,
    nx: u32,
    nz: u32,
    texture: wgpu::Texture,
    uniform: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    group: wgpu::BindGroup,
    version: u64,
    drawn: bool,
}

struct Gpu {
    body: wgpu::RenderPipeline,
    flow: wgpu::RenderPipeline,
    puddle: wgpu::RenderPipeline,
    under: wgpu::RenderPipeline,
    group1_layout: wgpu::BindGroupLayout,
    group2_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    grid_index: wgpu::Buffer,
    puddle_index: wgpu::Buffer,
    ripple: wgpu::Texture,
    ripple_n: u32,
    ripple_version: u64,
    puddle_tex: wgpu::Texture,
    group1: Option<wgpu::BindGroup>,
    flows: Vec<FlowGpu>,
}

/// What the puddle window holds.
#[derive(Default)]
struct PuddleWindow {
    /// World x/z of the corner of texel (0, 0).
    origin: DVec2,
    cell: f32,
    /// Ground height under each texel, world y.
    ground: Vec<f64>,
    /// The ground heights in the texture are relative to this world y.
    base: f64,
    /// Any standing water in the window.
    wet: bool,
    since_upload: f32,
    /// `WaterState::shifts` when the ground was read (a vertical shift keeps `origin`).
    shifts: u64,
}

/// The water [`RenderFeature`]. Added by [`crate::WaterPlugin`].
#[derive(Default)]
pub struct WaterFeature {
    gpu: Option<Gpu>,
    bodies: Vec<BodyDraw>,
    underwater: bool,
    puddles: PuddleWindow,
    stats: FeatureStats,
    showers: Option<QueryState<(&'static GlobalTransform, &'static RainShower)>>,
}

fn create_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    bytes_per_texel: u32,
    data: &[u8],
) {
    queue.write_texture(
        texture.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_texel),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Triangles of an `nx` × `nz` vertex grid (row-major, rows along +z), facing up.
///
/// With `heights` (one per vertex), each quad is split along the diagonal that follows them,
/// so a channel crossing the grid at an angle keeps straight banks.
fn grid_indices(nx: u32, nz: u32, heights: Option<&[f32]>) -> Vec<u32> {
    let mut out = Vec::with_capacity(((nx - 1) * (nz - 1) * 6) as usize);
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = j * nx + i;
            let b = a + 1;
            let c = a + nx;
            let d = c + 1;
            let along_ad = heights.is_some_and(|h| {
                let h = |v: u32| h[v as usize];
                (h(a) - h(d)).abs() < (h(b) - h(c)).abs()
            });
            if along_ad {
                out.extend_from_slice(&[a, c, d, a, d, b]);
            } else {
                out.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }
    out
}

/// Triangles of the polar grid: band `i` joins ring `i` to ring `i + 1`, `SEGMENTS * 6`
/// indices per band, facing up.
fn polar_indices() -> Vec<u32> {
    let s = SEGMENTS;
    let mut out = Vec::with_capacity(((MAX_RINGS - 1) * s * 6) as usize);
    for i in 0..MAX_RINGS - 1 {
        for j in 0..s {
            let a = i * s + j;
            let b = (i + 1) * s + j;
            let c = i * s + (j + 1) % s;
            let d = (i + 1) * s + (j + 1) % s;
            out.extend_from_slice(&[a, c, b, c, d, b]);
        }
    }
    out
}

/// Texels of a flow region: surface (m above its origin), depth, velocity x, z.
///
/// Cells under still water (lake reservoirs) are hidden: the lake draws them. A dry cell next
/// to water continues the water's surface when that surface is below its ground (so the
/// terrain cuts a smooth waterline), and drops under its ground otherwise (the foot of a
/// fall).
fn flow_texels(sim: &crate::flow::FlowSim) -> Vec<f32> {
    let (nx, nz) = (sim.nx, sim.nz);
    let wet = |idx: usize| sim.depth[idx] > 0.0015 && !sim.reservoir[idx].is_finite();
    let mut data = Vec::with_capacity(nx * nz * 4);
    for j in 0..nz {
        for i in 0..nx {
            let idx = j * nx + i;
            let bed = sim.bed[idx];
            if wet(idx) {
                let d = sim.depth[idx];
                // The shader works in world axes.
                let v = sim.world_dir(sim.vel[idx]);
                data.extend_from_slice(&[bed + d, d, v.x, v.y]);
                continue;
            }
            let mut best: Option<(f32, usize)> = None;
            if !sim.reservoir[idx].is_finite() {
                for (di, dj) in [
                    (-1i64, 0i64),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    let (ii, jj) = (i as i64 + di, j as i64 + dj);
                    if ii < 0 || jj < 0 || ii >= nx as i64 || jj >= nz as i64 {
                        continue;
                    }
                    let n = jj as usize * nx + ii as usize;
                    if wet(n) {
                        let level = sim.bed[n] + sim.depth[n];
                        if best.is_none_or(|(b, _)| level > b) {
                            best = Some((level, n));
                        }
                    }
                }
            }
            match best {
                Some((level, n)) if level <= bed + 0.03 => {
                    let v = sim.world_dir(sim.vel[n]);
                    data.extend_from_slice(&[level, 0.006, v.x, v.y]);
                }
                // Under its own ground: the water's edge is where the surface dips below it.
                _ => data.extend_from_slice(&[bed - 0.08, 0.0, 0.0, 0.0]),
            }
        }
    }
    data
}

/// Radius of ring `i` for inner spacing `s0` and growth `alpha` (matches `vs_body`).
fn ring_radius(i: f64, s0: f64, alpha: f64) -> f64 {
    s0 * ((alpha * i).exp() - 1.0) / alpha
}

/// First ring at or beyond radius `r`.
fn ring_at(r: f64, s0: f64, alpha: f64) -> f64 {
    (1.0 + r.max(0.0) * alpha / s0).ln() / alpha
}

/// Nearest and farthest horizontal distance from `p` to the body's water, meters.
fn extent_range(b: &BodyState, p: DVec2) -> (f64, f64) {
    let c = DVec2::new(b.center.x, b.center.z);
    let d = (p - c).length();
    match b.body.extent {
        WaterExtent::Infinite => (0.0, OCEAN_REACH),
        WaterExtent::Circle { radius } => {
            let r = radius as f64;
            ((d - r).max(0.0), d + r)
        }
        WaterExtent::Rect { half_size } => {
            let local = b.local(DVec3::new(p.x, 0.0, p.y)).as_dvec2();
            let h = DVec2::new(half_size[0] as f64, half_size[1] as f64);
            let outside = (local.abs() - h).max(DVec2::ZERO);
            let far = (local.abs() + h).length();
            (outside.length(), far)
        }
    }
}

impl WaterFeature {
    fn build(gpu: &GpuShared) -> Gpu {
        let device = &gpu.device;
        let shader = gpu.shader("water", SHADER, true);
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let tex = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: vf,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vf,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let group1_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                uniform_entry,
                tex(1, wgpu::TextureSampleType::Float { filterable: true }),
                tex(2, wgpu::TextureSampleType::Float { filterable: true }),
                tex(3, wgpu::TextureSampleType::Depth),
                tex(4, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let group2_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water flow"),
            entries: &[
                uniform_entry,
                tex(1, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let layout1 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&group1_layout)],
            immediate_size: 0,
        });
        let layout2 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water flow"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&group1_layout),
                Some(&group2_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        vs: &str,
                        fs: &str,
                        depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let always = wgpu::DepthStencilState {
            depth_compare: Some(wgpu::CompareFunction::Always),
            ..gpu.depth_state(false)
        };
        let body = pipeline(
            "water body",
            &layout1,
            "vs_body",
            "fs_body",
            gpu.depth_state(true),
        );
        let flow = pipeline(
            "water flow",
            &layout2,
            "vs_flow",
            "fs_flow",
            gpu.depth_state(true),
        );
        let puddle = pipeline(
            "water puddles",
            &layout1,
            "vs_puddle",
            "fs_puddle",
            gpu.depth_state(false),
        );
        let under = pipeline("underwater", &layout1, "vs_under", "fs_under", always);

        use wgpu::util::DeviceExt;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water uniform"),
            size: std::mem::size_of::<WaterUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let grid_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water grid"),
            contents: bytemuck::cast_slice(&polar_indices()),
            usage: wgpu::BufferUsages::INDEX,
        });
        let puddle_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("puddle grid"),
            contents: bytemuck::cast_slice(&grid_indices(PUDDLE_N, PUDDLE_N, None)),
            usage: wgpu::BufferUsages::INDEX,
        });
        let ripple = create_texture(
            device,
            "water ripples",
            8,
            8,
            wgpu::TextureFormat::Rgba16Float,
        );
        let puddle_tex = create_texture(
            device,
            "puddles",
            PUDDLE_N,
            PUDDLE_N,
            wgpu::TextureFormat::Rg32Float,
        );
        Gpu {
            body,
            flow,
            puddle,
            under,
            group1_layout,
            group2_layout,
            uniform,
            grid_index,
            puddle_index,
            ripple,
            ripple_n: 8,
            ripple_version: u64::MAX,
            puddle_tex,
            group1: None,
            flows: Vec::new(),
        }
    }

    /// Fill one body's uniform and decide which grid bands to draw.
    fn body_uniform(b: &BodyState, view: &ViewData) -> Option<(GpuBody, BodyDraw)> {
        let cam = view.origin;
        let cam_xz = DVec2::new(cam.x, cam.z);
        let (near, far) = extent_range(b, cam_xz);
        let level = b.center.y - cam.y;
        let height = level.abs() + 0.3;
        let spread = b.waves.waves.iter().map(|w| w.horiz).sum::<f32>() as f64 + 1.0;
        // Fine near the camera, coarser when looking down from high up.
        let s0 = (height * 0.025).clamp(0.04, 8.0);
        let alpha = TAU / SEGMENTS as f64;
        let first = ring_at(near - spread, s0, alpha).floor().max(0.0) as u32;
        let last = (ring_at(far + spread, s0, alpha).ceil() as u32 + 1).min(MAX_RINGS - 1);
        if first >= last {
            return None;
        }
        let r_max = ring_radius(last as f64, s0, alpha);
        let rel = |x: f64, z: f64| [(x - cam.x) as f32, (z - cam.z) as f32];
        let center = rel(b.center.x, b.center.z);
        let (kind, ext) = match b.body.extent {
            WaterExtent::Infinite => (0.0, [0.0, 0.0]),
            WaterExtent::Circle { radius } => (1.0, [radius, 0.0]),
            WaterExtent::Rect { half_size } => (2.0, half_size),
        };
        let (bed_kind, bed_depth, shore) = match b.body.bed {
            WaterBed::Ground => (0.0, b.mean_depth, 0.0),
            WaterBed::Flat { depth_m } => (1.0, depth_m, 0.0),
            WaterBed::Bowl { depth_m, shore_m } => (2.0, depth_m, shore_m),
        };
        let mut waves = [GpuWave::zeroed(); MAX_WAVES];
        // Waves are referenced to the camera (exactly, in f64) so positions stay small.
        let shift = cam_xz - DVec2::new(b.center.x, b.center.z);
        for (g, w) in waves.iter_mut().zip(&b.waves.waves) {
            let phase = (w.phase as f64 - w.k as f64 * w.dir.as_dvec2().dot(shift)).rem_euclid(TAU);
            g.a = [w.dir.x, w.dir.y, w.k, w.amp];
            g.b = [w.horiz, phase as f32, w.omega, 0.0];
        }
        let wind = b.wind;
        let speed = (wind.x * wind.x + wind.z * wind.z).sqrt();
        let dir = if speed > 1e-3 {
            [wind.x / speed, wind.z / speed]
        } else {
            [1.0, 0.0]
        };
        let (absorb, scatter) = optics_vectors(&b.body.optics);
        let o = &b.body.optics;
        let (sin, cos) = b.yaw.sin_cos();
        let gpu = GpuBody {
            origin: [0.0, level as f32, 0.0, kind],
            extent: [center[0], center[1], ext[0], ext[1]],
            rot: [cos, sin, bed_kind, bed_depth],
            bed: [
                shore,
                b.mean_depth,
                speed,
                b.waves.waves.len().min(MAX_WAVES) as f32,
            ],
            absorb,
            scatter,
            look: [
                o.foam,
                o.caustics,
                o.refraction,
                if b.body.interactive { 1.0 } else { 0.0 },
            ],
            grid: [0.0, 0.0, s0 as f32, alpha as f32],
            grid2: [
                (last + 1) as f32,
                SEGMENTS as f32,
                r_max as f32,
                b.waves.max_height().max(0.01),
            ],
            wind: [dir[0], dir[1], b.body.current[0], b.body.current[1]],
            waves,
        };
        Some((
            gpu,
            BodyDraw {
                first_band: first,
                last_band: last,
            },
        ))
    }

    fn upload_ripples(gpu: &mut Gpu, shared: &GpuShared, state: &WaterState) {
        let r = &state.ripples;
        let n = r.n as u32;
        if gpu.ripple_n != n {
            gpu.ripple = create_texture(
                &shared.device,
                "water ripples",
                n,
                n,
                wgpu::TextureFormat::Rgba16Float,
            );
            gpu.ripple_n = n;
            gpu.ripple_version = u64::MAX;
            gpu.group1 = None;
        }
        if gpu.ripple_version == r.version {
            return;
        }
        gpu.ripple_version = r.version;
        let n = r.n;
        let inv = 0.5 / r.cell;
        let mut data = vec![0u16; n * n * 4];
        for j in 0..n {
            for i in 0..n {
                let idx = j * n + i;
                let h = |ii: usize, jj: usize| r.h[jj * n + ii];
                let gx = (h((i + 1).min(n - 1), j) - h(i.saturating_sub(1), j)) * inv;
                let gz = (h(i, (j + 1).min(n - 1)) - h(i, j.saturating_sub(1))) * inv;
                let o = idx * 4;
                data[o] = f32_to_f16(r.h[idx]);
                data[o + 1] = f32_to_f16(r.foam[idx]);
                data[o + 2] = f32_to_f16(gx);
                data[o + 3] = f32_to_f16(gz);
            }
        }
        write_texture(
            &shared.queue,
            &gpu.ripple,
            n as u32,
            n as u32,
            8,
            bytemuck::cast_slice(&data),
        );
    }

    fn upload_flows(gpu: &mut Gpu, shared: &GpuShared, state: &WaterState, view: &ViewData) {
        let device = &shared.device;
        gpu.flows.retain(|f| {
            state
                .flows
                .iter()
                .any(|s| s.entity == f.entity && s.nx as u32 == f.nx && s.nz as u32 == f.nz)
        });
        for f in &mut gpu.flows {
            f.drawn = false;
        }
        for sim in &state.flows {
            if !sim.visible {
                continue;
            }
            let pos = match gpu.flows.iter().position(|f| f.entity == sim.entity) {
                Some(p) => p,
                None => {
                    use wgpu::util::DeviceExt;
                    let (nx, nz) = (sim.nx as u32, sim.nz as u32);
                    let texture = create_texture(
                        device,
                        "water flow",
                        nx,
                        nz,
                        wgpu::TextureFormat::Rgba32Float,
                    );
                    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("water flow"),
                        size: std::mem::size_of::<FlowUniform>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let indices = grid_indices(nx, nz, Some(&sim.bed));
                    let index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("water flow grid"),
                        contents: bytemuck::cast_slice(&indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                    let tex_view = texture.create_view(&Default::default());
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("water flow"),
                        layout: &gpu.group2_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniform.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&tex_view),
                            },
                        ],
                    });
                    gpu.flows.push(FlowGpu {
                        entity: sim.entity,
                        nx,
                        nz,
                        texture,
                        uniform,
                        index,
                        index_count: indices.len() as u32,
                        group,
                        version: u64::MAX,
                        drawn: false,
                    });
                    gpu.flows.len() - 1
                }
            };
            let f = &mut gpu.flows[pos];
            if !sim.depth.iter().any(|d| *d > 0.003) {
                continue;
            }
            f.drawn = true;
            let o = view.relative(sim.origin);
            let (absorb, scatter) = optics_vectors(&sim.optics);
            let u = FlowUniform {
                origin: [o.x, o.y, o.z, sim.cell],
                dims: [sim.nx as f32, sim.nz as f32, sim.yaw.cos(), sim.yaw.sin()],
                absorb,
                scatter,
                look: [
                    sim.optics.foam,
                    sim.optics.caustics,
                    sim.optics.refraction,
                    0.0,
                ],
            };
            shared
                .queue
                .write_buffer(&f.uniform, 0, bytemuck::bytes_of(&u));
            if f.version != sim.version {
                f.version = sim.version;
                let data = flow_texels(sim);
                write_texture(
                    &shared.queue,
                    &f.texture,
                    f.nx,
                    f.nz,
                    16,
                    bytemuck::cast_slice(&data),
                );
            }
        }
    }

    /// Standing water near the camera from `SurfaceLayers`. Returns the uniform's
    /// (puddle, puddle2) and whether to draw.
    fn upload_puddles(
        &mut self,
        world: &World,
        shared: &GpuShared,
        view: &ViewData,
    ) -> ([f32; 4], [f32; 4], bool) {
        let Some(gpu) = self.gpu.as_ref() else {
            return ([0.0; 4], [0.0; 4], false);
        };
        let Some(layers) = world.get_resource::<SurfaceLayers>() else {
            return ([0.0; 4], [0.0; 4], false);
        };
        let grid = &layers.grid;
        let w = &mut self.puddles;
        let cell = grid.cell;
        let n = PUDDLE_N as usize;
        // Window snapped to the grid's cells around the camera.
        let half = n as f64 * cell as f64 * 0.5;
        let corner =
            ((DVec2::new(view.origin.x, view.origin.z) - DVec2::splat(half) - grid.origin)
                / cell as f64)
                .round();
        let origin = grid.origin + corner * cell as f64;
        let shifts = world
            .get_resource::<SharedWater>()
            .map_or(0, |s| s.0.read().shifts);
        let moved =
            origin != w.origin || cell != w.cell || w.ground.len() != n * n || shifts != w.shifts;
        w.since_upload += view.delta.max(0.0);
        if moved {
            w.origin = origin;
            w.cell = cell;
            w.shifts = shifts;
            let ground = world.get_resource::<Ground>();
            let probe_y = view.origin.y + 200.0;
            w.ground = (0..n * n)
                .map(|idx| {
                    let p = origin
                        + DVec2::new((idx % n) as f64 + 0.5, (idx / n) as f64 + 0.5) * cell as f64;
                    ground
                        .and_then(|g| g.height(DVec3::new(p.x, probe_y, p.y)))
                        .unwrap_or(0.0)
                })
                .collect();
            w.base = w.ground.iter().copied().fold(f64::INFINITY, f64::min);
            if !w.base.is_finite() {
                w.base = 0.0;
            }
        }
        if moved || w.since_upload >= 0.2 {
            w.since_upload = 0.0;
            let mut data = vec![0f32; n * n * 2];
            let mut wet = false;
            for (idx, px) in data.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let (i, j) = (
                    corner.x as i64 + (idx % n) as i64,
                    corner.y as i64 + (idx / n) as i64,
                );
                let depth = if i >= 0 && j >= 0 && i < grid.width as i64 && j < grid.height as i64 {
                    grid.data[j as usize * grid.width as usize + i as usize].puddle_m
                } else {
                    0.0
                };
                wet |= depth > 0.0005;
                px[0] = depth;
                px[1] = (w.ground[idx] - w.base) as f32;
            }
            w.wet = wet;
            if wet || moved {
                write_texture(
                    &shared.queue,
                    &gpu.puddle_tex,
                    PUDDLE_N,
                    PUDDLE_N,
                    8,
                    bytemuck::cast_slice(&data),
                );
            }
        }
        let o = view.relative(DVec3::new(w.origin.x, 0.0, w.origin.y));
        (
            [o.x, o.z, cell, PUDDLE_N as f32],
            [(w.base - view.origin.y) as f32, 0.0, 0.0, 0.0],
            w.wet,
        )
    }
}

impl RenderFeature for WaterFeature {
    fn name(&self) -> &str {
        "water"
    }

    fn order(&self) -> i32 {
        -10
    }

    fn init(&mut self, gpu: &GpuShared) {
        self.gpu = Some(Self::build(gpu));
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        if let Some(g) = &mut self.gpu {
            g.group1 = None;
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        shared: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.bodies.clear();
        self.underwater = false;
        self.stats = FeatureStats::default();
        if self.gpu.is_none() {
            return;
        }
        let Some(water) = world.get_resource::<SharedWater>().cloned() else {
            return;
        };
        // Local rain (showers).
        let showers_q = self.showers.get_or_insert_with(|| world.query());
        let cam = view.origin;
        let mut showers: Vec<(f64, [f32; 4])> = showers_q
            .iter(world)
            .map(|(g, s)| {
                let p = g.translation();
                let d = DVec2::new(p.x - cam.x, p.z - cam.z).length() - s.radius_m as f64;
                (
                    d,
                    [
                        (p.x - cam.x) as f32,
                        (p.z - cam.z) as f32,
                        s.radius_m,
                        s.mm_h,
                    ],
                )
            })
            .collect();
        showers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let rain = world
            .get_resource::<WeatherState>()
            .filter(|w| w.is_raining())
            .map_or(0.0, |w| w.precipitation_mm_h);
        let (puddle, puddle2, puddles_on) = self.upload_puddles(world, shared, view);

        let state = water.0.read();
        let gpu = self.gpu.as_mut().expect("checked above");
        let mut uniform = WaterUniform::zeroed();

        // Nearest bodies first.
        let cam_xz = DVec2::new(cam.x, cam.z);
        let mut order: Vec<(f64, usize)> = state
            .bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.body.visible)
            .map(|(i, b)| (extent_range(b, cam_xz).0, i))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut under: Option<(usize, f32)> = None;
        for &(_, i) in order.iter() {
            if self.bodies.len() >= MAX_BODIES {
                break;
            }
            let b = &state.bodies[i];
            let Some((body, draw)) = Self::body_uniform(b, view) else {
                continue;
            };
            let slot = self.bodies.len();
            if under.is_none()
                && let Some((height, depth, ..)) = state.sample_body(b, cam)
                && cam.y < height - 0.02
                && cam.y > height - depth as f64 - 0.5
            {
                under = Some((slot, (height - cam.y) as f32));
            }
            uniform.bodies[slot] = body;
            self.bodies.push(draw);
        }

        let r = &state.ripples;
        let ripple_rel = view.relative(DVec3::new(r.origin.x, 0.0, r.origin.y));
        let mut g = Globals::zeroed();
        g.ripple = [
            ripple_rel.x,
            ripple_rel.z,
            r.size() as f32,
            if r.has_water { 1.0 } else { 0.0 },
        ];
        g.rain = [rain, view.time, -1.0, 0.0];
        for (dst, (_, s)) in g.showers.iter_mut().zip(&showers) {
            *dst = *s;
        }
        g.puddle = puddle;
        g.puddle2 = puddle2;
        g.counts = [
            self.bodies.len() as u32,
            showers.len().min(MAX_SHOWERS) as u32,
            puddles_on as u32,
            0,
        ];
        if let Some((slot, depth)) = under {
            let b = &uniform.bodies[slot];
            g.rain[2] = slot as f32;
            g.rain[3] = depth;
            g.under = b.absorb;
            g.under2 = [b.scatter[0], b.scatter[1], b.scatter[2], depth];
            self.underwater = true;
        }
        uniform.g = g;
        shared
            .queue
            .write_buffer(&gpu.uniform, 0, bytemuck::bytes_of(&uniform));

        Self::upload_ripples(gpu, shared, &state);
        Self::upload_flows(gpu, shared, &state, view);

        // Stats.
        let flows = gpu.flows.iter().filter(|f| f.drawn);
        self.stats.draw_calls = self.bodies.len() as u32
            + flows.clone().count() as u32
            + puddles_on as u32
            + self.underwater as u32;
        self.stats.instances = self.bodies.len() as u32;
        self.stats.triangles = self
            .bodies
            .iter()
            .map(|d| ((d.last_band - d.first_band) * SEGMENTS * 2) as u64)
            .sum::<u64>()
            + flows.map(|f| f.index_count as u64 / 3).sum::<u64>()
            + if puddles_on {
                ((PUDDLE_N - 1) * (PUDDLE_N - 1) * 2) as u64
            } else {
                0
            };
        self.puddles.wet = puddles_on;
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        // The scene copies are recreated on resize; rebuilding one bind group is cheap.
        let ripple = gpu.ripple.create_view(&Default::default());
        let puddle = gpu.puddle_tex.create_view(&Default::default());
        gpu.group1 = Some(
            ctx.gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water"),
                    layout: &gpu.group1_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: gpu.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&ripple),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_color_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&puddle),
                        },
                    ],
                }),
        );
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(group1) = &gpu.group1 else {
            return;
        };
        pass.set_bind_group(1, group1, &[]);
        if self.underwater {
            pass.set_pipeline(&gpu.under);
            pass.draw(0..3, 0..1);
        }
        if self.puddles.wet {
            pass.set_pipeline(&gpu.puddle);
            pass.set_index_buffer(gpu.puddle_index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..(PUDDLE_N - 1) * (PUDDLE_N - 1) * 6, 0, 0..1);
        }
        if gpu.flows.iter().any(|f| f.drawn) {
            pass.set_pipeline(&gpu.flow);
            for f in gpu.flows.iter().filter(|f| f.drawn) {
                pass.set_bind_group(2, &f.group, &[]);
                pass.set_index_buffer(f.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..f.index_count, 0, 0..1);
            }
        }
        if !self.bodies.is_empty() {
            pass.set_pipeline(&gpu.body);
            pass.set_index_buffer(gpu.grid_index.slice(..), wgpu::IndexFormat::Uint32);
            for (i, d) in self.bodies.iter().enumerate() {
                let per_band = SEGMENTS * 6;
                pass.draw_indexed(
                    d.first_band * per_band..d.last_band * per_band,
                    0,
                    i as u32..i as u32 + 1,
                );
            }
        }
    }

    fn needs_scene_copy(&self) -> bool {
        !self.bodies.is_empty()
            || self.puddles.wet
            || self
                .gpu
                .as_ref()
                .is_some_and(|g| g.flows.iter().any(|f| f.drawn))
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "bodies": self.bodies.len(),
            "bands": self.bodies.iter().map(|d| d.last_band - d.first_band).collect::<Vec<_>>(),
            "underwater": self.underwater,
            "puddles": self.puddles.wet,
            "flows": self.gpu.as_ref().map_or(0, |g| g.flows.iter().filter(|f| f.drawn).count()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_validates() {
        let code = engine_render::features::compose_shader(SHADER, true);
        engine_render::features::validate_wgsl("water", &code).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn uniform_layout_matches_the_shader() {
        // WGSL: Globals 7 vec4 + 4 showers; GpuBody 10 vec4 + 24 waves of 2 vec4.
        assert_eq!(std::mem::size_of::<Globals>(), (7 + 4) * 16);
        assert_eq!(std::mem::size_of::<GpuBody>(), (10 + MAX_WAVES * 2) * 16);
        assert_eq!(
            std::mem::size_of::<WaterUniform>(),
            std::mem::size_of::<Globals>() + MAX_BODIES * std::mem::size_of::<GpuBody>()
        );
        assert_eq!(std::mem::size_of::<FlowUniform>(), 5 * 16);
    }

    #[test]
    fn rings_cover_the_requested_radius() {
        let (s0, alpha) = (0.05, TAU / SEGMENTS as f64);
        for r in [0.5, 10.0, 300.0, 20_000.0] {
            let i = ring_at(r, s0, alpha);
            assert!((ring_radius(i, s0, alpha) - r).abs() < 1e-6 * r.max(1.0));
        }
        assert!(ring_radius(MAX_RINGS as f64, s0, alpha) > OCEAN_REACH);
    }

    #[test]
    fn grids_face_up() {
        // Positions of a 3×3 vertex grid (x = i, z = j); every triangle's normal points +y.
        // Either diagonal: a valley along +x+z on the second grid flips every quad.
        let valley: Vec<f32> = (0..9)
            .map(|v| ((v % 3) as f32 - (v / 3) as f32).abs())
            .collect();
        for idx in [grid_indices(3, 3, None), grid_indices(3, 3, Some(&valley))] {
            let pos = |v: u32| ((v % 3) as f32, (v / 3) as f32);
            for t in idx.chunks(3) {
                let (a, b, c) = (pos(t[0]), pos(t[1]), pos(t[2]));
                let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - a.0, c.1 - a.1));
                // y of (u × v) with u, v in (x, z): u.z * v.x − u.x * v.z.
                assert!(u.1 * v.0 - u.0 * v.1 > 0.0);
            }
        }
        assert_ne!(grid_indices(3, 3, None), grid_indices(3, 3, Some(&valley)));
    }
}
