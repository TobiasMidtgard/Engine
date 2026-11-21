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
//! * [`tris`]: the triangle records a hit reads of its triangle (uv, the corners' normals,
//!   vertex colour), written on the GPU from the mesh pool's pages as a structure is built.
//! * [`tiles`]: the ray tiles, a layer of a texture array a material's base colour each at
//!   128 texels down to its mean, filled on the GPU from what the raster draws with.
//!
//! The scene exists only on devices with ray queries that take two structures in a stage,
//! and only while [`crate::settings::trial_cut_more::RAY_WORLD`] is on: off, nothing here is
//! made, asked or built.
//!
//! Its bind group (group 1 of every ray pipeline, lib_ray.wgsl): 0 [`RayUniform`], 1 the
//! still structure, 2 the moving one, 3 the rows, 4 triangle records, 5 the ray tiles, 6 the
//! ground map (a stand-in until its step lands; so are the tiles until a first fill makes
//! them). Binding a structure
//! costs the render thread a walk of its instances at submit (0.02 ms a thousand on the
//! RTX 5080), and again whenever a group before it is set, so a frame's ray dispatches run in
//! one compute pass with groups 0 and 1 set once.

mod blas;
mod hook;
pub(crate) mod look;
mod tiles;
mod tlas;
mod tris;

use bevy_ecs::message::{MessageCursor, Messages};
use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_world::WorldShift;

pub(crate) use hook::RayTileSource;
pub use hook::{
    RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RaySphere, RayView,
};

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
/// [`RayView::cut`] (the reaches are settings, `RenderSettings::ray_caster_reach` and
/// `ray_far_reach`), the cut while the valve is open, and [`RayView::ground_patches`].
const CUT: f32 = 0.0175;
const VALVE_CUT: f32 = 0.05;
const GROUND_PATCHES: u32 = 512;
/// Frames the valve holds a step at least before it takes another, so that it cannot flap.
const VALVE_HOLD_FRAMES: u64 = 240;
/// Frames the still set stays past the cap, growing by no more than a 64th of it, before the
/// valve opens (a fill under way passes the cap on its way to its full set, and the set the
/// valve measures what it cut against is the full one), and frames a shut valve waits after a
/// jump of the camera or a change of a reach (the new area's slots may come in before the old
/// area's leave).
const VALVE_SETTLE_FRAMES: u64 = 60;
/// The valve closes once what it cut would fit again with this share of the cap to spare.
const VALVE_CLOSE_SHARE: f32 = 0.95;
/// `RayUniform::stand_ins` w (the scene's flags): cut-out cards are taken where a ray meets a
/// side they are drawn from, their texture unasked (`shade_skip::RAY_LEAVES_SOLID`, a
/// measurement).
const FLAG_LEAVES_SOLID: u32 = 1;
/// Frames between two sweeps for geometries nothing uses any more.
const FORGET_EVERY: u64 = 64;

/// How far the measurement's copies (`RenderSettings::ray_scene_stress`, in this scene or the
/// probes' own) spread from what they copy, meters each way along x and z (512 m across: about
/// what sun shadows and reflections will want of a ray scene), and along y.
pub(crate) const STRESS_REACH: [f32; 3] = [256.0, 3.0, 256.0];

/// Where copy `k` of the measurement's stands, from the instance it copies (meters): the R2
/// sequence over the reach, so any number of copies covers it evenly and none stand in rows.
pub(crate) fn stress_offset(k: u32) -> [f32; 3] {
    // 1/g, 1/g^2, 1/g^3 of the plastic number's three-dimensional cousin (g^4 = g + 1).
    const STEP: [f64; 3] = [
        0.819_172_513_396_164_4,
        0.671_043_606_703_789_2,
        0.549_700_477_901_970_3,
    ];
    let k = f64::from(k) + 1.0;
    [0, 1, 2].map(|axis| {
        (((0.5 + STEP[axis] * k).fract() * 2.0 - 1.0) * f64::from(STRESS_REACH[axis])) as f32
    })
}

