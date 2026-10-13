//! Volumetric fog: sunbeams through trees and windows, lamps glowing in mist, valley fog at dawn
//! and dusty light in caves.
//!
//! The fog lives in a froxel grid: frustum-shaped cells fitted to the view, a tile of pixels
//! across and a depth slice deep (`RenderSettings::volumetric_fog_quality`; slices are spaced
//! exponentially out to `volumetric_fog_range`), after Wronski 2014 ("Volumetric Fog: Unified
//! compute shader based solution to atmospheric scattering") and Hillaire 2015 ("Physically
//! Based and Unified Volumetric Rendering in Frostbite"). Two compute passes in
//! `volumetric_fog.wgsl` fill it every frame, after the shadow and sky occlusion maps and the
//! depth prepass:
//!
//! ```text
//! scatter (per froxel)   height fog (FogSettings, exact along the column's center ray) +
//!                        FogVolumes at a point jittered inside the froxel, lit there (from
//!                        above the ground at its pixel when just below it): by the sun
//!                        (shadow cascades, sun_transmittance, Henyey-Greenstein phase), the
//!                        sky (sky light × sky_visibility) and point lights, plus emission;
//!                        blended with last frame's value at the same
//!                        place in the world (90% history)          → rgba16float 3D, ping-pong
//! integrate (per column) front to back, energy conserving          → light scattered toward the
//!                        camera and transmittance to the far edge of every slice
//!                                                                  → frame binding 19
//! ```
//!
//! `apply_fog` (lib_frame.wgsl) reads it for every surface within the range, opaque or
//! transparent, and continues with the analytic height fog beyond it; the sky pass is fogged up
//! to the range. The lookup (`volumetric_fog_at`) blends the four columns around the pixel and
//! swaps their height fog for the pixel's own ray analytically. Values are pre-exposed; the
//! history is rescaled when the exposure changes.

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::components::{FogShape, FogVolume};
use crate::features::{GpuShared, GrowBuffer};
use crate::frame::ViewData;
use crate::renderer::layout_entry;
use crate::settings::{RenderSettings, VolumetricQuality};
use crate::targets::Targets;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Most fog volumes drawn per frame, nearest the camera first (`MAX_FOG_VOLUMES` in the shader).
pub const MAX_FOG_VOLUMES: usize = 64;
/// Weight of last frame's froxels in this frame's.
const HISTORY_WEIGHT: f32 = 0.9;
/// The grid never has more columns or rows than this; larger outputs get larger tiles.
const MAX_COLUMNS: u32 = 320;
const MAX_ROWS: u32 = 192;
/// Slice boundaries lie at `depth = k (2^(s c) - 1)` with `k = range / DEPTH_SCALE_DIVISOR`:
/// about evenly spaced near the camera (10 cm apart at the default range and quality),
/// exponentially beyond `k` (5 m apart at 200 m).
const DEPTH_SCALE_DIVISOR: f32 = 32.0;
/// Froxels per side of a scatter workgroup (`BLOCK` in the shader).
const BLOCK: u32 = 4;
/// Length of the per-frame jitter sequence.
const JITTER_SAMPLES: u64 = 16;

/// Froxel grid size (columns, rows, depth slices) for an output size and quality.
pub fn grid_size(width: u32, height: u32, quality: VolumetricQuality) -> [u32; 3] {
    let (tile, slices) = quality.tile_and_slices();
    let tile = tile
        .max(width.div_ceil(MAX_COLUMNS))
        .max(height.div_ceil(MAX_ROWS));
    [
        width.div_ceil(tile).max(1),
        height.div_ceil(tile).max(1),
        slices,
    ]
}

