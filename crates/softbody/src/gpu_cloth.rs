//! Flags, banners and pennants simulated on the GPU ([`SimSettings::gpu`]).
//!
//! [`GpuCloths`] belongs to the soft-body render feature. Every frame `prepare` picks up each
//! GPU cloth, gives it a slot in a persistent particle buffer and packs its parameters (wind
//! and gravity at its anchor, how far the anchor moved, link stiffness from its
//! compliances); `compute` runs `shaders/cloth_sim.wgsl`, one workgroup per cloth, which
//! keeps the cloth in workgroup memory for the frame's substeps and writes its vertices
//! straight into a vertex buffer. The regular surface and shadow pipelines draw them, so a
//! GPU flag looks exactly like a CPU one.
//!
//! [`SimSettings::gpu`]: crate::settings::SimSettings::gpu

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_render::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};
use engine_render::{GpuShared, GrowBuffer, ViewData, wgpu};
use engine_world::{Gravity, WindField, WorldShift};

use crate::cloth::{Cloth, ClothPins};
use crate::render::{GpuSoftMaterial, SoftVertex};
use crate::settings::SoftBodyConfig;
use crate::solver::AIR_DENSITY;

/// The simulation kernel.
pub const SIM_SHADER: &str = include_str!("shaders/cloth_sim.wgsl");
/// Most particles one GPU cloth can have (two per thread of a 256-wide workgroup); larger
/// grids are thinned evenly to fit.
pub const MAX_PARTICLES: u32 = 512;
/// Gust strength around the wind at the anchor (as the CPU solver's default).
const TURBULENCE: f32 = 0.25;
/// Most substeps in one frame (a long frame simulates less than it should, not forever).
const MAX_SUBSTEPS: u32 = 64;
/// Most substeps spent settling a new cloth in its first frame.
const MAX_PREWARM_SUBSTEPS: u32 = 1200;
/// Bytes per particle in the state buffer (position and velocity, `vec4` each).
const STATE_BYTES: u64 = 32;

/// One cloth's frame. Must match `struct Flag` in `cloth_sim.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuFlag {
    pub anchor: [f32; 4],
    pub rot: [f32; 4],
    pub shift: [f32; 4],
    pub wind: [f32; 4],
    pub gravity: [f32; 4],
    pub size: [f32; 4],
    pub stiff: [f32; 4],
    pub air: [f32; 4],
    pub grid: [u32; 4],
    pub pins: [u32; 4],
    pub turn: [f32; 4],
}

/// The kernel's pin bits for `pins` (left 1, top 2, right 4, bottom 8, top corners 16) and
/// the spacing along the top, or `None` for pins it can't do (bones, single points).
pub fn pin_mask(pins: &ClothPins) -> Option<(u32, u32)> {
    match pins {
        ClothPins::None => Some((0, 1)),
        ClothPins::LeftEdge => Some((1, 1)),
        ClothPins::TopEdge { every } => Some((2, (*every).max(1))),
        ClothPins::TopCorners => Some((16, 1)),
        ClothPins::Edges {
            left,
            top,
            right,
            bottom,
        } => Some((
            u32::from(*left)
                | u32::from(*top) << 1
                | u32::from(*right) << 2
                | u32::from(*bottom) << 3,
            1,
        )),
        ClothPins::Bones { .. } | ClothPins::Points(_) => None,
    }
}

/// Whether `spec` is simulated here: asked for, a plain rectangle, and pins the kernel does.
pub fn runs_on_gpu(spec: &Cloth) -> bool {
    spec.sim.gpu && spec.panel.is_none() && pin_mask(&spec.pins).is_some()
}

/// Columns and rows: the spec's, thinned evenly to fit [`MAX_PARTICLES`].
pub fn grid(spec: &Cloth) -> [u32; 2] {
    let [mut nu, mut nv] = spec.resolution.map(|n| n.max(2));
    while nu * nv > MAX_PARTICLES {
        let s = (MAX_PARTICLES as f32 / (nu * nv) as f32).sqrt();
        nu = ((nu as f32 * s) as u32).clamp(2, nu - 1);
        nv = ((nv as f32 * s) as u32).clamp(2, nv.max(3) - 1);
    }
    [nu, nv]
}

