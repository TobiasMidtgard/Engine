//! [`BodiesFeature`]: draws every far-view impostor in the transparent phase (before other
//! transparent features, as bodies are the farthest things in the scene), back to front.
//! Owns the bodies' GPU maps and bakes them: a small map at once, the full one on a
//! background thread when the body is big on screen.

use std::collections::HashMap;
use std::thread::JoinHandle;

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::{Atmosphere, DirectionalLight};
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, Image, RenderFeature, RenderPhase,
    RenderSettings, ViewData,
};

use super::bake::{SurfaceMaps, bake_rings, bake_surface};
use super::{
    BodyImpostor, BodyLook, BodyRings, FULL_MAP_WIDTH, ImpostorPart, LookBakes, RING_MAP_WIDTH,
    SMALL_MAP_WIDTH, spheroid_radii,
};
use crate::{CelestialBody, SpaceClock, SpaceSettings, StarLight};

/// The bodies shader (composed after `lib_types` and `lib_frame`).
pub const SHADER: &str = include_str!("bodies.wgsl");

/// Bodies whose disk is wider than this (radius, pixels) get the full-detail maps.
const FULL_DETAIL_PX: f32 = 40.0;

const F_RINGS: u32 = 1;
const F_CLOUDS: u32 = 2;
const F_RELIEF: u32 = 4;
const F_AIR: u32 = 8;
const F_FULLSCREEN: u32 = 16;
const F_HAZE: u32 = 32;
const F_SHELL_ON: u32 = 128;

/// Height of the cloud tops drawn as their own layer from high up, meters.
const SHELL_HEIGHT_M: f64 = 5_000.0;
/// The cloud layer is drawn from below this altitude (in body radii); farther out the body's
/// own disk carries its clouds.
const SHELL_RANGE: f64 = 0.4;
/// The cloud layer fades in between these camera altitudes, meters (below, the weather's
/// clouds are the sky).
const SHELL_FADE_M: (f64, f64) = (9_000.0, 25_000.0);

/// GPU layout of `Body` in `bodies.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuBody {
    center: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    axis_z: [f32; 4],
    sun: [f32; 4],
    light: [f32; 4],
    shade: [f32; 4],
    haze: [f32; 4],
    haze2: [f32; 4],
    haze3: [f32; 4],
    rings: [f32; 4],
    flags: [u32; 4],
}

/// What a body's maps depend on: a change re-bakes them.
#[derive(Debug, Clone, PartialEq)]
struct LookKey {
    name: String,
    look: BodyLook,
    color: [f32; 3],
    rings: Option<BodyRings>,
}

impl LookKey {
    fn of(b: &CelestialBody) -> Self {
        Self {
            name: b.name.clone(),
            look: b.look.clone(),
            color: b.color,
            rings: b.rings.clone(),
        }
    }
}

/// A body's maps on the GPU.
struct BodyMaps {
    key: LookKey,
    width: u32,
    group: wgpu::BindGroup,
    clouds: bool,
    relief: bool,
    _textures: Vec<wgpu::Texture>,
}

struct Job {
    body: Entity,
    key: LookKey,
    handle: JoinHandle<(SurfaceMaps, Option<Image>)>,
}

/// Draws planets and moons from afar: ray-traced spheroids with baked maps, rings and haze.
/// Added by `SpacePlugin`.
#[derive(Default)]
pub struct BodiesFeature {
    pipeline: Option<wgpu::RenderPipeline>,
    shell_pipeline: Option<wgpu::RenderPipeline>,
    layout1: Option<wgpu::BindGroupLayout>,
    layout2: Option<wgpu::BindGroupLayout>,
    instances: Option<GrowBuffer>,
    group1: Option<wgpu::BindGroup>,
    map_sampler: Option<wgpu::Sampler>,
    ring_sampler: Option<wgpu::Sampler>,
    blank: Option<(wgpu::Texture, wgpu::Texture)>,
    maps: HashMap<Entity, BodyMaps>,
    /// Bodies that want full-detail maps, with their size on screen (pixels).
    wanted: Vec<(Entity, f32)>,
    job: Option<Job>,
    scratch: Vec<(f64, Entity, GpuBody, bool)>,
    instance_data: Vec<GpuBody>,
    /// Bodies to draw, back to front, and whether each is a cloud shell.
    draws: Vec<(Entity, bool)>,
    stats: FeatureStats,
}