/// `volumetrics` in the frame uniform: range (m, 0 = off), 1 / depth scale (1/m),
/// 1 / log2(1 + range / depth scale), depth slices. Off for orthographic cameras.
pub fn frame_params(settings: &RenderSettings, view: &ViewData) -> [f32; 4] {
    let range = settings.volumetric_fog_range;
    if !settings.volumetric_fog || view.orthographic || !range.is_finite() || range <= 0.0 {
        return [0.0; 4];
    }
    let range = range.clamp(1.0, 10_000.0);
    let k = range / DEPTH_SCALE_DIVISOR;
    let slices = grid_size(view.width, view.height, settings.volumetric_fog_quality)[2];
    [
        range,
        1.0 / k,
        1.0 / (1.0 + range / k).log2(),
        slices as f32,
    ]
}

/// View depth (m) of slice boundary `s` (0 = the camera, slices = the range) for frame
/// parameters `p` (see [`frame_params`]). Matches `slice_depth` in the shader.
pub fn slice_depth(p: [f32; 4], s: f32) -> f32 {
    ((s / (p[3] * p[2])).exp2() - 1.0) / p[1]
}

/// GPU layout of `struct FogVolume` in `volumetric_fog.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuFogVolume {
    /// Rows of the camera-relative → unit shape transform (inside where every |q| < 1 for a
    /// box, |q| < 1 for an ellipsoid).
    pub to_local: [[f32; 4]; 3],
    /// Half extents in meters (the entity's scale applied), shape (0 box, 1 ellipsoid).
    pub shape: [f32; 4],
    /// Density (1/m), edge falloff (m), anisotropy, noise strength.
    pub medium: [f32; 4],
    /// Albedo RGB, noise scale (m).
    pub albedo: [f32; 4],
    /// Emitted radiance per meter RGB (cd/m² per m), unused.
    pub emission: [f32; 4],
    /// Bounding sphere: camera-relative center xyz, radius (m).
    pub bounds: [f32; 4],
}

impl GpuFogVolume {
    /// Pack `volume` placed at `transform` for a camera at `origin`. `None` when it holds no
    /// fog (no size, a degenerate scale, or neither density nor emission).
    pub fn new(volume: &FogVolume, transform: &GlobalTransform, origin: DVec3) -> Option<Self> {
        let half = Vec3::from(volume.half_extents);
        let m = transform.affine().matrix3.as_mat3();
        let density = volume.density.max(0.0);
        let emission = Vec3::from(volume.emission).max(Vec3::ZERO);
        // NaN sizes and scales fail these too.
        let sized = half.min_element() > 0.0 && m.determinant().abs() > 1e-12;
        if !sized || (density <= 0.0 && emission == Vec3::ZERO) {
            return None;
        }
        let center = (transform.translation() - origin).as_vec3();
        let inv = m.inverse();
        let row = |i: usize| {
            let r = inv.row(i) / half[i];
            [r.x, r.y, r.z, -r.dot(center)]
        };
        let world_half = Vec3::new(m.x_axis.length(), m.y_axis.length(), m.z_axis.length()) * half;
        let albedo = Vec3::from(volume.albedo).clamp(Vec3::ZERO, Vec3::ONE);
        Some(Self {
            to_local: [row(0), row(1), row(2)],
            shape: [
                world_half.x,
                world_half.y,
                world_half.z,
                match volume.shape {
                    FogShape::Box => 0.0,
                    FogShape::Ellipsoid => 1.0,
                },
            ],
            medium: [
                density,
                volume.edge_falloff.max(0.0),
                volume.anisotropy.clamp(-0.95, 0.95),
                volume.noise.clamp(0.0, 1.0),
            ],
            albedo: [albedo.x, albedo.y, albedo.z, volume.noise_scale.max(0.05)],
            emission: [emission.x, emission.y, emission.z, 0.0],
            bounds: [center.x, center.y, center.z, world_half.length()],
        })
    }

    /// Unit shape coordinates of camera-relative point `p` (|q| < 1 inside).
    pub fn local(&self, p: Vec3) -> Vec3 {
        let p = p.extend(1.0);
        Vec3::new(
            Vec4::from(self.to_local[0]).dot(p),
            Vec4::from(self.to_local[1]).dot(p),
            Vec4::from(self.to_local[2]).dot(p),
        )
    }
}

