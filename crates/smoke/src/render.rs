//! Drawing smoke: each volume's density goes to a 3D texture, a compute pass works out how
//! much sun, sky and firelight reaches every voxel through the smoke around it, a shadow pass
//! darkens the sunlit ground and props under the smoke, and a raymarch over the volume's box
//! draws it lit, shadowed, fogged and blended with what's behind.

use bytemuck::{Pod, Zeroable};
use engine_core::math::{UVec3, Vec3};
use engine_core::prelude::*;
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase, RenderSettings, ViewData,
};

use crate::domain::Domain;
use crate::settings::SmokeSettings;
use crate::sim::{SmokeClock, SmokeDomains, SmokeTunnels};

pub const SMOKE_SHADER: &str = include_str!("shaders/smoke.wgsl");
pub const SMOKE_LIGHT_SHADER: &str = include_str!("shaders/smoke_light.wgsl");
pub const SMOKE_SHADOW_SHADER: &str = include_str!("shaders/smoke_shadow.wgsl");

/// Bullet tunnels drawn per volume.
pub const MAX_TUNNELS: usize = 16;
/// Texels along each side of the tiling detail-noise texture.
pub const DETAIL_SIZE: u32 = 64;
/// Firelight is stored divided by this so it fits half floats.
pub const GLOW_SCALE: f32 = 64.0;
/// Volumes farther than this aren't drawn, m.
const VIEW_DISTANCE: f64 = 4000.0;
/// How far below a volume's floor its shadow still falls, m (as in smoke_shadow.wgsl).
const SHADOW_DROP: f32 = 30.0;
/// The longest a volume's shadow stretches away from a low sun, m (as in smoke_shadow.wgsl).
const SHADOW_REACH: f32 = 120.0;

/// GPU layout of `SmokeParams` in the shaders.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SmokeParams {
    /// Box min corner relative to the camera (xyz), voxel size (w).
    pub min_rel: [f32; 4],
    /// Box size m (xyz), blend from the previous to the current state (w).
    pub size: [f32; 4],
    /// Voxels (xyz), firelight scale (w).
    pub dims: [f32; 4],
    /// Detail, most steps, tunnel count, 1 when the light pass should run.
    pub look: [f32; 4],
    /// Per tunnel: from (xyz, camera-relative) + radius, to (xyz) + strength.
    pub tunnels: [[f32; 4]; MAX_TUNNELS * 2],
}

/// One volume on the GPU.
struct VolumeGpu {
    id: u32,
    dims: UVec3,
    /// Two density states (previous and current) so motion between sim steps is smooth.
    density: [wgpu::Texture; 2],
    density_views: [wgpu::TextureView; 2],
    light_view: wgpu::TextureView,
    glow_view: wgpu::TextureView,
    params: wgpu::Buffer,
    /// Draw bind group for each choice of current texture.
    draw_groups: [Option<wgpu::BindGroup>; 2],
    light_groups: [wgpu::BindGroup; 2],
    current: usize,
    version: u64,
    light_dirty: bool,
    /// Distance from the camera to the box center, for back-to-front drawing.
    distance: f64,
    /// The box is in view (draw the smoke).
    visible: bool,
    /// The ground it shades is in view (draw the shadow).
    shadow_visible: bool,
    seen: bool,
}

#[derive(Default)]
pub struct SmokeFeature {
    pipeline: Option<wgpu::RenderPipeline>,
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    light_pipeline: Option<wgpu::ComputePipeline>,
    draw_layout: Option<wgpu::BindGroupLayout>,
    light_layout: Option<wgpu::BindGroupLayout>,
    /// Billowy tiling noise the raymarch adds detail with (`noise::detail_noise`).
    detail: Option<(wgpu::Texture, wgpu::TextureView)>,
    volumes: Vec<VolumeGpu>,
    /// Indices into `volumes` whose smoke or shadow is in view, back to front.
    order: Vec<usize>,
    pack: Vec<[u16; 4]>,
    frames_since_light: u32,
    stats: FeatureStats,
}