/// `render.ray.class_*`, in the order of [`RayClass::ALL`].
const CLASS_ROWS: [&str; 8] = [
    "render.ray.class_solid",
    "render.ray.class_ground",
    "render.ray.class_sway",
    "render.ray.class_leaf",
    "render.ray.class_water",
    "render.ray.class_no_cast",
    "render.ray.class_small",
    "render.ray.class_shadow_only",
];

/// `RayUniform` in lib_ray.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RayUniform {
    /// The anchor, camera-relative.
    anchor: [f32; 4],
    /// Instances in the still structure, in the moving one; the frame; the epoch.
    counts: [u32; 4],
    /// Shadow stand-ins in the still structure, in the moving one; w: the scene's flags
    /// ([`FLAG_LEAVES_SOLID`]).
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
    /// Triangle records in use (a header a geometry with records, a record a triangle), the
    /// records' buffer (MiB), geometries that got none for want of room since the scene was
    /// made, and what the records in use and those the geometries held were refused would
    /// take (MiB).
    pub tris: u32,
    pub tris_mb: f32,
    pub tris_refused: u32,
    pub tris_wanted_mb: f32,
    /// Layers of the ray tiles holding a texture, textures waiting for one (or for new pixels
    /// in theirs), layers filled this frame, layers given to another texture since the scene
    /// was made, the array (MiB; 0 until a first fill makes it).
    pub tiles: u32,
    pub tiles_waiting: u32,
    pub tiles_filled: u32,
    pub tiles_evicted: u32,
    pub tiles_mb: f32,
    pub instances: u32,
    pub still: u32,
    pub moving: u32,
    pub parked: u32,
    /// Shadow stand-ins that hold a slot of either structure.
    pub stand_ins: u32,
    pub refused: u32,
    /// Members of a full structure that gave their slot to a nearer instance since the scene
    /// was made; what belongs in the still structure, with a slot or waiting for one; at the
    /// last ranking of the full still structure, its farthest member and the nearest that
    /// waits (meters, -1: none).
    pub instances_evicted: u32,
    pub still_set: u32,
    pub still_farthest_m: f32,
    pub parked_nearest_m: f32,
    /// Instances of each class ([`RayClass::ALL`]), the measuring fill's copies left out, and
    /// those copies.
    pub classes: [u32; 8],
    pub copies: u32,
    /// The valve is open: the cut is raised while the still set is past the cap.
    pub valve: bool,
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
    tris: tris::Records,
    tiles: tiles::Tiles,
    /// What the group is made of besides the above (made again when the records move to a
    /// larger buffer, and when the tiles are made).
    device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    ground: wgpu::TextureView,
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
    /// The reaches as the features were last told them (caster, far; `None`: not yet).
    reach: Option<[f32; 2]>,
    valve: Valve,
    /// A shut valve waits until this frame: a jump of the camera and a change of a reach
    /// start a fill again ([`VALVE_SETTLE_FRAMES`]).
    settle_until: u64,
}

/// The valve: while what belongs in the still structure is past its cap, once that set has
/// settled ([`VALVE_SETTLE_FRAMES`]), the cut between the reaches rises ([`VALVE_CUT`]), and
/// it falls again once what it cut would fit with room to spare. Each step is held
/// [`VALVE_HOLD_FRAMES`] at least and starts a new epoch. A jump of the camera or a change of
/// a reach shuts it at once, hold or not: what the cut took is measured anew, from the set the
/// new fill settles at.
#[derive(Default, Debug)]
struct Valve {
    open: bool,
    /// The frame of the last step (`None`: none yet).
    stepped: Option<u64>,
    /// While shut: the frame from which the still set has been past the cap without growing
    /// by more than a 64th of it, and the set then.
    over: Option<(u64, u32)>,
    /// The still set when it opened, and how much of it the raised cut took (read once the
    /// first hold since it opened had passed).
    before: u32,
    gain: Option<u32>,
}

