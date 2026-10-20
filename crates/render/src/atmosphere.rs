//! Atmosphere look-up tables (Hillaire 2020), computed on the GPU:
//!
//! - transmittance to the top of the atmosphere (256×64), and multiple scattering (all orders
//!   and the two parts of the second, so the light it scatters toward the eye can follow where
//!   it comes from) with the sky's light on the ground (four 32×32 tables in one 32×128
//!   texture), recomputed
//!   only when the planet or its air changes (its size, scale heights, ground, anisotropy, or
//!   any scattering or absorption coefficient: Earth's air to Mars' rebuilds them);
//! - the sky seen from the camera (192×108), every frame while the camera is inside the
//!   atmosphere (from above it, shaders integrate the sky per pixel with the first two).
//!
//! Shaders read them through frame bindings 13–15 (`lib_atmosphere.wgsl`).

use crate::features::GpuShared;
use crate::frame::FrameUniform;
use crate::renderer::layout_entry;
use crate::targets::Tex;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(crate) struct AtmosphereLuts {
    pub transmittance: Tex,
    pub multiscatter: Tex,
    pub sky_view: Tex,
    /// Transmittance, multiple scattering, sky view.
    passes: [LutPass; 3],
    /// Parameters the transmittance and multiple scattering tables were computed for.
    key: Option<[f32; KEY_LEN]>,
}

/// The planet's size, scale heights, ground, sun size, the air's four coefficients and its
/// anisotropy (RGB).
const KEY_LEN: usize = 8 + 5 * 3 + 1;

struct LutPass {
    pipeline: wgpu::ComputePipeline,
    inputs: wgpu::BindGroup,
    output: wgpu::BindGroup,
    size: (u32, u32),
}

impl AtmosphereLuts {
    /// `frame` is the frame uniform buffer the tables are computed from.
    pub fn new(shared: &GpuShared, frame: &wgpu::Buffer) -> Self {
        let device = &shared.device;
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING;
        let transmittance = Tex::new(device, "atmosphere transmittance", 256, 64, FORMAT, usage);
        let multiscatter = Tex::new(device, "atmosphere multiscatter", 32, 4 * 32, FORMAT, usage);
        let sky_view = Tex::new(device, "atmosphere sky view", 192, 108, FORMAT, usage);
        let module = shared.shader(
            "atmosphere tables",
            include_str!("shaders/atmosphere_luts.wgsl"),
            true,
        );

        use wgpu::ShaderStages as S;
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atmosphere table output"),
            entries: &[layout_entry(
                0,
                S::COMPUTE,
                wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: FORMAT,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
            )],
        });
        let table = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };

        // Each pass reads the frame uniform and the tables computed before it, through the same
        // group 0 bindings the rest of the renderer uses (lib_frame.wgsl).
        let pass = |label: &str, entry: &str, reads: &[&Tex], writes: &Tex| -> LutPass {
            let mut layout_entries = vec![layout_entry(
                0,
                S::COMPUTE,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )];
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }];
            if !reads.is_empty() {
                layout_entries.push(layout_entry(
                    4,
                    S::COMPUTE,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ));
                entries.push(wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
                });
            }
            for (i, tex) in reads.iter().enumerate() {
                let binding = 13 + i as u32;
                layout_entries.push(layout_entry(binding, S::COMPUTE, table));
                entries.push(wgpu::BindGroupEntry {
                    binding,
                    resource: wgpu::BindingResource::TextureView(&tex.view),
                });
            }
            let input_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &layout_entries,
            });
            let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &input_layout,
                entries: &entries,
            });
            let output = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &output_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&writes.view),
                }],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&input_layout), Some(&output_layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
            LutPass {
                pipeline,
                inputs,
                output,
                size: (writes.width, writes.height),
            }
        };
        let mut passes = [
            pass(
                "atmosphere transmittance",
                "cs_transmittance",
                &[],
                &transmittance,
            ),
            pass(
                "atmosphere multiscatter",
                "cs_multiscatter",
                &[&transmittance],
                &multiscatter,
            ),
            pass(
                "atmosphere sky view",
                "cs_sky_view",
                &[&transmittance, &multiscatter],
                &sky_view,
            ),
        ];
        // One invocation fills a texel of each of the four multiple scattering tables.
        passes[1].size = (32, 32);
        Self {
            transmittance,
            multiscatter,
            sky_view,
            passes,
            key: None,
        }
    }

    /// Record the table updates for this frame (`uniform` is what the frame buffer holds).
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, uniform: &FrameUniform) {
        let radius = uniform.planet[3];
        if radius <= 0.0 {
            return;
        }
        let key = table_key(uniform);
        let rebuild = self.key != Some(key);
        let inside = uniform.camera_world[3] < uniform.atmosphere[0] - 1.0;
        if !rebuild && !inside {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("atmosphere tables"),
            timestamp_writes: None,
        });
        let mut run = |p: &LutPass| {
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, &p.inputs, &[]);
            pass.set_bind_group(1, &p.output, &[]);
            pass.dispatch_workgroups(p.size.0.div_ceil(8), p.size.1.div_ceil(8), 1);
        };
        if rebuild {
            run(&self.passes[0]);
            run(&self.passes[1]);
            self.key = Some(key);
        }
        if inside {
            run(&self.passes[2]);
        }
    }
}

/// What the transmittance and multiple scattering tables depend on (the anisotropy through
/// the sky's light on the ground).
fn table_key(uniform: &FrameUniform) -> [f32; KEY_LEN] {
    let mut key = [0.0; KEY_LEN];
    key[..8].copy_from_slice(&[
        uniform.planet[3],
        uniform.atmosphere[0],
        uniform.atmosphere[1],
        uniform.atmosphere[2],
        uniform.ground[0],
        uniform.ground[1],
        uniform.ground[2],
        uniform.sun_dir[3],
    ]);
    let air = [
        uniform.air_rayleigh,
        uniform.air_mie,
        uniform.air_mie_extinction,
        uniform.air_ozone,
        uniform.air_mie_g,
    ];
    for (i, c) in air.iter().enumerate() {
        key[8 + 3 * i..11 + 3 * i].copy_from_slice(&c[..3]);
    }
    key[KEY_LEN - 1] = uniform.air_mie_g[3];
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn the_tables_follow_every_coefficient_of_the_air() {
        let base = FrameUniform::zeroed();
        let fields: [fn(&mut FrameUniform) -> &mut [f32; 4]; 5] = [
            |u| &mut u.air_rayleigh,
            |u| &mut u.air_mie,
            |u| &mut u.air_mie_extinction,
            |u| &mut u.air_ozone,
            |u| &mut u.air_mie_g,
        ];
        for field in fields {
            for c in 0..3 {
                let mut changed = base;
                field(&mut changed)[c] = 1e-6;
                assert_ne!(table_key(&changed), table_key(&base));
            }
        }
        let mut peak = base;
        peak.air_mie_g[3] = 1.0;
        assert_ne!(table_key(&peak), table_key(&base));
    }
}
