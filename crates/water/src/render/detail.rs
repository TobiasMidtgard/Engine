//! The GPU side of the fine wind waves ([`crate::detail`]): the spectrum amplitudes, the FFT
//! passes that evolve them each frame (`detail.wgsl`) and the mipped texture the water shader
//! samples.

use bytemuck::{Pod, Zeroable};
use engine_core::math::Vec2;
use engine_render::features::GpuShared;

use crate::detail::{self, CASCADES, DetailSea, N, PERIOD_S, Spectrum};

const SHADER: &str = include_str!("detail.wgsl");
const MIPS_SHADER: &str = include_str!("detail_mips.wgsl");
/// Mip levels of the tiles: 128 texels down to 1.
pub const MIPS: u32 = 8;
/// Texture layers: per tile, slopes and moments.
const LAYERS: u32 = CASCADES as u32 * 2;
/// How long whitecap foam lingers on the detail waves, seconds.
const FOAM_LIFE_S: f32 = 2.5;
/// The surface foams where the detail chop squeezes it under this Jacobian.
const FOAM_JACOBIAN: f32 = 0.55;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    sizes: [f32; 4],
    chop: [f32; 4],
}

/// What the water shader needs to sample the tiles, and the sea they were built for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetailState {
    /// The sea the amplitudes hold.
    pub sea: DetailSea,
    pub spectrum: Spectrum,
    /// Tile sizes, meters.
    pub sizes: [f32; CASCADES],
    /// Mean square slope the tiles hold at full resolution.
    pub slope_variance: f32,
}

impl DetailState {
    /// How much of the tiles body waves of `spectrum` show (amplitude scale): the tail of a
    /// JONSWAP spectrum falls as its level times ω⁻⁵ whatever the peak, so the levels' ratio.
    pub fn scale_for(&self, spectrum: &Spectrum) -> f32 {
        (spectrum.level / self.spectrum.level.max(1e-12)).sqrt()
    }
}

pub struct DetailGpu {
    evolve: wgpu::ComputePipeline,
    rows: wgpu::ComputePipeline,
    cols: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    down: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    spectrum: wgpu::Buffer,
    group: wgpu::BindGroup,
    down_groups: Vec<wgpu::BindGroup>,
    /// All layers and mips, for the water shader.
    pub view: wgpu::TextureView,
    /// What the tiles hold now; `None` until a sea with wind waves is seen.
    pub state: Option<DetailState>,
    /// Whether anything drew from the tiles this frame (otherwise they are not evolved).
    pub active: bool,
}

/// A layout entry for a compute-stage binding.
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

fn storage_texture() -> wgpu::BindingType {
    wgpu::BindingType::StorageTexture {
        access: wgpu::StorageTextureAccess::WriteOnly,
        format: wgpu::TextureFormat::Rgba16Float,
        view_dimension: wgpu::TextureViewDimension::D2Array,
    }
}

