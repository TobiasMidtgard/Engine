//! Grass: meadows of individually drawn blades that sway in the wind, with gusts rolling across
//! them, part around characters ([`GrassBender`]) and thin out smoothly with distance.
//!
//! A [`GrassField`] covers a rectangle of ground on its entity's local x/z plane (the entity's
//! rotation tilts it, for example onto a planet's surface) and carries a map of ground height,
//! density and dryness on a grid. Nothing per blade is stored: the vertex shader grows every
//! blade from its tile's grid coordinates and its index (`shaders/grass_common.wgsl`), so a
//! field of millions of blades costs a small texture. Blades go into the depth prepass (so
//! ambient occlusion, bounce light and anti-aliasing see them), the scene and, near the camera,
//! the sun's shadow cascades.
//!
//! ```text
//! prepare: fields → map textures (on change); tiles around the camera → cull (view frustum,
//!          cascades) → blade count from the nearest point's LOD → tile list + draw list
//! draw:    one instanced triangle-strip draw per tile (instance = tile << 16 | blade)
//! ```

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::features::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase,
};
use crate::frame::ViewData;
use crate::renderer::layout_entry;
use crate::settings::RenderSettings;
use crate::shadows::MAX_CASCADES;

/// Tile edge, meters (must match `GRASS_TILE` in the shader).
const TILE: f32 = 8.0;
/// Instance index bits for the blade within its tile.
const BLADE_BITS: u32 = 16;
const MAX_BLADES_PER_TILE: u32 = (1 << BLADE_BITS) - 1;
const MAX_BENDERS: usize = 16;
const MAX_BLADES_PER_M2: f32 = 1000.0;

/// A meadow: grass blades over a rectangle of ground.
///
/// The rectangle is `size` meters along the entity's local x and z, centered on the entity.
/// `heights`, `density` and `dryness` are maps on a `resolution[0] × resolution[1]` grid over
/// it (row by row along x, the first point at the -x -z corner, the last at the +x +z corner);
/// an empty map means flat ground, grass everywhere and `style.dryness`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct GrassField {
    /// Size along the entity's local x and z, meters.
    pub size: [f32; 2],
    /// Grid points along x and z of the maps.
    pub resolution: [u32; 2],
    /// Ground height at each grid point, meters along the entity's local up.
    pub heights: Vec<f32>,
    /// How thickly grass grows at each grid point, 0..1 (short and sparse toward 0).
    pub density: Vec<f32>,
    /// 0 = fresh green .. 1 = dry straw at each grid point.
    pub dryness: Vec<f32>,
    pub style: GrassStyle,
}

impl Default for GrassField {
    fn default() -> Self {
        Self {
            size: [40.0, 40.0],
            resolution: [0, 0],
            heights: Vec::new(),
            density: Vec::new(),
            dryness: Vec::new(),
            style: GrassStyle::default(),
        }
    }
}

impl GrassField {
    /// A flat meadow `x` by `z` meters.
    pub fn flat(x: f32, z: f32) -> Self {
        Self {
            size: [x, z],
            ..Default::default()
        }
    }

    /// Fill the maps by sampling `f(x, z) -> (height, density, dryness)` at every grid point
    /// (`x`, `z` in meters from the field's center along its local axes).
    pub fn sample(
        mut self,
        resolution: [u32; 2],
        mut f: impl FnMut(f32, f32) -> (f32, f32, f32),
    ) -> Self {
        let [rx, rz] = resolution.map(|r| r.max(2));
        self.resolution = [rx, rz];
        let n = (rx * rz) as usize;
        self.heights = Vec::with_capacity(n);
        self.density = Vec::with_capacity(n);
        self.dryness = Vec::with_capacity(n);
        for j in 0..rz {
            for i in 0..rx {
                let x = (i as f32 / (rx - 1) as f32 - 0.5) * self.size[0];
                let z = (j as f32 / (rz - 1) as f32 - 0.5) * self.size[1];
                let (h, d, dry) = f(x, z);
                self.heights.push(h);
                self.density.push(d);
                self.dryness.push(dry);
            }
        }
        self
    }

    pub fn with_style(mut self, style: GrassStyle) -> Self {
        self.style = style;
        self
    }
}

