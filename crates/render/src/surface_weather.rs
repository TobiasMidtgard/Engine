//! The ground's weather around the camera for shaders: [`SurfaceLayers`] (wetness, standing
//! water, snow, scorch on 1 m cells) uploaded as a texture that follows the grid, read with
//! `surface_weather(rel)` in `lib_frame.wgsl` (frame binding 21). So every pixel shows the
//! ground's own state: dry under a roof, charred where a fire burned, snowy in the open.
//!
//! The texture is `Rgba8Unorm` in the weather module's own encoding: wetness, standing water /
//! [`SURFACE_PUDDLE_SCALE_M`], snow / [`SURFACE_SNOW_SCALE_M`], scorch. It is uploaded only when
//! the layers change (change detection, at most every [`MIN_UPLOAD_INTERVAL_S`] while they keep
//! changing) or the grid moves, and drawn where the uploaded grid lay, so a grid that is waiting
//! for its next upload still lines up with the world.

use bevy_ecs::change_detection::{DetectChanges, Tick};
use engine_core::math::DVec2;
use engine_core::prelude::*;
use engine_world::{Grid2, SurfaceCell, SurfaceLayers};

use crate::features::GpuShared;

/// Standing water depth stored as 1.0 in the map, meters (the weather's deepest puddle by
/// default). `SURFACE_PUDDLE_SCALE_M` in `lib_frame.wgsl`.
pub const SURFACE_PUDDLE_SCALE_M: f32 = 0.04;
/// Snow depth stored as 1.0 in the map, meters. `SURFACE_SNOW_SCALE_M` in `lib_frame.wgsl`.
pub const SURFACE_SNOW_SCALE_M: f32 = 0.3;
/// Shortest time between uploads while the layers keep changing, seconds (a moved grid
/// uploads at once).
pub const MIN_UPLOAD_INTERVAL_S: f32 = 0.1;

/// The texture behind `surface_weather` and what the frame uniform needs to place it.
pub(crate) struct SurfaceWeatherMap {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    size: (u32, u32),
    /// Change tick of the layers last uploaded (`None`: nothing uploaded yet).
    uploaded: Option<Tick>,
    /// World X/Z of the uploaded grid's minimum corner, and its cell size (m).
    origin: DVec2,
    cell: f32,
    /// The texture holds a grid (otherwise the open-ground value applies everywhere).
    has_grid: bool,
    /// Open ground in the map's encoding (see [`open_ground`]).
    open: [f32; 4],
    since_upload: f32,
    bytes: Vec<u8>,
    /// Uploads so far (`render.surface_weather_uploads` in `Diagnostics`).
    pub uploads: u64,
}

