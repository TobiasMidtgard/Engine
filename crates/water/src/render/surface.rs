//! The GPU side of the dispersive ripples ([`crate::surface`]): the buffers and passes of
//! `surface.wgsl`, the pushes rasterized from the ripple grid's disturbances, and the ring of
//! readbacks that hands the heights back to floating things.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};
use engine_core::math::DVec2;
use engine_render::features::GpuShared;
use engine_render::texture::f32_to_f16;

use crate::ripples::RippleField;
use crate::surface::{
    DEPTH_M, MAX_DT, N, RING_S, Runner, SurfaceHeights, SurfaceLink, TENSION, VISCOSITY,
    cpu_ripples_forced, rasterize,
};

const SHADER: &str = include_str!("surface.wgsl");

// Readback slot states.
const FREE: u8 = 0;
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    grid: [f32; 4],
    damp: [f32; 4],
    shift: [i32; 4],
}

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// World x/z of the grid's corner when the heights were copied.
    origin: DVec2,
}

pub struct SurfaceGpu {
    spatial: wgpu::ComputePipeline,
    rows_fwd: wgpu::ComputePipeline,
    cols_fwd: wgpu::ComputePipeline,
    evolve: wgpu::ComputePipeline,
    rows_inv: wgpu::ComputePipeline,
    cols_inv: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    surface: wgpu::Buffer,
    pushes: wgpu::Buffer,
    medium: wgpu::Buffer,
    heights: wgpu::Buffer,
    group: wgpu::BindGroup,
    readbacks: Vec<Readback>,
    /// (height, dh/dx, dh/dz, 0) per cell, for the water shader.
    pub view: wgpu::TextureView,
    /// The ripple grid this surface follows (its corner, cell size and the world shifts seen),
    /// `None` until the first frame.
    at: Option<(DVec2, f32, u64)>,
    link: Option<Arc<SurfaceLink>>,
    /// The pushes this frame, rasterized; the rows written (to clear them again).
    push: Vec<f32>,
    pushed_rows: Option<(usize, usize)>,
    medium_version: u64,
    /// Seconds since the last push.
    quiet_s: f32,
    shift: [i32; 2],
    /// Whether the surface steps this frame.
    pub active: bool,
}

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