impl DetailGpu {
    pub fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water detail"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, storage(true)),
                entry(2, storage(false)),
                entry(3, storage(false)),
                entry(4, storage_texture()),
            ],
        });
        let down_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water detail mips"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                entry(1, storage_texture()),
            ],
        });
        let pipeline = |module: &wgpu::ShaderModule, layout: &wgpu::BindGroupLayout, name: &str| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let module = gpu.shader("water detail", SHADER, false);
        let mips_module = gpu.shader("water detail mips", MIPS_SHADER, false);
        let bins = (CASCADES * N * N) as u64;
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let params = buffer(
            "water detail params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let spectrum = buffer(
            "water detail spectrum",
            bins * 16,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let work = buffer(
            "water detail work",
            bins * 3 * 8,
            wgpu::BufferUsages::STORAGE,
        );
        let foam = buffer("water detail foam", bins * 4, wgpu::BufferUsages::STORAGE);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water detail"),
            size: wgpu::Extent3d {
                width: N as u32,
                height: N as u32,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let mip_view = |mip: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("water detail mip"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mips: Vec<_> = (0..MIPS).map(mip_view).collect();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water detail"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: spectrum.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: work.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: foam.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&mips[0]),
                },
            ],
        });
        let down_groups = (1..MIPS as usize)
            .map(|m| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water detail mips"),
                    layout: &down_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&mips[m - 1]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&mips[m]),
                        },
                    ],
                })
            })
            .collect();
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("water detail"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            evolve: pipeline(&module, &layout, "evolve"),
            rows: pipeline(&module, &layout, "fft_rows"),
            cols: pipeline(&module, &layout, "fft_cols"),
            resolve: pipeline(&module, &layout, "resolve"),
            down: pipeline(&mips_module, &down_layout, "downsample"),
            params,
            spectrum,
            group,
            down_groups,
            view,
            state: None,
            active: false,
        }
    }

    /// Follow `sea` (the nearest body with wind waves): rebuild the amplitudes when it has
    /// changed enough to matter, and set this frame's clock. `time` world seconds, `dt` the
    /// frame's step.
    pub fn update(&mut self, shared: &GpuShared, sea: Option<DetailSea>, time: f32, dt: f32) {
        self.active = false;
        let Some(sea) = sea else {
            return;
        };
        let Some(spectrum) = Spectrum::of(&sea) else {
            return;
        };
        let stale = self.state.is_none_or(|s| {
            let near = |a: f32, b: f32, tol: f32| (a - b).abs() <= tol * a.abs().max(b.abs());
            !(near(s.sea.wind, sea.wind, 0.08)
                && s.sea.dir.dot(sea.dir) > 0.998
                && near(s.sea.fetch_m, sea.fetch_m, 0.1)
                && near(s.sea.height_scale, sea.height_scale, 0.08)
                && (s.sea.choppiness - sea.choppiness).abs() < 0.05)
        });
        if stale {
            let sizes = detail::tile_sizes(spectrum.k_cut());
            let mut amps = Vec::with_capacity(CASCADES * N * N);
            let mut slope_variance = 0.0;
            for (c, &size) in sizes.iter().enumerate() {
                let band = detail::band(sizes, c, spectrum.k_cut());
                amps.extend(detail::amplitudes(
                    &spectrum,
                    sea.dir,
                    size,
                    band,
                    0x5EA + c as u64,
                ));
                slope_variance += detail::slope_variance(&spectrum, band.0, band.1);
            }
            shared
                .queue
                .write_buffer(&self.spectrum, 0, bytemuck::cast_slice(&amps));
            self.state = Some(DetailState {
                sea,
                spectrum,
                sizes,
                slope_variance,
            });
        }
        let s = self.state.expect("set above");
        let params = Params {
            sizes: [
                s.sizes[0],
                s.sizes[1],
                time.rem_euclid(PERIOD_S),
                (-dt.max(0.0) / FOAM_LIFE_S).exp(),
            ],
            chop: [s.sea.choppiness.clamp(0.0, 1.0), FOAM_JACOBIAN, 0.0, 0.0],
        };
        shared
            .queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        self.active = true;
    }

    /// Evolve the tiles to this frame and rebuild their mips.
    pub fn dispatch(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.active {
            return;
        }
        let n = N as u32;
        let tiles = CASCADES as u32;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water detail"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_pipeline(&self.evolve);
        pass.dispatch_workgroups(n / 8, n / 8, tiles);
        pass.set_pipeline(&self.rows);
        pass.dispatch_workgroups(n, 3, tiles);
        pass.set_pipeline(&self.cols);
        pass.dispatch_workgroups(n, 3, tiles);
        pass.set_pipeline(&self.resolve);
        pass.dispatch_workgroups(n / 8, n / 8, tiles);
        pass.set_pipeline(&self.down);
        for (m, group) in self.down_groups.iter().enumerate() {
            let size = (n >> (m + 1)).max(1);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), LAYERS);
        }
    }

    /// The vectors the water shader reads for the tiles: (1/size0, 1/size1, flow scale,
    /// unused) and (wind direction x, z the tiles were built for, 1 on, unused).
    pub fn globals(&self, frame_wind: Vec2) -> ([f32; 4], [f32; 4]) {
        let Some(s) = self.state.filter(|_| self.active) else {
            return ([0.0; 4], [0.0; 4]);
        };
        // Streams take the ripples a light breeze over a short fetch would raise.
        let breeze = DetailSea {
            wind: frame_wind.length().max(3.0),
            dir: s.sea.dir,
            fetch_m: 50.0,
            height_scale: 1.0,
            choppiness: s.sea.choppiness,
        };
        let flow = Spectrum::of(&breeze).map_or(0.0, |b| s.scale_for(&b));
        (
            [1.0 / s.sizes[0], 1.0 / s.sizes[1], flow, 0.0],
            [s.sea.dir.x, s.sea.dir.y, 1.0, 0.0],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_validate() {
        for (name, src) in [("detail", SHADER), ("detail mips", MIPS_SHADER)] {
            let code = engine_render::features::compose_shader(src, false);
            engine_render::features::validate_wgsl(name, &code).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn params_match_the_shader() {
        // WGSL: Params is two vec4.
        assert_eq!(std::mem::size_of::<Params>(), 2 * 16);
    }
}
