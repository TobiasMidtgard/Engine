//! Probe GI: dynamic diffuse global illumination (DDGI, Majercik et al. 2019) without
//! ray-tracing hardware. A grid of probes scrolls with the camera; each frame a share of them
//! shoots rays against a software hierarchy of the meshes around (`bvh.rs`): one per mesh, built
//! when it first shows up, and one over the instances near the grid, rebuilt every frame. A hit
//! is lit by the sun and the point lights through shadow rays, by its own glow and by last
//! frame's probes (so light bounces more than once over frames); a miss sees the environment
//! map. `cs_update` blends the rays into each probe's octahedral irradiance and distance maps
//! (the atlas, frame binding 23), and `probe_gi` in lib_frame.wgsl lights every surface from
//! the eight probes around it, with DDGI's visibility weights so light doesn't leak through
//! walls. See /engine-design/dynamic-gi.md in the project files for the research behind it.
//!
//! Meshes only for now ([`crate::MeshRenderer`]); terrain, foliage and water are not in the
//! rays' scene yet, and rays that miss see the environment map's ground below the horizon.

mod bvh;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::{GpuShared, GrowBuffer};
use crate::frame::{FrameUniform, ViewData};
use crate::material::{AlphaMode, StandardMaterial};
use crate::material_library::MaterialPreset;
use crate::mesh::Mesh;
use crate::settings::RenderSettings;
use engine_assets::Assets;

/// Texels along a probe's tile side (`PROBE_GI_TEXELS` in lib_frame.wgsl).
const TEXELS: u32 = 8;
/// Tiles per atlas row (`PROBE_GI_TILES_X`).
const TILES_X: u32 = 128;
/// Probes the atlas holds: 1024 × 1024 texels, two layers (16 MB).
pub(crate) const MAX_PROBES: u32 = TILES_X * TILES_X;
/// Rays per probe at most (`MAX_RAYS` in probe_gi.wgsl).
const MAX_RAYS: u32 = 256;
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Triangles whose hierarchies may be built per frame (at least one mesh is, however large).
const BUILD_TRIANGLES_PER_FRAME: usize = 150_000;
/// Frames a mesh's hierarchy stays after its last use.
const KEEP_UNUSED_FRAMES: u64 = 600;

/// `GiPass` in probe_gi.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GiPass {
    counts: [u32; 4],
    rotation: [[f32; 4]; 3],
    params: [f32; 4],
    world_origin: [i32; 4],
}

/// `Instance` in probe_gi.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuInstance {
    to_local: [[f32; 4]; 3],
    mesh: [u32; 4],
    albedo: [f32; 4],
    emissive: [f32; 4],
}

/// A mesh's hierarchy and where it sits in the shared buffers.
struct MeshEntry {
    version: u64,
    bvh: bvh::MeshBvh,
    root: u32,
    first_triangle: u32,
    last_used: u64,
}

/// The probes' placement: `dims` probes `spacing` apart from world probe `origin` (probe `i` of
/// the world lattice sits at `(i + 0.5) * spacing`).
#[derive(Clone, Copy, PartialEq)]
struct Grid {
    dims: UVec3,
    spacing: f64,
    origin: IVec3,
}

impl Grid {
    fn count(&self) -> u32 {
        self.dims.x * self.dims.y * self.dims.z
    }

    /// Storage slot of grid coordinate `g` (the storage wraps around as the grid scrolls).
    fn slot(&self, g: UVec3) -> u32 {
        let o = self.origin.rem_euclid(self.dims.as_ivec3()).as_uvec3();
        let c = (g + o) % self.dims;
        c.x + self.dims.x * (c.y + self.dims.y * c.z)
    }

    fn contains(&self, world: IVec3) -> bool {
        let g = world - self.origin;
        g.cmpge(IVec3::ZERO).all() && g.cmplt(self.dims.as_ivec3()).all()
    }
}

/// What `encode` records this frame.
struct Work {
    probes: u32,
    rays: u32,
    /// Atlas tile rows written, as (first, count) ranges.
    rows: Vec<(u32, u32)>,
}