/// How the blades of a [`GrassField`] look and behave.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrassStyle {
    /// Blades per square meter where density is 1 (at most 1000).
    pub blades_per_m2: f32,
    /// Typical blade height, meters.
    pub height: f32,
    /// 0..1: random spread of blade heights.
    pub height_variation: f32,
    /// Blade width at the root, meters.
    pub width: f32,
    /// How far blades lean over: 0 = straight up, 1 = about 45 degrees.
    pub bend: f32,
    /// Albedo (linear RGB) of fresh blades.
    pub lush_color: [f32; 3],
    /// Albedo (linear RGB) of dry blades.
    pub dry_color: [f32; 3],
    /// Dryness where the field has no dryness map.
    pub dryness: f32,
    /// How much the wind moves the blades (1 = normal, 0 = still).
    pub wind_response: f32,
    /// Blades thin out with distance and are gone at this distance, meters.
    pub draw_distance: f32,
    /// Blades cast sun shadows up to this distance from the camera, meters (0 = never).
    pub shadow_distance: f32,
}

impl Default for GrassStyle {
    fn default() -> Self {
        Self {
            blades_per_m2: 400.0,
            height: 0.4,
            height_variation: 0.4,
            width: 0.012,
            bend: 0.35,
            lush_color: [0.075, 0.16, 0.035],
            dry_color: [0.36, 0.30, 0.14],
            dryness: 0.15,
            wind_response: 1.0,
            draw_distance: 70.0,
            shadow_distance: 20.0,
        }
    }
}

/// Pushes grass aside and flattens it: a character's feet or body, a rolling boulder, a
/// landing ship. A sphere of `radius` meters around the entity; the 16 nearest the camera act.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct GrassBender {
    pub radius: f32,
}

impl Default for GrassBender {
    fn default() -> Self {
        Self { radius: 0.4 }
    }
}

fn scene_source() -> String {
    [
        include_str!("shaders/grass_common.wgsl"),
        include_str!("shaders/grass.wgsl"),
    ]
    .join("\n")
}

fn shadow_source() -> String {
    [
        include_str!("shaders/grass_common.wgsl"),
        include_str!("shaders/grass_shadow.wgsl"),
    ]
    .join("\n")
}

/// Fraction of blades drawn at distance `d` (must match `grass_lod` in the shader).
fn lod_factor(d: f32, full: f32, far: f32) -> f32 {
    let thin = (full / d.max(1e-3)).powf(1.5).min(1.0);
    let t = ((d - 0.7 * far) / (0.3 * far)).clamp(0.0, 1.0);
    thin * (1.0 - t * t * (3.0 - 2.0 * t))
}

/// Full density out to this distance.
fn full_density_distance(draw_distance: f32) -> f32 {
    (draw_distance * 0.2).clamp(4.0, 20.0)
}

