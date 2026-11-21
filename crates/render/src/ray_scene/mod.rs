//! The ray scene: what every ray of the frame (probe light, world reflections, sun shadows)
//! traces on ray-tracing hardware. One scene for all of them, filled by the features through
//! [`crate::RenderFeature::ray_scene`] on the render thread and bound once a frame.
//!
//! * [`hook`]: what a feature says: its geometries (triangles in its own GPU buffers, or on
//!   the CPU) and its instances (a geometry, a place, a class, a look).
//! * [`blas`]: a bottom-level structure per geometry, built within a frame's allowance,
//!   compacted, and kept until the GPU is past the frames that trace it.
//! * [`tlas`]: two top-level structures, one for what holds still (built again every fourth
//!   frame at most) and a small one for what moves (built again when a member changed), and
//!   the row a hit reads of its instance.
//!
//! The scene exists only on devices with ray queries that take two structures in a stage,
//! and only while [`crate::settings::trial_cut_more::RAY_WORLD`] is on: off, nothing here is
//! made, asked or built.
//!
//! Its bind group (group 1 of every ray pipeline, lib_ray.wgsl): 0 [`RayUniform`], 1 the
//! still structure, 2 the moving one, 3 the rows, 4 triangle records, 5 the ray tiles, 6 the
//! ground map (the last three are stand-ins until their steps land). Binding a structure
//! costs the render thread a walk of its instances at submit (0.02 ms a thousand on the
//! RTX 5080), and again whenever a group before it is set, so a frame's ray dispatches run in
//! one compute pass with groups 0 and 1 set once.

mod blas;
mod hook;
pub(crate) mod look;
mod tlas;

use bevy_ecs::message::{MessageCursor, Messages};
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_world::WorldShift;

pub use hook::{RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RayView};

use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::renderer::layout_entry;
use blas::{MOVING, STILL, Store};
use tlas::{Instances, MOVING_CAPACITY, RayRow, STILL_CAPACITY};

/// The anchor (the world position the structures' instances are placed around, so that their
/// `f32` placements stay exact) is put on a grid of this many meters when it moves; a world
/// shift carries it along, off the grid, until it next moves.
const ANCHOR_CELL: f64 = 64.0;
/// The anchor moves when the eye is farther from it than this along an axis (meters): both
/// structures are built again then, so it must be rare.
const ANCHOR_REACH: f64 = 256.0;
/// A camera that moved farther than this in a frame jumped (meters): [`RayView::epoch`]
/// changes, so features look at everything again at once.
const JUMP_M: f64 = 32.0;
/// [`RayView::caster_reach`], [`RayView::far_reach`], [`RayView::cut`],
/// [`RayView::ground_patches`].
const CASTER_REACH_M: f32 = 160.0;
const FAR_REACH_M: f32 = 384.0;
const CUT: f32 = 0.0175;
const GROUND_PATCHES: u32 = 512;
/// Frames between two sweeps for geometries nothing uses any more.
const FORGET_EVERY: u64 = 64;

/// `RayUniform` in lib_ray.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RayUniform {
    /// The anchor, camera-relative.
    anchor: [f32; 4],
    /// Instances in the still structure, in the moving one; the frame; the epoch.
    counts: [u32; 4],
    /// Shadow stand-ins in the still structure, in the moving one.
    stand_ins: [u32; 4],
    /// The sun's rays: reach of the pixels that get one, length (meters). Zeros until the
    /// sun's own rays read them (the measuring rays have a length of their own).
    sun: [f32; 4],
    /// Caster reach, far reach (meters), the cut.
    reach: [f32; 4],
    /// x: point lights a hit asks for their shadow, at most.
    lights: [f32; 4],
    /// The ground map: its center (camera-relative x, z), half width; w: whether it is drawn.
    ground: [f32; 4],
}

