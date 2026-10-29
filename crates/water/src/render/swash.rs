//! The swash band on the GPU (`swash.wgsl`), stepped like [`crate::swash::SwashField`]: the
//! shallow-water equations over the beaches under the ripple grid, driven by the dispersive
//! surface's waves through a strip offshore. Writes two textures the water shader reads: the
//! water drawn over the sand, and what the sand remembers.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use engine_core::math::DVec2;
use engine_render::features::GpuShared;

use crate::swash::{CELL_M, N, SwashBed, SwashField, TILE};

const SHADER: &str = include_str!("swash.wgsl");

/// Longest step the GPU takes, seconds: Courant 0.4 for water at the fastest speed allowed in
/// the band's depth (5 m/s over 1.5 m).
const MAX_STEP_S: f32 = 0.4 * CELL_M / (crate::swash::MAX_SPEED + 3.84);
/// The surface's levels, cells per side (`crate::surface::N`).
const SURFACE_N: f32 = crate::surface::N as f32;
/// A still level below this is none (`swash.wgsl`'s NONE).
const NONE: f32 = -1e4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Params {
    grid: [f32; 4],
    look: [f32; 4],
    fine: [f32; 4],
    coarse: [f32; 4],
    ripple: [f32; 4],
    shift: [i32; 4],
}

/// What the frame does before stepping.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Lay {
    Nothing,
    /// A fresh bed: the water at rest.
    Rest,
    /// The grid moved by whole cells: carry the water along.
    Shift([i32; 2]),
}

/// Where the swash lies this frame, for the water shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwashFrame {
    /// World x/z of the grid's corner.
    pub origin: DVec2,
    /// Its size, meters.
    pub size: f32,
    /// World height its levels are measured from.
    pub datum: f64,
}

pub struct SwashGpu {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 9],
    params: wgpu::Buffer,
    water: wgpu::Buffer,
    slopes: wgpu::Buffer,
    ground: wgpu::Buffer,
    sand: wgpu::Buffer,
    targets: wgpu::Buffer,
    tiles: wgpu::Buffer,
    /// The lake's long waves over the band: (dir x, z, k, amp), (ω, phase at the grid's
    /// corner, 0, 0) each.
    waves: wgpu::Buffer,
    view_a: wgpu::TextureView,
    view_b: wgpu::TextureView,
    /// The bed laid: (version, corner, cell size).
    laid: Option<(u64, DVec2, f32)>,
    frame: Option<SwashFrame>,
    tile_count: u32,
    lay: Lay,
    steps: u32,
    group: Option<wgpu::BindGroup>,
    /// `ENGINE_WATER_SWASH_DEBUG`: copies of the water, sand and targets read back now and
    /// then, and the frames since the last.
    probe: Option<(wgpu::Buffer, u32, bool)>,
}

// Pipelines by entry point.
const PREP_A: usize = 0;
const PREP_B: usize = 1;
const STAGE1: usize = 2;
const STAGE2: usize = 3;
const TARGET: usize = 4;
const AFTER: usize = 5;
const REST: usize = 6;
const SHIFT_OUT: usize = 7;
const SHIFT_IN: usize = 8;
const ENTRIES: [&str; 9] = [
    "prep_a",
    "prep_b",
    "stage1",
    "stage2",
    "sample_targets",
    "after",
    "rest",
    "shift_out",
    "shift_in",
];

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}

fn storage(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn texture() -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}

fn storage_texture() -> wgpu::BindingType {
    wgpu::BindingType::StorageTexture {
        access: wgpu::StorageTextureAccess::WriteOnly,
        format: wgpu::TextureFormat::Rgba16Float,
        view_dimension: wgpu::TextureViewDimension::D2,
    }
}

/// The tiles the GPU works on: the band's (top bit set) and the ones round it it reads.
fn tile_list(band: &[bool]) -> Vec<u32> {
    let t = N / TILE;
    let mut out = Vec::new();
    for tj in 0..t {
        for ti in 0..t {
            let idx = tj * t + ti;
            let near = (-1i32..=1).any(|dj| {
                (-1i32..=1).any(|di| {
                    let (x, z) = (ti as i32 + di, tj as i32 + dj);
                    x >= 0
                        && z >= 0
                        && (x as usize) < t
                        && (z as usize) < t
                        && band[z as usize * t + x as usize]
                })
            });
            if band[idx] {
                out.push(idx as u32 | 1 << 31);
            } else if near {
                out.push(idx as u32);
            }
        }
    }
    out
}