fn upload(gpu: &GpuShared, image: &Image, srgb: bool, label: &str) -> wgpu::Texture {
    let levels: Vec<(u32, u32, &[u8])> =
        std::iter::once((image.width, image.height, image.data.as_slice()))
            .chain(image.mips.iter().enumerate().map(|(i, m)| {
                let (w, h) = image.level_size(i as u32 + 1);
                (w, h, m.as_slice())
            }))
            .collect();
    let format = if srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (w, h, data)) in levels.iter().enumerate() {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(*h),
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
    texture
}

impl BodiesFeature {
    fn make_maps(
        &self,
        gpu: &GpuShared,
        key: LookKey,
        maps: &SurfaceMaps,
        rings: Option<&Image>,
    ) -> Option<BodyMaps> {
        let layout = self.layout2.as_ref()?;
        let (blank_aux, blank_ring) = self.blank.as_ref()?;
        let albedo = upload(gpu, &maps.albedo, true, "body albedo");
        let aux = (maps.clouds || maps.relief).then(|| upload(gpu, &maps.aux, false, "body aux"));
        let ring = rings.map(|r| upload(gpu, r, true, "body rings"));
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (va, vx, vr) = (
            view(&albedo),
            view(aux.as_ref().unwrap_or(blank_aux)),
            view(ring.as_ref().unwrap_or(blank_ring)),
        );
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("body maps"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&va),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&vx),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&vr),
                },
            ],
        });
        let mut textures = vec![albedo];
        textures.extend(aux);
        textures.extend(ring);
        Some(BodyMaps {
            key,
            width: maps.albedo.width,
            group,
            clouds: maps.clouds,
            relief: maps.relief,
            _textures: textures,
        })
    }

    /// Make sure `entity` has maps (bake the small ones now) and note whether it wants the
    /// full ones.
    fn ensure_maps(
        &mut self,
        gpu: &GpuShared,
        entity: Entity,
        body: &CelestialBody,
        radius_px: f32,
        bakes: &mut LookBakes,
    ) {
        let key = LookKey::of(body);
        let stale = self.maps.get(&entity).is_none_or(|m| m.key != key);
        if stale {
            let maps = bake_surface(body, SMALL_MAP_WIDTH);
            let rings = body.rings.as_ref().map(|r| bake_rings(r, RING_MAP_WIDTH));
            bakes
                .done
                .push((body.name.clone(), SMALL_MAP_WIDTH, maps.seconds));
            if let Some(m) = self.make_maps(gpu, key, &maps, rings.as_ref()) {
                self.maps.insert(entity, m);
            }
        }
        if radius_px > FULL_DETAIL_PX
            && self
                .maps
                .get(&entity)
                .is_some_and(|m| m.width < FULL_MAP_WIDTH)
        {
            self.wanted.push((entity, radius_px));
        }
    }

    /// Collect a finished background bake, or start the next one.
    fn run_jobs(&mut self, world: &mut World, gpu: &GpuShared, bakes: &mut LookBakes) {
        if self.job.as_ref().is_some_and(|j| j.handle.is_finished())
            && let Some(job) = self.job.take()
            && let Ok((maps, rings)) = job.handle.join()
        {
            let current = world
                .get::<CelestialBody>(job.body)
                .map(LookKey::of)
                .is_some_and(|k| k == job.key);
            bakes
                .done
                .push((job.key.name.clone(), maps.albedo.width, maps.seconds));
            if current && let Some(m) = self.make_maps(gpu, job.key, &maps, rings.as_ref()) {
                self.maps.insert(job.body, m);
            }
            let maps = &self.maps;
            self.wanted
                .retain(|(e, _)| maps.get(e).is_some_and(|m| m.width < FULL_MAP_WIDTH));
        }
        self.wanted.sort_by(|a, b| b.1.total_cmp(&a.1));
        if self.job.is_none()
            && let Some(&(entity, _)) = self.wanted.first()
            && let Some(body) = world.get::<CelestialBody>(entity).cloned()
        {
            let key = LookKey::of(&body);
            let handle = std::thread::Builder::new()
                .name(format!("bake {}", body.name))
                .spawn(move || {
                    let maps = bake_surface(&body, FULL_MAP_WIDTH);
                    let rings = body.rings.as_ref().map(|r| bake_rings(r, RING_MAP_WIDTH));
                    (maps, rings)
                });
            if let Ok(handle) = handle {
                self.job = Some(Job {
                    body: entity,
                    key,
                    handle,
                });
            }
        }
        bakes.pending = self.wanted.len().max(self.job.is_some() as usize);
    }
}