/// GPU layout of `struct Froxels` in `volumetric_fog.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct FroxelUniform {
    /// Columns, rows, slices, fog volume count.
    grid: [u32; 4],
    /// Sample position inside the froxel (0..1) xyz, history weight (0 = no history).
    jitter: [f32; 4],
    /// History scale (this frame's pre-exposure / the history's), unused.
    params: [f32; 4],
}

struct Volume {
    #[allow(dead_code)] // kept alive with its view
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// The volumes for one grid size.
struct Grid {
    size: [u32; 3],
    /// Scattered light and extinction, written alternately (the other one is the history).
    scatter: [Volume; 2],
    /// Frame binding 19.
    integrated: Volume,
    /// Indexed by the scatter volume written this frame.
    scatter_groups: [wgpu::BindGroup; 2],
    integrate_groups: [wgpu::BindGroup; 2],
}

/// GPU resources and temporal state of the volumetric fog.
pub(crate) struct Volumetrics {
    scatter_pipeline: wgpu::ComputePipeline,
    integrate_pipeline: wgpu::ComputePipeline,
    scatter_layout: wgpu::BindGroupLayout,
    integrate_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    volumes: GrowBuffer,
    /// Clear air (no light, transmittance 1): frame binding 19 in passes that write the volume
    /// and while volumetric fog is off.
    stand_in: wgpu::TextureView,
    /// This frame's depth prepass, depth and normals: the scatter pass lifts samples out of the
    /// surfaces (`fog_sample_point`).
    prepass: [wgpu::TextureView; 2],
    grid: Option<Grid>,
    query: Option<QueryState<(&'static FogVolume, &'static GlobalTransform)>>,
    nearest: Vec<(f32, GpuFogVolume)>,
    packed: Vec<GpuFogVolume>,
    /// Scatter volume written this frame.
    current: usize,
    /// Whether the other scatter volume holds last frame's froxels for these frame parameters.
    history_ready: bool,
    history_exposure: f32,
    history_params: [f32; 4],
    /// Whether the passes run this frame.
    active: bool,
}

impl Volumetrics {
    pub fn new(shared: &GpuShared, targets: &Targets) -> Self {
        use wgpu::ShaderStages as S;
        let device = &shared.device;
        let uniform = layout_entry(
            0,
            S::COMPUTE,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        );
        let volume_texture = |binding| {
            layout_entry(
                binding,
                S::COMPUTE,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
            )
        };
        let volume_output = |binding| {
            layout_entry(
                binding,
                S::COMPUTE,
                wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: FORMAT,
                    view_dimension: wgpu::TextureViewDimension::D3,
                },
            )
        };
        let scatter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volumetric fog scatter"),
            entries: &[
                uniform,
                layout_entry(
                    1,
                    S::COMPUTE,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                volume_texture(2),
                volume_output(3),
                layout_entry(
                    6,
                    S::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    7,
                    S::COMPUTE,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
            ],
        });
        let integrate_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volumetric fog integrate"),
            entries: &[uniform, volume_texture(4), volume_output(5)],
        });
        let module = shared.shader(
            "volumetric fog",
            include_str!("shaders/volumetric_fog.wgsl"),
            true,
        );
        let pipeline = |label: &str, entry: &str, layout: &wgpu::BindGroupLayout| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&shared.frame_layout), Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let scatter_pipeline = pipeline("volumetric fog scatter", "cs_scatter", &scatter_layout);
        let integrate_pipeline = pipeline(
            "volumetric fog integrate",
            "cs_integrate",
            &integrate_layout,
        );
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volumetric fog params"),
            size: std::mem::size_of::<FroxelUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let volumes = GrowBuffer::new(
            device,
            "fog volumes",
            wgpu::BufferUsages::STORAGE,
            8 * std::mem::size_of::<GpuFogVolume>() as u64,
        );

        // Clear air: no in-scattered light, transmittance 1 (half floats).
        let stand_in = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("no volumetric fog"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let one = 0x3c00u16.to_le_bytes();
        shared.queue.write_texture(
            stand_in.as_image_copy(),
            &[0, 0, 0, 0, 0, 0, one[0], one[1]],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let stand_in = stand_in.create_view(&Default::default());

        Self {
            scatter_pipeline,
            integrate_pipeline,
            scatter_layout,
            integrate_layout,
            params,
            volumes,
            stand_in,
            prepass: [
                targets.prepass_depth.view.clone(),
                targets.prepass_normal.view.clone(),
            ],
            grid: None,
            query: None,
            nearest: Vec::new(),
            packed: Vec::new(),
            current: 0,
            history_ready: false,
            history_exposure: 0.0,
            history_params: [0.0; 4],
            active: false,
        }
    }

    /// The view for frame binding 19 in passes that read the fog: the integrated volume, or
    /// the clear stand-in while volumetric fog is off.
    pub fn view(&self) -> &wgpu::TextureView {
        self.grid
            .as_ref()
            .map_or(&self.stand_in, |g| &g.integrated.view)
    }

    /// The clear stand-in for frame binding 19 in passes that write the volume.
    pub fn stand_in(&self) -> &wgpu::TextureView {
        &self.stand_in
    }

    /// Bind the depth prepass of new render targets (after a resize).
    pub fn resize(&mut self, shared: &GpuShared, targets: &Targets) {
        self.prepass = [
            targets.prepass_depth.view.clone(),
            targets.prepass_normal.view.clone(),
        ];
        if let Some(mut grid) = self.grid.take() {
            grid.scatter_groups = self.scatter_groups(shared, &grid.scatter);
            self.grid = Some(grid);
        }
    }

    /// Gather this frame's fog volumes, (re)create the grid for the view and upload the
    /// parameters. `params` is the `volumetrics` field of this frame's uniform. Returns true
    /// when [`view`](Self::view) changed (rebuild the frame groups).
    pub fn prepare(
        &mut self,
        world: &mut World,
        shared: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
        params: [f32; 4],
    ) -> bool {
        self.active = params[0] > 0.0;
        if !self.active {
            // Free the volumes and bind clear air.
            self.history_ready = false;
            return self.grid.take().is_some();
        }
        let mut regroup = false;
        let size = grid_size(view.width, view.height, settings.volumetric_fog_quality);
        if self.grid.as_ref().is_none_or(|g| g.size != size) {
            self.grid = Some(self.create_grid(shared, size));
            self.current = 0;
            self.history_ready = false;
            regroup = true;
        }
        if params != self.history_params {
            self.history_ready = false;
            self.history_params = params;
        }

        // Fog volumes within the range and the view, nearest first.
        let mut query = self
            .query
            .take()
            .unwrap_or_else(|| world.query::<(&FogVolume, &GlobalTransform)>());
        self.nearest.clear();
        let range = params[0];
        for (volume, transform) in query.iter(world) {
            let Some(gpu) = GpuFogVolume::new(volume, transform, view.origin) else {
                continue;
            };
            let [x, y, z, radius] = gpu.bounds;
            let center = Vec3::new(x, y, z);
            // The grid ends at view depth `range`.
            if view.view_depth(center) - radius > range
                || !view.frustum.intersects_sphere(center, radius)
            {
                continue;
            }
            self.nearest
                .push(((center.length() - radius).max(0.0), gpu));
        }
        self.query = Some(query);
        self.nearest.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.nearest.truncate(MAX_FOG_VOLUMES);
        self.packed.clear();
        self.packed.extend(self.nearest.iter().map(|(_, v)| *v));
        if self.packed.is_empty() {
            self.packed.push(GpuFogVolume::default());
        }
        let count = self.nearest.len() as u32;
        if self.volumes.write(
            &shared.device,
            &shared.queue,
            bytemuck::cast_slice(&self.packed),
        ) && let Some(mut grid) = self.grid.take()
        {
            // The storage buffer was recreated: rebind it.
            grid.scatter_groups = self.scatter_groups(shared, &grid.scatter);
            self.grid = Some(grid);
        }

        let history = self.history_ready && view.history_valid && self.history_exposure > 0.0;
        let i = view.frame_index % JITTER_SAMPLES + 1;
        let u = FroxelUniform {
            grid: [size[0], size[1], size[2], count],
            jitter: [
                crate::effects::halton(i, 2),
                crate::effects::halton(i, 3),
                crate::effects::halton(i, 5),
                if history { HISTORY_WEIGHT } else { 0.0 },
            ],
            params: [
                if history {
                    view.exposure / self.history_exposure
                } else {
                    1.0
                },
                0.0,
                0.0,
                0.0,
            ],
        };
        shared
            .queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&u));
        self.history_exposure = view.exposure;
        regroup
    }

    /// Record the scatter and integration passes. `frame_group` is a group 0 frame group with
    /// the stand-in at binding 19 (the prepass group).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, frame_group: &wgpu::BindGroup) {
        let Some(grid) = self.grid.as_ref().filter(|_| self.active) else {
            return;
        };
        let [w, h, d] = grid.size;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("volumetric fog"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_pipeline(&self.scatter_pipeline);
        pass.set_bind_group(1, &grid.scatter_groups[self.current], &[]);
        pass.dispatch_workgroups(w.div_ceil(BLOCK), h.div_ceil(BLOCK), d.div_ceil(BLOCK));
        pass.set_pipeline(&self.integrate_pipeline);
        pass.set_bind_group(1, &grid.integrate_groups[self.current], &[]);
        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        self.current = 1 - self.current;
        self.history_ready = true;
    }

    fn create_grid(&self, shared: &GpuShared, size: [u32; 3]) -> Grid {
        let device = &shared.device;
        let volume = |label: &str| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: size[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            Volume { texture, view }
        };
        let scatter = [
            volume("volumetric fog scatter 0"),
            volume("volumetric fog scatter 1"),
        ];
        let integrated = volume("volumetric fog");
        let scatter_groups = self.scatter_groups(shared, &scatter);
        let integrate_groups = [0, 1].map(|current| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("volumetric fog integrate"),
                layout: &self.integrate_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&scatter[current].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&integrated.view),
                    },
                ],
            })
        });
        Grid {
            size,
            scatter,
            integrated,
            scatter_groups,
            integrate_groups,
        }
    }

    /// Scatter pass groups, indexed by the volume written (the other one is the history).
    fn scatter_groups(&self, shared: &GpuShared, scatter: &[Volume; 2]) -> [wgpu::BindGroup; 2] {
        [0, 1].map(|current| {
            shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("volumetric fog scatter"),
                layout: &self.scatter_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.volumes.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&scatter[1 - current].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&scatter[current].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&self.prepass[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&self.prepass[1]),
                    },
                ],
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(width: u32, height: u32) -> ViewData {
        let mut world = World::new();
        world.insert_resource(RenderSettings::default());
        world.insert_resource(crate::settings::Atmosphere::default());
        world.insert_resource(crate::components::AmbientLight::default());
        world.insert_resource(Time::default());
        crate::frame::build_view(
            &mut world,
            width,
            height,
            None,
            Vec2::ZERO,
            &mut crate::frame::ExposureState::default(),
            0,
        )
    }

    #[test]
    fn grid_follows_the_output_and_stays_bounded() {
        assert_eq!(
            grid_size(1280, 720, VolumetricQuality::High),
            [160, 90, 128]
        );
        assert_eq!(grid_size(1280, 720, VolumetricQuality::Low), [80, 45, 64]);
        assert_eq!(grid_size(321, 181, VolumetricQuality::Medium), [41, 23, 64]);
        // A 4K output gets larger tiles instead of a huge grid.
        let [w, h, _] = grid_size(3840, 2160, VolumetricQuality::High);
        assert!(w <= MAX_COLUMNS && h <= MAX_ROWS, "{w}x{h}");
        assert_eq!(grid_size(0, 0, VolumetricQuality::High), [1, 1, 128]);
    }

    #[test]
    fn slices_start_at_the_camera_and_end_at_the_range() {
        let settings = RenderSettings::default();
        let p = frame_params(&settings, &view(1280, 720));
        assert_eq!(p[0], settings.volumetric_fog_range);
        let slices = p[3];
        assert_eq!(slice_depth(p, 0.0), 0.0);
        assert!((slice_depth(p, slices) - p[0]).abs() < 1e-2 * p[0]);
        let first = slice_depth(p, 1.0);
        let last = slice_depth(p, slices) - slice_depth(p, slices - 1.0);
        assert!(first > 0.02 && first < 0.3, "first slice {first} m deep");
        assert!(last > 2.0 && last < 12.0, "last slice {last} m deep");
        let mut previous = 0.0;
        for s in 1..=slices as u32 {
            let d = slice_depth(p, s as f32);
            assert!(d > previous, "slices out of order at {s}");
            previous = d;
        }
        // The shader's inverse: depth -> slice boundary.
        let d = 37.0f32;
        let s = slices * (1.0 + d * p[1]).log2() * p[2];
        assert!((slice_depth(p, s) - d).abs() < 1e-3);
    }

    #[test]
    fn off_when_disabled_or_orthographic() {
        let mut settings = RenderSettings {
            volumetric_fog: false,
            ..Default::default()
        };
        assert_eq!(frame_params(&settings, &view(64, 64))[0], 0.0);
        settings.volumetric_fog = true;
        let mut v = view(64, 64);
        v.orthographic = true;
        assert_eq!(frame_params(&settings, &v)[0], 0.0);
    }

    #[test]
    fn fog_volumes_follow_their_transform() {
        let volume = FogVolume::new([2.0, 1.0, 4.0]);
        let transform = GlobalTransform::from(
            Transform::from_xyz(110.0, 5.0, -20.0)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::splat(2.0)),
        );
        let origin = DVec3::new(100.0, 0.0, 0.0);
        let gpu = GpuFogVolume::new(&volume, &transform, origin).unwrap();
        // The local +x corner (2 m, scaled to 4 m) turned 90 degrees about y lands at -z.
        let center = Vec3::new(10.0, 5.0, -20.0);
        let q = gpu.local(center + Vec3::new(0.0, 0.0, -4.0));
        assert!((q - Vec3::X).length() < 1e-4, "{q}");
        let q = gpu.local(center + Vec3::new(8.0 * 0.99, 0.0, 0.0));
        assert!((q.z - 0.99).abs() < 1e-4, "{q}");
        assert!(gpu.local(center).length() < 1e-4);
        let half = Vec3::new(gpu.shape[0], gpu.shape[1], gpu.shape[2]);
        assert!((half - Vec3::new(4.0, 2.0, 8.0)).length() < 1e-4, "{half}");
        assert_eq!(&gpu.bounds[..3], &center.to_array());
        // Nothing to draw.
        assert!(GpuFogVolume::new(&volume.with_density(0.0), &transform, origin).is_none());
        assert!(
            GpuFogVolume::new(
                &volume.with_density(0.0).with_emission([1.0, 0.5, 0.0]),
                &transform,
                origin
            )
            .is_some()
        );
        assert!(GpuFogVolume::new(&FogVolume::new([0.0, 1.0, 1.0]), &transform, origin).is_none());
    }

    #[test]
    fn fog_volume_round_trips_through_json() {
        let v = FogVolume::ellipsoid([3.0, 1.0, 2.0])
            .with_density(0.2)
            .with_noise(0.5, 3.0)
            .with_emission([2.0, 1.0, 0.5]);
        let json = serde_json::to_value(v).unwrap();
        let back: FogVolume = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(v, back, "{json}");
        let partial: FogVolume = serde_json::from_str(r#"{"density": 0.3}"#).unwrap();
        assert_eq!(partial.density, 0.3);
        assert_eq!(partial.half_extents, FogVolume::default().half_extents);
    }
}