impl SwashGpu {
    pub fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water swash"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, storage(false)),
                entry(2, storage(false)),
                entry(3, storage(true)),
                entry(4, storage(false)),
                entry(5, storage(false)),
                entry(6, storage(true)),
                entry(7, storage(true)),
                entry(8, storage(true)),
                entry(9, texture()),
                entry(10, texture()),
                entry(11, storage_texture()),
                entry(12, storage_texture()),
                entry(13, storage(true)),
            ],
        });
        let module = gpu.shader("water swash", SHADER, false);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water swash"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = ENTRIES.map(|name| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let cells = (N * N) as u64;
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage_dst = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        // Read back by the debug probe.
        let storage_src = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let tex = |label: &str| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: N as u32,
                        height: N as u32,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let tiles_max = ((N / TILE) * (N / TILE)) as u64;
        Self {
            layout,
            pipelines,
            params: buffer(
                "water swash params",
                std::mem::size_of::<Params>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            water: buffer("water swash", cells * 32, storage_src),
            slopes: buffer(
                "water swash slopes",
                cells * 32,
                wgpu::BufferUsages::STORAGE,
            ),
            ground: buffer("water swash ground", cells * 16, storage_dst),
            sand: buffer("water swash sand", cells * 16, storage_src),
            targets: buffer("water swash targets", cells * 16, storage_src),
            tiles: buffer("water swash tiles", tiles_max * 4, storage_dst),
            waves: buffer(
                "water swash waves",
                (crate::waves::MAX_WAVES * 32) as u64,
                storage_dst,
            ),
            view_a: tex("water swash"),
            view_b: tex("water swash sand"),
            laid: None,
            frame: None,
            tile_count: 0,
            lay: Lay::Nothing,
            steps: 0,
            probe: std::env::var_os("ENGINE_WATER_SWASH_DEBUG").map(|_| {
                let probe = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("water swash probe"),
                    size: (3 * N * N * 16) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                (probe, 0, false)
            }),
            group: None,
        }
    }

    /// The water over the sand (level, depth, strip weight, foam).
    pub fn view_a(&self) -> &wgpu::TextureView {
        &self.view_a
    }

    /// What the sand remembers (sheen, damp, mark, bed).
    pub fn view_b(&self) -> &wgpu::TextureView {
        &self.view_b
    }

    /// Where the swash lies this frame, while it runs.
    pub fn frame(&self) -> Option<SwashFrame> {
        self.frame
    }

    /// Follow the ripple grid's beaches this frame (`bed`, `None` when no shore is near) and
    /// set the steps for `dt` seconds. `surface` binds the dispersive surface's levels, when
    /// it runs, `ripple` and `currents` the ripple grid's textures (corner, cell, cells).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        shared: &GpuShared,
        bed: Option<&Arc<SwashBed>>,
        surface: &super::surface::SurfaceGpu,
        ripple: (DVec2, f32, usize),
        ripple_views: (&wgpu::TextureView, &wgpu::TextureView),
        lake: Option<(DVec2, &[crate::waves::Wave])>,
        dt: f32,
        foam_life_s: f32,
    ) {
        self.frame = None;
        self.group = None;
        if let Some(bed) = bed {
            self.read_probe(shared, bed);
        }
        let Some(bed) = bed.filter(|b| b.n == N && dt > 0.0) else {
            self.laid = None;
            return;
        };
        if self
            .laid
            .is_none_or(|l| l.0 != bed.version || l.1 != bed.origin)
        {
            let mut ground = vec![[0.0f32; 4]; N * N];
            for (k, g) in ground.iter_mut().enumerate() {
                let (b, s) = (bed.bed[k], bed.still[k]);
                *g = if s.is_nan() {
                    [b, NONE, 0.0, 0.0]
                } else {
                    [b, s, SwashField::relax_weight(s - b), 0.0]
                };
            }
            shared
                .queue
                .write_buffer(&self.ground, 0, bytemuck::cast_slice(&ground));
            let tiles = tile_list(&bed.band);
            shared
                .queue
                .write_buffer(&self.tiles, 0, bytemuck::cast_slice(&tiles));
            self.tile_count = tiles.len() as u32;
            if std::env::var_os("ENGINE_WATER_SWASH_DEBUG").is_some() {
                let band = bed.band.iter().filter(|b| **b).count();
                let wet = bed.still.iter().filter(|s| !s.is_nan()).count();
                eprintln!(
                    "swash: v{} origin {:?} band tiles {band} dispatched {} still cells {wet}",
                    bed.version, bed.origin, self.tile_count
                );
            }
            self.lay = match self.laid {
                Some((_, origin, cell)) if cell == bed.cell => {
                    let step = ((bed.origin - origin) / cell as f64).round();
                    if step.x.abs() < N as f64 && step.y.abs() < N as f64 {
                        if step == DVec2::ZERO {
                            Lay::Nothing
                        } else {
                            Lay::Shift([step.x as i32, step.y as i32])
                        }
                    } else {
                        Lay::Rest
                    }
                }
                _ => Lay::Rest,
            };
            self.laid = Some((bed.version, bed.origin, bed.cell));
        }
        // Steps of at most MAX_STEP_S (a long frame is cut short: the water slows down
        // rather than blowing up).
        let dt = dt.min(1.0 / 15.0);
        self.steps = ((dt / MAX_STEP_S).ceil() as u32).clamp(1, crate::swash::MAX_STEPS);
        let step = dt / self.steps as f32;
        let rel = |corner: DVec2| (corner - bed.origin).as_vec2();
        let fine = surface.level_at(0);
        let coarse = surface.level_at(1);
        let level = |at: Option<(DVec2, f32)>| match at {
            Some((corner, cell)) => {
                let r = rel(corner);
                [r.x, r.y, cell, SURFACE_N]
            }
            None => [0.0, 0.0, 1.0, SURFACE_N],
        };
        let r = rel(ripple.0);
        // The lake's waves as the water shader draws them (`eval_waves`), their phases taken
        // to the grid's corner.
        let mut waves = [[[0.0f32; 4]; 2]; crate::waves::MAX_WAVES];
        let count = lake.map_or(0, |(center, w)| {
            let n = w.len().min(waves.len());
            for (g, w) in waves.iter_mut().zip(&w[..n]) {
                let shift = bed.origin - center;
                let phase = (w.phase as f64 - w.k as f64 * w.dir.as_dvec2().dot(shift))
                    .rem_euclid(std::f64::consts::TAU);
                *g = [
                    [w.dir.x, w.dir.y, w.k, w.amp],
                    [w.omega, phase as f32, 0.0, 0.0],
                ];
            }
            n
        });
        shared
            .queue
            .write_buffer(&self.waves, 0, bytemuck::cast_slice(&waves));
        let params = Params {
            grid: [N as f32, bed.cell, step, dt],
            look: [
                foam_life_s,
                count as f32,
                fine.is_some() as u32 as f32,
                coarse.is_some() as u32 as f32,
            ],
            fine: level(fine),
            coarse: level(coarse),
            ripple: [r.x, r.y, ripple.1, ripple.2 as f32],
            shift: match self.lay {
                Lay::Shift(s) => [s[0], s[1], 0, 0],
                _ => [0; 4],
            },
        };
        shared
            .queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        fn res(b: &wgpu::Buffer, binding: u32) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: b.as_entire_binding(),
            }
        }
        self.group = Some(shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water swash"),
            layout: &self.layout,
            entries: &[
                res(&self.params, 0),
                res(&self.water, 1),
                res(&self.slopes, 2),
                res(&self.ground, 3),
                res(&self.sand, 4),
                res(&self.targets, 5),
                res(&self.tiles, 6),
                res(surface.level_buffer(0), 7),
                res(surface.level_buffer(1), 8),
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(ripple_views.0),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(ripple_views.1),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(&self.view_a),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(&self.view_b),
                },
                res(&self.waves, 13),
            ],
        }));
        self.frame = Some(SwashFrame {
            origin: bed.origin,
            size: bed.cell * N as f32,
            datum: bed.datum,
        });
    }

    /// Print what the band holds (`ENGINE_WATER_SWASH_DEBUG`).
    fn read_probe(&mut self, shared: &GpuShared, bed: &SwashBed) {
        let Some((probe, _, pending @ true)) = &mut self.probe else {
            return;
        };
        *pending = false;
        probe.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = shared.device.poll(wgpu::PollType::wait_indefinitely());
        if let Ok(data) = probe.slice(..).get_mapped_range() {
            let all: &[[f32; 4]] = bytemuck::cast_slice(&data);
            let (water, rest) = all.split_at(N * N);
            let (sand, targets) = rest.split_at(N * N);
            let (mut up, mut up_max, mut damp, mut sheen, mut lo, mut hi) =
                (0, 0.0f32, 0.0f32, 0.0f32, f32::MAX, f32::MIN);
            let mut volume = 0.0f64;
            for k in 0..N * N {
                let (b, still) = (bed.bed[k], bed.still[k]);
                if still.is_nan() {
                    continue;
                }
                let h = water[k][0];
                volume += h as f64;
                if b > still && h > 5e-3 {
                    up += 1;
                    up_max = up_max.max(b + h - still);
                }
                damp = damp.max(sand[k][1]);
                sheen = sheen.max(sand[k][0]);
                let ti = (k % N) / TILE + (k / N) / TILE * (N / TILE);
                if bed.band[ti] && still - b > crate::swash::RELAX_FROM_M {
                    lo = lo.min(targets[k][0]);
                    hi = hi.max(targets[k][0]);
                }
            }
            // `ENGINE_WATER_SWASH_DEBUG=<dir>`: the whole grid too, as raw f32s (water, sand,
            // targets as 4 each, then bed and still), for a look with numpy.
            if let Some(dir) = std::env::var_os("ENGINE_WATER_SWASH_DEBUG").filter(|d| d != "1") {
                let mut raw: Vec<f32> = all.iter().flatten().copied().collect();
                raw.extend_from_slice(&bed.bed);
                raw.extend_from_slice(&bed.still);
                let path = std::path::Path::new(&dir).join(format!("swash-{}.bin", bed.version));
                let _ = std::fs::write(&path, bytemuck::cast_slice(&raw));
            }
            eprintln!(
                "swash probe: run-up cells {up} (highest {up_max:.3} m over still), damp {damp:.2}, \
                 sheen {sheen:.2}, strip waves {lo:.3}..{hi:.3} m, volume {:.1} m³",
                volume * (bed.cell * bed.cell) as f64
            );
        }
        probe.unmap();
    }

    /// Lay or move the water, then step it.
    pub fn dispatch(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(group) = &self.group else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water swash"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, group, &[]);
        let all = (N / 16) as u32;
        let mut run = |p: usize, groups: (u32, u32)| {
            pass.set_pipeline(&self.pipelines[p]);
            pass.dispatch_workgroups(groups.0, groups.1, 1);
        };
        match self.lay {
            Lay::Rest => run(REST, (all, all)),
            Lay::Shift(_) => {
                run(SHIFT_OUT, (all, all));
                run(SHIFT_IN, (all, all));
            }
            Lay::Nothing => {}
        }
        self.lay = Lay::Nothing;
        let tiles = (self.tile_count, 1);
        if self.tile_count == 0 {
            return;
        }
        run(TARGET, tiles);
        for _ in 0..self.steps {
            run(PREP_A, tiles);
            run(STAGE1, tiles);
            run(PREP_B, tiles);
            run(STAGE2, tiles);
        }
        run(AFTER, tiles);
        drop(pass);
        if let Some((probe, frames, pending)) = &mut self.probe {
            *frames += 1;
            if *frames >= 30 && !*pending {
                *frames = 0;
                *pending = true;
                let size = (N * N * 16) as u64;
                encoder.copy_buffer_to_buffer(&self.water, 0, probe, 0, size);
                encoder.copy_buffer_to_buffer(&self.sand, 0, probe, size, size);
                encoder.copy_buffer_to_buffer(&self.targets, 0, probe, 2 * size, size);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_validates() {
        let code = engine_render::features::compose_shader(SHADER, false);
        engine_render::features::validate_wgsl("swash", &code).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn params_match_the_shader() {
        assert_eq!(std::mem::size_of::<Params>(), 6 * 16);
    }

    #[test]
    fn the_band_brings_its_neighbours_along() {
        let t = N / TILE;
        let mut band = vec![false; t * t];
        band[5 * t + 5] = true;
        let list = tile_list(&band);
        assert_eq!(list.len(), 9);
        assert_eq!(list.iter().filter(|x| **x >> 31 == 1).count(), 1);
    }
}