/// Triangles of an `nu` × `nv` grid, alternating diagonals and facing +Z, as the CPU cloth.
pub fn grid_indices(nu: u32, nv: u32) -> Vec<u32> {
    let id = |i: u32, j: u32| j * nu + i;
    let mut out = Vec::with_capacity(((nu - 1) * (nv - 1) * 6) as usize);
    for j in 0..nv - 1 {
        for i in 0..nu - 1 {
            let (a, b, c, d) = (id(i, j), id(i, j + 1), id(i + 1, j), id(i + 1, j + 1));
            if (i + j) % 2 == 0 {
                out.extend_from_slice(&[a, b, c, c, b, d]);
            } else {
                out.extend_from_slice(&[a, b, d, a, d, c]);
            }
        }
    }
    out
}

/// A link's softness for the kernel: XPBD's `α / h²` for `compliance` over a substep `h`,
/// in units of the particles' inverse `mass` (so a link between two free particles closes
/// `2 / (2 + softness)` of its error each substep).
pub fn link_softness(compliance: f32, mass: f32, h: f32) -> f32 {
    compliance.max(0.0) * mass.max(1e-6) / (h * h).max(1e-12)
}

/// For tests and tools: insert this naming a GPU cloth, and from the frame after next it
/// holds that cloth's particles in the world, read back from the GPU. The read-back stalls
/// the frame, so don't leave it in a game.
#[derive(Resource, Debug, Clone, Default)]
pub struct GpuClothProbe {
    pub entity: Option<Entity>,
    /// Row by row from the top-left of the grid.
    pub positions: Vec<DVec3>,
    pub grid: [u32; 2],
}

/// World shifts not yet taken into the GPU cloths, composed into one.
#[derive(Resource, Debug, Default)]
pub struct GpuClothShift(pub Option<WorldShift>);

/// Keep this frame's world shifts for the render feature (in `WorldShiftSet::Apply`, before
/// the anchors' transforms move), so moving with the world doesn't count as the anchor
/// moving.
pub fn follow_world_shift(
    mut shifts: MessageReader<WorldShift>,
    mut pending: ResMut<GpuClothShift>,
) {
    for s in shifts.read() {
        pending.0 = Some(pending.0.map_or(*s, |p| p.then(s)));
    }
}

// Shadow masks are bytes: the cascades plus the sky occlusion map.
const _: () = assert!(MAX_CASCADES < 8);

/// One GPU cloth to draw this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuDraw {
    pub indices: Range<u32>,
    pub base_vertex: i32,
    pub in_view: bool,
    pub transparent: bool,
    /// Bit per shadow cascade, plus the sky occlusion map.
    pub shadows: u8,
}

struct Slot {
    first: u32,
    grid: [u32; 2],
    anchor: DVec3,
    fresh: bool,
    seen: bool,
}

struct Readback {
    buffer: wgpu::Buffer,
    anchor: DVec3,
    grid: [u32; 2],
}

/// Times the simulation pass with GPU timestamps (when the device has them), reading the
/// result back a frame or two later without stalling.
struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// Nanoseconds per tick.
    period: f32,
    /// The pass, milliseconds, averaged over recent frames.
    ms: Option<f32>,
}

const FREE: u8 = 0;
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

impl Timer {
    fn new(gpu: &GpuShared) -> Option<Self> {
        if !gpu.features.contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let device = &gpu.device;
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("softbody gpu cloth timer"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: buffer(
                "softbody gpu cloth timer resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: buffer(
                "softbody gpu cloth timer readback",
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            state: Arc::new(AtomicU8::new(FREE)),
            period: gpu.queue.get_timestamp_period(),
            ms: None,
        })
    }

    /// Start mapping last frame's copy (it has been submitted by now) and take in any that
    /// finished.
    fn collect(&mut self, device: &wgpu::Device) {
        if self.state.load(Ordering::Acquire) == COPIED {
            self.state.store(MAPPING, Ordering::Release);
            let state = self.state.clone();
            self.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
                });
        }
        let _ = device.poll(wgpu::PollType::Poll);
        match self.state.load(Ordering::Acquire) {
            MAPPED => {
                if let Ok(view) = self.readback.slice(..).get_mapped_range() {
                    let t: &[u64] = bytemuck::cast_slice(&view);
                    let ms = (t[1].saturating_sub(t[0]) as f64 * self.period as f64 * 1e-6) as f32;
                    // Ignore nonsense (a reset counter, a pass split across a frame boundary).
                    if ms < 1000.0 {
                        self.ms = Some(self.ms.map_or(ms, |old| old + (ms - old) * 0.1));
                    }
                }
                self.readback.unmap();
                self.state.store(FREE, Ordering::Release);
            }
            FAILED => {
                self.readback.unmap();
                self.state.store(FREE, Ordering::Release);
            }
            _ => {}
        }
    }

    fn writes(&self) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        }
    }

    /// After the pass: copy its timestamps out, unless the last copy is still being read.
    fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.state.load(Ordering::Acquire) != FREE {
            return;
        }
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, 16);
        self.state.store(COPIED, Ordering::Release);
    }
}