/// `render.ray.*`: what the scene holds and what the frame did to it.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct RayStats {
    pub geometries: u32,
    pub triangles: u64,
    /// Geometries built from a feature's buffers with more triangles than a mesh's ray level
    /// should have (60,000), and their triangles; geometries built from a copy on the CPU,
    /// and theirs.
    pub over_cap: u32,
    pub over_cap_triangles: u64,
    pub copied: u32,
    pub copied_triangles: u64,
    /// GPU memory under the bottom-level structures, MiB (`None`: not read yet).
    pub blas_mb: Option<f64>,
    pub instances: u32,
    pub still: u32,
    pub moving: u32,
    pub parked: u32,
    /// Shadow stand-ins that hold a slot of either structure.
    pub stand_ins: u32,
    pub refused: u32,
    pub built: u32,
    pub built_triangles: u32,
    pub compacted: u32,
    pub fences: u32,
    pub retired: u32,
    /// The structures built this frame (bit 0 the still one, bit 1 the moving one).
    pub structures_built: u8,
    pub slots_set: u32,
    /// Render-thread time of the features' hooks and the scene's own work (ms).
    pub gather_ms: f32,
    pub anchor_moves: u32,
    pub epoch: u32,
}

/// The frame's ray scene.
pub(crate) struct RayScene {
    queue: wgpu::Queue,
    store: Store,
    instances: Instances,
    uniform: wgpu::Buffer,
    rows: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// This frame (`ViewData::frame_index + 1`, as the mesh scene counts: 0 is never).
    frame: u64,
    epoch: u32,
    /// The camera last frame.
    eye: Option<DVec3>,
    shifts: MessageCursor<WorldShift>,
    /// The world shifts read since the last gather, as one.
    shift: Option<WorldShift>,
    /// The frame of the last world shift the scene followed (0: none).
    shifted: u64,
    /// The structures this frame builds.
    building: u8,
    started: Option<std::time::Instant>,
    gather_ms: f32,
    anchor_moves: u32,
}

/// Whether `settings` ask for the ray scene on this device: its switch, with probe GI on
/// ray-tracing hardware, where the device can hold it, unless the renderer gave it up for
/// the run (lost too often, `GpuShared::ray_scene_given_up`). The renderer makes the scene
/// by this, and the features that fill it prepare for it by this.
pub(crate) fn wanted(settings: &crate::RenderSettings, shared: &GpuShared) -> bool {
    settings.trial_cuts_more & crate::settings::trial_cut_more::RAY_WORLD != 0
        && settings.probe_gi
        && settings.gi_hardware_rays
        && RayScene::supported(shared)
        && !shared.ray_scene_given_up()
}

impl RayScene {
    /// Whether the device can hold the scene: ray queries with the hit's corners, and two
    /// structures in one shader stage (the least a device with ray queries must take is one).
    pub(crate) fn supported(shared: &GpuShared) -> bool {
        shared.ray_queries()
            && shared
                .device
                .limits()
                .max_acceleration_structures_per_shader_stage
                >= 2
    }