fn create_texture(shared: &GpuShared, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface weather"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// One cell in the map's encoding (`Rgba8Unorm`).
pub fn encode_cell(c: &SurfaceCell) -> [u8; 4] {
    [
        unorm(c.wetness),
        unorm(c.puddle_m / SURFACE_PUDDLE_SCALE_M),
        unorm(c.snow_m / SURFACE_SNOW_SCALE_M),
        unorm(c.scorch),
    ]
}

/// The open ground's wetness and snow in the map's encoding (wetness, 0, snow / 0.3 m, 0):
/// the upper quartile of the grid's outer four cells, so roofs, fires and a strip the grid has
/// just uncovered (empty until the weather fills it) don't count. The terrain uses the same
/// value beyond the grid. `surface_weather` returns it beyond the grid and gives it to roofs
/// over sheltered ground.
pub fn open_ground(g: &Grid2<SurfaceCell>) -> [f32; 4] {
    let ring = 4.min(g.width / 2).min(g.height / 2);
    let (mut wet, mut snow) = (Vec::new(), Vec::new());
    for y in 0..g.height {
        let row = &g.data[(y * g.width) as usize..((y + 1) * g.width) as usize];
        let edge_row = y < ring || y >= g.height - ring;
        let (left, rest) = row.split_at(if edge_row { row.len() } else { ring as usize });
        let right = &rest[rest.len() - if edge_row { 0 } else { ring as usize }..];
        for c in left.iter().chain(right) {
            wet.push(c.wetness);
            snow.push(c.snow_m);
        }
    }
    let upper_quartile = |v: &mut Vec<f32>| {
        if v.is_empty() {
            return 0.0;
        }
        let i = v.len() * 3 / 4;
        *v.select_nth_unstable_by(i, f32::total_cmp).1
    };
    [
        upper_quartile(&mut wet).clamp(0.0, 1.0),
        0.0,
        (upper_quartile(&mut snow) / SURFACE_SNOW_SCALE_M).clamp(0.0, 1.0),
        0.0,
    ]
}

impl SurfaceWeatherMap {
    /// A 1x1 dry stand-in until the layers are uploaded.
    pub fn new(shared: &GpuShared) -> Self {
        let (texture, view) = create_texture(shared, 1, 1);
        Self {
            texture,
            view,
            size: (1, 1),
            uploaded: None,
            origin: DVec2::ZERO,
            cell: 1.0,
            has_grid: false,
            open: [0.0; 4],
            since_upload: 0.0,
            bytes: Vec::new(),
            uploads: 0,
        }
    }

    /// Upload the world's [`SurfaceLayers`] if they changed or moved since the last upload.
    /// Returns true when the texture was recreated (the frame bind groups must be rebuilt).
    pub fn prepare(&mut self, world: &World, shared: &GpuShared, real_dt: f32) -> bool {
        self.since_upload += real_dt.max(0.0);
        let Some(layers) = world.get_resource_ref::<SurfaceLayers>() else {
            self.has_grid = false;
            self.open = [0.0; 4];
            self.uploaded = None;
            return false;
        };
        let changed = layers.last_changed();
        let g = &layers.grid;
        if self.uploaded == Some(changed) || g.width == 0 || g.height == 0 {
            return false;
        }
        let moved = self.uploaded.is_none()
            || g.origin != self.origin
            || g.cell != self.cell
            || (g.width, g.height) != self.size;
        if !moved && self.since_upload < MIN_UPLOAD_INTERVAL_S {
            return false;
        }
        let resized = (g.width, g.height) != self.size;
        if resized {
            (self.texture, self.view) = create_texture(shared, g.width, g.height);
            self.size = (g.width, g.height);
        }
        self.bytes.clear();
        self.bytes.reserve(g.data.len() * 4);
        for c in &g.data {
            self.bytes.extend_from_slice(&encode_cell(c));
        }
        shared.queue.write_texture(
            self.texture.as_image_copy(),
            &self.bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(g.width * 4),
                rows_per_image: Some(g.height),
            },
            wgpu::Extent3d {
                width: g.width,
                height: g.height,
                depth_or_array_layers: 1,
            },
        );
        self.open = open_ground(g);
        self.origin = g.origin;
        self.cell = g.cell;
        self.has_grid = true;
        self.uploaded = Some(changed);
        self.since_upload = 0.0;
        self.uploads += 1;
        resized
    }

    /// `frame.surface_map` (the grid's minimum corner relative to the camera at `origin`, x and
    /// z, then its size along x and z in meters; size 0 = no grid) and `frame.surface_open`.
    pub fn frame_params(&self, origin: DVec3) -> ([f32; 4], [f32; 4]) {
        if !self.has_grid {
            return ([0.0; 4], self.open);
        }
        let corner = self.origin - DVec2::new(origin.x, origin.z);
        (
            [
                corner.x as f32,
                corner.y as f32,
                self.size.0 as f32 * self.cell,
                self.size.1 as f32 * self.cell,
            ],
            self.open,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_encoded_like_the_weather_module_does() {
        let c = SurfaceCell {
            wetness: 0.5,
            puddle_m: 0.02,
            snow_m: 0.6,
            scorch: 1.0,
        };
        assert_eq!(encode_cell(&c), [128, 128, 255, 255]);
        assert_eq!(encode_cell(&SurfaceCell::default()), [0, 0, 0, 0]);
    }

    #[test]
    fn open_ground_ignores_sheltered_and_burnt_patches() {
        let snowy = SurfaceCell {
            wetness: 0.8,
            snow_m: 0.15,
            ..Default::default()
        };
        let mut g = Grid2::new(DVec2::ZERO, 1.0, 64, 48, snowy);
        // A roof's dry footprint on the rim and a strip the grid has just uncovered.
        for x in 0..10 {
            for y in 0..4 {
                g.set(engine_core::math::IVec2::new(x, y), SurfaceCell::default());
            }
        }
        for y in 0..48 {
            g.set(engine_core::math::IVec2::new(63, y), SurfaceCell::default());
        }
        let open = open_ground(&g);
        assert_eq!(open[0], 0.8);
        assert!((open[2] - 0.5).abs() < 1e-6, "snow {}", open[2]);
    }
}