impl Valve {
    /// Frame `frame` with `still_set` of the still structure's `cap` belonging in it: true
    /// when the valve took a step. `stress`: the measuring fill is on (it never opens then).
    /// `settling`: the scene is filling again after a jump or a change of a reach (an open
    /// valve shuts, a shut one waits, and the set is measured afresh after).
    fn step(&mut self, still_set: u32, cap: u32, frame: u64, stress: bool, settling: bool) -> bool {
        if self.open && settling {
            // (What it cut was measured in the area before, or is about to be: neither tells
            // what it would cut here.)
            *self = Valve::default();
            return true;
        }
        let held = self
            .stepped
            .is_none_or(|at| frame >= at + VALVE_HOLD_FRAMES);
        if !self.open {
            if stress || settling || still_set <= cap {
                self.over = None;
                return false;
            }
            match self.over {
                Some((since, at)) if still_set <= at + cap / 64 => {
                    if held && frame >= since + VALVE_SETTLE_FRAMES {
                        self.open = true;
                        self.stepped = Some(frame);
                        self.before = still_set;
                        self.gain = None;
                        self.over = None;
                        return true;
                    }
                }
                _ => self.over = Some((frame, still_set)),
            }
            return false;
        }
        if !held {
            return false;
        }
        let gain = *self
            .gain
            .get_or_insert(self.before.saturating_sub(still_set));
        if stress || (still_set + gain) as f32 <= VALVE_CLOSE_SHARE * cap as f32 {
            self.open = false;
            self.stepped = Some(frame);
            return true;
        }
        false
    }
}