impl SurfaceGpu {
    pub fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water surface"),
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
                entry(4, storage(true)),
                entry(5, storage(false)),
                entry(
                    6,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        });
        let module = gpu.shader("water surface", SHADER, false);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water surface"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |name: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
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
        let params = buffer(
            "water surface params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let surface = buffer("water surface state", cells * 8, storage_dst);
        let work = buffer(
            "water surface work",
            cells * 16,
            wgpu::BufferUsages::STORAGE,
        );
        let pushes = buffer("water surface pushes", cells * 4, storage_dst);
        let medium = buffer("water surface medium", cells * 8, storage_dst);
        let heights = buffer(
            "water surface heights",
            cells * 4,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readbacks = (0..3)
            .map(|_| Readback {
                buffer: buffer(
                    "water surface readback",
                    cells * 4,
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                ),
                state: Arc::new(AtomicU8::new(FREE)),
                origin: DVec2::ZERO,
            })
            .collect();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water surface"),
            size: wgpu::Extent3d {
                width: N as u32,
                height: N as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water surface"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: surface.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: work.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pushes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: medium.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: heights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Self {
            spatial: pipeline("spatial"),
            rows_fwd: pipeline("fft_rows_fwd"),
            cols_fwd: pipeline("fft_cols_fwd"),
            evolve: pipeline("evolve"),
            rows_inv: pipeline("fft_rows_inv"),
            cols_inv: pipeline("fft_cols_inv"),
            resolve: pipeline("resolve"),
            params,
            surface,
            pushes,
            medium,
            heights,
            group,
            readbacks,
            view,
            at: None,
            link: None,
            push: vec![0.0; N * N],
            pushed_rows: None,
            medium_version: u64::MAX,
            quiet_s: f32::INFINITY,
            shift: [0; 2],
            active: false,
        }
    }

    /// Follow the ripple grid this frame: take its pushes, its currents and wet cells, hand
    /// back heights read since the last frame, and set the step. `shifts` counts the world
    /// shifts followed so far, `dt` the frame's step, `damping` the waves' (1/s).
    pub fn update(
        &mut self,
        shared: &GpuShared,
        r: &RippleField,
        shifts: u64,
        dt: f32,
        damping: f32,
    ) {
        self.active = false;
        self.collect(shared, r);
        // The transforms are written for the default grid; other sizes keep the grid's waves.
        let fits = r.n == N && r.has_water && !cpu_ripples_forced();
        if self
            .link
            .as_ref()
            .is_none_or(|l| !Arc::ptr_eq(l, &r.surface))
        {
            if let Some(old) = self.link.take() {
                old.release(Runner::Gpu);
            }
            self.at = None;
        }
        if !fits {
            if let Some(l) = self.link.take() {
                l.release(Runner::Gpu);
            }
            self.at = None;
            return;
        }
        let link = self.link.get_or_insert_with(|| r.surface.clone()).clone();
        // Picking up from the CPU, if it ran the surface until now.
        let handed = (link.runner() == Runner::Cpu)
            .then(|| link.heights())
            .flatten()
            .filter(|h| h.n == N && h.cell == r.cell && h.phi.len() == N * N);
        link.claim(Runner::Gpu);
        // Where the grid went: whole cells when it follows the focus; a world shift moves the
        // world under it instead (nothing to move).
        let mut fresh = false;
        self.shift = [0; 2];
        match self.at {
            Some((origin, cell, seen)) if cell == r.cell => {
                if seen == shifts {
                    let d = ((r.origin - origin) / cell as f64).round();
                    if d.x.abs() >= N as f64 || d.y.abs() >= N as f64 {
                        fresh = true;
                    } else {
                        self.shift = [d.x as i32, d.y as i32];
                    }
                }
            }
            _ => fresh = true,
        }
        self.at = Some((r.origin, r.cell, shifts));
        // Pushes, rasterized here where the grid is known.
        let stamps = link.take_stamps();
        if let Some((j0, j1)) = self.pushed_rows.take() {
            self.push[j0 * N..(j1 + 1) * N].fill(0.0);
        }
        for d in &stamps {
            if let Some((j0, j1)) = rasterize(&mut self.push, N, r.cell, r.origin, d) {
                self.pushed_rows = Some(
                    self.pushed_rows
                        .map_or((j0, j1), |(a, b)| (a.min(j0), b.max(j1))),
                );
            }
        }
        if let Some((j0, j1)) = self.pushed_rows {
            shared.queue.write_buffer(
                &self.pushes,
                (j0 * N * 4) as u64,
                bytemuck::cast_slice(&self.push[j0 * N..(j1 + 1) * N]),
            );
            self.quiet_s = 0.0;
        } else {
            self.quiet_s += dt;
        }
        if let Some(h) = handed {
            // Where the CPU left it, moved to this frame's grid.
            let d = ((r.origin - h.origin) / r.cell as f64).round();
            let (sx, sz, n) = (d.x as i64, d.y as i64, N as i64);
            let mut state = vec![[0.0f32; 2]; N * N];
            for j in 0..n {
                for i in 0..n {
                    let (oi, oj) = (i + sx, j + sz);
                    if oi >= 0 && oi < n && oj >= 0 && oj < n {
                        let o = (oj * n + oi) as usize;
                        state[(j * n + i) as usize] = [h.h[o], h.phi[o]];
                    }
                }
            }
            shared
                .queue
                .write_buffer(&self.surface, 0, bytemuck::cast_slice(&state));
            self.shift = [0; 2];
            self.quiet_s = 0.0;
            self.medium_version = u64::MAX;
        } else if fresh {
            // Nothing carried over: start from still water.
            shared
                .queue
                .write_buffer(&self.surface, 0, &vec![0u8; N * N * 8]);
            self.medium_version = u64::MAX;
        }
        if self.medium_version != r.version {
            self.medium_version = r.version;
            let mut medium = vec![0u16; N * N * 4];
            for j in 0..N {
                for i in 0..N {
                    let k = j * N + i;
                    let wet = r.cell_wet(k);
                    if wet <= 0.0 {
                        continue;
                    }
                    let v = r.cell_current(i, j) + r.cell_drift(k);
                    medium[k * 4] = f32_to_f16(v.x);
                    medium[k * 4 + 1] = f32_to_f16(v.y);
                    medium[k * 4 + 2] = f32_to_f16(wet);
                }
            }
            shared
                .queue
                .write_buffer(&self.medium, 0, bytemuck::cast_slice(&medium));
        }
        link.beat();
        if self.quiet_s > RING_S && self.shift == [0; 2] {
            return;
        }
        let params = Params {
            grid: [N as f32, r.cell, dt.clamp(0.0, MAX_DT), DEPTH_M],
            damp: [
                damping.max(0.0),
                VISCOSITY,
                TENSION,
                if self.pushed_rows.is_some() { 1.0 } else { 0.0 },
            ],
            shift: [self.shift[0], self.shift[1], 0, 0],
        };
        shared
            .queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        self.active = true;
    }

    /// Step the surface, and copy its heights out for floating things when a readback slot
    /// is free.
    pub fn dispatch(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.active {
            return;
        }
        let n = N as u32;
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("water surface"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &self.group, &[]);
            for (pipeline, groups) in [
                (&self.spatial, (n / 8, n / 8)),
                (&self.rows_fwd, (n, 1)),
                (&self.cols_fwd, (n, 1)),
                (&self.evolve, (n / 8, n / 8)),
                (&self.rows_inv, (n, 1)),
                (&self.cols_inv, (n, 1)),
                (&self.resolve, (n / 8, n / 8)),
            ] {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups(groups.0, groups.1, 1);
            }
        }
        let origin = self.at.map_or(DVec2::ZERO, |a| a.0);
        if let Some(rb) = self
            .readbacks
            .iter_mut()
            .find(|rb| rb.state.load(Ordering::Acquire) == FREE)
        {
            encoder.copy_buffer_to_buffer(&self.heights, 0, &rb.buffer, 0, (N * N * 4) as u64);
            rb.origin = origin;
            rb.state.store(COPIED, Ordering::Release);
        }
    }

    /// Start mapping heights copied last frame, and hand back any that have arrived.
    fn collect(&mut self, shared: &GpuShared, r: &RippleField) {
        for rb in &self.readbacks {
            if rb.state.load(Ordering::Acquire) == COPIED {
                rb.state.store(MAPPING, Ordering::Release);
                let state = rb.state.clone();
                rb.buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |res| {
                        state.store(if res.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
                    });
            }
        }
        let _ = shared.device.poll(wgpu::PollType::Poll);
        // The newest arrival wins.
        let mut newest = None;
        for rb in &self.readbacks {
            match rb.state.load(Ordering::Acquire) {
                MAPPED => {
                    if let Ok(view) = rb.buffer.slice(..).get_mapped_range() {
                        newest = Some((rb.origin, bytemuck::cast_slice::<u8, f32>(&view).to_vec()));
                    }
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                FAILED => {
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
        if let (Some((origin, h)), Some(link)) = (newest, &self.link)
            && Arc::ptr_eq(link, &r.surface)
        {
            link.publish(SurfaceHeights {
                origin,
                n: N,
                cell: r.cell,
                h,
                phi: Vec::new(),
            });
        }
    }

    /// Whether the water shader should draw this surface's heights instead of the CPU grid's.
    pub fn drawn(&self) -> bool {
        self.link
            .as_ref()
            .is_some_and(|l| l.runner() == Runner::Gpu)
    }
}

impl Drop for SurfaceGpu {
    fn drop(&mut self) {
        if let Some(l) = &self.link {
            l.release(Runner::Gpu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_validates() {
        let code = engine_render::features::compose_shader(SHADER, false);
        engine_render::features::validate_wgsl("surface", &code).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn params_match_the_shader() {
        assert_eq!(std::mem::size_of::<Params>(), 3 * 16);
    }
}
