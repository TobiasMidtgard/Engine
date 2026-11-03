//! The aerial perspective table: the haze and the atmosphere between the camera and every
//! distance out to [`REACH`], over a coarse grid of screen columns, built once a frame by
//! `cs_aerial` (shaders/aerial.wgsl) and read by `surface_fog` in lib_frame.wgsl (frame
//! binding 24) with one lookup a pixel. After Hillaire 2020, "A Scalable and Production Ready
//! Sky and Atmosphere Rendering Technique" (section 5.4: a 32 × 32 × 32 camera volume over
//! 32 km, 0.04 ms on a GTX 1080), which it extends with the analytic height fog beyond the
//! volumetric fog's range, lit as before and shaded by the far shadow map's mountains and the
//! cloud shadows: each pixel used to march the atmosphere in 8 steps and sample those shadows
//! 8 times along its ray, the largest share of a distant pixel's cost after its lighting.
//!
//! The table is three volumes side by side in one texture (columns `0..C`, `C..2C`, `2C..3C`):
//! the height fog's light (pre-exposed) and transmittance; the atmosphere's light
//! (pre-exposed, times the sun's color) and red transmittance; its green and blue
//! transmittance. Slice `k` holds what lies between the camera and its far edge; slices are
//! spaced as the volumetric fog's (`log2(1 + d / scale)`), fine near the camera.
//! `RenderSettings::aerial_table` off (or orthographic cameras, or surfaces past the reach)
//! march per pixel as before.

use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::settings::RenderSettings;

/// Screen columns of the table; rows follow the view's aspect.
pub const COLUMNS: u32 = 96;
/// Depth slices.
pub const SLICES: u32 = 64;
/// Meters the table reaches; farther surfaces (the ground from orbit) march per pixel.
pub const REACH: f32 = 400_000.0;
/// Slices are spaced as `log2(1 + d / DEPTH_SCALE)`, meters.
const DEPTH_SCALE: f32 = 40.0;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// `aerial` in the frame uniform: reach (m, 0 = off), 1 / depth scale, 1 / log2(1 + reach /
/// depth scale), slices. Matches `aerial_slice_distance` in the shaders.
pub fn frame_params(settings: &RenderSettings, view: &ViewData) -> [f32; 4] {
    if !settings.aerial_table || view.orthographic {
        return [0.0; 4];
    }
    [
        REACH,
        1.0 / DEPTH_SCALE,
        1.0 / (1.0 + REACH / DEPTH_SCALE).log2(),
        SLICES as f32,
    ]
}

/// Distance (m) of slice boundary `s` (0 = the camera, slices = the reach) for frame
/// parameters `p` (the shaders' `aerial_slice_distance`).
#[cfg(test)]
fn slice_distance(p: [f32; 4], s: f32) -> f32 {
    ((s / (p[3] * p[2])).exp2() - 1.0) / p[1]
}

/// Rows of the table for a view `width` × `height`.
pub fn rows(width: u32, height: u32) -> u32 {
    ((COLUMNS as f32 * height as f32 / width.max(1) as f32).round() as u32).clamp(8, 2 * COLUMNS)
}

struct Table {
    size: [u32; 3],
    #[allow(dead_code)] // kept alive with its view
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
}

/// One view's aerial perspective table.
pub(crate) struct AerialTable {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    table: Option<Table>,
    /// Clear air (no light, transmittance 1): frame binding 24 in passes that build the table
    /// and while it is off.
    stand_in: wgpu::TextureView,
    active: bool,
}

impl AerialTable {
    pub fn new(shared: &GpuShared) -> Self {
        let device = &shared.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aerial perspective"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: FORMAT,
                    view_dimension: wgpu::TextureViewDimension::D3,
                },
                count: None,
            }],
        });
        let module = shared.shader(
            "aerial perspective",
            include_str!("shaders/aerial.wgsl"),
            true,
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aerial perspective"),
            bind_group_layouts: &[Some(&shared.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("aerial perspective"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_aerial"),
            compilation_options: Default::default(),
            cache: None,
        });

        // One column of each part: no light, transmittance 1.
        let stand_in = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("no aerial perspective table"),
            size: wgpu::Extent3d {
                width: 3,
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
        let (zero, one) = (0u16, 0x3c00u16);
        let texels: Vec<u8> = [
            [zero, zero, zero, one],
            [zero, zero, zero, one],
            [one, one, zero, zero],
        ]
        .iter()
        .flatten()
        .flat_map(|h| h.to_le_bytes())
        .collect();
        shared.queue.write_texture(
            stand_in.as_image_copy(),
            &texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(24),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 3,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        Self {
            pipeline,
            layout,
            table: None,
            stand_in: stand_in.create_view(&Default::default()),
            active: false,
        }
    }

    /// The view for frame binding 24 in passes that read the table.
    pub fn view(&self) -> &wgpu::TextureView {
        self.table.as_ref().map_or(&self.stand_in, |t| &t.view)
    }

    /// The clear stand-in for frame binding 24 in passes that build the table.
    pub fn stand_in(&self) -> &wgpu::TextureView {
        &self.stand_in
    }

    /// (Re)create the table for the view; `params` is the frame uniform's `aerial`. True
    /// when [`view`](Self::view) changed (rebuild the frame groups).
    pub fn prepare(&mut self, shared: &GpuShared, view: &ViewData, params: [f32; 4]) -> bool {
        self.active = params[0] > 0.0;
        if !self.active {
            return self.table.take().is_some();
        }
        let size = [3 * COLUMNS, rows(view.width, view.height), SLICES];
        if self.table.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let device = &shared.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aerial perspective table"),
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
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aerial perspective"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.table = Some(Table {
            size,
            texture,
            view,
            group,
        });
        true
    }

    /// Build the table. `frame_group` is a group 0 frame group with the stand-in at binding
    /// 24 (the prepass group); the far shadow map and the cloud shadows are drawn.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, frame_group: &wgpu::BindGroup) {
        let Some(table) = self.table.as_ref().filter(|_| self.active) else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("aerial perspective"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(1, &table.group, &[]);
        pass.dispatch_workgroups(COLUMNS.div_ceil(8), table.size[1].div_ceil(8), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::prelude::*;

    #[test]
    fn slices_run_from_the_camera_to_the_reach() {
        let mut settings = RenderSettings::default();
        let mut world = World::new();
        world.insert_resource(RenderSettings::default());
        world.insert_resource(crate::settings::Atmosphere::default());
        world.insert_resource(crate::components::AmbientLight::default());
        world.insert_resource(Time::default());
        let view = crate::frame::build_view(
            &mut world,
            1600,
            900,
            None,
            Vec2::ZERO,
            &mut crate::frame::ExposureState::default(),
            0,
        );
        let p = frame_params(&settings, &view);
        assert_eq!(slice_distance(p, 0.0), 0.0);
        assert!((slice_distance(p, SLICES as f32) / REACH - 1.0).abs() < 1e-3);
        // Fine near the camera: the first slice is a few meters deep.
        assert!(slice_distance(p, 1.0) < 10.0, "{}", slice_distance(p, 1.0));
        assert_eq!(rows(1600, 900), 54);
        settings.aerial_table = false;
        assert_eq!(frame_params(&settings, &view)[0], 0.0);
    }
}
