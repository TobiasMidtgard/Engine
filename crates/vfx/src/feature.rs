//! [`ParticleFeature`]: simulates and draws particles on the GPU, and draws trails and beams.
//!
//! Each frame:
//!
//! 1. `prepare` turns the [`ParticleFrame`] built by [`crate::emitter::tick_effects`] into
//!    GPU tables: one [`GpuEmitter`] per live emitter (camera-relative), update jobs covering
//!    every emitter's ring of slots, and spawn jobs for the new particles. Trails and beams
//!    become camera-facing ribbon triangles.
//! 2. `compute` clears freshly allocated rings, then runs `cs_update` (move, collide and
//!    relight every live particle) and `cs_spawn` (write the new ones) from
//!    `shaders/particles_sim.wgsl`. World-space particles are stored camera-relative and
//!    shifted by the camera's movement, so they stay precise anywhere in a solar system.
//! 3. `draw` (transparent phase) draws ribbons, then each visible emitter far to near as one
//!    instanced quad per slot (`shaders/particles_draw.wgsl`), softened against the scene
//!    depth copy.
//!
//! A frame rendered twice (a screenshot right after the scheduled render) is simulated once.

use bevy_ecs::prelude::*;
use bytemuck::Zeroable;
use engine_core::diagnostics::Diagnostics;
use engine_core::math::{DVec3, Vec3};
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase, RenderSettings,
    ViewData,
};

use crate::emitter::{ParticleFrame, PoolRange, VfxSettings};
use crate::gpu::{CHUNK, GpuEmitter, GpuParticle, GpuSim, GpuSpawn, GpuUpdate, RibbonVertex};
use crate::trail::{Ribbons, build_ribbon};

/// Shared particle structs and helpers.
pub const SHADER_COMMON: &str = include_str!("shaders/particles_common.wgsl");
/// Simulation kernels (`cs_update`, `cs_spawn`).
pub const SHADER_SIM: &str = include_str!("shaders/particles_sim.wgsl");
/// Particle sprites (`vs_particle`, `fs_particle`).
pub const SHADER_DRAW: &str = include_str!("shaders/particles_draw.wgsl");
/// Trails and beams (`vs_ribbon`, `fs_ribbon`).
pub const SHADER_RIBBONS: &str = include_str!("shaders/ribbons.wgsl");

/// A particle shader's own source with the shared particle code in front (then composed with
/// `lib_types` + `lib_frame` by the renderer).
pub fn particle_shader(source: &str) -> String {
    format!("{SHADER_COMMON}\n{source}")
}

const PARTICLE_BYTES: u64 = std::mem::size_of::<GpuParticle>() as u64;
const INITIAL_SLOTS: u32 = 1 << 16;
/// Workgroups per dispatch dimension.
const MAX_GROUPS: u32 = 65_535;
/// Exposure that glow values are authored for (EV100 14, a bright overcast day).
const GLOW_REFERENCE_EV100: f32 = 14.0;

/// Multiplier for authored glow (cd/m²) at the camera's current pre-exposure, so effects keep
/// their color at night and still read by day. `weight` 1 is physical, 0 ignores exposure.
pub fn glow_scale(exposure: f32, weight: f32) -> f32 {
    let reference = engine_render::frame::ev100_to_exposure(GLOW_REFERENCE_EV100);
    if exposure <= 0.0 {
        return 1.0;
    }
    (reference / exposure).powf(1.0 - weight.clamp(0.0, 1.0))
}

struct Gpu {
    update: wgpu::ComputePipeline,
    spawn: wgpu::ComputePipeline,
    draw: wgpu::RenderPipeline,
    ribbon: wgpu::RenderPipeline,
    sim_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    particles: wgpu::Buffer,
    slots: u32,
    /// The previous particle buffer and its size, copied into the new one after growing.
    old_particles: Option<(wgpu::Buffer, u64)>,
    emitters: GrowBuffer,
    spawns: GrowBuffer,
    updates: GrowBuffer,
    sim: wgpu::Buffer,
    ribbons: GrowBuffer,
    sim_group: Option<wgpu::BindGroup>,
    draw_group: Option<wgpu::BindGroup>,
}