    /// The layout of the scene's bind group (the same object for every caller).
    pub(crate) fn layout(shared: &GpuShared) -> wgpu::BindGroupLayout {
        let stages = wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT;
        let structure = wgpu::BindingType::AccelerationStructure {
            vertex_return: true,
        };
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let read_only = buffer(wgpu::BufferBindingType::Storage { read_only: true });
        let texture = |view_dimension| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension,
            multisampled: false,
        };
        shared.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray scene"),
            entries: &[
                layout_entry(0, stages, buffer(wgpu::BufferBindingType::Uniform)),
                layout_entry(1, stages, structure),
                layout_entry(2, stages, structure),
                layout_entry(3, stages, read_only),
                layout_entry(4, stages, read_only),
                layout_entry(5, stages, texture(wgpu::TextureViewDimension::D2Array)),
                layout_entry(6, stages, texture(wgpu::TextureViewDimension::D2)),
            ],
        })
    }

    /// An empty scene. `epoch`: past every epoch an earlier scene told the features
    /// ([`epoch`](Self::epoch)), so that they start over.
    pub(crate) fn new(shared: &GpuShared, epoch: u32) -> Self {
        let device = &shared.device;
        let instances = Instances::new(device);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray scene uniform"),
            size: std::mem::size_of::<RayUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rows = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray scene rows"),
            size: (STILL_CAPACITY + MOVING_CAPACITY) as u64 * std::mem::size_of::<RayRow>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Stand-ins for what later steps bind: triangle records, the tiles, the ground map.
        let tris = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray scene triangle records (none yet)"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let stand_in = |label, dimension| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(dimension),
                    ..Default::default()
                })
        };
        let tiles = stand_in(
            "ray scene tiles (none yet)",
            wgpu::TextureViewDimension::D2Array,
        );
        let ground = stand_in(
            "ray scene ground map (none yet)",
            wgpu::TextureViewDimension::D2,
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray scene"),
            layout: &Self::layout(shared),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instances.tlas(STILL).as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: instances.tlas(MOVING).as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: rows.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: tris.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&tiles),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&ground),
                },
            ],
        });
        Self {
            queue: shared.queue.clone(),
            store: Store::new(device),
            instances,
            uniform,
            rows,
            group,
            frame: 0,
            epoch,
            eye: None,
            shifts: MessageCursor::default(),
            shift: None,
            shifted: 0,
            building: 0,
            started: None,
            gather_ms: 0.0,
            anchor_moves: 0,
        }
    }

    /// The scene's bind group (group 1 of the ray pipelines). Valid once a frame's builds
    /// were recorded ([`encode_builds`](Self::encode_builds)): a submit that binds a
    /// structure never built fails.
    pub(crate) fn group(&self) -> &wgpu::BindGroup {
        &self.group
    }

    /// A submission holding the scene's builds failed: what the GPU has of it is unknown.
    /// The renderer drops the scene and starts another.
    pub(crate) fn lost(&self) -> bool {
        self.store.lost()
    }

    /// The epoch the features were last told ([`RayView::epoch`]).
    pub(crate) fn epoch(&self) -> u32 {
        self.epoch
    }

    /// On the main thread, in the extract: read the frame's world shifts (the gather follows
    /// them), and swap in the structures whose compaction is ready. Only while no other
    /// thread submits (see `Store::swap_compacted`).
    pub(crate) fn extract(&mut self, world: &World) {
        if let Some(messages) = world.get_resource::<Messages<WorldShift>>() {
            let read = self
                .shifts
                .read(messages)
                .copied()
                .reduce(|acc, next| acc.then(&next));
            self.shift = match (self.shift.take(), read) {
                (Some(before), Some(now)) => Some(before.then(&now)),
                (before, now) => before.or(now),
            };
        }
        self.store.swap_compacted(&self.queue);
    }

    /// Start the frame's gather, on the render thread: what the features' hooks are told.
    pub(crate) fn begin(&mut self, view: &ViewData) -> RayView {
        self.started = Some(std::time::Instant::now());
        self.frame = view.frame_index + 1;
        self.store.begin_frame();
        self.instances.begin_frame();
        if let Some(shift) = self.shift.take() {
            // The world moved to a new frame (a floating-origin move, a jump that re-anchors
            // it): every placement moves with it, so that what the features place next is
            // where the scene has it already and stays in its structure.
            let affine = DAffine3::from_rotation_translation(shift.rotation, shift.translation);
            self.instances
                .shift(&affine, shift.is_translation_only(), self.frame);
            self.eye = self.eye.map(|eye| affine.transform_point3(eye));
            self.shifted = self.frame;
        }
        let eye = view.origin;
        let jumped = self.eye.is_none_or(|last| last.distance(eye) > JUMP_M);
        if jumped {
            self.epoch = self.epoch.wrapping_add(1);
        }
        let first = self.eye.is_none();
        self.eye = Some(eye);
        let off = (eye - self.instances.anchor()).abs().max_element();
        if first || off > ANCHOR_REACH {
            self.instances
                .set_anchor((eye / ANCHOR_CELL).round() * ANCHOR_CELL);
            self.anchor_moves += 1;
        }
        RayView {
            eye,
            frame: self.frame,
            epoch: self.epoch,
            shifted: self.shifted,
            caster_reach: CASTER_REACH_M,
            far_reach: FAR_REACH_M,
            cut: CUT,
            ground_patches: GROUND_PATCHES,
        }
    }

    /// The scene for feature number `feature` to fill.
    pub(crate) fn out(&mut self, feature: u16) -> RaySceneOut<'_> {
        RaySceneOut {
            scene: self,
            feature,
        }
    }

    /// End the frame's gather: settle what the hooks placed into the two structures and
    /// write the rows and the uniform. `lights`: point lights a hit may ask for their shadow.
    pub(crate) fn finish(&mut self, view: &ViewData, lights: u32) {
        for geometry in self.store.take_changed() {
            self.instances.geometry_changed(geometry);
        }
        self.building = self.instances.settle(&mut self.store, self.frame);
        for (first, rows) in self.instances.take_changed_rows() {
            self.queue.write_buffer(
                &self.rows,
                first as u64 * std::mem::size_of::<RayRow>() as u64,
                bytemuck::cast_slice(rows),
            );
        }
        let anchor = (self.instances.anchor() - view.origin).as_vec3();
        let [still, moving] = self.instances.counts();
        let [still_stand_ins, moving_stand_ins] = self.instances.stand_ins();
        let uniform = RayUniform {
            anchor: [anchor.x, anchor.y, anchor.z, 0.0],
            counts: [still, moving, self.frame as u32, self.epoch],
            stand_ins: [still_stand_ins, moving_stand_ins, 0, 0],
            sun: [0.0; 4],
            reach: [CASTER_REACH_M, FAR_REACH_M, CUT, 0.0],
            lights: [lights as f32, 0.0, 0.0, 0.0],
            ground: [0.0; 4],
        };
        self.queue
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        if self.frame.is_multiple_of(FORGET_EVERY) {
            self.store.forget_unused(self.frame);
        }
        if let Some(started) = self.started.take() {
            self.gather_ms = started.elapsed().as_secs_f32() * 1e3;
        }
    }

    /// Record the frame's builds: the geometries' new structures and the top-level
    /// structures that changed, in one call. Before any pass that binds the scene.
    pub(crate) fn encode_builds(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let building = std::mem::take(&mut self.building);
        let mut tlas: [Option<&wgpu::Tlas>; 2] = [None; 2];
        for level in [STILL, MOVING] {
            if building & (1 << level) != 0 {
                tlas[level] = Some(self.instances.tlas(level));
            }
        }
        let tlas: Vec<&wgpu::Tlas> = tlas.into_iter().flatten().collect();
        self.store.encode(encoder, &tlas, building);
        self.instances.built(&mut self.store, self.frame);
    }

    /// After the frame's submit (`failed`: wgpu rejected it).
    pub(crate) fn after_submit(&mut self, failed: bool) {
        self.store.after_submit(&self.queue, failed, self.frame);
    }

    pub(crate) fn stats(&self) -> RayStats {
        let s = self.store.stats();
        let i = self.instances.stats();
        RayStats {
            geometries: s.geometries,
            triangles: s.held.triangles,
            over_cap: s.held.over_cap,
            over_cap_triangles: s.held.over_cap_triangles,
            copied: s.held.copied,
            copied_triangles: s.held.copied_triangles,
            blas_mb: s.blas_bytes.map(|b| b as f64 / (1u64 << 20) as f64),
            instances: self.instances.len() as u32,
            still: i.still,
            moving: i.moving,
            parked: i.parked,
            stand_ins: self.instances.stand_ins().iter().sum(),
            refused: i.refused,
            built: s.built,
            built_triangles: s.built_triangles,
            compacted: s.compacted,
            fences: s.fences,
            retired: s.retired,
            structures_built: i.built,
            slots_set: i.set,
            gather_ms: self.gather_ms,
            anchor_moves: self.anchor_moves,
            epoch: self.epoch,
        }
    }

    /// The scene's numbers, as `render.ray.*`.
    pub(crate) fn publish(&self, d: &mut Diagnostics) {
        Self::publish_stats(&self.stats(), d);
    }

    /// The rows of a scene that is gone: zeros (its last numbers would stand otherwise,
    /// beside `render.ray.on` 0).
    pub(crate) fn publish_none(d: &mut Diagnostics) {
        Self::publish_stats(
            &RayStats {
                blas_mb: Some(0.0),
                ..Default::default()
            },
            d,
        );
    }

    fn publish_stats(s: &RayStats, d: &mut Diagnostics) {
        d.set("render.ray.geometries", s.geometries as f64);
        d.set("render.ray.triangles", s.triangles as f64);
        d.set("render.ray.over_cap", s.over_cap as f64);
        d.set("render.ray.over_cap_triangles", s.over_cap_triangles as f64);
        d.set("render.ray.copied", s.copied as f64);
        d.set("render.ray.copied_triangles", s.copied_triangles as f64);
        // (Only once read: a row of zeros would read as a scene without structures.)
        if let Some(mb) = s.blas_mb {
            d.set("render.ray.blas_mb", mb);
        }
        d.set("render.ray.instances", s.instances as f64);
        d.set("render.ray.still_instances", s.still as f64);
        d.set("render.ray.moving_instances", s.moving as f64);
        d.set("render.ray.parked", s.parked as f64);
        d.set("render.ray.stand_ins", s.stand_ins as f64);
        d.set("render.ray.refused", s.refused as f64);
        d.set("render.ray.built", s.built as f64);
        d.set("render.ray.built_triangles", s.built_triangles as f64);
        d.set("render.ray.compacted", s.compacted as f64);
        d.set("render.ray.fences", s.fences as f64);
        d.set("render.ray.retired", s.retired as f64);
        d.set("render.ray.structures_built", s.structures_built as f64);
        d.set("render.ray.slots_set", s.slots_set as f64);
        d.set("render.ray.ms_gather", s.gather_ms as f64);
        d.set("render.ray.anchor_moves", s.anchor_moves as f64);
        d.set("render.ray.epoch", s.epoch as f64);
    }

    // What the hook calls (`RaySceneOut`), each with the feature's number first.

    fn geometry(
        &mut self,
        feature: u16,
        key: RayKey,
        version: u64,
        opaque: bool,
        source: RaySource<'_>,
    ) -> RayGeometryState {
        self.store
            .want((feature, key), version, opaque, source, self.frame)
    }

    fn has_geometry(&mut self, feature: u16, key: RayKey, version: u64) -> bool {
        self.store.has((feature, key), version, self.frame)
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        feature: u16,
        id: RayKey,
        geometry: RayKey,
        to_world: &DAffine3,
        class: RayClass,
        still_from: u64,
        look: &RayLook,
    ) -> bool {
        let Some(geometry) = self.store.index_of((feature, geometry)) else {
            return false;
        };
        self.instances.place(
            &mut self.store,
            (feature, id),
            geometry,
            to_world,
            class,
            still_from,
            RayRow::new(class, look),
            self.frame,
        )
    }

    fn remove(&mut self, feature: u16, id: RayKey) {
        self.instances
            .remove(&mut self.store, (feature, id), self.frame);
    }

    fn is_live(&self, feature: u16, id: RayKey) -> bool {
        self.instances.is_live((feature, id))
    }

    fn has(&self, feature: u16, id: RayKey) -> bool {
        self.instances.has((feature, id))
    }

    fn remove_unseen(&mut self, feature: u16, since: u64) {
        self.instances
            .remove_unseen(&mut self.store, feature, since, self.frame);
    }

    fn geometry_version(&self, feature: u16, key: RayKey) -> Option<u64> {
        let index = self.store.index_of((feature, key))?;
        self.store.get(index).map(|g| g.version)
    }

    fn geometry_users(&self, feature: u16, key: RayKey) -> u32 {
        self.store.users((feature, key))
    }
}