/// GPU resources, made when probe GI first turns on.
struct Active {
    trace: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    /// What `cs_update` writes; copied into the atlas the frame samples.
    next: wgpu::Texture,
    next_view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    probe_state: wgpu::Buffer,
    tlas: GrowBuffer,
    instances: GrowBuffer,
    blas: GrowBuffer,
    triangles: GrowBuffer,
    rays: GrowBuffer,
    updates: GrowBuffer,
    group: Option<wgpu::BindGroup>,
}

/// Probe GI's state, owned by the renderer for the main view (extra views go without).
pub(crate) struct ProbeGi {
    atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    active: Option<Active>,
    meshes: HashMap<u32, MeshEntry>,
    /// The meshes' hierarchies changed: upload them again.
    blas_dirty: bool,
    grid: Option<Grid>,
    /// Bumped when every probe's history is void (a jump, a new grid): `probe_gi` ignores probes
    /// last written in another epoch.
    epoch: u32,
    /// The next slot the round robin updates.
    cursor: u32,
    frame: u64,
    work: Option<Work>,
    stats: Stats,
}

#[derive(Default, Clone, Copy)]
struct Stats {
    probes: u32,
    instances: u32,
    meshes: u32,
    built_triangles: u32,
    build_ms: f32,
    extract_ms: f32,
}

fn atlas_texture(device: &wgpu::Device, size: u32, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe gi atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage,
        view_formats: &[],
    })
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// Diffuse albedo and glow (cd/m², not pre-exposed) of a material, as the probes' rays see it.
fn material_light(m: &StandardMaterial) -> (Vec3, Vec3) {
    let tint = Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]);
    let albedo = match m.preset {
        Some(p) => tint * Vec3::from(p.mean_albedo()),
        // A texture's mean is unknown here: about half of the tint.
        None if m.base_color_texture.is_some() => tint * 0.5,
        None => tint * (1.0 - m.metallic.clamp(0.0, 1.0)),
    };
    let emissive = Vec3::from(m.emissive) * m.emissive_strength;
    (albedo.min(Vec3::splat(0.95)), emissive)
}

/// The world box of a mesh's local box under `affine`, relative to `origin`.
fn world_box(local: (Vec3, Vec3), affine: &DAffine3, origin: DVec3) -> (Vec3, Vec3) {
    let (lo, hi) = (local.0.as_dvec3(), local.1.as_dvec3());
    let center = affine.transform_point3((lo + hi) * 0.5) - origin;
    let half = (hi - lo) * 0.5;
    let m = affine.matrix3;
    let extent = DVec3::new(
        m.row(0).abs().dot(half),
        m.row(1).abs().dot(half),
        m.row(2).abs().dot(half),
    );
    ((center - extent).as_vec3(), (center + extent).as_vec3())
}

/// This frame's ray rotation (DDGI turns the ray set randomly every frame so the probes see
/// every direction over time).
fn rotation(frame: u64) -> Mat3 {
    let mut h = frame.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        (h >> 40) as f32 / (1u64 << 24) as f32
    };
    // A uniform random rotation (Shoemake 1992).
    let (u1, u2, u3) = (next(), next(), next());
    let (a, b) = ((1.0 - u1).sqrt(), u1.sqrt());
    let q = Quat::from_xyzw(
        a * (std::f32::consts::TAU * u2).sin(),
        a * (std::f32::consts::TAU * u2).cos(),
        b * (std::f32::consts::TAU * u3).sin(),
        b * (std::f32::consts::TAU * u3).cos(),
    );
    Mat3::from_quat(q.normalize())
}