impl RenderFeature for BodiesFeature {
    fn name(&self) -> &str {
        "space bodies"
    }

    /// First among the transparent features: bodies are behind everything else.
    fn order(&self) -> i32 {
        -50
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let layout1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("space bodies"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("space body maps"),
            entries: &[tex(0), tex(1), tex(2)],
        });
        let shader = gpu.shader("space bodies", SHADER, true);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("space bodies"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout1), Some(&layout2)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = |label: &str, entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_body"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(gpu.depth_state(false)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: Some(premultiplied),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.pipeline = Some(pipeline("space bodies", "fs_body"));
        self.shell_pipeline = Some(pipeline("space cloud shells", "fs_shell"));
        self.map_sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("body maps"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        }));
        self.ring_sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("body rings"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        }));
        // Flat aux map (no clouds, flat slopes) and empty rings for bodies without them.
        let flat = Image::solid([0, 128, 128, 255], false);
        let empty = Image::solid([0, 0, 0, 0], true);
        self.blank = Some((
            upload(gpu, &flat, false, "flat aux"),
            upload(gpu, &empty, true, "no rings"),
        ));
        self.instances = Some(GrowBuffer::new(
            device,
            "space bodies",
            wgpu::BufferUsages::STORAGE,
            16 * std::mem::size_of::<GpuBody>() as u64,
        ));
        self.layout1 = Some(layout1);
        self.layout2 = Some(layout2);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.draws.clear();
        self.wanted.clear();
        self.stats = FeatureStats::default();
        let Some(space) = world.get_resource::<SpaceSettings>().cloned() else {
            return;
        };
        let mut bakes = world.remove_resource::<LookBakes>().unwrap_or_default();

        // Stars light the bodies; the StarLight's color tints it.
        let mut q = world.query::<(&CelestialBody, &GlobalTransform)>();
        let stars: Vec<(DVec3, f64)> = q
            .iter(world)
            .filter(|(b, _)| b.luminosity_w > 0.0)
            .map(|(b, gt)| (gt.translation(), b.luminosity_w))
            .collect();
        let mut q = world.query_filtered::<&DirectionalLight, With<StarLight>>();
        let light_color = q
            .iter(world)
            .next()
            .map(|l| Vec3::from(l.color))
            .unwrap_or(Vec3::new(1.0, 0.96, 0.9));
        let air = world
            .get_resource::<Atmosphere>()
            .filter(|a| a.enabled)
            .map(|a| a.planet_center);
        let clock = world
            .get_resource::<SpaceClock>()
            .map(|c| c.seconds)
            .unwrap_or(0.0);

        let mut q = world.query::<(&ChildOf, &ImpostorPart, &BodyImpostor, &GlobalTransform)>();
        let parts: Vec<(Entity, f64, GlobalTransform)> = q
            .iter(world)
            .filter(|(_, part, ..)| **part == ImpostorPart::Body)
            .map(|(p, _, imp, gt)| (p.parent(), imp.radius_m, *gt))
            .collect();

        self.scratch.clear();
        for (entity, depth_radius, gt) in parts {
            let Some(body) = world.get::<CelestialBody>(entity).cloned() else {
                continue;
            };
            if !space.draw_bodies || body.look == BodyLook::None {
                continue;
            }
            let m = gt.0.matrix3;
            let scale = m.x_axis.length().max(1e-9);
            let center = gt.translation();
            let rel = center - view.origin;
            let d = rel.length();
            let radius = body.radius_m * scale;
            let (r_eq, r_pol) = spheroid_radii(radius, body.flattening);
            let haze = body.haze.filter(|h| h.optical_depth > 0.0);
            let haze_h = haze.map_or(0.0, |h| h.scale_height_m * scale / r_eq);
            let ring_out = body
                .rings
                .as_ref()
                .map_or(0.0, |r| r.outer_m * scale / r_eq);
            let bound = r_eq * (1.0 + 12.0 * haze_h).max(ring_out);
            let rel_f = rel.as_vec3();
            if !view.frustum.intersects_sphere(rel_f, bound as f32) {
                continue;
            }
            // Down on a solid body, the terrain (or the sky's ground) takes over; from high up
            // its cloud tops are still drawn, as a layer of their own.
            let altitude = (d - radius) / scale;
            let ground = body.look.solid() && d - radius < radius * 0.002;
            let shell_wanted = body.look.clouds()
                && altitude < body.radius_m * SHELL_RANGE
                && altitude > SHELL_FADE_M.0;
            if ground && !shell_wanted {
                continue;
            }
            let radius_px = view.projected_radius_px(r_eq as f32, d as f32);
            self.ensure_maps(gpu, entity, &body, radius_px, &mut bakes);
            let Some(maps) = self.maps.get(&entity) else {
                continue;
            };
            let shell = shell_wanted && maps.clouds;
            if ground && !shell {
                continue;
            }

            let axes = [
                (m.x_axis / scale).normalize(),
                (m.y_axis / scale).normalize(),
                (m.z_axis / scale).normalize(),
            ];
            // Sunlight at the body.
            let (sun_dir, lux) = stars
                .iter()
                .map(|&(p, l)| {
                    let to = p - center;
                    let lux = l / (4.0 * std::f64::consts::PI * to.length_squared().max(1.0))
                        * space.lumens_per_watt;
                    (to.normalize_or(DVec3::Y), lux)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(d, l)| (d, Vec3::splat(l as f32) * light_color))
                .unwrap_or((view.sun_dir.as_dvec3(), view.sun_color));
            let sun_local = [
                sun_dir.dot(axes[0]) as f32,
                sun_dir.dot(axes[1]) as f32,
                sun_dir.dot(axes[2]) as f32,
            ];
            let shading = body.look.shading();
            let mut flags = 0;
            let in_air = body.atmosphere.is_some()
                && air.is_some_and(|c| (c - center).length() < radius * 1e-6 + 1.0);
            if in_air {
                flags |= F_AIR;
            } else if haze.is_some() {
                flags |= F_HAZE;
            }
            if body.rings.is_some() {
                flags |= F_RINGS;
            }
            if maps.clouds {
                flags |= F_CLOUDS;
            }
            if shell {
                flags |= F_SHELL_ON;
            }
            if maps.relief && shading.relief > 0.0 {
                flags |= F_RELIEF;
            }
            if d < bound * 1.15 {
                flags |= F_FULLSCREEN;
            }
            let h = haze.unwrap_or_default();
            let (inner, outer) = body.rings.as_ref().map_or((0.0, 0.0), |r| {
                (
                    (r.inner_m * scale / r_eq) as f32,
                    (r.outer_m * scale / r_eq) as f32,
                )
            });
            let v4 = |v: DVec3, w: f64| [v.x as f32, v.y as f32, v.z as f32, w as f32];
            // Clouds drift east about 1.5° a day relative to the ground.
            let drift = ((clock / 86_400.0 * 0.004) % 1.0) as f32;
            let gpu_body = GpuBody {
                center: [rel_f.x, rel_f.y, rel_f.z, bound as f32],
                axis_x: v4(axes[0], r_eq),
                axis_y: v4(axes[1], r_pol),
                axis_z: v4(axes[2], depth_radius * scale),
                sun: [sun_local[0], sun_local[1], sun_local[2], drift],
                light: [lux.x, lux.y, lux.z, shading.relief],
                shade: [
                    shading.minnaert,
                    shading.lunar,
                    shading.glint,
                    (8_000.0 / body.radius_m) as f32 * 3.0,
                ],
                haze: [
                    h.extinction[0],
                    h.extinction[1],
                    h.extinction[2],
                    h.optical_depth,
                ],
                haze2: [h.albedo[0], h.albedo[1], h.albedo[2], haze_h as f32],
                haze3: [h.forward, 0.0, 0.0, 0.0],
                rings: [inner, outer, 1.0, 0.0],
                flags: [flags, 0, 0, 0],
            };
            if !ground {
                self.scratch.push((d, entity, gpu_body, false));
            }
            if shell {
                // The cloud tops, a few kilometers up, just in front of the body.
                let k = 1.0 + SHELL_HEIGHT_M * scale / r_eq;
                let t = ((altitude - SHELL_FADE_M.0) / (SHELL_FADE_M.1 - SHELL_FADE_M.0))
                    .clamp(0.0, 1.0);
                let fade = t * t * (3.0 - 2.0 * t);
                let mut g = gpu_body;
                g.center[3] = (r_eq * k) as f32;
                g.flags[0] = (flags & F_AIR) | if d < r_eq * k * 1.15 { F_FULLSCREEN } else { 0 };
                g.haze3 = [
                    0.0,
                    k as f32,
                    fade as f32,
                    (space.cloud_hole_m * scale / r_eq) as f32,
                ];
                self.scratch.push((d * (1.0 - 1e-6), entity, g, true));
            }
        }
        self.run_jobs(world, gpu, &mut bakes);
        world.insert_resource(bakes);

        // Back to front.
        self.scratch.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.instance_data.clear();
        for (_, e, g, shell) in &self.scratch {
            self.instance_data.push(*g);
            self.draws.push((*e, *shell));
        }
        if let Some(buf) = &mut self.instances
            && !self.instance_data.is_empty()
            && buf.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&self.instance_data),
            )
        {
            self.group1 = None;
        }
        self.stats.instances = self.draws.len() as u32;
        self.stats.draw_calls = self.draws.len() as u32;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("space.bodies_drawn", self.draws.len() as f64);
        }
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if self.group1.is_none()
            && let (Some(layout), Some(buf), Some(ms), Some(rs)) = (
                &self.layout1,
                &self.instances,
                &self.map_sampler,
                &self.ring_sampler,
            )
        {
            self.group1 = Some(
                ctx.gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("space bodies"),
                        layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: buf.buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(ms),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(rs),
                            },
                        ],
                    }),
            );
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent || self.draws.is_empty() {
            return;
        }
        let (Some(pipeline), Some(shells), Some(group1)) =
            (&self.pipeline, &self.shell_pipeline, &self.group1)
        else {
            return;
        };
        pass.set_bind_group(1, group1, &[]);
        for (i, (e, shell)) in self.draws.iter().enumerate() {
            if let Some(m) = self.maps.get(e) {
                pass.set_pipeline(if *shell { shells } else { pipeline });
                pass.set_bind_group(2, &m.group, &[]);
                pass.draw(0..6, i as u32..i as u32 + 1);
            }
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "bodies": self.draws.len(),
            "maps": self.maps.values().map(|m| (m.key.name.clone(), m.width)).collect::<Vec<_>>(),
            "baking": self.job.as_ref().map(|j| j.key.name.clone()),
        })
    }
}