/// Draws particles, trails and beams. Added by [`crate::VfxPlugin`].
#[derive(Default)]
pub struct ParticleFeature {
    gpu: Option<Gpu>,
    /// Most particle slots one storage binding can hold on this device.
    device_slots: u32,
    emitters: Vec<GpuEmitter>,
    spawns: Vec<GpuSpawn>,
    updates: Vec<GpuUpdate>,
    cleared: Vec<PoolRange>,
    /// Visible emitters: view depth, first slot, slot count (drawn far to near).
    draws: Vec<(f32, u32, u32)>,
    ribbon_vertices: Vec<RibbonVertex>,
    ribbon_count: u32,
    last_frame: Option<u64>,
    last_origin: Option<DVec3>,
    /// Run the simulation in this frame's `compute` (false when re-rendering a frame).
    simulate: bool,
    soft: bool,
    skipped: u32,
    stats: FeatureStats,
}

impl ParticleFeature {
    /// Particle slots the GPU buffer holds now.
    pub fn slots(&self) -> u32 {
        self.gpu.as_ref().map_or(0, |g| g.slots)
    }
}

fn storage(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn particle_buffer(device: &wgpu::Device, slots: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vfx particles"),
        size: slots as u64 * PARTICLE_BYTES,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Premultiplied blending: alpha 0 adds light, alpha 1 covers what's behind.
const PREMULTIPLIED: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

const RIBBON_ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Float32, 4 => Float32, 5 => Float32,
    6 => Float32
];

/// Workgroup counts for `jobs` one-workgroup jobs (2D past the per-dimension limit).
fn dispatch_size(jobs: u32) -> (u32, u32) {
    if jobs <= MAX_GROUPS {
        (jobs, 1)
    } else {
        (MAX_GROUPS, jobs.div_ceil(MAX_GROUPS))
    }
}

impl RenderFeature for ParticleFeature {
    fn name(&self) -> &str {
        "vfx_particles"
    }

    /// After other transparent geometry (water surfaces), so splashes draw over them.
    fn order(&self) -> i32 {
        100
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let limit = device.limits().max_storage_buffer_binding_size;
        self.device_slots = (limit / PARTICLE_BYTES).min(u32::MAX as u64) as u32;

        let compute = wgpu::ShaderStages::COMPUTE;
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx sim"),
            entries: &[
                storage(0, false, compute),
                storage(1, true, compute),
                storage(2, true, compute),
                storage(3, true, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx draw"),
            entries: &[
                storage(0, true, wgpu::ShaderStages::VERTEX),
                storage(1, true, wgpu::ShaderStages::VERTEX),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let sim_shader = gpu.shader("vfx sim", &particle_shader(SHADER_SIM), true);
        let sim_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vfx sim"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&sim_layout)],
            immediate_size: 0,
        });
        let kernel = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&sim_pipeline_layout),
                module: &sim_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let update = kernel("cs_update");
        let spawn = kernel("cs_spawn");

        let target = [Some(wgpu::ColorTargetState {
            format: gpu.formats.hdr,
            blend: Some(PREMULTIPLIED),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let draw_shader = gpu.shader("vfx particles", &particle_shader(SHADER_DRAW), true);
        let draw_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vfx particles"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&draw_layout)],
            immediate_size: 0,
        });
        let draw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vfx particles"),
            layout: Some(&draw_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: Some("vs_particle"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(gpu.depth_state(false)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
                entry_point: Some("fs_particle"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });

        let ribbon_shader = gpu.shader("vfx ribbons", &particle_shader(SHADER_RIBBONS), true);
        let ribbon_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vfx ribbons"),
                bind_group_layouts: &[Some(&gpu.frame_layout)],
                immediate_size: 0,
            });
        let ribbon = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vfx ribbons"),
            layout: Some(&ribbon_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &ribbon_shader,
                entry_point: Some("vs_ribbon"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RibbonVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &RIBBON_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(gpu.depth_state(false)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &ribbon_shader,
                entry_point: Some("fs_ribbon"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });

        let storage_usage = wgpu::BufferUsages::STORAGE;
        let slots = INITIAL_SLOTS.min(self.device_slots.max(1024));
        self.gpu = Some(Gpu {
            update,
            spawn,
            draw,
            ribbon,
            sim_layout,
            draw_layout,
            particles: particle_buffer(device, slots),
            slots,
            old_particles: None,
            emitters: GrowBuffer::new(
                device,
                "vfx emitters",
                storage_usage,
                64 * std::mem::size_of::<GpuEmitter>() as u64,
            ),
            spawns: GrowBuffer::new(device, "vfx spawns", storage_usage, 64 * 1024),
            updates: GrowBuffer::new(device, "vfx updates", storage_usage, 64 * 1024),
            sim: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vfx sim"),
                size: std::mem::size_of::<GpuSim>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ribbons: GrowBuffer::new(
                device,
                "vfx ribbons",
                wgpu::BufferUsages::VERTEX,
                4096 * std::mem::size_of::<RibbonVertex>() as u64,
            ),
            sim_group: None,
            draw_group: None,
        });
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        // The scene depth copy is recreated with the other size-dependent targets.
        if let Some(g) = &mut self.gpu {
            g.draw_group = None;
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.stats = FeatureStats::default();
        self.simulate = false;
        let Some(g) = self.gpu.as_mut() else {
            return;
        };
        let settings = world
            .get_resource::<VfxSettings>()
            .cloned()
            .unwrap_or_default();
        if !settings.enabled {
            self.draws.clear();
            self.ribbon_count = 0;
            return;
        }
        self.soft = settings.soft_particles;

        // Ribbons follow the camera every time the frame is drawn.
        self.ribbon_vertices.clear();
        if let Some(ribbons) = world.get_resource::<Ribbons>() {
            let out = &mut self.ribbon_vertices;
            ribbons.for_each_ribbon(|points, style, additive| {
                build_ribbon(points, view.origin, style, additive, out);
            });
        }
        let glow = glow_scale(view.exposure, settings.glow_exposure);
        for v in &mut self.ribbon_vertices {
            for c in &mut v.color[..3] {
                *c *= glow;
            }
        }
        self.ribbon_count = self.ribbon_vertices.len() as u32;
        if self.ribbon_count > 0 {
            g.ribbons.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&self.ribbon_vertices),
            );
        }

        let Some(frame) = world.get_resource::<ParticleFrame>() else {
            self.draws.clear();
            return;
        };
        if self.last_frame == Some(frame.frame) {
            // Drawn again in the same frame (a screenshot): show the same particles.
            self.fill_stats();
            return;
        }
        self.last_frame = Some(frame.frame);
        self.simulate = true;
        let shift = self
            .last_origin
            .map_or(Vec3::ZERO, |o| (o - view.origin).as_vec3());
        self.last_origin = Some(view.origin);

        // Grow the particle buffer to the pool's high-water mark.
        let need = frame.pool_high_water.min(self.device_slots);
        if need > g.slots {
            let slots = need
                .next_power_of_two()
                .max(INITIAL_SLOTS)
                .min(self.device_slots);
            let old = std::mem::replace(&mut g.particles, particle_buffer(&gpu.device, slots));
            // Keep any copy still waiting from an earlier growth (it holds the live data).
            if g.old_particles.is_none() {
                g.old_particles = Some((old, g.slots as u64 * PARTICLE_BYTES));
            }
            g.slots = slots;
            g.sim_group = None;
            g.draw_group = None;
        }

        // Emitter table, update and spawn jobs, and the draw list.
        self.emitters.clear();
        self.updates.clear();
        self.spawns.clear();
        self.draws.clear();
        self.skipped = 0;
        let max_draw = settings.draw_distance.max(1.0);
        for fe in &frame.emitters {
            if fe.range.end() > g.slots {
                self.skipped += 1;
                continue;
            }
            let index = self.emitters.len() as u32;
            let rel = view.relative(fe.origin);
            let prev = view.relative(fe.prev_origin);
            let mut e = fe.gpu;
            e.origin[..3].copy_from_slice(&rel.to_array());
            e.prev[..3].copy_from_slice(&prev.to_array());
            match fe.plane {
                Some((point, normal)) => {
                    let d = -normal.dot(view.relative(point));
                    e.plane = [normal.x, normal.y, normal.z, d];
                    e.collide[3] = 1.0;
                }
                None => e.collide[3] = 0.0,
            }
            e.shade[0] *= glow;
            if !self.soft {
                e.shade[3] = 0.0;
            }
            self.emitters.push(e);

            let PoolRange { base, len } = fe.range;
            let mut k = 0;
            while k < len {
                self.updates.push(GpuUpdate {
                    emitter: index,
                    first: base + k,
                    count: CHUNK.min(len - k),
                    _pad: 0,
                });
                k += CHUNK;
            }
            for run in &fe.runs {
                let count = run.count.min(len);
                let from = view.relative(run.from);
                let to = view.relative(run.to);
                let mut k = 0;
                while k < count {
                    self.spawns.push(GpuSpawn {
                        emitter: index,
                        base,
                        capacity: len,
                        ring_start: (run.ring_start + k) % len.max(1),
                        count: CHUNK.min(count - k),
                        index: k,
                        run_count: count,
                        seed: run.seed,
                        from: [from.x, from.y, from.z, run.dt],
                        to: [to.x, to.y, to.z, run.pre_age],
                    });
                    k += CHUNK;
                }
            }

            let reach = fe.reach.max(0.5);
            if rel.length() - reach > max_draw || !view.frustum.intersects_sphere(rel, reach) {
                self.stats.culled += 1;
                continue;
            }
            self.draws.push((view.view_depth(rel), base, len));
        }
        // Far to near, so nearer effects blend over farther ones.
        self.draws.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.cleared.clear();
        self.cleared
            .extend(frame.cleared.iter().filter(|r| r.end() <= g.slots).copied());

        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut grown = false;
        if !self.emitters.is_empty() {
            grown |= g
                .emitters
                .write(device, queue, bytemuck::cast_slice(&self.emitters));
        }
        if !self.updates.is_empty() {
            grown |= g
                .updates
                .write(device, queue, bytemuck::cast_slice(&self.updates));
        }
        if !self.spawns.is_empty() {
            grown |= g
                .spawns
                .write(device, queue, bytemuck::cast_slice(&self.spawns));
        }
        if grown {
            g.sim_group = None;
            g.draw_group = None;
        }
        let sim = GpuSim {
            shift: [shift.x, shift.y, shift.z, view.time],
            counts: [
                self.updates.len() as u32,
                self.spawns.len() as u32,
                (view.frame_index & 0xFFFF_FFFF) as u32,
                0,
            ],
        };
        queue.write_buffer(&g.sim, 0, bytemuck::bytes_of(&sim));

        let slots = g.slots;
        self.fill_stats();
        let skipped = self.skipped;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("vfx.gpu_slots", slots as f64);
            d.set("vfx.drawn_emitters", self.draws.len() as f64);
            d.set("vfx.skipped_emitters", skipped as f64);
        }
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let Some(g) = self.gpu.as_mut() else {
            return;
        };
        let device = &ctx.gpu.device;
        if let Some((old, bytes)) = g.old_particles.take() {
            encoder.copy_buffer_to_buffer(&old, 0, &g.particles, 0, bytes);
        }
        if g.sim_group.is_none() {
            g.sim_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vfx sim"),
                layout: &g.sim_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: g.particles.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: g.emitters.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: g.spawns.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: g.updates.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: g.sim.as_entire_binding(),
                    },
                ],
            }));
        }
        if g.draw_group.is_none() {
            g.draw_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("vfx draw"),
                layout: &g.draw_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: g.particles.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: g.emitters.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                    },
                ],
            }));
        }
        if !self.simulate {
            return;
        }
        for r in &self.cleared {
            encoder.clear_buffer(
                &g.particles,
                r.base as u64 * PARTICLE_BYTES,
                Some(r.len as u64 * PARTICLE_BYTES),
            );
        }
        let Some(group) = &g.sim_group else {
            return;
        };
        for (pipeline, jobs, label) in [
            (&g.update, self.updates.len() as u32, "vfx update"),
            (&g.spawn, self.spawns.len() as u32, "vfx spawn"),
        ] {
            if jobs == 0 {
                continue;
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, group, &[]);
            let (x, y) = dispatch_size(jobs);
            pass.dispatch_workgroups(x, y, 1);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let Some(g) = &self.gpu else {
            return;
        };
        if self.ribbon_count > 0 {
            pass.set_pipeline(&g.ribbon);
            pass.set_vertex_buffer(0, g.ribbons.buffer.slice(..));
            pass.draw(0..self.ribbon_count, 0..1);
        }
        if let (false, Some(group)) = (self.draws.is_empty(), &g.draw_group) {
            pass.set_pipeline(&g.draw);
            pass.set_bind_group(1, group, &[]);
            for &(_, base, len) in &self.draws {
                pass.draw(0..4, base..base + len);
            }
        }
    }

    fn needs_scene_copy(&self) -> bool {
        self.soft && !self.draws.is_empty()
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "gpu_slots": self.slots(),
            "emitters": self.emitters.len(),
            "drawn_emitters": self.draws.len(),
            "update_jobs": self.updates.len(),
            "spawn_jobs": self.spawns.len(),
            "ribbon_vertices": self.ribbon_count,
            "skipped_emitters": self.skipped,
        })
    }
}

