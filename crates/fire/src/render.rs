//! Drawing fire: flame sprites over burning cells and objects, embers, and a screen-space
//! decal that chars the ground and shows glowing coals.

use bytemuck::{Pod, Zeroable};
use engine_core::math::{DVec2, DVec3, IVec2, Vec3};
use engine_core::prelude::*;
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase, RenderSettings,
    ViewData,
};
use engine_world::{Combustible, WindField};

use crate::embers::Embers;
use crate::field::{FireField, TILE};
use crate::objects::{Burning, FlameShape};
use crate::settings::FireSettings;

pub const FLAMES_SHADER: &str = include_str!("shaders/flames.wgsl");
pub const SCORCH_SHADER: &str = include_str!("shaders/scorch.wgsl");

/// Vertices per flame: a strip of four quads up its height (`FLAME_SEGMENTS` in flames.wgsl).
const FLAME_VERTICES: u32 = 4 * 6;

/// GPU layout of `FlameInstance` in `flames.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct FlameInstance {
    pub pos_height: [f32; 4],
    pub shape: [f32; 4],
    pub extra: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ScorchParams {
    rect: [f32; 4],
    ground: [f32; 4],
}

/// The fire map uploaded for the scorch decal: a window over the field.
struct FireMap {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

/// Largest fire map side, texels (at 0.5 m cells, 512 m).
const MAX_MAP: u32 = 1024;

#[derive(Default)]
pub struct FireFeature {
    flame_pipeline: Option<wgpu::RenderPipeline>,
    haze_pipeline: Option<wgpu::RenderPipeline>,
    /// The flame flipbook (made on a worker thread at startup, uploaded when first needed).
    flipbook: Option<(wgpu::Texture, wgpu::TextureView)>,
    scorch_pipeline: Option<wgpu::RenderPipeline>,
    flame_layout: Option<wgpu::BindGroupLayout>,
    scorch_layout: Option<wgpu::BindGroupLayout>,
    instances: Option<GrowBuffer>,
    params: Option<wgpu::Buffer>,
    flame_group: Option<wgpu::BindGroup>,
    scorch_group: Option<wgpu::BindGroup>,
    map: Option<FireMap>,
    scratch: Vec<FlameInstance>,
    keyed: Vec<(f32, FlameInstance)>,
    map_data: Vec<[u16; 4]>,
    /// Field version the map was built from.
    map_version: u64,
    /// Map window: world X/Z of its min corner, texels, base height.
    map_origin: DVec2,
    map_texels: [u32; 2],
    map_base_y: f64,
    map_valid: bool,
    count: u32,
    stats: FeatureStats,
}

impl FireFeature {
    fn build_instances(&mut self, world: &mut World, view: &ViewData) {
        self.scratch.clear();
        let mut q = world.query::<(
            Entity,
            &GlobalTransform,
            &Combustible,
            &Burning,
            Option<&FlameShape>,
        )>();
        let objects: Vec<(u32, DVec3, f32, Burning, Option<FlameShape>)> = q
            .iter(world)
            .filter(|(.., b, _)| b.intensity > 0.02)
            .map(|(e, gt, c, b, s)| (e.index_u32(), gt.translation(), c.fuel_kg, *b, s.copied()))
            .collect();
        let Some(field) = world.get_resource::<FireField>() else {
            return;
        };
        let max_d = world
            .get_resource::<FireSettings>()
            .map(|s| s.view_distance_m)
            .unwrap_or(1500.0) as f64;
        let wind = world.get_resource::<WindField>();
        let cell = field.cell_m;
        let tile_size = field.tile_size();
        let lean_of = |p: DVec3, h: f32| {
            let w = wind
                .map(|w| w.sample(p + DVec3::Y * h as f64 * 0.5))
                .unwrap_or(Vec3::ZERO);
            // Flames lean with the wind but their own buoyancy keeps them up; tall flames (big,
            // buoyant fires) stand up better than knee-high grass flames.
            let l = w / (3.0 + w.length() * 0.35) * (1.5 / h.max(1.5)).sqrt();
            [l.x, l.z]
        };
        let origin = view.origin;
        for tile in &field.tiles {
            if tile.burning == 0 {
                continue;
            }
            let o = field.tile_origin(tile.key);
            let center = o + DVec2::splat(tile_size * 0.5);
            let d = DVec2::new(center.x - origin.x, center.y - origin.z).length();
            if d > max_d + tile_size {
                continue;
            }
            let rel_c = (DVec3::new(center.x, tile.base_y, center.y) - origin).as_vec3();
            if !view
                .frustum
                .intersects_sphere(rel_c, tile_size as f32 + 8.0)
            {
                continue;
            }
            // Merge cells far away: one sprite per 2×2 or 4×4 block.
            let block = if d < 70.0 {
                1
            } else if d < 220.0 {
                2
            } else {
                4
            };
            for by in (0..TILE).step_by(block as usize) {
                for bx in (0..TILE).step_by(block as usize) {
                    let mut burn = 0.0f32;
                    let mut flame = 0.0f32;
                    let mut gy = 0.0f32;
                    let mut n = 0;
                    for y in by..by + block {
                        for x in bx..bx + block {
                            let c = tile.cell(IVec2::new(x, y));
                            if c.is_burning() {
                                burn = burn.max(c.burn);
                                flame = flame.max(c.flame_length());
                                gy += c.ground_y;
                                n += 1;
                            }
                        }
                    }
                    if n == 0 {
                        continue;
                    }
                    let gx = (tile.key.x * TILE + bx) as i64;
                    let gz = (tile.key.y * TILE + by) as i64;
                    let r1 = engine_core::math::hash01(gx, gz, 1, 0xF1);
                    let r2 = engine_core::math::hash01(gx, gz, 2, 0xF1);
                    let r3 = engine_core::math::hash01(gx, gz, 3, 0xF1);
                    let size = block as f64 * cell as f64;
                    let p = DVec3::new(
                        o.x + bx as f64 * cell as f64 + size * (0.25 + 0.5 * r1 as f64),
                        tile.base_y + (gy / n as f32) as f64 - 0.05,
                        o.y + by as f64 * cell as f64 + size * (0.25 + 0.5 * r2 as f64),
                    );
                    let h = flame * (0.4 + 1.2 * r3 * r3) * (1.0 + 0.15 * (block as f32 - 1.0));
                    if h < 0.12 {
                        continue;
                    }
                    // The flipbook's flame fills about 70% of the sprite's width at its base.
                    let w = (size as f32 * 0.8 + h * 0.3).max(0.3);
                    let rel = (p - origin).as_vec3();
                    let lean = lean_of(p, h);
                    self.scratch.push(FlameInstance {
                        pos_height: [rel.x, rel.y, rel.z, h.max(0.2)],
                        // Ground flames overlap a lot along a front: each one is dimmer.
                        shape: [w, burn.min(1.0) * 0.55, r1 * 97.0 + r2 * 13.0, 0.0],
                        extra: [lean[0], lean[1], burn, 0.0],
                    });
                }
            }
        }

        // Burning objects: a cluster of sprites filling their flame volume.
        for (id, pos, fuel_kg, b, shape) in objects {
            let shape = shape.unwrap_or_else(|| FlameShape::from_fuel(fuel_kg));
            let base = shape.base(pos);
            let rel_c = (shape.center(pos) - origin).as_vec3();
            if rel_c.length() as f64 > max_d
                || !view
                    .frustum
                    .intersects_sphere(rel_c, shape.radius + shape.height)
            {
                continue;
            }
            let count = ((shape.radius * shape.height * 2.0).clamp(7.0, 32.0) * b.intensity.sqrt())
                .ceil() as u32;
            let id = id as i64;
            for i in 0..count {
                let r = |k: i64| engine_core::math::hash01(id, i as i64, k, 0xF2);
                let a = r(1) * std::f32::consts::TAU;
                // The first flame is the tall one in the middle; the others crowd round it.
                let (rad, y0, tall) = if i == 0 {
                    (0.0, 0.0, 1.0)
                } else {
                    (
                        r(2).sqrt() * shape.radius * 0.75,
                        r(3) * r(3) * shape.height * 0.4,
                        0.5 + 0.4 * r(4),
                    )
                };
                let h = shape.height * tall * b.intensity.sqrt().max(0.6);
                let p =
                    base + DVec3::new((a.cos() * rad) as f64, y0 as f64, (a.sin() * rad) as f64);
                let rel = (p - origin).as_vec3();
                let lean = lean_of(p, h);
                self.scratch.push(FlameInstance {
                    pos_height: [rel.x, rel.y, rel.z, h],
                    shape: [
                        // No wider than about half the height: the flipbook's tongues smear
                        // sideways when stretched; wide fires get more sprites instead.
                        (shape.radius * 0.8 + h * 0.2).min(h * 0.45).max(0.25)
                            * if i == 0 { 1.2 } else { 1.0 },
                        b.intensity.min(1.0),
                        r(5) * 113.0,
                        0.0,
                    ],
                    extra: [lean[0], lean[1], b.intensity, 0.0],
                });
            }
        }

        // Embers.
        if let Some(embers) = world.get_resource::<Embers>() {
            for e in &embers.list {
                let rel = (e.position - origin).as_vec3();
                if rel.length() > 400.0 || !view.frustum.intersects_sphere(rel, 1.0) {
                    continue;
                }
                self.scratch.push(FlameInstance {
                    pos_height: [rel.x, rel.y, rel.z, 0.0],
                    shape: [
                        e.size.max(0.01) * 1.5,
                        1.0,
                        e.age * 3.1 + e.size * 1000.0,
                        1.0,
                    ],
                    extra: [
                        e.velocity.x,
                        e.velocity.y,
                        e.velocity.z,
                        e.heat.clamp(0.0, 1.0),
                    ],
                });
            }
        }

        // Back to front (alpha is small, but it keeps tall flames in front looking right).
        self.keyed.clear();
        self.keyed.extend(self.scratch.iter().map(|i| {
            let p = Vec3::new(i.pos_height[0], i.pos_height[1], i.pos_height[2]);
            (p.length_squared(), *i)
        }));
        self.keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.scratch.clear();
        self.scratch.extend(self.keyed.iter().map(|(_, i)| *i));
    }