/// Pack a domain into (σ, σ·albedo) half floats: extinction 1/m and scattering color.
pub fn pack_density(domain: &Domain, extinction_m2_kg: f32, out: &mut Vec<[u16; 4]>) {
    out.clear();
    out.reserve(domain.voxels.len());
    for v in &domain.voxels {
        if v.density <= 1e-9 {
            out.push([0; 4]);
            continue;
        }
        let sigma = v.density * extinction_m2_kg;
        let albedo = v.albedo();
        out.push([
            f16(sigma),
            f16(sigma * albedo.x),
            f16(sigma * albedo.y),
            f16(sigma * albedo.z),
        ]);
    }
}

/// f32 → IEEE half (round to nearest; values here are finite and non-negative).
pub fn f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x7f_ffff;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exp >= 31 {
        return sign | 0x7bff;
    }
    let h = sign as u32 | ((exp as u32) << 10) | (mant >> 13);
    (h + ((mant >> 12) & 1)).min(sign as u32 | 0x7bff) as u16
}

fn texture_3d(
    gpu: &GpuShared,
    label: &str,
    dims: UVec3,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: dims.x,
            height: dims.y,
            depth_or_array_layers: dims.z,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage,
        view_formats: &[],
    })
}

impl SmokeFeature {
    fn create_volume(&self, gpu: &GpuShared, id: u32, dims: UVec3) -> Option<VolumeGpu> {
        let light_layout = self.light_layout.as_ref()?;
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let storage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let density = [
            texture_3d(gpu, "smoke density a", dims, sampled),
            texture_3d(gpu, "smoke density b", dims, sampled),
        ];
        let density_views = [
            density[0].create_view(&Default::default()),
            density[1].create_view(&Default::default()),
        ];
        let light = texture_3d(gpu, "smoke light", dims, storage);
        let glow = texture_3d(gpu, "smoke firelight", dims, storage);
        let light_view = light.create_view(&Default::default());
        let glow_view = glow.create_view(&Default::default());
        let params = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("smoke params"),
            size: std::mem::size_of::<SmokeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_group = |cur: usize| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("smoke light"),
                layout: light_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&density_views[cur]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&light_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&glow_view),
                    },
                ],
            })
        };
        let light_groups = [light_group(0), light_group(1)];
        Some(VolumeGpu {
            id,
            dims,
            density,
            density_views,
            light_view,
            glow_view,
            params,
            draw_groups: [None, None],
            light_groups,
            current: 0,
            version: 0,
            light_dirty: true,
            distance: 0.0,
            visible: false,
            shadow_visible: false,
            seen: true,
        })
    }

    fn upload(&mut self, gpu: &GpuShared, slot: usize, domain: &Domain, extinction: f32) {
        pack_density(domain, extinction, &mut self.pack);
        let v = &mut self.volumes[slot];
        let first = v.version == 0;
        v.current = 1 - v.current;
        let dims = v.dims;
        let write = |tex: &wgpu::Texture| {
            gpu.queue.write_texture(
                tex.as_image_copy(),
                bytemuck::cast_slice(&self.pack),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dims.x * 8),
                    rows_per_image: Some(dims.y),
                },
                wgpu::Extent3d {
                    width: dims.x,
                    height: dims.y,
                    depth_or_array_layers: dims.z,
                },
            );
        };
        write(&v.density[v.current]);
        if first {
            write(&v.density[1 - v.current]);
        }
        v.version = domain.version;
        v.light_dirty = true;
    }
}

impl RenderFeature for SmokeFeature {
    fn name(&self) -> &str {
        "smoke"
    }

    /// After fire, so smoke veils the flames.
    fn order(&self) -> i32 {
        50
    }