impl ProbeGi {
    pub fn new(device: &wgpu::Device) -> Self {
        // A one-tile stand-in until probe GI turns on.
        let atlas = atlas_texture(
            device,
            TEXELS,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let atlas_view = array_view(&atlas);
        Self {
            atlas,
            atlas_view,
            active: None,
            meshes: HashMap::new(),
            blas_dirty: true,
            grid: None,
            epoch: 1,
            cursor: 0,
            frame: 0,
            work: None,
            stats: Stats::default(),
        }
    }

    /// The atlas frame binding 23 samples.
    pub fn atlas_view(&self) -> &wgpu::TextureView {
        &self.atlas_view
    }

    fn activate(&mut self, shared: &GpuShared) {
        let device = &shared.device;
        let size = TEXELS * TILES_X;
        self.atlas = atlas_texture(
            device,
            size,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        self.atlas_view = array_view(&self.atlas);
        let next = atlas_texture(
            device,
            size,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let next_view = array_view(&next);
        use crate::renderer::layout_entry;
        use wgpu::ShaderStages as S;
        let storage = |read_only: bool| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe gi"),
            entries: &[
                layout_entry(
                    0,
                    S::COMPUTE,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(1, S::COMPUTE, storage(true)),
                layout_entry(2, S::COMPUTE, storage(true)),
                layout_entry(3, S::COMPUTE, storage(true)),
                layout_entry(4, S::COMPUTE, storage(true)),
                layout_entry(5, S::COMPUTE, storage(false)),
                layout_entry(6, S::COMPUTE, storage(false)),
                layout_entry(
                    7,
                    S::COMPUTE,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: ATLAS_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                ),
                layout_entry(8, S::COMPUTE, storage(true)),
            ],
        });
        let module = shared.shader("probe gi", include_str!("../shaders/probe_gi.wgsl"), true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe gi"),
            bind_group_layouts: &[Some(&shared.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let usage = wgpu::BufferUsages::STORAGE;
        self.active = Some(Active {
            trace: pipeline("cs_trace"),
            update: pipeline("cs_update"),
            layout,
            next,
            next_view,
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe gi pass"),
                size: std::mem::size_of::<GiPass>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            probe_state: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe gi state"),
                size: MAX_PROBES as u64 * 16,
                usage,
                mapped_at_creation: false,
            }),
            tlas: GrowBuffer::new(device, "probe gi tlas", usage, 4096),
            instances: GrowBuffer::new(device, "probe gi instances", usage, 4096),
            blas: GrowBuffer::new(device, "probe gi blas", usage, 1 << 16),
            triangles: GrowBuffer::new(device, "probe gi triangles", usage, 1 << 16),
            rays: GrowBuffer::new(device, "probe gi rays", usage, 1 << 20),
            updates: GrowBuffer::new(device, "probe gi updates", usage, 4096),
            group: None,
        });
        self.blas_dirty = true;
        self.grid = None;
    }

    /// The main view's half on the main thread: place the grid, gather the instances near it,
    /// build what hierarchies are missing, upload, and fill the frame uniform's probe fields.
    /// Returns true when the atlas was recreated (rebuild the frame groups).
    pub fn extract(
        &mut self,
        world: &mut World,
        shared: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        uniform: &mut FrameUniform,
    ) -> bool {
        self.work = None;
        self.frame += 1;
        if !settings.probe_gi {
            if self.active.is_some() {
                // Off: free the GPU memory, keep a one-tile stand-in.
                *self = Self::new(&shared.device);
                return true;
            }
            return false;
        }
        let started = std::time::Instant::now();
        let mut recreated = false;
        if self.active.is_none() {
            self.activate(shared);
            recreated = true;
        }

        // The grid, centered on the camera and snapped to whole probes.
        let spacing = settings.gi_probe_spacing.max(0.1) as f64;
        let mut dims = UVec3::from(settings.gi_probe_grid).max(UVec3::splat(2));
        while dims.x * dims.y * dims.z > MAX_PROBES {
            dims = (dims / 2).max(UVec3::splat(2));
        }
        let center = (view.origin / spacing).floor();
        let origin = center.as_ivec3() - (dims / 2).as_ivec3();
        let grid = Grid {
            dims,
            spacing,
            origin,
        };
        let count = grid.count();
        let per_frame = settings.gi_probes_per_frame.clamp(1, count);
        let rays = settings.gi_rays_per_probe.clamp(16, MAX_RAYS);

        // Probes that scrolled in go first, then the rest in turn.
        let mut updates: Vec<u32> = Vec::with_capacity(per_frame as usize);
        match self.grid {
            Some(old) if old.dims == dims && old.spacing == spacing => {
                if old.origin != origin {
                    for z in 0..dims.z {
                        for y in 0..dims.y {
                            for x in 0..dims.x {
                                let g = UVec3::new(x, y, z);
                                if !old.contains(origin + g.as_ivec3()) {
                                    updates.push(grid.slot(g));
                                }
                            }
                        }
                    }
                    if updates.len() > per_frame as usize {
                        // A jump: every probe starts over.
                        updates.clear();
                        self.epoch += 1;
                    }
                }
            }
            _ => self.epoch += 1,
        }
        if self.epoch >= 2048 {
            self.epoch = 1;
        }
        self.grid = Some(grid);
        while updates.len() < per_frame as usize {
            updates.push(self.cursor % count);
            self.cursor = (self.cursor + 1) % count;
        }

        // Instances near the grid.
        let build_started = std::time::Instant::now();
        let mut built = 0usize;
        // Probes sit between whole multiples of the spacing: level design snaps walls and floors
        // to whole meters, and a probe on a surface sees through it along its plane.
        let rel_origin = (origin.as_dvec3() + 0.5) * spacing - view.origin;
        let margin = (spacing * 16.0) as f32;
        let region = (
            rel_origin.as_vec3() - margin,
            (rel_origin + (dims - 1).as_dvec3() * spacing).as_vec3() + margin,
        );
        let mut found: Vec<(GpuInstance, (Vec3, Vec3))> = Vec::new();
        let mut query =
            world.query::<(&crate::ResolvedMesh, &GlobalTransform, &crate::MeshRenderer)>();
        let world: &World = world;
        let meshes = world.get_resource::<Assets<Mesh>>();
        let materials = world.get_resource::<Assets<StandardMaterial>>();
        let default_material = StandardMaterial::default();
        if let Some(meshes) = meshes {
            for (resolved, transform, renderer) in query.iter(world) {
                if !renderer.visible {
                    continue;
                }
                let material = materials
                    .and_then(|m| m.get(resolved.material))
                    .unwrap_or(&default_material);
                if material.unlit || matches!(material.alpha_mode, AlphaMode::Blend) {
                    continue;
                }
                let handle = meshes.canonical(resolved.mesh);
                let key = handle.index();
                let version = meshes.version(handle);
                let entry = match self.meshes.get_mut(&key) {
                    Some(e) if e.version == version => e,
                    _ => {
                        let Some(mesh) = meshes.get(handle) else {
                            continue;
                        };
                        let triangles = mesh.indices.len() / 3;
                        if built > 0 && built + triangles > BUILD_TRIANGLES_PER_FRAME {
                            continue;
                        }
                        built += triangles;
                        let positions: Vec<[f32; 3]> =
                            mesh.vertices.iter().map(|v| v.position).collect();
                        let bvh = bvh::build_mesh(&positions, &mesh.indices);
                        self.blas_dirty = true;
                        self.meshes.insert(
                            key,
                            MeshEntry {
                                version,
                                bvh,
                                root: 0,
                                first_triangle: 0,
                                last_used: self.frame,
                            },
                        );
                        self.meshes.get_mut(&key).expect("just inserted")
                    }
                };
                entry.last_used = self.frame;
                let root = entry.bvh.nodes[0];
                if root.count == 0 && root.min[0] > root.max[0] {
                    continue;
                }
                let local = (Vec3::from(root.min), Vec3::from(root.max));
                let bounds = world_box(local, &transform.0, view.origin);
                if bounds.1.cmplt(region.0).any() || bounds.0.cmpgt(region.1).any() {
                    continue;
                }
                // Camera-relative -> mesh.
                let mut rel = transform.0;
                rel.translation -= view.origin;
                let inv = rel.inverse();
                let m = inv.matrix3.as_mat3();
                let t = inv.translation.as_vec3();
                let row = |i: usize| {
                    let r = m.row(i);
                    [r.x, r.y, r.z, t[i]]
                };
                let (albedo, emissive) = material_light(material);
                found.push((
                    GpuInstance {
                        to_local: [row(0), row(1), row(2)],
                        mesh: [0, 0, material.double_sided as u32, key],
                        albedo: albedo.extend(0.0).to_array(),
                        emissive: emissive.extend(0.0).to_array(),
                    },
                    bounds,
                ));
            }
        }
        // Forget meshes nobody used in a while.
        let frame = self.frame;
        let before = self.meshes.len();
        self.meshes
            .retain(|_, e| frame - e.last_used < KEEP_UNUSED_FRAMES);
        self.blas_dirty |= self.meshes.len() != before;
        let build_ms = build_started.elapsed().as_secs_f32() * 1000.0;

        let active = self.active.as_mut().expect("activated above");
        let device = &shared.device;
        let queue = &shared.queue;
        let mut regroup = recreated || active.group.is_none();
        if self.blas_dirty {
            let mut nodes: Vec<bvh::Node> = Vec::new();
            let mut triangles: Vec<[f32; 4]> = Vec::new();
            for e in self.meshes.values_mut() {
                e.root = nodes.len() as u32;
                e.first_triangle = (triangles.len() / 3) as u32;
                nodes.extend_from_slice(&e.bvh.nodes);
                triangles.extend_from_slice(&e.bvh.triangles);
            }
            if triangles.is_empty() {
                triangles.resize(3, [0.0; 4]);
            }
            regroup |= active
                .blas
                .write(device, queue, bytemuck::cast_slice(&nodes));
            regroup |= active
                .triangles
                .write(device, queue, bytemuck::cast_slice(&triangles));
            self.blas_dirty = false;
        }
        for (inst, _) in &mut found {
            let e = &self.meshes[&inst.mesh[3]];
            inst.mesh[0] = e.root;
            inst.mesh[1] = e.first_triangle;
        }
        let boxes: Vec<(Vec3, Vec3)> = found.iter().map(|(_, b)| *b).collect();
        let (tlas, order) = bvh::build(&boxes);
        let mut instances: Vec<GpuInstance> = order.iter().map(|&i| found[i as usize].0).collect();
        let instance_count = instances.len();
        if instances.is_empty() {
            instances.push(GpuInstance::zeroed());
        }
        regroup |= active
            .tlas
            .write(device, queue, bytemuck::cast_slice(&tlas));
        regroup |= active
            .instances
            .write(device, queue, bytemuck::cast_slice(&instances));
        regroup |= active
            .updates
            .write(device, queue, bytemuck::cast_slice(&updates));
        regroup |= active
            .rays
            .reserve(device, updates.len() as u64 * rays as u64 * 16);
        let r = rotation(self.frame);
        let pass = GiPass {
            counts: [updates.len() as u32, rays, 0, count],
            rotation: [0, 1, 2].map(|i| r.row(i).extend(0.0).to_array()),
            params: [
                settings.gi_hysteresis.clamp(0.0, 0.995),
                50.0,
                instance_count as f32,
                settings.gi_shadowed_lights as f32,
            ],
            world_origin: origin.extend(0).to_array(),
        };
        queue.write_buffer(&active.uniform, 0, bytemuck::bytes_of(&pass));
        if regroup {
            fn buffer(b: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
                b.as_entire_binding()
            }
            active.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe gi"),
                layout: &active.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer(&active.uniform),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffer(&active.tlas.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer(&active.instances.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffer(&active.blas.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: buffer(&active.triangles.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: buffer(&active.rays.buffer),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: buffer(&active.probe_state),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&active.next_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: buffer(&active.updates.buffer),
                    },
                ],
            }));
        }

        // The atlas rows the update writes, merged into ranges.
        let mut rows: Vec<u32> = updates.iter().map(|s| s / TILES_X).collect();
        rows.sort_unstable();
        rows.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for row in rows {
            match ranges.last_mut() {
                Some((first, n)) if *first + *n == row => *n += 1,
                _ => ranges.push((row, 1)),
            }
        }
        self.work = Some(Work {
            probes: updates.len() as u32,
            rays,
            rows: ranges,
        });

        let offset = origin.rem_euclid(dims.as_ivec3()).as_vec3();
        uniform.gi_volume = [
            rel_origin.x as f32,
            rel_origin.y as f32,
            rel_origin.z as f32,
            spacing as f32,
        ];
        uniform.gi_size = [dims.x as f32, dims.y as f32, dims.z as f32, 1.0];
        uniform.gi_grid = [offset.x, offset.y, offset.z, self.epoch as f32];
        self.stats = Stats {
            probes: updates.len() as u32,
            instances: instance_count as u32,
            meshes: self.meshes.len() as u32,
            built_triangles: built as u32,
            build_ms,
            extract_ms: started.elapsed().as_secs_f32() * 1000.0,
        };
        recreated
    }

    /// Trace and update this frame's probes (after the environment map, before anything shades
    /// with them). `frame_group`: the view's scene group (it binds the atlas being read).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, frame_group: &wgpu::BindGroup) {
        let (Some(work), Some(active)) = (self.work.take(), self.active.as_ref()) else {
            return;
        };
        let Some(group) = &active.group else {
            return;
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("probe gi"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.set_pipeline(&active.trace);
            pass.dispatch_workgroups((work.probes * work.rays).div_ceil(64), 1, 1);
            pass.set_pipeline(&active.update);
            pass.dispatch_workgroups(work.probes, 1, 1);
        }
        let size = TEXELS * TILES_X;
        for (first, n) in work.rows {
            let origin = wgpu::Origin3d {
                x: 0,
                y: first * TEXELS,
                z: 0,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &active.next,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: size,
                    height: n * TEXELS,
                    depth_or_array_layers: 2,
                },
            );
        }
    }

    /// `render.gi.*` diagnostics.
    pub fn publish(&self, d: &mut Diagnostics) {
        let on = self.active.is_some();
        d.set("render.gi.on", on as u8 as f64);
        if !on {
            return;
        }
        let s = self.stats;
        d.set("render.gi.probes_updated", s.probes as f64);
        d.set(
            "render.gi.probes",
            self.grid.map_or(0, |g| g.count()) as f64,
        );
        d.set("render.gi.instances", s.instances as f64);
        d.set("render.gi.meshes", s.meshes as f64);
        d.set("render.gi.built_triangles", s.built_triangles as f64);
        d.set("render.gi.build_ms", s.build_ms as f64);
        d.set("render.gi.extract_ms", s.extract_ms as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_slots_wrap_and_stay_with_their_probe_as_it_scrolls() {
        let a = Grid {
            dims: UVec3::new(4, 3, 5),
            spacing: 1.0,
            origin: IVec3::new(-7, 2, 11),
        };
        let b = Grid {
            origin: a.origin + IVec3::new(1, -1, 2),
            ..a
        };
        let mut seen = std::collections::HashSet::new();
        for z in 0..5 {
            for y in 0..3 {
                for x in 0..4 {
                    let g = UVec3::new(x, y, z);
                    assert!(seen.insert(a.slot(g)), "slots are unique");
                    // A probe both grids hold keeps its slot.
                    let world = a.origin + g.as_ivec3();
                    if b.contains(world) {
                        let gb = (world - b.origin).as_uvec3();
                        assert_eq!(a.slot(g), b.slot(gb));
                    }
                }
            }
        }
        assert_eq!(seen.len() as u32, a.count());
    }

    #[test]
    fn shader_validates() {
        let code = crate::features::compose_shader(include_str!("../shaders/probe_gi.wgsl"), true);
        crate::features::validate_wgsl("probe gi", &code).unwrap();
    }

    #[test]
    fn ray_rotations_are_rotations() {
        for frame in 0..20 {
            let r = rotation(frame);
            assert!((r.determinant() - 1.0).abs() < 1e-4);
            assert!((r * r.transpose()).abs_diff_eq(Mat3::IDENTITY, 1e-4));
        }
        assert!(!rotation(1).abs_diff_eq(rotation(2), 1e-3));
    }
}