/// lib_ray.wgsl with its bindings in group `group` (`RAY_GROUP` in the shader): 1 in the
/// frame's ray pass (`probe_gi::shader_source`). lib_ray reads the frame's `linear_repeat`, so
/// it is composed with lib_frame.
pub(crate) fn shader_lib(group: u32) -> String {
    format!(
        "const RAY_GROUP: u32 = {group}u;\n{}",
        include_str!("../shaders/lib_ray.wgsl")
    )
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
        let tris = tris::Records::new(device);
        // Stand-ins for the tiles until a first fill makes them, and for the ground map until
        // its step lands.
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
        let tiles = tiles::Tiles::new(stand_in(
            "ray scene tiles (none yet)",
            wgpu::TextureViewDimension::D2Array,
        ));
        let ground = stand_in(
            "ray scene ground map (none yet)",
            wgpu::TextureViewDimension::D2,
        );
        let layout = Self::layout(shared);
        let group = Self::make_group(
            device,
            &layout,
            [&uniform, &rows, tris.buffer()],
            &instances,
            [tiles.view(), &ground],
        );
        Self {
            queue: shared.queue.clone(),
            store: Store::new(device),
            instances,
            uniform,
            rows,
            tris,
            tiles,
            device: device.clone(),
            layout,
            ground,
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
            reach: None,
            valve: Valve::default(),
            settle_until: 0,
        }
    }

    /// The scene's bind group of `buffers` (the uniform, the rows, the triangle records), the
    /// two structures and `views` (the tiles, the ground map).
    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        [uniform, rows, tris]: [&wgpu::Buffer; 3],
        instances: &Instances,
        [tiles, ground]: [&wgpu::TextureView; 2],
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray scene"),
            layout,
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
                    resource: wgpu::BindingResource::TextureView(tiles),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(ground),
                },
            ],
        })
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

    /// Start the frame's gather, on the render thread: what the features' hooks are told. A
    /// change of a reach, a step of the valve and a jump of the camera each start a new epoch
    /// (the features look at everything again).
    pub(crate) fn begin(&mut self, view: &ViewData, settings: &crate::RenderSettings) -> RayView {
        self.started = Some(std::time::Instant::now());
        self.frame = view.frame_index + 1;
        self.store.begin_frame(&mut self.tris);
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
        let caster_reach = settings.ray_caster_reach.max(0.0);
        let reach = [caster_reach, settings.ray_far_reach.max(caster_reach)];
        let reach_changed = self.reach.is_some_and(|r| r != reach);
        self.reach = Some(reach);
        if jumped || reach_changed {
            self.settle_until = self.frame + VALVE_SETTLE_FRAMES;
        }
        let stats = self.instances.stats();
        let valve_stepped = self.valve.step(
            stats.still_set,
            STILL_CAPACITY,
            self.frame,
            settings.ray_scene_stress > 0,
            self.frame < self.settle_until,
        );
        if jumped || reach_changed || valve_stepped {
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
            caster_reach: reach[0],
            far_reach: reach[1],
            cut: self.cut(),
            ground_patches: GROUND_PATCHES,
        }
    }

    /// The cut between the reaches ([`RayView::cut`]).
    fn cut(&self) -> f32 {
        if self.valve.open { VALVE_CUT } else { CUT }
    }

    /// The scene for feature number `feature` to fill.
    pub(crate) fn out(&mut self, feature: u16) -> RaySceneOut<'_> {
        RaySceneOut {
            scene: self,
            feature,
        }
    }

    /// End the frame's gather: add the measuring fill's copies (`ray_scene_stress`), settle
    /// what the hooks placed into the two structures and write the rows and the uniform.
    pub(crate) fn finish(&mut self, view: &ViewData, settings: &crate::RenderSettings) {
        for geometry in self.store.take_changed() {
            self.instances.geometry_changed(geometry);
        }
        self.instances.stress(
            &mut self.store,
            settings.ray_scene_stress,
            self.frame,
            |k| Vec3::from(stress_offset(k)).as_dvec3(),
        );
        self.building = self
            .instances
            .settle(&mut self.store, self.frame, view.origin);
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
        let flags = if settings.debug_skip & crate::settings::shade_skip::RAY_LEAVES_SOLID != 0 {
            FLAG_LEAVES_SOLID
        } else {
            0
        };
        let [caster_reach, far_reach] = self.reach.unwrap_or_default();
        let uniform = RayUniform {
            anchor: [anchor.x, anchor.y, anchor.z, 0.0],
            counts: [still, moving, self.frame as u32, self.epoch],
            stand_ins: [still_stand_ins, moving_stand_ins, 0, flags],
            sun: [0.0; 4],
            reach: [caster_reach, far_reach, self.cut(), 0.0],
            lights: [settings.gi_shadowed_lights as f32, 0.0, 0.0, 0.0],
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
    /// structures that changed, in one call, the triangle records of what was built, and the
    /// frame's tile fills. Before any pass that binds the scene, and before
    /// [`group`](Self::group) is asked for.
    pub(crate) fn encode_builds(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let building = std::mem::take(&mut self.building);
        let mut tlas: [Option<&wgpu::Tlas>; 2] = [None; 2];
        for level in [STILL, MOVING] {
            if building & (1 << level) != 0 {
                tlas[level] = Some(self.instances.tlas(level));
            }
        }
        let tlas: Vec<&wgpu::Tlas> = tlas.into_iter().flatten().collect();
        let regroup = self.store.encode(encoder, &tlas, building, &mut self.tris);
        self.instances.built(&mut self.store, self.frame);
        // The textures this frame fills, the nearest asks first (a layer goes to another
        // texture only once no row has named it for a while).
        let instances = &self.instances;
        let made = self.tiles.encode(
            &self.device,
            encoder,
            |layer| instances.tile_users(layer),
            self.frame,
        );
        if regroup || made {
            self.group = Self::make_group(
                &self.device,
                &self.layout,
                [&self.uniform, &self.rows, self.tris.buffer()],
                &self.instances,
                [self.tiles.view(), &self.ground],
            );
        }
    }

    /// After the frame's submit (`failed`: wgpu rejected it).
    pub(crate) fn after_submit(&mut self, failed: bool) {
        self.store.after_submit(&self.queue, failed, self.frame);
    }

    pub(crate) fn stats(&self) -> RayStats {
        let s = self.store.stats();
        let i = self.instances.stats();
        let (tris, tris_mb, tris_refused, tris_wanted_mb) =
            self.tris.stats(self.store.missing_records());
        let tiles = self.tiles.stats();
        RayStats {
            geometries: s.geometries,
            triangles: s.held.triangles,
            over_cap: s.held.over_cap,
            over_cap_triangles: s.held.over_cap_triangles,
            copied: s.held.copied,
            copied_triangles: s.held.copied_triangles,
            blas_mb: s.blas_bytes.map(|b| b as f64 / (1u64 << 20) as f64),
            tris,
            tris_mb,
            tris_refused,
            tris_wanted_mb,
            tiles: tiles.layers,
            tiles_waiting: tiles.waiting,
            tiles_filled: tiles.filled,
            tiles_evicted: tiles.evicted,
            tiles_mb: tiles.mb,
            instances: self.instances.len() as u32,
            still: i.still,
            moving: i.moving,
            parked: i.parked,
            stand_ins: self.instances.stand_ins().iter().sum(),
            refused: i.refused,
            instances_evicted: i.evicted,
            still_set: i.still_set,
            still_farthest_m: i.still_farthest_m,
            parked_nearest_m: i.parked_nearest_m,
            classes: i.classes,
            copies: self.instances.copies(),
            valve: self.valve.open,
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
        d.set("render.ray.tris", s.tris as f64);
        d.set("render.ray.tris_mb", s.tris_mb as f64);
        d.set("render.ray.tris_refused", s.tris_refused as f64);
        d.set("render.ray.tris_wanted_mb", s.tris_wanted_mb as f64);
        d.set("render.ray.tiles", s.tiles as f64);
        d.set("render.ray.tiles_waiting", s.tiles_waiting as f64);
        d.set("render.ray.tiles_filled", s.tiles_filled as f64);
        d.set("render.ray.tiles_evicted", s.tiles_evicted as f64);
        d.set("render.ray.tiles_mb", s.tiles_mb as f64);
        d.set("render.ray.instances", s.instances as f64);
        d.set("render.ray.still_instances", s.still as f64);
        d.set("render.ray.moving_instances", s.moving as f64);
        d.set("render.ray.parked", s.parked as f64);
        d.set("render.ray.stand_ins", s.stand_ins as f64);
        d.set("render.ray.refused", s.refused as f64);
        d.set("render.ray.instances_evicted", s.instances_evicted as f64);
        d.set("render.ray.still_set", s.still_set as f64);
        d.set("render.ray.still_farthest_m", s.still_farthest_m as f64);
        d.set("render.ray.parked_nearest_m", s.parked_nearest_m as f64);
        for (row, count) in CLASS_ROWS.iter().zip(s.classes) {
            d.set(row, count as f64);
        }
        d.set("render.ray.copies", s.copies as f64);
        d.set("render.ray.valve", s.valve as u8 as f64);
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

    /// `pool`: `source` is the mesh pool's (`PoolVertex`, `u32` indices), and the scene
    /// writes the geometry's triangle records too.
    fn geometry(
        &mut self,
        feature: u16,
        key: RayKey,
        version: u64,
        opaque: bool,
        source: RaySource<'_>,
        pool: bool,
    ) -> RayGeometryState {
        let records = pool.then_some(&mut self.tris);
        self.store
            .want((feature, key), version, opaque, source, self.frame, records)
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
        sphere: RaySphere,
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
            sphere,
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

    fn tile(
        &mut self,
        feature: u16,
        key: RayKey,
        distance: f32,
        source: impl FnOnce() -> Option<RayTileSource>,
    ) -> Option<u16> {
        self.tiles.ask((feature, key), distance, self.frame, source)
    }

    fn is_live(&self, feature: u16, id: RayKey) -> bool {
        self.instances.is_live((feature, id))
    }

    fn keep(&mut self, feature: u16, id: RayKey) -> bool {
        self.instances.keep((feature, id), self.frame)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The valve opens once what belongs in the still structure has been past its cap for a
    /// while without growing (a fill under way passes it), holds each step, and closes once
    /// what it cut would fit again with room to spare; the measuring fill closes it, and it
    /// never opens while that runs, nor while the scene settles after a jump.
    #[test]
    fn the_valve_cuts_far_things_while_the_still_structure_is_over_its_cap() {
        const HOLD: u64 = VALVE_HOLD_FRAMES;
        const SETTLE: u64 = VALVE_SETTLE_FRAMES;
        let mut valve = Valve::default();
        assert!(!valve.step(90, 100, 0, false, false));
        // Past the cap while still growing, then settled at 120 from frame 2.
        assert!(!valve.step(110, 100, 1, false, false));
        assert!(!valve.step(120, 100, 2, false, false));
        assert!(!valve.step(120, 100, 1 + SETTLE, false, false));
        assert!(valve.step(120, 100, 2 + SETTLE, false, false));
        assert!(valve.open);
        let opened = 2 + SETTLE;
        // Each step is held.
        assert!(!valve.step(10, 100, opened + 1, false, false));
        assert!(!valve.step(10, 100, opened + HOLD - 1, false, false));
        // The first look past the hold measures what the cut took (120 - 90): 90 and that
        // would not fit with room to spare.
        assert!(!valve.step(90, 100, opened + HOLD, false, false));
        assert!(!valve.step(66, 100, opened + HOLD + 1, false, false));
        assert!(valve.open);
        assert!(valve.step(64, 100, opened + HOLD + 2, false, false));
        assert!(!valve.open);
        let closed = opened + HOLD + 2;
        // Over its cap again at once: held first (the set settles meanwhile, and the hold
        // goes on past it).
        assert!(!valve.step(120, 100, closed + 1, false, false));
        assert!(!valve.step(120, 100, closed + SETTLE + 1, false, false));
        assert!(!valve.step(120, 100, closed + HOLD - 1, false, false));
        assert!(valve.step(120, 100, closed + HOLD, false, false));
        let reopened = closed + HOLD;
        assert!(valve.step(120, 100, reopened + HOLD, true, false));
        assert!(!valve.open);
        assert!(!valve.step(500, 100, reopened + 3 * HOLD, true, false));
        // A set that falls back under the cap before it settles opens nothing.
        let mut valve = Valve::default();
        assert!(!valve.step(120, 100, 0, false, false));
        assert!(!valve.step(99, 100, SETTLE / 2, false, false));
        assert!(!valve.step(120, 100, SETTLE, false, false));
        assert!(!valve.step(120, 100, SETTLE + SETTLE / 2, false, false));
        assert!(valve.step(120, 100, 2 * SETTLE, false, false));
        // While the scene settles after a jump the valve waits, and it measures the set
        // afresh after.
        let mut valve = Valve::default();
        assert!(!valve.step(120, 100, 0, false, true));
        assert!(!valve.step(120, 100, SETTLE, false, true));
        assert!(!valve.step(120, 100, SETTLE + 1, false, false));
        assert!(!valve.step(120, 100, 2 * SETTLE, false, false));
        assert!(valve.step(120, 100, 2 * SETTLE + 1, false, false));
        // A jump while it is open shuts it at once, within its hold, and it may open again
        // once the new set has settled past the cap (no hold after such a shut).
        let opened = 2 * SETTLE + 1;
        assert!(valve.step(30, 100, opened + 10, false, true));
        assert!(!valve.open);
        assert!(!valve.step(120, 100, opened + 11, false, true));
        assert!(!valve.step(120, 100, opened + 10 + SETTLE, false, false));
        assert!(valve.step(120, 100, opened + 10 + 2 * SETTLE, false, false));
    }

    #[test]
    fn the_ray_library_takes_its_group_where_it_is_composed() {
        for group in [1u32, 3] {
            let src = format!(
                "{}\n@compute @workgroup_size(1) fn main() {{ _ = ray_scene.counts; }}",
                shader_lib(group)
            );
            let code = crate::features::compose_shader(&src, true);
            crate::features::validate_wgsl("lib_ray", &code).unwrap();
            let module = wgpu::naga::front::wgsl::parse_str(&code).unwrap();
            let at = module
                .global_variables
                .iter()
                .find(|(_, v)| v.name.as_deref() == Some("ray_scene"))
                .and_then(|(_, v)| v.binding.as_ref().map(|b| b.group));
            assert_eq!(at, Some(group));
        }
    }
}