    /// Rebuild the fire map texture when the field changed.
    fn build_map(&mut self, world: &World, gpu: &GpuShared, view: &ViewData) {
        let Some(field) = world.get_resource::<FireField>() else {
            self.map_valid = false;
            return;
        };
        let version: u64 = field
            .tiles
            .iter()
            .map(|t| t.version)
            .sum::<u64>()
            .wrapping_add(field.tiles.len() as u64 * 1_000_003);
        let cell = field.cell_m as f64;
        let tile_size = field.tile_size();
        let max_d = world
            .get_resource::<FireSettings>()
            .map(|s| s.view_distance_m)
            .unwrap_or(1500.0) as f64;
        // Window: tiles with something to show near the camera, clamped around the camera.
        let mut min = DVec2::splat(f64::INFINITY);
        let mut max = DVec2::splat(f64::NEG_INFINITY);
        let mut base_y = f64::INFINITY;
        let cam = DVec2::new(view.origin.x, view.origin.z);
        let half = MAX_MAP as f64 * cell * 0.5;
        for t in &field.tiles {
            let o = field.tile_origin(t.key);
            let c = o + DVec2::splat(tile_size * 0.5);
            if (c - cam).length() > max_d.min(half * 1.5) + tile_size {
                continue;
            }
            if !t
                .cells
                .iter()
                .any(|c| c.scorch > 0.0 || c.glow > 0.0 || c.burn > 0.0)
            {
                continue;
            }
            min = min.min(o);
            max = max.max(o + DVec2::splat(tile_size));
            base_y = base_y.min(t.base_y);
        }
        if !min.x.is_finite() {
            self.map_valid = false;
            return;
        }
        min = min.max(cam - DVec2::splat(half));
        max = max.min(cam + DVec2::splat(half));
        if max.x <= min.x || max.y <= min.y {
            self.map_valid = false;
            return;
        }
        let texels = [
            (((max.x - min.x) / cell).round() as u32).clamp(1, MAX_MAP),
            (((max.y - min.y) / cell).round() as u32).clamp(1, MAX_MAP),
        ];
        let same_window = self.map_valid
            && self.map_origin == min
            && self.map_texels == texels
            && self.map_base_y == base_y;
        if same_window && version == self.map_version {
            return;
        }
        self.map_version = version;
        self.map_origin = min;
        self.map_texels = texels;
        self.map_base_y = base_y;
        self.map_valid = true;

        let [w, h] = texels;
        self.map_data.clear();
        self.map_data.resize((w * h) as usize, [0; 4]);
        let first = field.cell_coord(DVec3::new(min.x + cell * 0.5, 0.0, min.y + cell * 0.5));
        for t in &field.tiles {
            let o = t.key * TILE;
            for ly in 0..TILE {
                let ty = o.y + ly - first.y;
                if ty < 0 || ty >= h as i32 {
                    continue;
                }
                for lx in 0..TILE {
                    let tx = o.x + lx - first.x;
                    if tx < 0 || tx >= w as i32 {
                        continue;
                    }
                    let c = t.cell(IVec2::new(lx, ly));
                    let ground = (t.base_y + c.ground_y as f64 - base_y) as f32;
                    self.map_data[(ty as u32 * w + tx as u32) as usize] = [
                        f16(c.scorch),
                        f16(c.glow.max(c.burn * 0.6)),
                        f16(c.burn),
                        f16(ground),
                    ];
                }
            }
        }
        let recreate = self.map.as_ref().is_none_or(|m| m.size != texels);
        if recreate {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fire map"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.map = Some(FireMap {
                texture,
                view,
                size: texels,
            });
            self.scorch_group = None;
        }
        let map = self.map.as_ref().expect("fire map");
        gpu.queue.write_texture(
            map.texture.as_image_copy(),
            bytemuck::cast_slice(&self.map_data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// The flame flipbook as a 3D texture (across, up, frame), simulating it first if the worker
/// thread hasn't finished.
fn upload_flipbook(gpu: &GpuShared) -> (wgpu::Texture, wgpu::TextureView) {
    let book = crate::flipbook::flipbook();
    let [w, h, frames] = book.size;
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: frames,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fire flipbook"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rg8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&book.texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 2),
            rows_per_image: Some(h),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// f32 → IEEE half (round to nearest, no NaN handling needed for these values).
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
        return sign | 0x7c00;
    }
    let h = sign as u32 | ((exp as u32) << 10) | (mant >> 13);
    // Round half up on the dropped bits.
    (h + ((mant >> 12) & 1)) as u16
}

impl RenderFeature for FireFeature {
    fn name(&self) -> &str {
        "fire"
    }

    /// After water and before smoke, so smoke drawn later veils the flames.
    fn order(&self) -> i32 {
        40
    }

    fn needs_scene_copy(&self) -> bool {
        true
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let depth_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let flame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fire flames"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                depth_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let scorch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fire scorch"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                depth_entry(2),
            ],
        });
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
        let pipeline = |label: &str,
                        src: &str,
                        layout: &wgpu::BindGroupLayout,
                        vs: &str,
                        fs: &str,
                        depth_test: bool| {
            let shader = gpu.shader(label, src, true);
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&gpu.frame_layout), Some(layout)],
                immediate_size: 0,
            });
            let mut depth = gpu.depth_state(false);
            if !depth_test {
                depth.depth_compare = Some(wgpu::CompareFunction::Always);
            }
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
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
                        blend: Some(premultiplied),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.flame_pipeline = Some(pipeline(
            "fire flames",
            FLAMES_SHADER,
            &flame_layout,
            "vs_flame",
            "fs_flame",
            true,
        ));
        self.haze_pipeline = Some(pipeline(
            "fire heat haze",
            FLAMES_SHADER,
            &flame_layout,
            "vs_haze",
            "fs_haze",
            true,
        ));
        // Simulate the flame flipbook in the background; it's waited for only if a flame needs
        // drawing before it's done.
        std::thread::spawn(|| {
            crate::flipbook::flipbook();
        });
        self.scorch_pipeline = Some(pipeline(
            "fire scorch",
            SCORCH_SHADER,
            &scorch_layout,
            "vs_scorch",
            "fs_scorch",
            false,
        ));
        self.instances = Some(GrowBuffer::new(
            device,
            "fire instances",
            wgpu::BufferUsages::STORAGE,
            4096 * std::mem::size_of::<FlameInstance>() as u64,
        ));
        self.params = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fire scorch params"),
            size: std::mem::size_of::<ScorchParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.flame_layout = Some(flame_layout);
        self.scorch_layout = Some(scorch_layout);
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        // The scene depth copy is recreated with the other size-dependent targets.
        self.flame_group = None;
        self.scorch_group = None;
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.stats = FeatureStats::default();
        self.count = 0;
        if world
            .get_resource::<FireSettings>()
            .is_some_and(|s| !s.enabled)
        {
            self.map_valid = false;
            return;
        }
        self.build_instances(world, view);
        self.build_map(world, gpu, view);
        if !self.scratch.is_empty() && self.flipbook.is_none() {
            self.flipbook = Some(upload_flipbook(gpu));
            self.flame_group = None;
        }
        if let Some(buf) = &mut self.instances {
            if !self.scratch.is_empty()
                && buf.write(&gpu.device, &gpu.queue, bytemuck::cast_slice(&self.scratch))
            {
                self.flame_group = None;
            }
            self.count = self.scratch.len() as u32;
        }
        if self.map_valid
            && let Some(params) = &self.params
        {
            let cell = world
                .get_resource::<FireField>()
                .map(|f| f.cell_m)
                .unwrap_or(0.5);
            let rel_min = DVec2::new(
                self.map_origin.x - view.origin.x,
                self.map_origin.y - view.origin.z,
            );
            let p = ScorchParams {
                rect: [
                    rel_min.x as f32,
                    rel_min.y as f32,
                    self.map_texels[0] as f32 * cell,
                    self.map_texels[1] as f32 * cell,
                ],
                ground: [
                    (self.map_base_y - view.origin.y) as f32,
                    cell,
                    view.time,
                    0.0,
                ],
            };
            gpu.queue.write_buffer(params, 0, bytemuck::bytes_of(&p));
        }
        self.stats.instances = self.count;
        self.stats.draw_calls = self.count.min(1) + self.map_valid as u32;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("fire.sprites", self.count as f64);
        }
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let device = &ctx.gpu.device;
        if self.flame_group.is_none()
            && let (Some(layout), Some(buf), Some((_, book))) =
                (&self.flame_layout, &self.instances, &self.flipbook)
        {
            self.flame_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fire flames"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buf.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(book),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(ctx.scene_color_copy),
                    },
                ],
            }));
        }
        if self.scorch_group.is_none()
            && let (Some(layout), Some(params), Some(map)) =
                (&self.scorch_layout, &self.params, &self.map)
        {
            self.scorch_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fire scorch"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&map.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                    },
                ],
            }));
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        // Heat haze first: it replaces pixels with the bent scene copy (taken before the
        // transparent pass), so it must come before anything the fire draws on top, the scorch
        // decal included.
        let flames = match (&self.haze_pipeline, &self.flame_pipeline, &self.flame_group) {
            (Some(haze), Some(p), Some(g)) if self.count > 0 => {
                pass.set_pipeline(haze);
                pass.set_bind_group(1, g, &[]);
                pass.draw(0..FLAME_VERTICES, 0..self.count);
                Some((p, g))
            }
            _ => None,
        };
        if self.map_valid
            && let (Some(p), Some(g)) = (&self.scorch_pipeline, &self.scorch_group)
        {
            pass.set_pipeline(p);
            pass.set_bind_group(1, g, &[]);
            pass.draw(0..3, 0..1);
        }
        if let Some((p, g)) = flames {
            pass.set_pipeline(p);
            pass.set_bind_group(1, g, &[]);
            pass.draw(0..FLAME_VERTICES, 0..self.count);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn shaders_validate() {
        for (name, src) in [("flames", FLAMES_SHADER), ("scorch", SCORCH_SHADER)] {
            validate_wgsl(name, &compose_shader(src, true)).unwrap();
        }
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16(0.0), 0);
        assert_eq!(f16(1.0), 0x3c00);
        assert_eq!(f16(-2.0), 0xc000);
        assert_eq!(f16(0.5), 0x3800);
        assert_eq!(f16(65504.0), 0x7bff);
    }
}