/// The GPU cloth simulation: kernel, buffers and slots.
pub struct GpuCloths {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    flags: GrowBuffer,
    state: wgpu::Buffer,
    /// Particles the state and vertex buffers hold.
    capacity: u32,
    /// The old state buffer and its size, copied into the new one before the next dispatch.
    grown_from: Option<(wgpu::Buffer, u64)>,
    pub vertices: wgpu::Buffer,
    pub indices: GrowBuffer,
    index_data: Vec<u32>,
    layouts: HashMap<[u32; 2], Range<u32>>,
    query: Option<QueryState<(Entity, &'static Cloth, &'static GlobalTransform)>>,
    slots: HashMap<Entity, Slot>,
    /// Unused particle ranges (first, count).
    free: Vec<(u32, u32)>,
    /// Particles handed out from the start of the buffers.
    used: u32,
    frame: Vec<GpuFlag>,
    pub draws: Vec<GpuDraw>,
    time: f32,
    probe: Option<(u32, [u32; 2], DVec3)>,
    readback: Option<Readback>,
    timer: Option<Timer>,
}

impl GpuCloths {
    pub fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let entry = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("softbody gpu cloth"),
            entries: &[entry(0, true), entry(1, false), entry(2, false)],
        });
        let module = gpu.shader("softbody gpu cloth", SIM_SHADER, false);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("softbody gpu cloth"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("softbody gpu cloth"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_simulate"),
            compilation_options: Default::default(),
            cache: None,
        });
        let flags = GrowBuffer::new(
            device,
            "softbody gpu cloth flags",
            wgpu::BufferUsages::STORAGE,
            64 * std::mem::size_of::<GpuFlag>() as u64,
        );
        let capacity = 8192;
        let (state, vertices) = Self::particle_buffers(device, capacity);
        let indices = GrowBuffer::new(
            device,
            "softbody gpu cloth indices",
            wgpu::BufferUsages::INDEX,
            1 << 16,
        );
        let group = Self::bind_group(device, &layout, &flags, &state, &vertices);
        Self {
            pipeline,
            layout,
            group,
            flags,
            state,
            capacity,
            grown_from: None,
            vertices,
            indices,
            index_data: Vec::new(),
            layouts: HashMap::new(),
            query: None,
            slots: HashMap::new(),
            free: Vec::new(),
            used: 0,
            frame: Vec::new(),
            draws: Vec::new(),
            time: 0.0,
            probe: None,
            readback: None,
            timer: Timer::new(gpu),
        }
    }

    fn particle_buffers(device: &wgpu::Device, particles: u32) -> (wgpu::Buffer, wgpu::Buffer) {
        let state = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softbody gpu cloth state"),
            size: particles as u64 * STATE_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("softbody gpu cloth vertices"),
            size: particles as u64 * std::mem::size_of::<SoftVertex>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        (state, vertices)
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        flags: &GrowBuffer,
        state: &wgpu::Buffer,
        vertices: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("softbody gpu cloth"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: flags.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: state.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: vertices.as_entire_binding(),
                },
            ],
        })
    }

    /// A range of `count` particles: a freed one that fits, or more at the end.
    fn allocate(&mut self, count: u32) -> u32 {
        if let Some(k) = self.free.iter().position(|&(_, n)| n >= count) {
            let (first, n) = self.free[k];
            if n == count {
                self.free.swap_remove(k);
            } else {
                self.free[k] = (first + count, n - count);
            }
            return first;
        }
        self.used += count;
        self.used - count
    }

    fn layout(&mut self, grid: [u32; 2]) -> Range<u32> {
        if let Some(r) = self.layouts.get(&grid) {
            return r.clone();
        }
        let start = self.index_data.len() as u32;
        self.index_data.extend(grid_indices(grid[0], grid[1]));
        let r = start..self.index_data.len() as u32;
        self.layouts.insert(grid, r.clone());
        r
    }

    /// Gather this frame's GPU cloths: slots, parameters, materials (appended to
    /// `materials`) and draws.
    pub fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        materials: &mut Vec<GpuSoftMaterial>,
    ) {
        self.frame.clear();
        self.draws.clear();
        self.read_probe(world, gpu);
        if let Some(timer) = self.timer.as_mut() {
            timer.collect(&gpu.device);
        }
        let config = world
            .get_resource::<SoftBodyConfig>()
            .cloned()
            .unwrap_or_default();
        if !config.enabled {
            return;
        }
        let dt = world
            .get_resource::<Time>()
            .map_or(0.0, |t| t.delta_for(None))
            .clamp(0.0, 0.1);
        self.time = (self.time + dt) % 3600.0;
        let probe = world.get_resource::<GpuClothProbe>().and_then(|p| p.entity);
        // A world shift moves the anchors and turns the kept state; the frame's velocity
        // doesn't reach the cloth (flags are tied to the ground, which moves with them).
        let shift = world
            .get_resource_mut::<GpuClothShift>()
            .and_then(|mut s| s.0.take());
        let turn = shift.map_or(Quat::IDENTITY, |s| s.rotation.as_quat());
        if let Some(s) = &shift {
            for slot in self.slots.values_mut() {
                slot.anchor = s.point(slot.anchor);
            }
        }
        self.probe = None;
        let query = self
            .query
            .get_or_insert_with(|| world.query::<(Entity, &Cloth, &GlobalTransform)>());
        let mut found: Vec<(Entity, Cloth, GlobalTransform)> = Vec::new();
        for (e, spec, t) in query.iter(world) {
            if runs_on_gpu(spec) {
                found.push((e, spec.clone(), *t));
            }
        }
        let wind_field = world.get_resource::<WindField>();
        let gravity = world.get_resource::<Gravity>();
        for slot in self.slots.values_mut() {
            slot.seen = false;
        }
        let mut new_layouts = false;
        for (e, spec, t) in &found {
            let grid = grid(spec);
            let anchor = t.translation();
            let reuse = self.slots.get(e).is_some_and(|s| s.grid == grid);
            if !reuse {
                if let Some(old) = self.slots.remove(e) {
                    self.free.push((old.first, old.grid[0] * old.grid[1]));
                }
                let first = self.allocate(grid[0] * grid[1]);
                self.slots.insert(
                    *e,
                    Slot {
                        first,
                        grid,
                        anchor,
                        fresh: true,
                        seen: false,
                    },
                );
            }
            new_layouts |= !self.layouts.contains_key(&grid);
            let indices = self.layout(grid);
            let slot = self.slots.get_mut(e).expect("slot");
            slot.seen = true;
            let moved = (anchor - slot.anchor).as_vec3();
            // A teleport starts the cloth over rather than dragging it across.
            let [w0, h0] = spec.size;
            slot.fresh |= moved.length() > 2.0 * (w0 + h0).max(4.0);
            let shift = if slot.fresh { Vec3::ZERO } else { moved };
            slot.anchor = anchor;
            let fresh = std::mem::take(&mut slot.fresh);
            let first = slot.first;

            let s = &spec.sim;
            let scale = t.scale();
            let [w, h] = [spec.size[0] * scale.x, spec.size[1] * scale.y].map(|v| v.max(0.01));
            let n = grid[0] * grid[1];
            let mass = spec.mass_per_m2.max(1e-4) * w * h / n as f32;
            let rate = s.substeps.max(1) as f32 * 60.0 * config.quality.max(0.1);
            let (substeps, step) = if fresh {
                let steps = (s.prewarm_secs.max(0.5) * rate).ceil() as u32;
                (steps.min(MAX_PREWARM_SUBSTEPS), 1.0 / rate)
            } else if dt > 0.0 {
                let steps = ((dt * rate).ceil() as u32).clamp(1, MAX_SUBSTEPS);
                (steps, dt / steps as f32)
            } else {
                (0, 1.0 / rate)
            };
            let wind = wind_field.map_or(Vec3::ZERO, |f| f.sample(anchor)) * s.wind_scale;
            let g = gravity.map_or(Vec3::NEG_Y * 9.81, |g| g.at(anchor)) * s.gravity_scale;
            let (mask, every) = pin_mask(&spec.pins).unwrap_or((1, 1));
            let rot = t.rotation();
            let material = materials.len() as u32;
            let mut m = GpuSoftMaterial::new(&spec.material, 0.0, Vec2::new(w, h), 0..0);
            m.params[3] = 1.0;
            materials.push(m);
            let rel = view.relative(anchor);
            self.frame.push(GpuFlag {
                anchor: [rel.x, rel.y, rel.z, self.time],
                rot: rot.to_array(),
                shift: [shift.x, shift.y, shift.z, step],
                wind: [wind.x, wind.y, wind.z, TURBULENCE],
                gravity: [g.x, g.y, g.z, s.damping.max(0.0)],
                size: [w, h, spec.anchor[0], spec.anchor[1]],
                stiff: [
                    link_softness(spec.stretch_compliance, mass, step),
                    link_softness(spec.shear_compliance, mass, step),
                    link_softness(spec.bend_compliance, mass, step),
                    0.5 * AIR_DENSITY / spec.mass_per_m2.max(1e-4),
                ],
                air: [
                    spec.aero_drag.max(0.0),
                    spec.aero_friction.max(0.0),
                    s.max_speed.max(0.1),
                    ((anchor.x * 0.37 + anchor.z * 0.73).rem_euclid(97.0)) as f32,
                ],
                grid: [grid[0], grid[1], first, material],
                pins: [mask, every, substeps, u32::from(fresh)],
                turn: turn.to_array(),
            });
            if probe == Some(*e) {
                self.probe = Some((first, grid, anchor));
            }

            // Cull: the cloth stays within its diagonal of the anchor.
            let radius = (w * w + h * h).sqrt() + 0.1;
            let dist = rel.length();
            let in_view = view.frustum.intersects_sphere(rel, radius)
                && view.projected_radius_px(radius, dist) > 0.3;
            let mut shadows = 0u8;
            for c in 0..view.cascades.count as usize {
                if view.cascades.sphere_in_cascade(c, rel, radius) {
                    shadows |= 1 << c;
                }
            }
            if view.sky_occlusion.enabled && view.sky_occlusion.contains_sphere(rel, radius) {
                shadows |= 1 << SKY_OCCLUSION_VIEW;
            }
            if in_view || shadows != 0 {
                self.draws.push(GpuDraw {
                    indices,
                    base_vertex: first as i32,
                    in_view,
                    transparent: spec.material.alpha < 0.999,
                    shadows,
                });
            }
        }
        // Slots of cloths that are gone (or moved to the CPU) become free.
        let gone: Vec<Entity> = self
            .slots
            .iter()
            .filter(|(_, s)| !s.seen)
            .map(|(e, _)| *e)
            .collect();
        for e in gone {
            if let Some(s) = self.slots.remove(&e) {
                self.free.push((s.first, s.grid[0] * s.grid[1]));
            }
        }
        if self.slots.is_empty() {
            self.free.clear();
            self.used = 0;
        }

        let device = &gpu.device;
        let mut regroup = self
            .flags
            .write(device, &gpu.queue, bytemuck::cast_slice(&self.frame));
        if new_layouts {
            self.indices
                .write(device, &gpu.queue, bytemuck::cast_slice(&self.index_data));
        }
        if self.used > self.capacity {
            let capacity = self.used.next_power_of_two();
            let (state, vertices) = Self::particle_buffers(device, capacity);
            let old = std::mem::replace(&mut self.state, state);
            // Keep the oldest copy if the buffers grew twice before a dispatch.
            if self.grown_from.is_none() {
                self.grown_from = Some((old, self.capacity as u64 * STATE_BYTES));
            }
            self.vertices = vertices;
            self.capacity = capacity;
            regroup = true;
        }
        if regroup {
            self.group = Self::bind_group(
                device,
                &self.layout,
                &self.flags,
                &self.state,
                &self.vertices,
            );
        }
    }

    /// Simulate this frame's cloths.
    pub fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, device: &wgpu::Device) {
        if let Some((old, size)) = self.grown_from.take() {
            encoder.copy_buffer_to_buffer(&old, 0, &self.state, 0, size);
        }
        if self.frame.is_empty() {
            return;
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("softbody gpu cloth"),
                timestamp_writes: self.timer.as_ref().map(Timer::writes),
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(self.frame.len() as u32, 1, 1);
        }
        if let Some(timer) = &self.timer {
            timer.copy(encoder);
        }
        if let Some((first, grid, anchor)) = self.probe.take()
            && self.readback.is_none()
        {
            let size = (grid[0] * grid[1]) as u64 * STATE_BYTES;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("softbody gpu cloth probe"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(
                &self.state,
                first as u64 * STATE_BYTES,
                &buffer,
                0,
                size,
            );
            self.readback = Some(Readback {
                buffer,
                anchor,
                grid,
            });
        }
    }

    /// Finish last frame's probe read-back (blocking) into [`GpuClothProbe`].
    fn read_probe(&mut self, world: &mut World, gpu: &GpuShared) {
        let Some(r) = self.readback.take() else {
            return;
        };
        let slice = r.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        if gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
            || !matches!(rx.recv(), Ok(Ok(())))
        {
            return;
        }
        let positions = match slice.get_mapped_range() {
            Ok(data) => bytemuck::cast_slice::<u8, f32>(&data)
                .as_chunks::<8>()
                .0
                .iter()
                .map(|p| r.anchor + Vec3::new(p[0], p[1], p[2]).as_dvec3())
                .collect(),
            Err(_) => Vec::new(),
        };
        r.buffer.unmap();
        if let Some(mut probe) = world.get_resource_mut::<GpuClothProbe>() {
            probe.positions = positions;
            probe.grid = r.grid;
        }
    }

    /// GPU cloths simulated this frame.
    pub fn count(&self) -> usize {
        self.frame.len()
    }

    /// GPU time of the simulation pass in milliseconds, averaged over recent frames (`None`
    /// without timestamp queries or before the first reading).
    pub fn gpu_ms(&self) -> Option<f32> {
        self.timer.as_ref().and_then(|t| t.ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn the_kernel_validates() {
        validate_wgsl("gpu cloth", &compose_shader(SIM_SHADER, false)).unwrap();
    }

    #[test]
    fn gpu_flag_matches_the_kernel_layout() {
        assert_eq!(std::mem::size_of::<GpuFlag>(), 176);
    }

    #[test]
    fn grids_are_thinned_to_fit_a_workgroup() {
        let mut spec = Cloth::flag([3.0, 2.0], Default::default());
        spec.resolution = [40, 30];
        let [nu, nv] = grid(&spec);
        assert!(nu * nv <= MAX_PARTICLES);
        assert!(nu >= 24 && nv >= 18, "{nu} x {nv}");
        spec.resolution = [16, 10];
        assert_eq!(grid(&spec), [16, 10]);
    }

    #[test]
    fn grid_triangles_match_the_cpu_cloth() {
        let spec = Cloth {
            resolution: [7, 5],
            ..Cloth::flag([1.4, 0.8], Default::default())
        };
        let body = crate::cloth::build(
            &spec,
            Entity::PLACEHOLDER,
            &GlobalTransform::default(),
            None,
            false,
        )
        .unwrap();
        let cpu: Vec<u32> = body
            .surface
            .as_ref()
            .unwrap()
            .triangles
            .iter()
            .flatten()
            .copied()
            .collect();
        assert_eq!(grid_indices(7, 5), cpu);
    }

    #[test]
    fn only_rectangles_with_edge_pins_run_on_the_gpu() {
        let mut flag = Cloth::flag([1.0, 0.6], Default::default());
        assert!(!runs_on_gpu(&flag), "off unless asked");
        flag.sim.gpu = true;
        assert!(runs_on_gpu(&flag));
        let mut caped = flag.clone();
        caped.pins = ClothPins::Bones {
            rows: 2,
            bones: vec!["chest".into()],
        };
        assert!(!runs_on_gpu(&caped));
        let mut sail = Cloth::sail(
            [[0.0, 2.0], [1.0, 2.5], [1.5, 0.2], [0.0, 0.2]],
            Default::default(),
        );
        sail.sim.gpu = true;
        assert!(!runs_on_gpu(&sail));
        assert_eq!(
            pin_mask(&ClothPins::Edges {
                left: true,
                top: false,
                right: true,
                bottom: true
            }),
            Some((13, 1))
        );
    }

    #[test]
    fn stiff_links_close_most_of_their_error_each_substep() {
        let m = 0.15 * 1.0 * 0.6 / 160.0;
        let closes = |compliance| 2.0 / (2.0 + link_softness(compliance, m, 1.0 / 600.0));
        assert!(closes(1e-6) > 0.99);
        let shear = closes(2e-2);
        assert!(shear > 0.2 && shear < 0.5, "{shear}");
    }
}