impl ParticleFeature {
    fn fill_stats(&mut self) {
        let culled = self.stats.culled;
        self.stats = FeatureStats {
            draw_calls: self.draws.len() as u32 + u32::from(self.ribbon_count > 0),
            instances: self.draws.iter().map(|d| d.2).sum(),
            triangles: self.draws.iter().map(|d| d.2 as u64 * 2).sum::<u64>()
                + self.ribbon_count as u64 / 3,
            culled,
        };
    }
}

/// An all-zero (dead) particle, for tests and tools.
pub fn dead_particle() -> GpuParticle {
    GpuParticle::zeroed()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(label: &str, source: &str) {
        let code = engine_render::compose_shader(&particle_shader(source), true);
        engine_render::validate_wgsl(label, &code).unwrap();
    }

    #[test]
    fn shaders_validate() {
        validate("particles_sim", SHADER_SIM);
        validate("particles_draw", SHADER_DRAW);
        validate("ribbons", SHADER_RIBBONS);
    }

    #[test]
    fn ribbon_attributes_match_the_vertex() {
        let last = RIBBON_ATTRIBUTES.last().unwrap();
        assert_eq!(last.offset + 4, std::mem::size_of::<RibbonVertex>() as u64);
        assert_eq!(RIBBON_ATTRIBUTES[2].offset, 16);
    }

    #[test]
    fn glow_keeps_its_color_at_night_and_reads_by_day() {
        use engine_render::frame::ev100_to_exposure;
        let on_screen = |ev: f32, w: f32| {
            let e = ev100_to_exposure(ev);
            20_000.0 * glow_scale(e, w) * e
        };
        // Physical: 2000x brighter on screen at dusk than at noon (11 stops).
        assert!(on_screen(4.0, 1.0) / on_screen(15.0, 1.0) > 1000.0);
        // Default: a few times brighter, so a torch blooms at night without turning white.
        let ratio = on_screen(4.0, 0.15) / on_screen(15.0, 0.15);
        assert!((2.0..5.0).contains(&ratio), "{ratio}");
        assert!((glow_scale(ev100_to_exposure(14.0), 0.15) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn big_dispatches_go_2d() {
        assert_eq!(dispatch_size(10), (10, 1));
        let (x, y) = dispatch_size(200_000);
        assert!(x * y >= 200_000 && x <= MAX_GROUPS);
    }

    #[test]
    fn shader_structs_match_rust_sizes() {
        // The WGSL Emitter has 18 vec4s, 16 colors and 4 size vec4s.
        assert!(SHADER_COMMON.contains("color: array<vec4<f32>, 16>"));
        assert!(SHADER_COMMON.contains("size_curve: array<vec4<f32>, 4>"));
        assert_eq!(std::mem::size_of::<GpuEmitter>(), 38 * 16);
        assert!(dead_particle().age >= dead_particle().life);
    }
}