    fn needs_scene_copy(&self) -> bool {
        true
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let tex3d = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        };
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        };
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let fs = wgpu::ShaderStages::FRAGMENT;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smoke draw"),
            entries: &[
                uniform(0, vf),
                tex3d(1, fs),
                tex3d(2, fs),
                tex3d(3, fs),
                tex3d(4, fs),
                tex3d(6, fs),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: fs,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let cs = wgpu::ShaderStages::COMPUTE;
        let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smoke light"),
            entries: &[uniform(0, cs), tex3d(1, cs), storage(2), storage(3)],
        });

        let shader = gpu.shader("smoke", SMOKE_SHADER, true);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("smoke"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&draw_layout)],
            immediate_size: 0,
        });
        let mut depth = gpu.depth_state(false);
        // The shader stops the march at the scene depth itself.
        depth.depth_compare = Some(wgpu::CompareFunction::Always);
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("smoke"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_smoke"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(depth.clone()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_smoke"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: Some(premultiplied),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );

        // Shadows multiply the scene: dst × src.
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let shadow_shader = gpu.shader("smoke shadow", SMOKE_SHADOW_SHADER, true);
        self.shadow_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("smoke shadow"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_shadow"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(depth.clone()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shadow_shader,
                    entry_point: Some("fs_shadow"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: Some(multiply),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            },
        ));

        let light_shader = gpu.shader("smoke light", SMOKE_LIGHT_SHADER, true);
        let light_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("smoke light"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&light_layout)],
            immediate_size: 0,
        });
        self.light_pipeline = Some(device.create_compute_pipeline(
            &wgpu::ComputePipelineDescriptor {
                label: Some("smoke light"),
                layout: Some(&light_pl),
                module: &light_shader,
                entry_point: Some("cs_light"),
                compilation_options: Default::default(),
                cache: None,
            },
        ));
        self.draw_layout = Some(draw_layout);
        self.light_layout = Some(light_layout);

        let n = DETAIL_SIZE;
        let detail = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("smoke detail noise"),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            detail.as_image_copy(),
            bytemuck::cast_slice(&crate::noise::detail_noise(n)),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 4),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        let view = detail.create_view(&Default::default());
        self.detail = Some((detail, view));
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        // Draw groups hold the scene depth copy, which is recreated on resize.
        for v in &mut self.volumes {
            v.draw_groups = [None, None];
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.stats = FeatureStats::default();
        self.order.clear();
        let Some(settings) = world.get_resource::<SmokeSettings>().cloned() else {
            return;
        };
        if !settings.enabled || self.light_pipeline.is_none() {
            return;
        }
        let blend = world
            .get_resource::<SmokeClock>()
            .map(|c| c.progress(&settings))
            .unwrap_or(1.0);
        let tunnels: Vec<_> = world
            .get_resource::<SmokeTunnels>()
            .map(|t| t.list.clone())
            .unwrap_or_default();
        let Some(domains) = world.get_resource::<SmokeDomains>() else {
            return;
        };
        for v in &mut self.volumes {
            v.seen = false;
            v.visible = false;
            v.shadow_visible = false;
        }
        let sun =
            (view.sun_dir.y > 0.0 && view.sun_color.length_squared() > 0.0).then_some(view.sun_dir);
        // Light again now and then even when the smoke is still: the sun and fires move.
        self.frames_since_light += 1;
        let relight = self.frames_since_light >= 6;
        if relight {
            self.frames_since_light = 0;
        }
        let mut uploads = Vec::new();
        for d in &domains.list {
            let dom = &d.domain;
            let slot = match self.volumes.iter().position(|v| v.id == d.id) {
                Some(i) if self.volumes[i].dims == dom.dims => i,
                found => {
                    if let Some(i) = found {
                        self.volumes.swap_remove(i);
                    }
                    let Some(v) = self.create_volume(gpu, d.id, dom.dims) else {
                        continue;
                    };
                    self.volumes.push(v);
                    self.volumes.len() - 1
                }
            };
            let v = &mut self.volumes[slot];
            v.seen = true;
            if dom.max_density <= 0.0 {
                continue;
            }
            let rel_min = (dom.origin - view.origin).as_vec3();
            let size = dom.size().as_vec3();
            let center = dom.center();
            v.distance = (center - view.origin).length();
            let radius = size.length() * 0.5;
            if v.distance > VIEW_DISTANCE + radius as f64 {
                continue;
            }
            v.visible = view.frustum.intersects_sphere(rel_min + size * 0.5, radius);
            if let Some(sun) = sun {
                let (lo, hi) = shadow_bounds(rel_min, size, sun);
                let half = (hi - lo) * 0.5;
                v.shadow_visible = view.frustum.intersects_sphere(lo + half, half.length());
            }
            if !v.visible && !v.shadow_visible {
                continue;
            }
            if v.version != dom.version {
                uploads.push(slot);
            }
            if relight {
                v.light_dirty = true;
            }
            // Bullet tunnels crossing this volume, newest first.
            let mut packed = [[0.0f32; 4]; MAX_TUNNELS * 2];
            let mut count = 0;
            for t in tunnels.iter().rev() {
                if count == MAX_TUNNELS {
                    break;
                }
                if dom.clip(t.from, t.to).is_none() {
                    continue;
                }
                let life = t.age / settings.tunnel_secs.max(0.05);
                let strength = (1.0 - life).clamp(0.0, 1.0).powf(0.7);
                // The hole widens a little as the smoke flows back in.
                let radius = t.radius * (1.0 + 1.5 * life);
                let a = (t.from - view.origin).as_vec3();
                let b = (t.to - view.origin).as_vec3();
                packed[count * 2] = [a.x, a.y, a.z, radius];
                packed[count * 2 + 1] = [b.x, b.y, b.z, strength];
                count += 1;
            }
            let p = SmokeParams {
                min_rel: [rel_min.x, rel_min.y, rel_min.z, dom.cell],
                size: [size.x, size.y, size.z, blend],
                dims: [
                    dom.dims.x as f32,
                    dom.dims.y as f32,
                    dom.dims.z as f32,
                    GLOW_SCALE,
                ],
                look: [
                    settings.detail,
                    settings.max_steps.clamp(8, 512) as f32,
                    count as f32,
                    0.0,
                ],
                tunnels: packed,
            };
            gpu.queue.write_buffer(&v.params, 0, bytemuck::bytes_of(&p));
        }
        for slot in uploads {
            let id = self.volumes[slot].id;
            if let Some(d) = domains.list.iter().find(|d| d.id == id) {
                self.upload(gpu, slot, &d.domain, settings.extinction_m2_kg);
            }
        }
        self.volumes.retain(|v| v.seen);
        self.order = (0..self.volumes.len())
            .filter(|&i| self.volumes[i].visible || self.volumes[i].shadow_visible)
            .collect();
        self.order.sort_by(|&a, &b| {
            self.volumes[b]
                .distance
                .total_cmp(&self.volumes[a].distance)
        });
        let drawn = self
            .order
            .iter()
            .filter(|&&i| self.volumes[i].visible)
            .count();
        let shaded = self
            .order
            .iter()
            .filter(|&&i| self.volumes[i].shadow_visible)
            .count();
        self.stats.instances = drawn as u32;
        self.stats.draw_calls = (drawn + shaded) as u32;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("smoke.drawn", drawn as f64);
            d.set("smoke.shadows", shaded as f64);
        }
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let device = &ctx.gpu.device;
        let (Some(layout), Some((_, detail))) = (&self.draw_layout, &self.detail) else {
            return;
        };
        for &i in &self.order {
            let v = &mut self.volumes[i];
            let cur = v.current;
            if v.draw_groups[cur].is_none() {
                v.draw_groups[cur] = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("smoke draw"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: v.params.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&v.density_views[1 - cur]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&v.density_views[cur]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&v.light_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&v.glow_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(detail),
                        },
                    ],
                }));
            }
        }
        let Some(pipeline) = &self.light_pipeline else {
            return;
        };
        let dirty: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&i| self.volumes[i].light_dirty)
            .collect();
        if dirty.is_empty() {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("smoke light"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, ctx.frame_group, &[]);
        for i in dirty {
            let v = &mut self.volumes[i];
            pass.set_bind_group(1, &v.light_groups[v.current], &[]);
            let groups = (v.dims + UVec3::splat(3)) / 4;
            pass.dispatch_workgroups(groups.x, groups.y, groups.z);
            v.light_dirty = false;
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let (Some(pipeline), Some(shadow)) = (&self.pipeline, &self.shadow_pipeline) else {
            return;
        };
        // Shadows on the scene first, so the smoke itself isn't darkened by them.
        pass.set_pipeline(shadow);
        for &i in &self.order {
            let v = &self.volumes[i];
            if let (true, Some(group)) = (v.shadow_visible, &v.draw_groups[v.current]) {
                pass.set_bind_group(1, group, &[]);
                pass.draw(0..36, 0..1);
            }
        }
        pass.set_pipeline(pipeline);
        for &i in &self.order {
            let v = &self.volumes[i];
            if let (true, Some(group)) = (v.visible, &v.draw_groups[v.current]) {
                pass.set_bind_group(1, group, &[]);
                pass.draw(0..36, 0..1);
            }
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }
}

/// A volume's box (camera-relative min and size) and everything its shadow can fall on: swept
/// away from the sun and down past its floor, as in smoke_shadow.wgsl.
pub fn shadow_bounds(min: Vec3, size: Vec3, sun: Vec3) -> (Vec3, Vec3) {
    let reach = ((size.y + SHADOW_DROP) / sun.y.max(0.05)).min(SHADOW_REACH);
    let away = -sun * reach;
    let max = min + size;
    (min.min(min + away), max.max(max + away))
}

/// Where a ray from the camera enters and leaves a box (camera-relative), if it hits it.
pub fn ray_box(dir: Vec3, min: Vec3, max: Vec3) -> Option<(f32, f32)> {
    let inv = Vec3::ONE / dir;
    let a = min * inv;
    let b = max * inv;
    let t0 = a.min(b).max_element().max(0.0);
    let t1 = a.max(b).min_element();
    (t1 > t0).then_some((t0, t1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::DVec3;
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn shaders_validate() {
        for (name, src) in [
            ("smoke", SMOKE_SHADER),
            ("smoke light", SMOKE_LIGHT_SHADER),
            ("smoke shadow", SMOKE_SHADOW_SHADER),
        ] {
            validate_wgsl(name, &compose_shader(src, true)).unwrap();
        }
    }

    #[test]
    fn shadow_bounds_reach_away_from_the_sun() {
        let min = Vec3::new(-10.0, 0.0, -10.0);
        let size = Vec3::new(20.0, 10.0, 20.0);
        // High sun from +x: the shadow falls toward -x and below the floor.
        let sun = Vec3::new(0.5, 0.8, 0.0).normalize();
        let (lo, hi) = shadow_bounds(min, size, sun);
        assert!(lo.x < min.x - 10.0 && lo.y < min.y - 10.0, "{lo}");
        assert_eq!(hi, min + size);
        // A sun near the horizon is capped.
        let (lo, _) = shadow_bounds(min, size, Vec3::new(1.0, 0.01, 0.0).normalize());
        assert!(lo.x >= min.x - SHADOW_REACH - 1e-3, "{lo}");
    }

    #[test]
    fn params_match_the_shader_layout() {
        // 4 vec4 + 2 per tunnel.
        assert_eq!(
            std::mem::size_of::<SmokeParams>(),
            16 * (4 + MAX_TUNNELS * 2)
        );
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16(0.0), 0);
        assert_eq!(f16(1.0), 0x3c00);
        assert_eq!(f16(0.5), 0x3800);
        assert_eq!(f16(65504.0), 0x7bff);
        // Too big for a half: clamped to the largest finite value, never infinity.
        assert_eq!(f16(1.0e6), 0x7bff);
    }

    #[test]
    fn packs_extinction_and_color() {
        let mut d = Domain::new(DVec3::ZERO, 1.0, UVec3::splat(4));
        d.splat(
            DVec3::new(2.5, 2.5, 2.5),
            0.8,
            0.01,
            0.0,
            Vec3::ZERO,
            Vec3::new(0.9, 0.1, 0.1),
        );
        let mut out = Vec::new();
        pack_density(&d, 1000.0, &mut out);
        assert_eq!(out.len(), 64);
        let i = d.index(2, 2, 2);
        let v = d.voxels[i];
        assert!(v.density > 0.0);
        let half = |h: u16| {
            // Decode normal halves only (enough for this test).
            let e = ((h >> 10) & 0x1f) as i32;
            let m = (h & 0x3ff) as f32;
            (1.0 + m / 1024.0) * 2f32.powi(e - 15)
        };
        let sigma = half(out[i][0]);
        assert!((sigma - v.density * 1000.0).abs() < sigma * 0.01);
        assert!(half(out[i][1]) > half(out[i][2]) * 5.0);
    }

    #[test]
    fn rays_hit_boxes_from_inside_and_out() {
        let min = Vec3::new(-1.0, -1.0, 5.0);
        let max = Vec3::new(1.0, 1.0, 7.0);
        let (t0, t1) = ray_box(Vec3::Z, min, max).unwrap();
        assert!((t0 - 5.0).abs() < 1e-5 && (t1 - 7.0).abs() < 1e-5);
        assert!(ray_box(Vec3::X, min, max).is_none());
        let (t0, t1) = ray_box(Vec3::X, Vec3::splat(-2.0), Vec3::splat(2.0)).unwrap();
        assert_eq!(t0, 0.0);
        assert!((t1 - 2.0).abs() < 1e-5);
    }
}