/// Quads per blade at distance `d`.
fn segments_at(d: f32) -> u32 {
    match d {
        d if d < 8.0 => 4,
        d if d < 20.0 => 3,
        d if d < 40.0 => 2,
        _ => 1,
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuTile {
    corner: [f32; 2],
    field: u32,
    segments: u32,
    blades: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuField {
    origin: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    axis_z: [f32; 4],
    grid: [u32; 4],
    shape: [f32; 4],
    lush: [f32; 4],
    dry: [f32; 4],
    lod: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuBenders {
    count: [u32; 4],
    spheres: [[f32; 4]; MAX_BENDERS],
}

/// One field's map on the GPU and what culling needs to know about it.
struct FieldMap {
    group: wgpu::BindGroup,
    resolution: [u32; 2],
    tiles: [u32; 2],
    /// Per tile: lowest and highest ground, highest density.
    tile_bounds: Vec<[f32; 3]>,
    _texture: wgpu::Texture,
}

impl FieldMap {
    fn new(gpu: &GpuShared, layout: &wgpu::BindGroupLayout, field: &GrassField) -> Self {
        let [mut rx, mut rz] = field.resolution;
        let n = (rx as usize) * (rz as usize);
        let usable = |v: &Vec<f32>| v.len() == n && n > 0;
        if !(usable(&field.heights) || usable(&field.density) || usable(&field.dryness)) {
            rx = 2;
            rz = 2;
        }
        rx = rx.max(2);
        rz = rz.max(2);
        let n = (rx * rz) as usize;
        let get = |v: &Vec<f32>, i: usize, default: f32| {
            if v.len() == n { v[i] } else { default }
        };
        let texels: Vec<[f32; 4]> = (0..n)
            .map(|i| {
                [
                    get(&field.heights, i, 0.0),
                    get(&field.density, i, 1.0).clamp(0.0, 1.0),
                    get(&field.dryness, i, field.style.dryness).clamp(0.0, 1.0),
                    0.0,
                ]
            })
            .collect();
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grass map"),
            size: wgpu::Extent3d {
                width: rx,
                height: rz,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rx * 16),
                rows_per_image: Some(rz),
            },
            wgpu::Extent3d {
                width: rx,
                height: rz,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grass map"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });

        // Tile bounds from the grid points that fall in or next to each tile.
        let size = [field.size[0].max(0.01), field.size[1].max(0.01)];
        let tiles = [
            (size[0] / TILE).ceil().max(1.0) as u32,
            (size[1] / TILE).ceil().max(1.0) as u32,
        ];
        let point_range = |t: u32, axis: usize, res: u32| {
            let to_grid = (res - 1) as f32 / size[axis];
            let lo = ((t as f32 * TILE) * to_grid).floor().max(0.0) as u32;
            let hi = (((t + 1) as f32 * TILE) * to_grid).ceil().min((res - 1) as f32) as u32;
            lo..=hi.max(lo)
        };
        let mut tile_bounds = Vec::with_capacity((tiles[0] * tiles[1]) as usize);
        for tz in 0..tiles[1] {
            for tx in 0..tiles[0] {
                let mut b = [f32::MAX, f32::MIN, 0.0f32];
                for j in point_range(tz, 1, rz) {
                    for i in point_range(tx, 0, rx) {
                        let t = texels[(j * rx + i) as usize];
                        b[0] = b[0].min(t[0]);
                        b[1] = b[1].max(t[0]);
                        b[2] = b[2].max(t[1]);
                    }
                }
                tile_bounds.push(b);
            }
        }
        Self {
            group,
            resolution: [rx, rz],
            tiles,
            tile_bounds,
            _texture: texture,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Draw {
    /// Index into `FieldMap`s drawn this frame (`GrassFeature::order`).
    field: usize,
    segments: u32,
    first: u32,
    count: u32,
}

/// Draws every [`GrassField`]. Added by `RenderPlugin`.
#[derive(Default)]
pub struct GrassFeature {
    data_layout: Option<wgpu::BindGroupLayout>,
    map_layout: Option<wgpu::BindGroupLayout>,
    pipeline: Option<wgpu::RenderPipeline>,
    prepass_pipeline: Option<wgpu::RenderPipeline>,
    shadow_pipeline: Option<wgpu::RenderPipeline>,
    tiles_buffer: Option<GrowBuffer>,
    fields_buffer: Option<GrowBuffer>,
    benders_buffer: Option<wgpu::Buffer>,
    data_group: Option<wgpu::BindGroup>,
    maps: HashMap<Entity, FieldMap>,
    /// Fields drawn this frame, in `GpuField` order.
    order: Vec<Entity>,
    tiles: Vec<GpuTile>,
    fields: Vec<GpuField>,
    draws: Vec<Draw>,
    shadow_draws: [Vec<Draw>; MAX_CASCADES],
    stats: FeatureStats,
}

impl GrassFeature {
    fn draw_list(&self, pass: &mut wgpu::RenderPass<'_>, draws: &[Draw]) {
        let Some(group) = &self.data_group else {
            return;
        };
        pass.set_bind_group(1, group, &[]);
        let mut bound = usize::MAX;
        for d in draws {
            if d.field != bound {
                let Some(map) = self.maps.get(&self.order[d.field]) else {
                    continue;
                };
                pass.set_bind_group(2, &map.group, &[]);
                bound = d.field;
            }
            pass.draw(0..2 * d.segments + 1, d.first..d.first + d.count);
        }
    }
}

impl RenderFeature for GrassFeature {
    fn name(&self) -> &str {
        "grass"
    }

    fn order(&self) -> i32 {
        10
    }

    fn init(&mut self, gpu: &GpuShared) {
        use wgpu::ShaderStages as S;
        let device = &gpu.device;
        let storage = |binding| {
            layout_entry(
                binding,
                S::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )
        };
        let data_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass data"),
            entries: &[
                storage(0),
                storage(1),
                layout_entry(
                    2,
                    S::VERTEX,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let map_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grass map"),
            entries: &[layout_entry(
                0,
                S::VERTEX,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )],
        });
        let shader = gpu.shader("grass", &scene_source(), true);
        let shadow_shader = gpu.shader("grass shadow", &shadow_source(), false);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&data_layout),
                Some(&map_layout),
            ],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass shadow"),
            bind_group_layouts: &[
                Some(&gpu.shadow_view_layout),
                Some(&data_layout),
                Some(&map_layout),
            ],
            immediate_size: 0,
        });
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        };
        let scene_pipeline = |label, vs, fs, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive,
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.pipeline = Some(scene_pipeline(
            "grass",
            "vs_main",
            "fs_main",
            &[Some(wgpu::ColorTargetState {
                format: gpu.formats.hdr,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        ));
        self.prepass_pipeline = Some(scene_pipeline(
            "grass prepass",
            "vs_prepass",
            "fs_prepass",
            &gpu.prepass_targets(),
        ));
        self.shadow_pipeline = Some(device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("grass shadow"),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_shadow"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    unclipped_depth: gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
                    ..primitive
                },
                depth_stencil: Some(gpu.shadow_depth_state()),
                multisample: Default::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            },
        ));
        self.tiles_buffer = Some(GrowBuffer::new(
            device,
            "grass tiles",
            wgpu::BufferUsages::STORAGE,
            256 * std::mem::size_of::<GpuTile>() as u64,
        ));
        self.fields_buffer = Some(GrowBuffer::new(
            device,
            "grass fields",
            wgpu::BufferUsages::STORAGE,
            4 * std::mem::size_of::<GpuField>() as u64,
        ));
        self.benders_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grass benders"),
            size: std::mem::size_of::<GpuBenders>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.data_layout = Some(data_layout);
        self.map_layout = Some(map_layout);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
        self.order.clear();
        self.tiles.clear();
        self.fields.clear();
        self.draws.clear();
        for d in &mut self.shadow_draws {
            d.clear();
        }
        self.stats = FeatureStats::default();
        let Some(map_layout) = &self.map_layout else {
            return;
        };

        let scale = settings.grass_density.max(0.0);
        let cascades = if view.sun_shadows {
            (view.cascades.count as usize).min(MAX_CASCADES)
        } else {
            0
        };
        let mut seen = Vec::new();
        let mut q = world.query::<(Entity, Ref<GrassField>, &GlobalTransform)>();
        for (entity, field, gt) in q.iter(world) {
            seen.push(entity);
            if field.is_changed() || !self.maps.contains_key(&entity) {
                self.maps
                    .insert(entity, FieldMap::new(gpu, map_layout, &field));
            }
            let map = &self.maps[&entity];
            let style = &field.style;
            let per_m2 = (style.blades_per_m2 * scale).clamp(0.0, MAX_BLADES_PER_M2);
            let far = style.draw_distance.max(1.0);
            if per_m2 <= 0.0 || field.size[0] <= 0.0 || field.size[1] <= 0.0 {
                continue;
            }
            let full = full_density_distance(far);
            let max_blades = ((per_m2 * TILE * TILE).ceil() as u32).clamp(1, MAX_BLADES_PER_TILE);
            let rotation = gt.rotation();
            let [ax, ay, az] = [Vec3::X, Vec3::Y, Vec3::Z].map(|a| rotation * a);
            let half = DVec3::new(field.size[0] as f64 * 0.5, 0.0, field.size[1] as f64 * 0.5);
            let corner = gt.translation() - rotation.as_dquat() * half;
            let origin = view.relative(corner);
            // The camera in field-local coordinates.
            let cam = rotation.inverse() * -origin;
            let blade_top = style.height * (1.0 + style.height_variation.max(0.0));

            let field_index = self.order.len();
            let mut drew = false;
            let tx0 = ((cam.x - far) / TILE).floor().max(0.0) as u32;
            let tz0 = ((cam.z - far) / TILE).floor().max(0.0) as u32;
            let tx1 = ((cam.x + far) / TILE).floor().max(-1.0) as i64;
            let tz1 = ((cam.z + far) / TILE).floor().max(-1.0) as i64;
            let tx1 = tx1.min(map.tiles[0] as i64 - 1);
            let tz1 = tz1.min(map.tiles[1] as i64 - 1);
            for tz in tz0 as i64..=tz1 {
                for tx in tx0 as i64..=tx1 {
                    let b = map.tile_bounds[(tz * map.tiles[0] as i64 + tx) as usize];
                    if b[2] <= 0.0 {
                        continue;
                    }
                    let lo = Vec3::new(tx as f32 * TILE, b[0], tz as f32 * TILE);
                    let hi = Vec3::new(
                        ((tx + 1) as f32 * TILE).min(field.size[0]),
                        b[1] + blade_top,
                        ((tz + 1) as f32 * TILE).min(field.size[1]),
                    );
                    let d = (cam.clamp(lo, hi) - cam).length();
                    if d >= far {
                        continue;
                    }
                    let count = ((max_blades as f32) * lod_factor(d, full, far) * b[2])
                        .ceil()
                        .min(max_blades as f32) as u32;
                    if count == 0 {
                        continue;
                    }
                    let center_local = (lo + hi) * 0.5;
                    let center = origin + ax * center_local.x + ay * center_local.y + az * center_local.z;
                    let radius = ((hi - lo) * 0.5).length();
                    let visible = view.frustum.intersects_sphere(center, radius);
                    let shadows = d < style.shadow_distance;
                    let in_cascade: Vec<usize> = if shadows {
                        (0..cascades)
                            .filter(|&c| view.cascades.sphere_in_cascade(c, center, radius))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    if !visible && in_cascade.is_empty() {
                        continue;
                    }
                    let tile_index = self.tiles.len() as u32;
                    if tile_index >= 1 << (32 - BLADE_BITS) {
                        continue;
                    }
                    let segments = segments_at(d);
                    self.tiles.push(GpuTile {
                        corner: [lo.x, lo.z],
                        field: field_index as u32,
                        segments,
                        blades: max_blades,
                        _pad: [0; 3],
                    });
                    let draw = Draw {
                        field: field_index,
                        segments,
                        first: tile_index << BLADE_BITS,
                        count,
                    };
                    if visible {
                        self.draws.push(draw);
                        self.stats.instances += count;
                        self.stats.triangles += count as u64 * (2 * segments as u64 - 1);
                    }
                    for c in in_cascade {
                        self.shadow_draws[c].push(Draw {
                            segments: segments.min(2),
                            ..draw
                        });
                    }
                    drew = true;
                }
            }
            if !drew {
                continue;
            }
            let seed = (entity.to_bits() as u32) ^ ((entity.to_bits() >> 32) as u32).wrapping_mul(0x9e37_79b9);
            self.order.push(entity);
            self.fields.push(GpuField {
                origin: origin.extend(0.0).to_array(),
                axis_x: ax.extend(field.size[0]).to_array(),
                axis_y: ay.extend(field.size[1]).to_array(),
                axis_z: az.extend(0.0).to_array(),
                grid: [map.resolution[0], map.resolution[1], 0, seed],
                shape: [
                    style.height.max(0.0),
                    style.height_variation.clamp(0.0, 0.95),
                    style.width.max(0.0),
                    style.bend.clamp(0.0, 2.0),
                ],
                lush: [
                    style.lush_color[0],
                    style.lush_color[1],
                    style.lush_color[2],
                    0.0,
                ],
                dry: [
                    style.dry_color[0],
                    style.dry_color[1],
                    style.dry_color[2],
                    style.wind_response.max(0.0),
                ],
                lod: [full, far, 0.0, 0.0],
            });
        }
        self.maps.retain(|e, _| seen.contains(e));
        self.stats.draw_calls = self.draws.len() as u32;

        // Benders nearest the camera.
        let mut benders: Vec<[f32; 4]> = world
            .query::<(&GrassBender, &GlobalTransform)>()
            .iter(world)
            .map(|(b, gt)| view.relative(gt.translation()).extend(b.radius.max(0.0)).to_array())
            .filter(|s| s[3] > 0.0)
            .collect();
        benders.sort_by(|a, b| {
            let d = |s: &[f32; 4]| s[0] * s[0] + s[1] * s[1] + s[2] * s[2];
            d(a).total_cmp(&d(b))
        });
        benders.truncate(MAX_BENDERS);
        let mut gpu_benders = GpuBenders::zeroed();
        gpu_benders.count[0] = benders.len() as u32;
        gpu_benders.spheres[..benders.len()].copy_from_slice(&benders);

        // Upload.
        if self.order.is_empty() {
            return;
        }
        let device = &gpu.device;
        let queue = &gpu.queue;
        let (Some(tiles), Some(fields), Some(benders_buffer), Some(data_layout)) = (
            &mut self.tiles_buffer,
            &mut self.fields_buffer,
            &self.benders_buffer,
            &self.data_layout,
        ) else {
            return;
        };
        let grown_tiles = tiles.write(device, queue, bytemuck::cast_slice(&self.tiles));
        let grown_fields = fields.write(device, queue, bytemuck::cast_slice(&self.fields));
        queue.write_buffer(benders_buffer, 0, bytemuck::bytes_of(&gpu_benders));
        if grown_tiles || grown_fields || self.data_group.is_none() {
            self.data_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grass data"),
                layout: data_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: tiles.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: fields.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: benders_buffer.as_entire_binding(),
                    },
                ],
            }));
        }
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let (Some(pipeline), false) = (&self.prepass_pipeline, self.draws.is_empty()) {
            pass.set_pipeline(pipeline);
            self.draw_list(pass, &self.draws);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque || self.draws.is_empty() {
            return;
        }
        if let Some(pipeline) = &self.pipeline {
            pass.set_pipeline(pipeline);
            self.draw_list(pass, &self.draws);
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let Some(draws) = self.shadow_draws.get(cascade) else {
            return;
        };
        if let (Some(pipeline), false) = (&self.shadow_pipeline, draws.is_empty()) {
            pass.set_pipeline(pipeline);
            self.draw_list(pass, draws);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "fields": self.order.len(),
            "tiles": self.tiles.len(),
            "draws": self.draws.len(),
            "blades": self.stats.instances,
            "shadow_draws": self.shadow_draws.iter().map(|d| d.len()).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_shaders_compile() {
        use crate::features::{compose_shader, validate_wgsl};
        validate_wgsl("grass", &compose_shader(&scene_source(), true)).unwrap();
        validate_wgsl("grass shadow", &compose_shader(&shadow_source(), false)).unwrap();
    }

    #[test]
    fn lod_is_full_near_and_gone_at_the_draw_distance() {
        assert_eq!(lod_factor(1.0, 14.0, 70.0), 1.0);
        assert_eq!(lod_factor(14.0, 14.0, 70.0), 1.0);
        let mid = lod_factor(30.0, 14.0, 70.0);
        assert!(mid > 0.2 && mid < 0.5, "{mid}");
        assert_eq!(lod_factor(70.0, 14.0, 70.0), 0.0);
        // Never increases with distance.
        let mut prev = 1.0;
        for i in 0..200 {
            let f = lod_factor(i as f32 * 0.5, 14.0, 70.0);
            assert!(f <= prev + 1e-6);
            prev = f;
        }
    }

    #[test]
    fn sampled_maps_cover_the_field_corner_to_corner() {
        let f = GrassField::flat(10.0, 20.0).sample([3, 5], |x, z| (x, z, 0.0));
        assert_eq!(f.heights.len(), 15);
        assert_eq!((f.heights[0], f.density[0]), (-5.0, -10.0));
        assert_eq!((f.heights[14], f.density[14]), (5.0, 10.0));
        assert_eq!(f.heights[1], 0.0);
    }
}
