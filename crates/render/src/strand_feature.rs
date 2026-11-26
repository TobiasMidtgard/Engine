//! The strand renderer (characters week R2): draws every [`StrandInstance`] with the shape of
//! the 2023-2025 shipped hair renderers (see `shaders/strands_raster.wgsl`).
//!
//! A frame, in the renderer's order:
//!
//! 1. `prepare` copies the instances out of the world: transform, level of detail, guides
//!    (those of the instances it draws: a [`StrandGuidesWhenDrawn`] is asked only for them).
//!    A groom the frame draws that the GPU does not hold is packed here ([`pack_groom`]: its
//!    strands in [`spread_order`], so any prefix of them spreads over the whole groom and a
//!    level of detail is a prefix), within the frame's upload allowance. A groom nobody
//!    draws is never packed or sent.
//! 2. `prepare_frame` sends the packed grooms into free room in the shared groom arrays (no
//!    other groom is sent again when one comes or goes), then the instances and the guides'
//!    offsets from rest.
//! 3. `compute`: every drawn point posed by its guides (`strands_pose.wgsl`), its fibre added to
//!    its instance's density volume (64³ cells over the groom).
//! 4. `draw_shadow`/`draw_inset_shadow`: the strands as hashed ribbons into the sun's maps.
//! 5. `before_scene` (the prepass, ambient occlusion and fog are ready): the farthest opaque
//!    depth of every 8x8 tile; every point placed on screen; the points a pixel can show shaded
//!    once (Marschner/Karis fibre model, the shadow maps, the light through the hair from the
//!    density volume, capsule occlusion, sky, point lights, fog), which leaves out whatever
//!    stands behind a hill or a wall and the far side of a coat whose body fills whole tiles;
//!    the segments counted per tile (those in front of something in it), summed, the tiles
//!    listed and filled; each tile rasterized in compute with box-filtered coverage, four
//!    nearest fibres per pixel and the rest as one, tested against the prepass depth; then the
//!    hair's motion over the prepass's where it covers a pixel.
//! 6. `draw(Transparent)`: the hair over the scene, premultiplied, a quad per touched tile.
//!
//! Levels of detail ([`strands_wanted`]): up close every strand; further out a share of them,
//! each wider by the inverse share so the hair keeps its cover; far away as many as its pixels
//! can show apart, and enough of them that none is drawn wider than a pixel (a coat of 200 000
//! hairs twenty pixels tall draws a thousand, not twelve thousand); past a few pixels nothing.
//! The number follows the size on screen without steps: the last strand drawn comes in by
//! degrees. `StrandLod::Cards` and `Shells` draw strands for now (cards and shells come later);
//! `Hidden` draws nothing. Extra views (editor splits, portals, mirrors) draw no hair yet.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};
use engine_assets::Assets;
use engine_core::math::{Mat3, Vec3};
use engine_core::prelude::*;
use engine_world::HairGuidesPosed;

use crate::features::{FeatureStats, FrameContext, GpuShared, RenderFeature, RenderPhase};
use crate::frame::ViewData;
use crate::settings::{RenderSettings, shade_skip};
use crate::strands::{
    StrandBodyWhenDrawn, StrandBox, StrandCapsule, StrandFiber, StrandGroom, StrandGuideState,
    StrandGuidesWhenDrawn, StrandInstance, StrandKeptOutWhenDrawn, StrandLod, StrandStateWhenDrawn,
};

const COMMON: &str = include_str!("shaders/strands_common.wgsl");
const POSE: &str = include_str!("shaders/strands_pose.wgsl");
const RASTER: &str = include_str!("shaders/strands_raster.wgsl");
const COMPOSITE: &str = include_str!("shaders/strands_composite.wgsl");
const SHADOW: &str = include_str!("shaders/strands_shadow.wgsl");

/// Instances drawn at once, nearest first.
pub(crate) const MAX_INSTANCES: usize = 32;
/// Shapes one instance's strands are kept out of ([`StrandKeptOutWhenDrawn`]).
const MAX_CAPSULES: usize = 32;
/// Boxes of a measured body one instance's strands are kept out of ([`StrandBodyWhenDrawn`]),
/// and the entries each takes ahead of the instance's guide offsets (`strands_pose.wgsl`).
const MAX_BOXES: usize = 12;
const BOX_ENTRIES: usize = 6;
/// A guide's first points whose last entries carry its state (`strands_pose.wgsl`).
const STATE_POINTS: usize = 3;
/// A body's samples nobody drew for this many frames leave the GPU.
const BODY_KEPT_FRAMES: u64 = 600;
/// Cells along each side of an instance's density volume.
pub(crate) const VOXELS: u32 = 64;
/// Points drawn in a frame; past it every instance draws a smaller share.
pub(crate) const MAX_POINTS: usize = 2 << 20;
/// Tile side (px), as `STRAND_TILE` in strands_common.wgsl.
const TILE: u32 = 8;
const WORKGROUP: u32 = 256;
/// Segment references per tile list, at most and at least.
const MAX_REFS: u64 = 16 << 20;
const MIN_REFS: u64 = 1 << 20;
/// Readbacks of the references a frame wanted in flight at once.
const REF_READBACKS: usize = 3;
/// A groom smaller than this on screen (its bounds' height, px) is not drawn; at
/// [`FULL_PX`] and above every strand is.
const MIN_PX: f32 = 3.0;
const FULL_PX: f32 = 500.0;
/// The share of strands the near rule draws never goes below this (wider strands make up the
/// cover), and an instance's density volume is filled from at least this share of its groom
/// however few strands it draws.
const MIN_SHARE: f32 = 1.0 / 16.0;
/// Far away a groom draws this many strands for every pixel of the square its bounds are
/// tall: enough that every pixel of it blends several (with one a pixel a deer's coat a
/// hundred pixels tall is twice as grainy as with the near rule's five times as many, and its
/// skin shows between them; with four it is as smooth)...
const STRANDS_PER_PX2: f32 = 4.0;
/// (...and with `shade_skip::STRANDS_SPARSE` this many, for the PC to tell what the finer
/// number costs.)
const SPARSE_PER_PX2: f32 = 1.0;
/// ...and enough of them that its thickest fibre, widened to keep the cover, stays within
/// this half width (px): no strand is drawn wider than a pixel...
const MAX_WIDE_HALF_PX: f32 = 0.5;
/// ...and never fewer than this.
const MIN_STRANDS: f32 = 16.0;
/// Strand slots an instance takes for the strands it draws by its pixels, and the most it
/// keeps before it gives some back (both times the strands drawn): room for the number to
/// drift with the size on screen while every strand's slot, and its history, stays put.
const SLOT_ROOM: f32 = 1.25;
const SLOT_KEEP: f32 = 1.6;
/// How opaque one fibre is across its width: hair, fur.
const OPACITY: [f32; 2] = [0.95, 0.9];
/// Strands cast into the cascades: one in this many, as much wider.
const CASCADE_SHARE: u32 = 4;
/// Grooms nobody drew in this many frames leave the GPU, the longest unseen first, while the
/// grooms held that nobody draws are more than [`KEEP_POINTS`].
const EVICT_FRAMES: u64 = 600;
/// Points of grooms the GPU keeps although nobody draws them, beside those it draws: animals
/// that walk out of view and back are not sent again (each return was a coat packed and 19 MB
/// written, and fur missing until its turn came). 16 bytes a point and 32 a strand: 770 MB of
/// short coats at the most, where the showcase's 30 coats are 24 M points, 580 MB. Where the
/// device's largest buffer holds less, what nobody draws leaves as soon as a groom in view
/// needs its room ([`StrandFeature::grooms_in_the_way`]).
const KEEP_POINTS: u32 = 32 << 20;
/// The shared groom arrays start with room for this many points and strands and double...
const FIRST_POINTS: u32 = 1 << 16;
const FIRST_STRANDS: u32 = 1 << 13;
/// ...up to this many bytes, and grow by as much from there (twice 300 MB of coats would
/// reserve 200 MB that nothing uses).
const ARRAY_STEP_BYTES: u64 = 64 << 20;
/// Bytes a point and a strand take in them.
const POINT_BYTES: u64 = 16;
const STRAND_BYTES: u64 = std::mem::size_of::<GpuInfo>() as u64;
/// The upload allowance's key for a groom ([`crate::features::UploadBudget`]), beside
/// `MESH_UPLOAD_KEY` and the skins' 1 << 62.
pub(crate) const GROOM_UPLOAD_KEY: u64 = 1 << 61;
/// Strands a thread packs at a time ([`pack_groom`]).
const PACK_CHUNK: usize = 8192;
/// Grooms workers pack at a time in play (each holds a copy of its groom, 19 MB for a coat).
const PACK_JOBS: usize = 2;
/// Frames a packed groom waits for an instance to be drawn with it (one whose wearer left the
/// view meanwhile), and frames a worker's groom is waited for before it is asked again.
const PACKED_KEEP_FRAMES: u64 = 120;
const PACK_GIVE_UP_FRAMES: u64 = 600;
/// A groom the arrays had no room for (they are as large as the device lets a buffer be) is
/// tried again after this many frames, not packed in vain every frame.
const NO_ROOM_FRAMES: u64 = 120;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GpuParams {
    size: [u32; 4],
    misc: [u32; 4],
    tiles: [u32; 4],
    screen: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default, Debug)]
struct GpuInstance {
    rows: [[f32; 4]; 3],
    inv: [[f32; 4]; 3],
    grid: [f32; 4],
    tint: [f32; 4],
    first: [u32; 4],
    out: [u32; 4],
    more: [u32; 4],
    sphere: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GpuInfo {
    color: [u32; 4],
    follow: [u32; 4],
}

/// A groom as uploaded: where its strands are in the shared arrays. (Its points and strands
/// are on the GPU only: no copy is kept here.)
struct GroomSlot {
    version: u64,
    /// Guides' rest points (not reordered: `follow` indexes them).
    guides: Vec<Vec3>,
    points_per: u32,
    strands: u32,
    fiber: StrandFiber,
    thin: bool,
    /// Model-space bounds of the rest points (with their radii).
    lo: Vec3,
    hi: Vec3,
    first_point: u32,
    first_strand: u32,
    last_seen: u64,
}

impl GroomSlot {
    fn points(&self) -> u32 {
        self.strands * self.points_per
    }
}

/// What `prepare` learns of a groom when an instance first wears it, and again when the asset
/// changes: whether its arrays agree, and its bounds. Kept for every groom worn, on the GPU
/// or not, so an instance out of view is found to be so without its groom being sent.
struct GroomFacts {
    version: u64,
    valid: bool,
    /// Model-space bounds of the rest points (with their radii; `lo > hi`: no points).
    lo: Vec3,
    hi: Vec3,
    /// The thickest fibre's radius (m).
    max_radius: f32,
    /// The arrays had no room for it: not packed again before this frame ([`NO_ROOM_FRAMES`]).
    no_room_until: u64,
}

impl GroomFacts {
    fn of(groom: &StrandGroom, version: u64) -> Self {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut max_radius = 0.0f32;
        let valid = groom.is_valid();
        if valid {
            for p in &groom.points {
                let v = Vec3::new(p[0], p[1], p[2]);
                lo = lo.min(v - p[3]);
                hi = hi.max(v + p[3]);
                max_radius = max_radius.max(p[3]);
            }
        }
        Self {
            version,
            valid,
            lo,
            hi,
            max_radius,
            no_room_until: 0,
        }
    }
}

/// A groom's arrays as the GPU holds them: its strands in [`spread_order`].
struct PackedGroom {
    rest: Vec<[f32; 4]>,
    info: Vec<GpuInfo>,
    guides: Vec<Vec3>,
    points_per: u32,
    fiber: StrandFiber,
    thin: bool,
    lo: Vec3,
    hi: Vec3,
}

/// A groom a worker packed: its asset index and version, the groom, the frame it came, and
/// how long the worker took (ms).
struct Arrived {
    index: u32,
    version: u64,
    groom: PackedGroom,
    frame: u64,
    ms: f32,
}

/// Where the workers send the grooms they packed (index, version, the groom, their time).
struct PackChannel {
    send: crossbeam_channel::Sender<(u32, u64, PackedGroom, f32)>,
    arrive: crossbeam_channel::Receiver<(u32, u64, PackedGroom, f32)>,
}

impl Default for PackChannel {
    fn default() -> Self {
        let (send, arrive) = crossbeam_channel::unbounded();
        Self { send, arrive }
    }
}

/// `groom` copied for a worker to pack: packing reads every strand out of place, 10 ms on
/// the main thread for a coat of 200,000 strands on the PC, where this copy runs at memory
/// speed (its points, most of it, side by side on the frame's leaf threads).
fn copy_groom(groom: &StrandGroom) -> StrandGroom {
    const CHUNK: usize = 1 << 16;
    let mut points = vec![[0.0f32; 4]; groom.points.len()];
    engine_core::tasks::par_chunks_mut(&mut points, CHUNK, |chunk, out| {
        let from = chunk * CHUNK;
        out.copy_from_slice(&groom.points[from..from + out.len()]);
    });
    StrandGroom {
        fiber: groom.fiber,
        points_per: groom.points_per,
        points,
        roots: groom.roots.clone(),
        tips: groom.tips.clone(),
        guides: groom.guides.clone(),
        follow: groom.follow.clone(),
        thin: groom.thin,
    }
}

/// Free room in one of the shared groom arrays, in elements (points or strands).
#[derive(Default, Debug, Clone)]
struct Spans {
    /// Elements the array holds.
    capacity: u32,
    /// The free spans (start, length) by start; no two touch.
    free: Vec<(u32, u32)>,
}

impl Spans {
    /// An array of `capacity` elements whose first `used` are taken.
    fn new(capacity: u32, used: u32) -> Self {
        let mut free = Vec::new();
        if used < capacity {
            free.push((used, capacity - used));
        }
        Self { capacity, free }
    }

    /// `len` elements taken from the first free span that has them: where they start.
    fn take(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return Some(0);
        }
        let at = self.free.iter().position(|&(_, n)| n >= len)?;
        let (start, n) = self.free[at];
        if n == len {
            self.free.remove(at);
        } else {
            self.free[at] = (start + len, n - len);
        }
        Some(start)
    }

    /// `len` elements from `start` given back (they join the free spans beside them).
    fn give(&mut self, start: u32, len: u32) {
        if len == 0 {
            return;
        }
        let at = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(at, (start, len));
        if at + 1 < self.free.len() && start + len == self.free[at + 1].0 {
            self.free[at].1 += self.free[at + 1].1;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].0 + self.free[at - 1].1 == start {
            self.free[at - 1].1 += self.free[at].1;
            self.free.remove(at);
        }
    }

    fn used(&self) -> u32 {
        self.capacity - self.free.iter().map(|&(_, n)| n).sum::<u32>()
    }
}

/// How many of `sizes` (a groom's points and strands) find room in the two arrays, taken in
/// order, each from the first free span that has it.
fn placed(mut points: Spans, mut strands: Spans, sizes: &[(u32, u32)]) -> usize {
    sizes
        .iter()
        .filter(|&&(p, s)| {
            let Some(at) = points.take(p) else {
                return false;
            };
            if strands.take(s).is_some() {
                return true;
            }
            points.give(at, p);
            false
        })
        .count()
}

/// Elements of `bytes` each that a buffer of `most_bytes` holds (the largest the device binds).
fn most_elements(bytes: u64, most_bytes: u64) -> u32 {
    (most_bytes / bytes).min(1 << 31) as u32
}

/// Room for `want` elements of `bytes` each in a shared groom array: `first` at least, a power
/// of two up to [`ARRAY_STEP_BYTES`] and whole steps of that beyond, and no more than a buffer
/// of `most_bytes` holds.
fn capacity_for(want: u64, first: u32, bytes: u64, most_bytes: u64) -> u32 {
    let step = ARRAY_STEP_BYTES / bytes;
    let want = want.max(first as u64);
    let size = if want <= step {
        want.next_power_of_two()
    } else {
        want.div_ceil(step) * step
    };
    size.min(most_elements(bytes, most_bytes) as u64) as u32
}

/// Whether making the shared arrays again places more of `sizes` than their room does as it
/// lies: when they would be larger (`capacities`: what [`capacity_for`] gives them now), or
/// when their free room, gathered in one piece, holds more. Arrays as large as the device lets
/// them be, and full, are left as they are: making them again copies every groom held.
fn worth_making_again(
    points: &Spans,
    strands: &Spans,
    sizes: &[(u32, u32)],
    capacities: (u32, u32),
) -> bool {
    if capacities.0 > points.capacity || capacities.1 > strands.capacity {
        return true;
    }
    let gathered = placed(
        Spans::new(points.capacity, points.used()),
        Spans::new(strands.capacity, strands.used()),
        sizes,
    );
    gathered > placed(points.clone(), strands.clone(), sizes)
}

/// One instance this frame, as `prepare` copied it.
struct Draw {
    entity: Entity,
    /// Its groom asset's index, and the version of it this frame draws.
    groom: u32,
    version: u64,
    /// Model -> camera-relative.
    rotation_scale: Mat3,
    translation: Vec3,
    tint: [f32; 3],
    width: f32,
    /// Strands to draw ([`strands_wanted`]), and whether the near rule set the number.
    wanted: f32,
    stepped: bool,
    distance: f32,
    /// Posed guide points, when they match the groom.
    guides: Option<Vec<Vec3>>,
    /// Who poses them now that it is drawn ([`StrandGuidesWhenDrawn`]), and how many points
    /// the groom's guides have.
    pose: Option<(StrandGuidesWhenDrawn, usize)>,
    /// The shapes its strands are kept out of ([`StrandKeptOutWhenDrawn`]), asked with the
    /// guides.
    kept_out: Option<StrandKeptOutWhenDrawn>,
    capsules: Vec<StrandCapsule>,
    /// The measured body they are kept out of ([`StrandBodyWhenDrawn`]), asked with them.
    body: Option<StrandBodyWhenDrawn>,
    boxes: Vec<StrandBox>,
    /// Its guides' state ([`StrandStateWhenDrawn`]), asked with them: one a guide, or none.
    state: Option<StrandStateWhenDrawn>,
    states: Vec<StrandGuideState>,
}

/// A box's samples on the GPU (`buffers.bodies`): where, how many, the frame they were last
/// drawn with, whether they are sent, and the arrays themselves (held, so their address
/// stays theirs).
struct BodySlot {
    start: u32,
    len: u32,
    seen: u64,
    sent: bool,
    distance: std::sync::Arc<[i16]>,
    weight: std::sync::Arc<[u8]>,
}

/// What the frame draws, as `prepare_frame` laid it out.
struct FrameDraws {
    instances: Vec<GpuInstance>,
    /// Points drawn by the largest instance (the point passes' dispatch width), and the
    /// points of the instance that poses the most to fill its volume (the pose pass's).
    widest: u32,
    widest_pose: u32,
    tiles: u32,
    tiles_x: u32,
    tiles_y: u32,
    /// Points and strands drawn, and the strands in the drawn instances' grooms.
    points: u64,
    strands: u64,
    groom_strands: u64,
}

struct Pipelines {
    pose_layout: wgpu::BindGroupLayout,
    raster_layout: wgpu::BindGroupLayout,
    args_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    shadow_layout: wgpu::BindGroupLayout,
    pose: wgpu::ComputePipeline,
    tile_depth: wgpu::ComputePipeline,
    project: wgpu::ComputePipeline,
    shade: wgpu::ComputePipeline,
    count: wgpu::ComputePipeline,
    scan_tiles: wgpu::ComputePipeline,
    scan_blocks: wgpu::ComputePipeline,
    scan_finish: wgpu::ComputePipeline,
    args: wgpu::ComputePipeline,
    fill: wgpu::ComputePipeline,
    raster: wgpu::ComputePipeline,
    composite: wgpu::RenderPipeline,
    motion: wgpu::RenderPipeline,
    shadow_cascade: wgpu::RenderPipeline,
    shadow_inset: wgpu::RenderPipeline,
}

/// How many tile references a frame wanted (the tile counts' total), read back. A head of
/// long hair filling the screen at 1440p wants more than [`MAX_REFS`]: the references are
/// handed out in tile order, so every tile below the row where they ran out drew no hair (the
/// hair stopped in a level line across the neck). With the number known, the next frames draw
/// fewer strands, each wider, until it fits.
struct RefReadback {
    buffer: wgpu::Buffer,
    /// [`REF_IDLE`], [`REF_COPIED`] (recorded in a frame), [`REF_MAPPING`], [`REF_READY`].
    state: Arc<AtomicU8>,
}

const REF_IDLE: u8 = 0;
const REF_COPIED: u8 = 1;
const REF_MAPPING: u8 = 2;
const REF_READY: u8 = 3;

/// The share of strands to leave out next, from the share left out now and the references
/// that wanted (`wanted`) against the room for them (`room`): enough to fit with a margin,
/// and back by degrees once there is room again.
fn next_ref_cut(cut: f32, wanted: u64, room: u64) -> f32 {
    let drawn = 1.0 - cut;
    let fits = room as f32 * 0.85 / wanted.max(1) as f32;
    if fits < 1.0 {
        (1.0 - drawn * fits).clamp(0.0, 0.9)
    } else {
        // (Back toward every strand, a little a frame, without overshooting the room.)
        (1.0 - (drawn * 1.05).min(drawn * fits)).clamp(0.0, 0.9)
    }
}

/// Record copying the frame's wanted references (`cs_scan_blocks` leaves the tile counts'
/// total after the active count: word 4 x tiles + blocks + 1) into an idle readback.
fn copy_refs_wanted(
    gpu: &GpuShared,
    readbacks: &mut Vec<RefReadback>,
    tiles_buffer: &wgpu::Buffer,
    encoder: &mut wgpu::CommandEncoder,
    tiles: u32,
) {
    if readbacks.is_empty() {
        readbacks.extend((0..REF_READBACKS).map(|_| RefReadback {
            buffer: gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand references wanted"),
                size: 4,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(REF_IDLE)),
        }));
    }
    let Some(rb) = readbacks
        .iter()
        .find(|r| r.state.load(Ordering::Acquire) == REF_IDLE)
    else {
        return;
    };
    let word = 4 * tiles as u64 + tiles.div_ceil(1024) as u64 + 1;
    encoder.copy_buffer_to_buffer(tiles_buffer, word * 4, &rb.buffer, 0, 4);
    rb.state.store(REF_COPIED, Ordering::Release);
}

/// Buffers sized for the frame's points, tiles and grooms.
#[derive(Default)]
struct Buffers {
    params: Option<wgpu::Buffer>,
    instances: Option<wgpu::Buffer>,
    rest: Option<wgpu::Buffer>,
    info: Option<wgpu::Buffer>,
    guide_offsets: Option<wgpu::Buffer>,
    guide_capacity: u64,
    bodies: Option<wgpu::Buffer>,
    body_capacity: u64,
    posed: Option<wgpu::Buffer>,
    history: Option<wgpu::Buffer>,
    lit: Option<wgpu::Buffer>,
    point_capacity: u64,
    voxels: Option<wgpu::Buffer>,
    voxel_capacity: u64,
    tiles: Option<wgpu::Buffer>,
    tile_capacity: u64,
    refs: Option<wgpu::Buffer>,
    ref_capacity: u64,
    args: Option<wgpu::Buffer>,
    color: Option<(wgpu::Texture, wgpu::TextureView)>,
    motion: Option<(wgpu::Texture, wgpu::TextureView)>,
    size: (u32, u32),
}

struct Groups {
    pose: wgpu::BindGroup,
    raster: wgpu::BindGroup,
    args: wgpu::BindGroup,
    composite: wgpu::BindGroup,
    shadow: wgpu::BindGroup,
}

/// The strand renderer. Registered by the render plugin.
#[derive(Default)]
pub(crate) struct StrandFeature {
    pipelines: Option<Pipelines>,
    /// The grooms on the GPU, by asset index.
    grooms: HashMap<u32, GroomSlot>,
    /// What is known of every groom an instance wears, on the GPU or not.
    facts: HashMap<u32, GroomFacts>,
    /// Free room in the shared groom arrays: points (`buffers.rest`), strands (`buffers.info`).
    point_room: Spans,
    strand_room: Spans,
    /// Packed grooms for `prepare_frame` to send (drawn, and new or changed), how long the
    /// main thread worked for them this frame (packing them, or copying one for a worker),
    /// and how long workers packed those among them that came from one (ms).
    incoming: Vec<(u32, u64, PackedGroom)>,
    pack_ms: f32,
    worker_pack_ms: f32,
    /// In play a groom is packed by a worker, from a copy: the grooms under way (asset index,
    /// version, the frame it was asked), where they arrive, and those that arrived and wait
    /// for an instance to be drawn with them within the frame's upload allowance (with the
    /// frame they came and the worker's time).
    packing: Vec<(u32, u64, u64)>,
    packed_from: PackChannel,
    packed: Vec<Arrived>,
    /// Grooms workers packed so far.
    groom_worker_packs: u64,
    /// Grooms sent and grooms dropped so far, and how often the arrays were made again.
    groom_uploads: u64,
    groom_evictions: u64,
    groom_repacks: u64,
    draws: Option<Vec<Draw>>,
    /// Measured bodies' samples on the GPU, by the address of their distances, and how many
    /// samples `buffers.bodies` holds.
    bodies: HashMap<usize, BodySlot>,
    body_used: u32,
    /// Last frame's layout (entity, groom, point slots): history is valid while it holds.
    layout: Vec<(Entity, u32, u32)>,
    /// Strand slots each instance had last frame ([`strand_slots`]).
    slots: HashMap<Entity, u32>,
    buffers: Buffers,
    /// The tile references recent frames wanted, read back ([`RefReadback`]), and the share of
    /// strands left out for want of room for their references (0: none).
    ref_readbacks: Vec<RefReadback>,
    ref_cut: f32,
    /// The references the latest frame read back wanted.
    refs_wanted: u64,
    groups: Option<Groups>,
    frame: Option<FrameDraws>,
    frame_index: u32,
    /// Frames prepared with the strands on (what a groom's `last_seen` counts in).
    frames: u64,
    stats: FeatureStats,
    /// Warned once of a groom whose arrays do not agree, and once of one without room.
    warned: bool,
    warned_no_room: bool,
}

/// The length of the line through `points` (their first three numbers).
fn polyline_length<'a, const N: usize>(points: impl Iterator<Item = &'a [f32; N]>) -> f32 {
    let mut last: Option<Vec3> = None;
    let mut length = 0.0;
    for p in points {
        let p = Vec3::new(p[0], p[1], p[2]);
        length += last.map_or(0.0, |l| l.distance(p));
        last = Some(p);
    }
    length
}

/// A guide a strand follows, as the pose pass reads it: the guide's number in the low
/// [`FOLLOW_GUIDE_BITS`] bits, and above them how far along the guide the strand reaches, 0..1
/// in 12 bits (its own length over the guide's). A strand moves with its guide length for
/// length: a hair 3 cm long beside a lock of 35 cm takes the motion of the lock's first 3 cm,
/// not of its tip (point for point, the short hairs of a head were drawn out sideways into a
/// comb whenever the long locks swung). `u32::MAX` stays no guide.
fn follow_bits(guide: u32, length: f32, guide_length: Option<f32>) -> u32 {
    let Some(guide_length) = guide_length.filter(|_| guide < (1 << FOLLOW_GUIDE_BITS)) else {
        return u32::MAX;
    };
    let reach = if guide_length > 1e-9 {
        (length / guide_length).clamp(0.0, 1.0)
    } else {
        1.0
    };
    // (4095 in the reach with every guide bit set would read as no guide: one less.)
    guide | (((reach * 4094.0).round() as u32) << FOLLOW_GUIDE_BITS)
}

/// Bits of a guide's number in a packed strand's `follow` (a groom has up to a million
/// guides).
const FOLLOW_GUIDE_BITS: u32 = 20;

/// `groom` as the GPU holds it: its strands in [`spread_order`], their colours and guides
/// packed. `lo` and `hi` are its bounds ([`GroomFacts`]). The loops run on the frame's leaf
/// threads, a few thousand strands to each.
fn pack_groom(groom: &StrandGroom, lo: Vec3, hi: Vec3) -> PackedGroom {
    let per = (groom.points_per as usize).max(1);
    let order = spread_order(groom);
    let default_rough = match groom.fiber {
        StrandFiber::Human => 0.33,
        StrandFiber::Fur => 0.42,
    };
    let mut rest = vec![[0.0f32; 4]; order.len() * per];
    engine_core::tasks::par_chunks_mut(&mut rest, PACK_CHUNK * per, |chunk, out| {
        let order = &order[chunk * PACK_CHUNK..];
        for (points, &s) in out.chunks_exact_mut(per).zip(order) {
            let s = s as usize;
            points.copy_from_slice(&groom.points[s * per..(s + 1) * per]);
        }
    });
    let guide_lengths: Vec<f32> = groom
        .guides
        .chunks_exact(per)
        .map(|g| polyline_length(g.iter()))
        .collect();
    let mut info = vec![GpuInfo::default(); order.len()];
    engine_core::tasks::par_chunks_mut(&mut info, PACK_CHUNK, |chunk, out| {
        let order = &order[chunk * PACK_CHUNK..];
        for (info, &s) in out.iter_mut().zip(order) {
            let s = s as usize;
            let root = groom.roots[s];
            let tip =
                groom
                    .tips
                    .get(s)
                    .copied()
                    .unwrap_or([root[0], root[1], root[2], default_rough]);
            let pack = |a: f32, b: f32| f16_bits(a) | (f16_bits(b) << 16);
            let follow = groom.follow.get(s).copied().unwrap_or_default();
            let unorm = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u32;
            let length = polyline_length(groom.points[s * per..(s + 1) * per].iter());
            let guide = |g: u32| follow_bits(g, length, guide_lengths.get(g as usize).copied());
            *info = GpuInfo {
                color: [
                    pack(root[0], root[1]),
                    pack(root[2], root[3]),
                    pack(tip[0], tip[1]),
                    pack(tip[2], tip[3]),
                ],
                follow: [
                    guide(follow.guides[0]),
                    guide(follow.guides[1]),
                    guide(follow.guides[2]),
                    unorm(follow.weights[0]) | (unorm(follow.weights[1]) << 16),
                ],
            };
        }
    });
    PackedGroom {
        rest,
        info,
        guides: groom
            .guides
            .iter()
            .map(|g| Vec3::new(g[0], g[1], g[2]))
            .collect(),
        points_per: groom.points_per,
        fiber: groom.fiber,
        thin: groom.thin,
        lo,
        hi,
    }
}

impl StrandFeature {
    /// Take in the readbacks of references wanted that have arrived (the latest counts), and
    /// start mapping those recorded in a frame since submitted (`prepare_frame` runs after the
    /// last frame's submit).
    fn read_refs(&mut self) {
        let mut latest = None;
        for rb in &self.ref_readbacks {
            match rb.state.load(Ordering::Acquire) {
                REF_READY => {
                    if let Ok(bytes) = rb.buffer.slice(..).get_mapped_range() {
                        latest = Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
                    }
                    rb.buffer.unmap();
                    rb.state.store(REF_IDLE, Ordering::Release);
                }
                REF_COPIED => {
                    rb.state.store(REF_MAPPING, Ordering::Release);
                    let state = rb.state.clone();
                    rb.buffer
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |r| {
                            state.store(
                                if r.is_ok() { REF_READY } else { REF_IDLE },
                                Ordering::Release,
                            );
                        });
                }
                _ => {}
            }
        }
        if let Some(wanted) = latest {
            self.refs_wanted = wanted as u64;
            self.ref_cut = next_ref_cut(self.ref_cut, wanted as u64, MAX_REFS);
        }
    }

    /// The grooms `prepare` packed, into the shared arrays: each into free room of its own, so
    /// no other groom is sent again. (Until 17 Nov 2026 every groom's strands were kept here
    /// and both arrays were built and sent whole whenever one groom came or left, and grooms
    /// came whether drawn or not and left 600 frames later: with the showcase's 30 coats,
    /// 500 MB again about once a second, each time a frame of 0.3 to 0.4 s on the PC.)
    fn send_grooms(&mut self, gpu: &GpuShared) {
        let incoming = std::mem::take(&mut self.incoming);
        if incoming.is_empty() {
            return;
        }
        // A changed groom's old strands leave first.
        for (index, _, _) in &incoming {
            self.drop_groom(*index);
        }
        // Room for all of them, or the arrays again where that makes room: larger, or with
        // their free room in one piece.
        let sizes: Vec<(u32, u32)> = incoming
            .iter()
            .map(|(_, _, p)| (p.rest.len() as u32, p.info.len() as u32))
            .collect();
        let fit = placed(self.point_room.clone(), self.strand_room.clone(), &sizes) == sizes.len();
        if !fit || self.buffers.rest.is_none() {
            let more = |of: fn(&(u32, u32)) -> u32| sizes.iter().map(|s| of(s) as u64).sum::<u64>();
            let coming = (more(|s| s.0), more(|s| s.1));
            let most = most_bytes(gpu);
            // (Arrays that may grow no further: what nobody draws leaves for what is drawn.)
            for index in self.grooms_in_the_way(coming, most) {
                self.drop_groom(index);
                self.groom_evictions += 1;
            }
            let points = self.point_room.used() as u64 + coming.0;
            let strands = self.strand_room.used() as u64 + coming.1;
            let capacities = (
                capacity_for(points, FIRST_POINTS, POINT_BYTES, most),
                capacity_for(strands, FIRST_STRANDS, STRAND_BYTES, most),
            );
            if self.buffers.rest.is_none()
                || worth_making_again(&self.point_room, &self.strand_room, &sizes, capacities)
            {
                self.repack(gpu, points, strands);
            }
        }
        let (Some(rest), Some(info)) = (&self.buffers.rest, &self.buffers.info) else {
            return;
        };
        for (index, version, packed) in incoming {
            let (points, strands) = (packed.rest.len() as u32, packed.info.len() as u32);
            let room = self.point_room.take(points).and_then(|first_point| {
                let first_strand = self.strand_room.take(strands);
                if first_strand.is_none() {
                    self.point_room.give(first_point, points);
                }
                Some((first_point, first_strand?))
            });
            let Some((first_point, first_strand)) = room else {
                // (`prepare` packs only what has room beside the grooms in view, and those out
                // of view have left for it: not expected. Asked for again in a while, so a
                // groom is not packed in vain every frame.)
                if let Some(facts) = self.facts.get_mut(&index) {
                    facts.no_room_until = self.frames + NO_ROOM_FRAMES;
                }
                if !self.warned_no_room {
                    engine_core::log::warn!(
                        "strands: no room for a groom of {strands} strands on the GPU; not drawn"
                    );
                    self.warned_no_room = true;
                }
                continue;
            };
            if points > 0 {
                gpu.queue.write_buffer(
                    rest,
                    first_point as u64 * POINT_BYTES,
                    bytemuck::cast_slice(&packed.rest),
                );
                gpu.queue.write_buffer(
                    info,
                    first_strand as u64 * STRAND_BYTES,
                    bytemuck::cast_slice(&packed.info),
                );
            }
            self.grooms.insert(
                index,
                GroomSlot {
                    version,
                    guides: packed.guides,
                    points_per: packed.points_per,
                    strands,
                    fiber: packed.fiber,
                    thin: packed.thin,
                    lo: packed.lo,
                    hi: packed.hi,
                    first_point,
                    first_strand,
                    last_seen: self.frames,
                },
            );
            self.groom_uploads += 1;
        }
    }

    /// Takes a groom off the GPU: its room in the arrays is free again (nothing is copied).
    fn drop_groom(&mut self, index: u32) {
        if let Some(g) = self.grooms.remove(&index) {
            self.point_room.give(g.first_point, g.points());
            self.strand_room.give(g.first_strand, g.strands);
        }
    }

    /// The grooms to take off the GPU so that `coming` more points and strands have room in
    /// arrays no larger than a buffer of `most_bytes` (the largest the device binds): of those
    /// this frame does not draw, the longest unseen first, and no more than it takes. None
    /// where the arrays may still grow to hold everything. (`prepare` packs a groom only where
    /// it has room beside what the frame draws, so what leaves here is always enough.)
    fn grooms_in_the_way(&self, coming: (u64, u64), most_bytes: u64) -> Vec<u32> {
        let most = (
            most_elements(POINT_BYTES, most_bytes) as u64,
            most_elements(STRAND_BYTES, most_bytes) as u64,
        );
        let mut held = (
            self.point_room.used() as u64 + coming.0,
            self.strand_room.used() as u64 + coming.1,
        );
        let frames = self.frames;
        let mut unseen: Vec<(u64, u32)> = self
            .grooms
            .iter()
            .filter(|(_, g)| g.last_seen < frames)
            .map(|(&index, g)| (g.last_seen, index))
            .collect();
        unseen.sort_unstable();
        let mut leaving = Vec::new();
        for (_, index) in unseen {
            if held.0 <= most.0 && held.1 <= most.1 {
                break;
            }
            let g = &self.grooms[&index];
            held.0 -= g.points() as u64;
            held.1 -= g.strands as u64;
            leaving.push(index);
        }
        leaving
    }

    /// The grooms to take off the GPU now: those nobody drew for [`EVICT_FRAMES`], the longest
    /// unseen first, for as long as more than [`KEEP_POINTS`] are held that this frame does
    /// not draw. (What the frame draws is not counted: with a herd in view that alone is
    /// several times `KEEP_POINTS`, and every animal that walked out of view would leave the
    /// GPU 600 frames later and be sent again on its way back.)
    fn grooms_to_drop(&self) -> Vec<u32> {
        let frames = self.frames;
        let mut held: u64 = self
            .grooms
            .values()
            .filter(|g| g.last_seen < frames)
            .map(|g| g.points() as u64)
            .sum();
        if held <= KEEP_POINTS as u64 {
            return Vec::new();
        }
        let mut stale: Vec<(u64, u32, u32)> = self
            .grooms
            .iter()
            .filter(|(_, g)| g.last_seen + EVICT_FRAMES < frames)
            .map(|(&index, g)| (g.last_seen, index, g.points()))
            .collect();
        stale.sort_unstable();
        let mut leaving = Vec::new();
        for (_, index, points) in stale {
            if held <= KEEP_POINTS as u64 {
                break;
            }
            held -= points as u64;
            leaving.push(index);
        }
        leaving
    }

    /// Drops [`Self::grooms_to_drop`]; arrays that are then three quarters empty are made
    /// again at twice what they hold.
    fn evict_grooms(&mut self, gpu: &GpuShared) {
        let leaving = self.grooms_to_drop();
        if leaving.is_empty() {
            return;
        }
        for index in leaving {
            self.drop_groom(index);
            self.groom_evictions += 1;
        }
        let (points, strands) = (self.point_room.used(), self.strand_room.used());
        if points as u64 * 4 <= self.point_room.capacity as u64
            && self.point_room.capacity > FIRST_POINTS
        {
            self.repack(gpu, points as u64 * 2, strands as u64 * 2);
        }
    }

    /// The shared groom arrays made again with room for `points` and `strands`
    /// ([`capacity_for`]: at most what the device lets a storage buffer hold), and the grooms
    /// held copied into them one after another, on the GPU: to grow, or to close the gaps of
    /// grooms that left. Nothing crosses from the CPU.
    fn repack(&mut self, gpu: &GpuShared, points: u64, strands: u64) {
        let most = most_bytes(gpu);
        let point_capacity = capacity_for(points, FIRST_POINTS, POINT_BYTES, most);
        let strand_capacity = capacity_for(strands, FIRST_STRANDS, STRAND_BYTES, most);
        let make = |label: &'static str, size: u64| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let rest = make("strand rest points", point_capacity as u64 * POINT_BYTES);
        let info = make("strand info", strand_capacity as u64 * STRAND_BYTES);
        let mut order: Vec<u32> = self.grooms.keys().copied().collect();
        order.sort_unstable_by_key(|index| self.grooms[index].first_point);
        let mut encoder = None;
        let (mut point, mut strand) = (0u32, 0u32);
        for index in order {
            let g = &self.grooms[&index];
            let (points, strands) = (g.points(), g.strands);
            if point as u64 + points as u64 > point_capacity as u64
                || strand as u64 + strands as u64 > strand_capacity as u64
            {
                // (No room in arrays the device caps: it leaves, and is sent again later.)
                self.grooms.remove(&index);
                continue;
            }
            if points > 0
                && let (Some(old_rest), Some(old_info)) = (&self.buffers.rest, &self.buffers.info)
            {
                let encoder = encoder.get_or_insert_with(|| {
                    gpu.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("strand grooms"),
                        })
                });
                encoder.copy_buffer_to_buffer(
                    old_rest,
                    g.first_point as u64 * POINT_BYTES,
                    &rest,
                    point as u64 * POINT_BYTES,
                    points as u64 * POINT_BYTES,
                );
                encoder.copy_buffer_to_buffer(
                    old_info,
                    g.first_strand as u64 * STRAND_BYTES,
                    &info,
                    strand as u64 * STRAND_BYTES,
                    strands as u64 * STRAND_BYTES,
                );
            }
            let g = self.grooms.get_mut(&index).expect("listed");
            g.first_point = point;
            g.first_strand = strand;
            point += points;
            strand += strands;
        }
        if let Some(encoder) = encoder {
            gpu.queue.submit([encoder.finish()]);
        }
        self.buffers.rest = Some(rest);
        self.buffers.info = Some(info);
        self.point_room = Spans::new(point_capacity, point);
        self.strand_room = Spans::new(strand_capacity, strand);
        self.groups = None;
        self.groom_repacks += 1;
    }
}

/// The largest storage buffer the device binds, bytes: what a shared groom array may grow to.
fn most_bytes(gpu: &GpuShared) -> u64 {
    let limits = gpu.device.limits();
    limits
        .max_buffer_size
        .min(limits.max_storage_buffer_binding_size)
}

/// `v` as a half float's bits (round to nearest; tiny values flush to 0, huge ones clamp), as
/// WGSL's `unpack2x16float` reads them.
fn f16_bits(v: f32) -> u32 {
    let x = v.to_bits();
    let sign = (x >> 16) & 0x8000;
    let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
    let mant = x & 0x7f_ffff;
    if v.is_nan() {
        0
    } else if exp <= 0 {
        sign
    } else if exp >= 31 {
        sign | 0x7bff
    } else {
        (sign | ((exp as u32) << 10) | (mant >> 13)) + ((mant >> 12) & 1)
    }
}

/// A buffer of at least `size` bytes in `slot` (a power of two, grown when short): true when
/// it was made again.
fn ensure(
    gpu: &GpuShared,
    slot: &mut Option<wgpu::Buffer>,
    capacity: &mut u64,
    size: u64,
    label: &'static str,
    usage: wgpu::BufferUsages,
) -> bool {
    if slot.is_some() && *capacity >= size {
        return false;
    }
    let size = size.max(256).next_power_of_two();
    *slot = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    }));
    *capacity = size;
    true
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
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

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, storage: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: if storage {
            wgpu::ShaderStages::COMPUTE
        } else {
            wgpu::ShaderStages::FRAGMENT
        },
        ty: if storage {
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2,
            }
        } else {
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            }
        },
        count: None,
    }
}

/// The WGSL of a strand shader: the shared types, then its own.
pub(crate) fn strand_source(own: &str) -> String {
    format!("{COMMON}\n{own}")
}

/// Every strand shader composed as the renderer compiles it, for validation tests.
#[cfg(test)]
pub(crate) fn composed_shaders() -> Vec<(&'static str, String)> {
    use crate::features::compose_shader;
    vec![
        ("strands pose", compose_shader(&strand_source(POSE), false)),
        (
            "strands raster",
            compose_shader(&strand_source(RASTER), true),
        ),
        (
            "strands composite",
            compose_shader(&strand_source(COMPOSITE), true),
        ),
        (
            "strands shadow",
            compose_shader(&strand_source(SHADOW), false),
        ),
    ]
}

impl Pipelines {
    fn new(gpu: &GpuShared) -> Self {
        use wgpu::ShaderStages as S;
        let c = S::COMPUTE;
        let pose_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands pose"),
            entries: &[
                uniform_entry(0, c),
                storage_entry(1, c, true),
                storage_entry(2, c, true),
                storage_entry(3, c, true),
                storage_entry(4, c, true),
                storage_entry(5, c, false),
                storage_entry(6, c, false),
                storage_entry(7, c, true),
            ],
        });
        let raster_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands raster"),
            entries: &[
                uniform_entry(0, c),
                storage_entry(1, c, true),
                storage_entry(2, c, true),
                storage_entry(3, c, true),
                storage_entry(4, c, true),
                storage_entry(5, c, false),
                storage_entry(6, c, false),
                storage_entry(7, c, false),
                storage_entry(8, c, false),
                texture_entry(10, true),
                texture_entry(11, true),
            ],
        });
        let args_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands args"),
            entries: &[storage_entry(0, c, false)],
        });
        let composite_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands composite"),
            entries: &[
                uniform_entry(0, S::VERTEX_FRAGMENT),
                storage_entry(1, S::VERTEX, true),
                texture_entry(2, false),
                texture_entry(3, false),
            ],
        });
        let shadow_layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strands shadow"),
            entries: &[
                uniform_entry(0, S::VERTEX),
                storage_entry(1, S::VERTEX, true),
                storage_entry(2, S::VERTEX, true),
            ],
        });

        let pose_module = gpu.shader("strands pose", &strand_source(POSE), false);
        let raster_module = gpu.shader("strands raster", &strand_source(RASTER), true);
        let composite_module = gpu.shader("strands composite", &strand_source(COMPOSITE), true);
        let shadow_module = gpu.shader("strands shadow", &strand_source(SHADOW), false);

        let pose_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands pose"),
            bind_group_layouts: &[Some(&pose_layout)],
            immediate_size: 0,
        });
        let raster_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands raster"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&raster_layout)],
            immediate_size: 0,
        });
        let args_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands args"),
            bind_group_layouts: &[
                Some(&gpu.frame_layout),
                Some(&raster_layout),
                Some(&args_layout),
            ],
            immediate_size: 0,
        });
        let composite_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands composite"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&composite_layout)],
            immediate_size: 0,
        });
        let shadow_pl = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strands shadow"),
            bind_group_layouts: &[Some(&gpu.shadow_view_layout), Some(&shadow_layout)],
            immediate_size: 0,
        });
        let compute = |layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, entry: &str| {
            gpu.compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pose = compute(&pose_pl, &pose_module, "cs_pose");
        let tile_depth = compute(&raster_pl, &raster_module, "cs_tile_depth");
        let project = compute(&raster_pl, &raster_module, "cs_project");
        let shade = compute(&raster_pl, &raster_module, "cs_shade");
        let count = compute(&raster_pl, &raster_module, "cs_count");
        let scan_tiles = compute(&raster_pl, &raster_module, "cs_scan_tiles");
        let scan_blocks = compute(&raster_pl, &raster_module, "cs_scan_blocks");
        let scan_finish = compute(&raster_pl, &raster_module, "cs_scan_finish");
        let args = compute(&args_pl, &raster_module, "cs_args");
        let fill = compute(&raster_pl, &raster_module, "cs_fill");
        let raster = compute(&raster_pl, &raster_module, "cs_raster");

        let tile_pipeline = |fs: &str,
                             format: wgpu::TextureFormat,
                             blend: Option<wgpu::BlendState>,
                             depth: bool| {
            gpu.render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&composite_pl),
                vertex: wgpu::VertexState {
                    module: &composite_module,
                    entry_point: Some("vs_tile"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: depth.then(|| wgpu::DepthStencilState {
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    ..gpu.depth_state(false)
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &composite_module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let composite = tile_pipeline(
            "fs_composite",
            gpu.formats.hdr,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            true,
        );
        let motion = tile_pipeline("fs_motion", gpu.formats.motion, None, false);
        let shadow = |vs: &str| {
            gpu.render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(vs),
                layout: Some(&shadow_pl),
                vertex: wgpu::VertexState {
                    module: &shadow_module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(gpu.shadow_depth_state()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shadow_module,
                    entry_point: Some("fs_shadow"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            shadow_cascade: shadow("vs_shadow_cascade"),
            shadow_inset: shadow("vs_shadow_inset"),
            pose_layout,
            raster_layout,
            args_layout,
            composite_layout,
            shadow_layout,
            pose,
            tile_depth,
            project,
            shade,
            count,
            scan_tiles,
            scan_blocks,
            scan_finish,
            args,
            fill,
            raster,
            composite,
            motion,
        }
    }
}

/// The share of a groom's strands the near rule draws when its bounds are `px` pixels tall:
/// all of them from [`FULL_PX`], fewer below (in steps of 1/16, so the layout seldom changes),
/// none under [`MIN_PX`].
pub(crate) fn strand_share(px: f32) -> f32 {
    if px < MIN_PX {
        return 0.0;
    }
    let share = (px / FULL_PX).clamp(MIN_SHARE, 1.0);
    (share * 16.0).ceil() / 16.0
}

/// The order a groom's strands are stored in: any first n of them spread evenly over the
/// groom, so a groom that draws a share of its strands covers every part of itself alike.
///
/// The roots are sorted along a curve that fills their bounds (Morton order: neighbors on the
/// curve are neighbors in space), and the strands are taken from that line at evenly spaced
/// places, halving the spaces each round (the bit-reversed index). The first n are then one
/// strand from each of n patches of about the same number of roots, where a shuffle (as it was
/// until 17 Nov 2026) leaves some patches bare and others crowded: at two strands a pixel,
/// which is what a coat far away draws, that showed as a mottled pelt.
pub(crate) fn spread_order(groom: &StrandGroom) -> Vec<u32> {
    let per = (groom.points_per as usize).max(1);
    let strands = groom.strand_count();
    let root = |s: usize| {
        let p = groom.points[s * per];
        Vec3::new(p[0], p[1], p[2])
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for s in 0..strands {
        lo = lo.min(root(s));
        hi = hi.max(root(s));
    }
    // 21 bits an axis, interleaved.
    let scale = Vec3::splat(2_097_151.0) / (hi - lo).max(Vec3::splat(1e-9));
    let part = |v: f32| {
        let mut x = (v.clamp(0.0, 2_097_151.0) as u64) & 0x1f_ffff;
        x = (x | (x << 32)) & 0x001f_0000_0000_ffff;
        x = (x | (x << 16)) & 0x001f_0000_ff00_00ff;
        x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
        x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
        (x | (x << 2)) & 0x1249_2492_4924_9249
    };
    let mut line: Vec<(u64, u32)> = (0..strands)
        .map(|s| {
            let q = (root(s) - lo) * scale;
            (part(q.x) | (part(q.y) << 1) | (part(q.z) << 2), s as u32)
        })
        .collect();
    sort_line(&mut line);
    let bits = strands.next_power_of_two().trailing_zeros();
    let mut order = Vec::with_capacity(strands);
    for i in 0..(1u64 << bits) {
        let at = if bits == 0 {
            0
        } else {
            (i.reverse_bits() >> (64 - bits)) as usize
        };
        if at < strands {
            order.push(line[at].1);
        }
    }
    order
}

/// `line` (a strand's place on the curve, the strand) sorted, as `sort_unstable` leaves it,
/// on the frame's leaf threads: the curve's first six bits cut a long line into 64 runs, and
/// each run is sorted on its own. (A coat of 200 000 hairs: 5 ms on one thread.)
fn sort_line(line: &mut Vec<(u64, u32)>) {
    const RUNS: usize = 64;
    if line.len() < 4 * PACK_CHUNK {
        line.sort_unstable();
        return;
    }
    // (A place on the curve has 63 bits.)
    let run = |place: u64| (place >> 57) as usize;
    let mut starts = [0usize; RUNS + 1];
    for &(place, _) in line.iter() {
        starts[run(place) + 1] += 1;
    }
    for r in 0..RUNS {
        starts[r + 1] += starts[r];
    }
    let mut placed = vec![(0u64, 0u32); line.len()];
    let mut next = starts;
    for &entry in line.iter() {
        let r = run(entry.0);
        placed[next[r]] = entry;
        next[r] += 1;
    }
    let mut runs = Vec::with_capacity(RUNS);
    let mut left = placed.as_mut_slice();
    for r in 0..RUNS {
        let (run, after) = left.split_at_mut(starts[r + 1] - starts[r]);
        left = after;
        if run.len() > 1 {
            runs.push(run);
        }
    }
    match engine_core::tasks::split_pool() {
        Some(pool) if runs.len() > 1 => {
            pool.scope(|scope| {
                for run in runs {
                    scope.spawn(async move { run.sort_unstable() });
                }
            });
        }
        _ => runs.into_iter().for_each(|run| run.sort_unstable()),
    }
    *line = placed;
}

/// The near rule's number: [`strand_share`] of a groom's `strands`, whole.
fn near_strands(strands: u32, px: f32) -> f32 {
    let all = strands as f32;
    (all * strand_share(px)).ceil().min(all)
}

/// How many of a groom's `strands` are drawn when its bounds are `px` pixels tall and its
/// thickest fibre `half_px` pixels wide either side of its axis (0: not drawn), and whether
/// the near rule set the number (a whole number that changes in steps).
///
/// The near rule ([`strand_share`]) thins a groom by its height alone, and never below a
/// sixteenth: right for a head of hair, but a coat of 200 000 hairs on an animal twenty
/// pixels tall then draws 12 500 of them into a few hundred pixels, and a herd of those was
/// most of the frame (30 ms on an RTX 5080, 16 Nov 2026). So far away the pixels set the
/// number: [`STRANDS_PER_PX2`] strands for every pixel of the square the bounds are tall, and
/// as many as it takes for the widened strands to keep the groom's cover without one of them
/// passing a pixel in width ([`MAX_WIDE_HALF_PX`]). Whichever rule asks for fewer wins: a head
/// of hair (20 000 strands) stays the near rule's down to 18 px, a coat of 200 000 hairs
/// down to about 110 px (at 50 px it draws 10 000 strands for the near rule's 12 500, at
/// 30 px 3 600, at 20 px 1 600, each as much wider).
pub(crate) fn strands_wanted(strands: u32, px: f32, half_px: f32, per_px2: f32) -> (f32, bool) {
    let near = near_strands(strands, px);
    if near <= 0.0 {
        return (0.0, true);
    }
    let all = strands as f32;
    let cover = all * half_px.max(0.0) / MAX_WIDE_HALF_PX;
    let seen = (per_px2 * px * px).max(cover).max(MIN_STRANDS);
    if seen < near {
        (seen.max(1.0), false)
    } else {
        (near, true)
    }
}

/// `count` strands or fewer, in steps of a sixteenth of the power of two under it (whole
/// numbers under 32): a number that stands still while `count` drifts a little.
fn stepped_count(count: f32) -> f32 {
    let step = 2f32.powi(count.max(1.0).log2().floor() as i32 - 4).max(1.0);
    ((count / step).floor() * step).max(1.0)
}

/// Strand slots for an instance that draws `drawn` of its groom's `strands` by its pixels,
/// having had `before` last frame: the same while they hold the strands drawn without much
/// to spare, else the strands drawn and some room.
fn strand_slots(drawn: u32, strands: u32, before: Option<u32>) -> u32 {
    if let Some(before) = before
        && before >= drawn
        && before <= strands
        && before as f32 <= drawn as f32 * SLOT_KEEP
    {
        return before;
    }
    ((drawn as f32 * SLOT_ROOM).ceil() as u32).clamp(drawn, strands.max(drawn))
}

impl RenderFeature for StrandFeature {
    fn name(&self) -> &str {
        "strands"
    }

    fn order(&self) -> i32 {
        // After the other transparent features' opaque work, before particles.
        50
    }

    fn init(&mut self, gpu: &GpuShared) {
        if gpu.warm_pipelines {
            self.pipelines = Some(Pipelines::new(gpu));
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
        if view.extra_view {
            return;
        }
        self.draws = None;
        self.incoming.clear();
        if settings.debug_skip & shade_skip::STRANDS != 0 {
            // (No draws, not no word: the slots and their history end here. The frame is not
            // counted: with the strands switched off the grooms held do not grow old, or a
            // sweep's `no_strands` row would send them away and the next row fetch them.)
            self.draws = Some(Vec::new());
            return;
        }
        self.frames += 1;
        let near_count = settings.debug_skip
            & (shade_skip::STRANDS_OLD_WAY | shade_skip::STRANDS_NEAR_COUNT)
            != 0;
        let per_px2 = if settings.debug_skip & shade_skip::STRANDS_SPARSE != 0 {
            SPARSE_PER_PX2
        } else {
            STRANDS_PER_PX2
        };
        let mut q = world.query::<(
            Entity,
            &StrandInstance,
            &GlobalTransform,
            Option<&HairGuidesPosed>,
            Option<&StrandGuidesWhenDrawn>,
            Option<&StrandKeptOutWhenDrawn>,
            Option<&StrandBodyWhenDrawn>,
            Option<&StrandStateWhenDrawn>,
        )>();
        let Some(assets) = world.get_resource::<Assets<StrandGroom>>() else {
            return;
        };
        let mut draws = Vec::new();
        // The grooms the draws wear, by asset index.
        let mut worn: HashMap<u32, &StrandGroom> = HashMap::new();
        let origin = view.origin;
        let px_per_m = view.proj.y_axis.y * view.height as f32 * 0.5;
        for (entity, inst, gt, posed, when_drawn, kept_out, body, state) in q.iter(world) {
            if !inst.visible || inst.lod == Some(StrandLod::Hidden) {
                continue;
            }
            let Some(groom) = assets.get(inst.groom) else {
                continue;
            };
            let index = inst.groom.index();
            let version = assets.version(inst.groom);
            // Its bounds, read from the groom once (not from the GPU's copy: a groom out of
            // view has none).
            let facts = self
                .facts
                .entry(index)
                .or_insert_with(|| GroomFacts::of(groom, version));
            if facts.version != version {
                *facts = GroomFacts::of(groom, version);
            }
            if !facts.valid {
                if !self.warned {
                    engine_core::log::warn!(
                        "strands: a groom's arrays don't agree with its points_per; not drawn"
                    );
                    self.warned = true;
                }
                continue;
            }
            let (lo, hi, max_radius) = (facts.lo, facts.hi, facts.max_radius);
            if lo.x > hi.x {
                continue;
            }
            let rotation_scale = Mat3::from_quat(gt.rotation()) * Mat3::from_diagonal(gt.scale());
            let translation = (gt.translation() - origin).as_vec3();
            let center = rotation_scale * ((lo + hi) * 0.5) + translation;
            let radius = (hi - lo).length() * 0.5 * gt.scale().max_element();
            // Hair just off screen still casts its shadow on screen.
            if !view.frustum.intersects_sphere(center, radius + 0.5) {
                continue;
            }
            let distance = center.length().max(view.near);
            // Pixels per meter where the groom is.
            let per_m = if view.orthographic {
                px_per_m
            } else {
                px_per_m / distance
            };
            let px = radius * 2.0 * per_m;
            let width = inst.width_scale.max(0.0);
            let strands = groom.strand_count() as u32;
            let (wanted, stepped) = if near_count {
                (near_strands(strands, px), true)
            } else {
                // (A fibre is as thick as groomed whatever the entity's scale: `cs_project`.)
                strands_wanted(strands, px, max_radius * width * per_m, per_px2)
            };
            if wanted <= 0.0 {
                continue;
            }
            let guides = posed
                .filter(|p| !groom.guides.is_empty() && p.points.len() == groom.guides.len())
                .map(|p| p.points.clone());
            let pose = when_drawn
                .filter(|_| guides.is_none() && !groom.guides.is_empty())
                .map(|w| (*w, groom.guides.len()));
            worn.insert(index, groom);
            let guides_from_entity = guides.is_some();
            draws.push(Draw {
                entity,
                groom: index,
                version,
                rotation_scale,
                translation,
                tint: inst.tint,
                width,
                wanted,
                stepped,
                distance,
                guides,
                pose,
                kept_out: kept_out.copied(),
                // (Its own posed guides bring the shapes they were held out of.)
                capsules: posed
                    .filter(|_| guides_from_entity)
                    .map(|p| {
                        p.capsules
                            .iter()
                            .take(MAX_CAPSULES)
                            .map(|c| StrandCapsule {
                                a: c.a,
                                ra: c.ra,
                                b: c.b,
                                rb: c.rb,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                body: body.copied(),
                boxes: Vec::new(),
                state: state.copied(),
                // (Its own posed guides bring their state.)
                states: posed
                    .filter(|p| {
                        guides_from_entity
                            && !(p.wet.is_empty() && p.cut.is_empty() && p.shed.is_empty())
                    })
                    .map(|p| {
                        (0..groom.guide_count())
                            .map(|g| StrandGuideState {
                                wet: p.wet.get(g).copied().unwrap_or(0.0),
                                cut: p.cut.get(g).copied().unwrap_or(1.0),
                                shed: p.shed.get(g).copied().unwrap_or(1.0),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
        draws.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        draws.truncate(MAX_INSTANCES);
        // The grooms these wear that the GPU does not hold (or holds as they were before a
        // change) are packed and sent, the nearest first, as far as the frame's upload
        // allowance goes (all of them while loading or headless). An instance whose groom has
        // to wait is not drawn until it is there. Nor is one whose groom has no room beside
        // those the frame draws, in arrays as large as the device lets a buffer be: asked
        // before the allowance is taken and the groom packed, so it costs nothing while that
        // lasts.
        let most = most_bytes(gpu);
        let most = (
            most_elements(POINT_BYTES, most) as u64,
            most_elements(STRAND_BYTES, most) as u64,
        );
        // Points and strands of the grooms held that the frame draws, and of those it packs.
        let mut in_view = (0u64, 0u64);
        let mut counted: Vec<u32> = Vec::new();
        for d in &draws {
            let held = self.grooms.get(&d.groom).filter(|g| g.version == d.version);
            if let Some(g) = held
                && !counted.contains(&d.groom)
            {
                counted.push(d.groom);
                in_view.0 += g.points() as u64;
                in_view.1 += g.strands as u64;
            }
        }
        // In play a worker packs a groom from a copy and the instance is drawn once it has
        // come back (the frame after next at the earliest); where uploads are not paced
        // (headless, behind a loading screen) it is packed here and drawn this frame.
        let paced = gpu.uploads.paced();
        let frames = self.frames;
        while let Ok((index, version, groom, ms)) = self.packed_from.arrive.try_recv() {
            self.packing.retain(|p| (p.0, p.1) != (index, version));
            self.groom_worker_packs += 1;
            self.packed.push(Arrived {
                index,
                version,
                groom,
                frame: frames,
                ms,
            });
        }
        self.packing.retain(|p| p.2 + PACK_GIVE_UP_FRAMES > frames);
        self.packed
            .retain(|a| a.frame + PACKED_KEEP_FRAMES > frames);
        // Packed here this frame, and the one a worker is asked for (a copy a frame).
        let mut here: Vec<(u32, u64, &StrandGroom)> = Vec::new();
        let mut for_a_worker: Option<(u32, u64, &StrandGroom)> = None;
        self.worker_pack_ms = 0.0;
        let mut no_room = None;
        draws.retain(|d| {
            let held = self.grooms.get(&d.groom).map(|g| g.version);
            if held == Some(d.version)
                || here.iter().any(|p| p.0 == d.groom)
                || self
                    .incoming
                    .iter()
                    .any(|i| (i.0, i.1) == (d.groom, d.version))
            {
                return true;
            }
            if self.facts[&d.groom].no_room_until > frames {
                return false;
            }
            let groom = worn[&d.groom];
            let (points, strands) = (groom.points.len() as u64, groom.strand_count() as u64);
            if in_view.0 + points > most.0 || in_view.1 + strands > most.1 {
                no_room.get_or_insert(strands);
                return false;
            }
            let bytes = points * POINT_BYTES + strands * STRAND_BYTES;
            let key = GROOM_UPLOAD_KEY | d.groom as u64;
            let arrived = self
                .packed
                .iter()
                .position(|a| (a.index, a.version) == (d.groom, d.version));
            // (A groom that changed is packed here as before: its instance would not be drawn
            // while a worker had it, and what the GPU holds of it is the old shape.)
            if arrived.is_none() && paced && held.is_none() {
                // A worker has it, or is asked for it now (the nearest first; the others in
                // later frames).
                if !self
                    .packing
                    .iter()
                    .any(|p| (p.0, p.1) == (d.groom, d.version))
                    && self.packing.len() < PACK_JOBS
                    && for_a_worker.is_none()
                {
                    for_a_worker = Some((d.groom, d.version, groom));
                }
                gpu.uploads.waiting(key, bytes);
                return false;
            }
            if !gpu.uploads.take(key, bytes, held.is_some()) {
                return false;
            }
            in_view.0 += points;
            in_view.1 += strands;
            match arrived {
                Some(at) => {
                    let a = self.packed.swap_remove(at);
                    self.worker_pack_ms += a.ms;
                    self.incoming.push((a.index, a.version, a.groom));
                }
                None => here.push((d.groom, d.version, groom)),
            }
            true
        });
        if let Some(strands) = no_room
            && !self.warned_no_room
        {
            engine_core::log::warn!(
                "strands: no room on the GPU for a groom of {strands} strands beside those in \
                 view; not drawn until some of them leave it"
            );
            self.warned_no_room = true;
        }
        let packing_since = std::time::Instant::now();
        if let Some((index, version, groom)) = for_a_worker {
            let (lo, hi) = (self.facts[&index].lo, self.facts[&index].hi);
            let copy = copy_groom(groom);
            let send = self.packed_from.send.clone();
            // ("async": its loops split on the background's threads, not the frame's.)
            let spawned = std::thread::Builder::new()
                .name("async groom pack".into())
                .spawn(move || {
                    let started = std::time::Instant::now();
                    let packed = pack_groom(&copy, lo, hi);
                    let ms = started.elapsed().as_secs_f32() * 1000.0;
                    let _ = send.send((index, version, packed, ms));
                });
            match spawned {
                Ok(_) => self.packing.push((index, version, frames)),
                Err(e) => engine_core::log::warn!("strands: no thread to pack a groom on: {e}"),
            }
        }
        for (index, version, groom) in here {
            let facts = &self.facts[&index];
            self.incoming
                .push((index, version, pack_groom(groom, facts.lo, facts.hi)));
        }
        self.pack_ms = packing_since.elapsed().as_secs_f32() * 1000.0;
        // Guides posed when drawn: only now, for what the frame draws, side by side (a coat
        // out of view costs nothing; one in view the same skinning it cost every frame).
        let world: &World = world;
        crate::mesh_feature::run_jobs(draws.iter_mut().filter(|d| d.pose.is_some()).map(|d| {
            move || {
                let Some((pose, points)) = d.pose else {
                    return;
                };
                let mut out = Vec::with_capacity(points);
                if (pose.0)(world, d.entity, &mut out) && out.len() == points {
                    d.guides = Some(out);
                    if let Some(kept_out) = d.kept_out {
                        (kept_out.0)(world, d.entity, &mut d.capsules);
                        d.capsules.truncate(MAX_CAPSULES);
                    }
                    if let Some(body) = d.body {
                        (body.0)(world, d.entity, &mut d.boxes);
                        d.boxes.truncate(MAX_BOXES);
                        d.boxes.retain(|b| {
                            let n = b.size.iter().map(|s| *s as usize).product::<usize>();
                            b.size.iter().all(|s| *s >= 2)
                                && b.cell > 0.0
                                && b.distance.len() == n
                                && b.weight.len() == n
                        });
                    }
                    if let Some(state) = d.state {
                        (state.0)(world, d.entity, &mut d.states);
                    }
                }
            }
        }));
        self.draws = Some(draws);
    }

    fn prepare_frame(
        &mut self,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
        _diagnostics: &mut engine_core::diagnostics::Diagnostics,
    ) {
        if view.extra_view {
            return;
        }
        self.frame = None;
        self.stats = FeatureStats::default();
        let grooms = (!self.incoming.is_empty()).then(std::time::Instant::now);
        // (Grooms, strands and bytes on their way, for the log.)
        let coming = self
            .incoming
            .iter()
            .fold((0u32, 0u64, 0u64), |sum, (_, _, p)| {
                (
                    sum.0 + 1,
                    sum.1 + p.info.len() as u64,
                    sum.2 + p.rest.len() as u64 * POINT_BYTES + p.info.len() as u64 * STRAND_BYTES,
                )
            });
        let repacks = self.groom_repacks;
        // What the frame draws is seen now, before anything is sent: room for what comes is
        // never made of a groom in view.
        let frames = self.frames;
        for d in self.draws.iter().flatten() {
            if let Some(g) = self.grooms.get_mut(&d.groom) {
                g.last_seen = frames;
            }
        }
        self.send_grooms(gpu);
        let Some(draws) = self.draws.take() else {
            return;
        };
        self.evict_grooms(gpu);
        if let Some(started) = grooms {
            // (Named in the slow-frame log, inside "prepare on the render thread".)
            let sent_ms = started.elapsed().as_secs_f32() * 1000.0;
            gpu.prepare_steps
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(("strands: grooms", sent_ms));
            // A groom's first sight is rare and is what a frame pays for it: one line each
            // time, so a log shows grooms that come again and again, what each cost, and
            // what making the arrays again for them cost.
            let mb = |bytes: u64| bytes as f64 / (1 << 20) as f64;
            engine_core::log::info!(
                "strands: {} groom(s) of {} strands to the GPU ({:.1} MB): packed in {:.1} ms \
                 on the frame and {:.1} ms on workers, sent in {:.1} ms{}; {} held in arrays \
                 of {:.0} MB",
                coming.0,
                coming.1,
                mb(coming.2),
                self.pack_ms,
                self.worker_pack_ms,
                sent_ms,
                if self.groom_repacks != repacks {
                    " with the arrays made again"
                } else {
                    ""
                },
                self.grooms.len(),
                mb(self.point_room.capacity as u64 * POINT_BYTES
                    + self.strand_room.capacity as u64 * STRAND_BYTES)
            );
        }
        if draws.is_empty() {
            self.layout.clear();
            self.slots.clear();
            return;
        }
        if self.pipelines.is_none() {
            self.pipelines = Some(Pipelines::new(gpu));
        }

        // Points drawn per instance, scaled down together past the budget.
        let wanted: usize = draws
            .iter()
            .filter_map(|d| {
                let g = self
                    .grooms
                    .get(&d.groom)
                    .filter(|g| g.version == d.version)?;
                Some(d.wanted.ceil() as usize * g.points_per as usize)
            })
            .sum();
        self.read_refs();
        let budget = if wanted > MAX_POINTS {
            MAX_POINTS as f32 / wanted as f32
        } else {
            1.0
        } * (1.0 - self.ref_cut);
        // Points the budget leaves over: the room instances may keep beyond what they draw.
        let mut spare = MAX_POINTS.saturating_sub(wanted) as u64;

        let mut instances = Vec::with_capacity(draws.len());
        let mut layout = Vec::with_capacity(draws.len());
        let mut slots_now = HashMap::with_capacity(draws.len());
        let mut offsets: Vec<[f32; 4]> = Vec::new();
        // Room on the GPU for the measured bodies this frame's draws are kept out of; what
        // nobody drew for a while goes, and the rest moves up.
        let frames = self.frames;
        let before = self.bodies.len();
        self.bodies
            .retain(|_, s| frames.saturating_sub(s.seen) < BODY_KEPT_FRAMES);
        if self.bodies.len() != before {
            self.body_used = 0;
            let mut left: Vec<&mut BodySlot> = self.bodies.values_mut().collect();
            left.sort_by_key(|s| s.start);
            for slot in left {
                slot.start = self.body_used;
                slot.sent = false;
                self.body_used += slot.len;
            }
        }
        for b in draws.iter().flat_map(|d| &d.boxes) {
            let slot = self
                .bodies
                .entry(b.distance.as_ptr() as usize)
                .or_insert_with(|| {
                    let start = self.body_used;
                    self.body_used += b.distance.len() as u32;
                    BodySlot {
                        start,
                        len: b.distance.len() as u32,
                        seen: frames,
                        sent: false,
                        distance: b.distance.clone(),
                        weight: b.weight.clone(),
                    }
                });
            slot.seen = frames;
        }
        let mut out = 0u32;
        let mut strand_base = 0u32;
        let (mut widest, mut widest_pose) = (0u32, 0u32);
        let (mut drawn_points, mut groom_strands) = (0u64, 0u64);
        for d in &draws {
            let Some(g) = self.grooms.get(&d.groom).filter(|g| g.version == d.version) else {
                continue;
            };
            // The strands drawn, as a number with a fraction: the last of them is drawn that
            // share of its width, so it comes in by degrees as the groom grows on screen.
            let most = g.strands.max(1);
            let mut count = (d.wanted * budget).clamp(1.0, most as f32);
            let mut drawn = (count.ceil() as u32).clamp(1, most);
            // Its slots: with room where the number follows the pixels, so they stay put
            // while it drifts...
            let mut slots = drawn;
            let mut roomy = false;
            if !d.stepped && budget >= 1.0 {
                let wide = strand_slots(drawn, g.strands, self.slots.get(&d.entity).copied());
                let room = (wide - drawn) as u64 * g.points_per as u64;
                if room <= spare {
                    spare -= room;
                    slots = wide;
                    roomy = true;
                }
            }
            // ...and where there is no room (the budget is spent, and every number moves with
            // every other), the number goes in steps, sixteen to a doubling, so the slots
            // stand still between them as they do under the near rule.
            if !roomy && (!d.stepped || budget < 1.0) {
                count = stepped_count(count);
                drawn = (count as u32).clamp(1, most);
                slots = drawn;
            }
            let last = (count - (drawn - 1) as f32).clamp(0.0, 1.0);
            let points = drawn * g.points_per;
            // The strands that fill its density volume: those drawn, and where they are few
            // (far away) as many as the near rule's least share, as it was filled before the
            // pixels set the number. A few hundred wide strands would leave most of a coat's
            // cells empty and the rest too dense: speckled, too bright self-shadowing.
            let filling = drawn.max(((g.strands as f32 * MIN_SHARE).ceil() as u32).min(most));
            slots_now.insert(d.entity, slots);
            let guide_base = match &d.guides {
                Some(posed) if posed.len() == g.guides.len() => {
                    // Ahead of its offsets, the measured body's boxes its strands are kept out
                    // of ([`BOX_ENTRIES`] each) and how many, then the shapes (two entries
                    // each) and how many there are (`strands_pose.wgsl`).
                    for b in &d.boxes {
                        let start = self.bodies[&(b.distance.as_ptr() as usize)].start;
                        let entries = offsets.len();
                        let m = b.to_box.transpose().to_cols_array_2d();
                        offsets.extend_from_slice(&m[..3]);
                        offsets.push([b.min.x, b.min.y, b.min.z, 1.0 / b.cell]);
                        offsets.push([b.size[0] as f32, b.size[1] as f32, b.size[2] as f32, 0.0]);
                        offsets.push([(start & 0xffff) as f32, (start >> 16) as f32, 0.0, 0.0]);
                        debug_assert_eq!(offsets.len() - entries, BOX_ENTRIES);
                    }
                    offsets.push([d.boxes.len() as f32, 0.0, 0.0, 0.0]);
                    for c in &d.capsules {
                        offsets.push([c.a.x, c.a.y, c.a.z, c.ra]);
                        offsets.push([c.b.x, c.b.y, c.b.z, c.rb]);
                    }
                    // (With the shapes' number, the guide points whose state follows the
                    // offsets: none, or all of them.)
                    let per = (g.points_per as usize).max(1);
                    // (All of them too under a measured body: a lock's hairs are laid on it
                    // where the lock's guide lies, which takes the guide at rest.)
                    let stated = per >= STATE_POINTS
                        && (!d.boxes.is_empty()
                            || d.states.len() * per == g.guides.len()
                                && d.states.iter().any(|s| *s != StrandGuideState::default()));
                    let state_points = if stated { g.guides.len() } else { 0 };
                    // (And how wet the hair that follows no guide is: as the guides are, on
                    // average. Hair combed to a tie or wound in a knot is rained on too.)
                    let held_wet = if d.states.is_empty() {
                        0.0
                    } else {
                        d.states.iter().map(|s| s.wet.clamp(0.0, 1.0)).sum::<f32>()
                            / d.states.len() as f32
                    };
                    offsets.push([d.capsules.len() as f32, state_points as f32, held_wet, 0.0]);
                    let base = offsets.len() as u32;
                    offsets.extend(
                        posed
                            .iter()
                            .zip(&g.guides)
                            .map(|(p, r)| [p.x - r.x, p.y - r.y, p.z - r.z, 0.0]),
                    );
                    // The guides at rest (a wet hair gathers to its guide: where that is),
                    // their state in the first points' last entries: wet, cut, shed.
                    if stated {
                        offsets.extend(g.guides.iter().enumerate().map(|(i, r)| {
                            let s = d.states.get(i / per).copied().unwrap_or_default();
                            let w = match i % per {
                                0 => s.wet.clamp(0.0, 1.0),
                                1 => s.cut.clamp(0.0, 1.0),
                                2 => s.shed.clamp(0.0, 1.0),
                                _ => 0.0,
                            };
                            [r.x, r.y, r.z, w]
                        }));
                    }
                    base
                }
                _ => u32::MAX,
            };
            // The density volume: the groom's bounds, grown for hair the guides move.
            let size = (g.hi - g.lo).max_element() * 1.3 + 1e-3;
            let cell = size / VOXELS as f32;
            let corner = (g.lo + g.hi) * 0.5 - Vec3::splat(size * 0.5);
            let m = d.rotation_scale;
            let inv = m.inverse();
            let row = |r: usize, w: f32| [m.row(r).x, m.row(r).y, m.row(r).z, w];
            let inv_row = |r: usize| [inv.row(r).x, inv.row(r).y, inv.row(r).z, 0.0];
            let center = m * ((g.lo + g.hi) * 0.5) + d.translation;
            let radius = (g.hi - g.lo).length()
                * 0.5
                * m.x_axis
                    .length()
                    .max(m.y_axis.length())
                    .max(m.z_axis.length());
            // Fewer strands, each wider: the same cover.
            let width = d.width * (g.strands as f32 / count);
            let k = instances.len() as u32;
            instances.push(GpuInstance {
                rows: [
                    row(0, d.translation.x),
                    row(1, d.translation.y),
                    row(2, d.translation.z),
                ],
                inv: [inv_row(0), inv_row(1), inv_row(2)],
                grid: [corner.x, corner.y, corner.z, cell],
                tint: [d.tint[0], d.tint[1], d.tint[2], width],
                first: [g.first_point, g.first_strand, g.points_per, drawn],
                out: [out, guide_base, last.to_bits(), k * VOXELS.pow(3)],
                more: [
                    strand_base,
                    matches!(g.fiber, StrandFiber::Fur) as u32,
                    filling,
                    g.thin as u32,
                ],
                sphere: [center.x, center.y, center.z, radius],
            });
            layout.push((d.entity, d.groom, slots * g.points_per));
            out += slots * g.points_per;
            strand_base += drawn;
            widest = widest.max(points);
            widest_pose = widest_pose.max(filling * g.points_per);
            drawn_points += points as u64;
            groom_strands += g.strands as u64;
            self.stats.instances += 1;
        }
        self.slots = slots_now;
        if instances.is_empty() {
            self.layout.clear();
            return;
        }
        // Instances keep their slots while those before them do: from the first whose slots
        // changed on, last frame's positions lie elsewhere and are no history (a far coat
        // that takes new slots costs the hair nearer than it nothing).
        let kept = layout
            .iter()
            .zip(&self.layout)
            .take_while(|(now, before)| now == before)
            .count() as u32;
        self.layout = layout;
        self.frame_index = self.frame_index.wrapping_add(1).max(2);

        let width = view.width.max(1);
        let height = view.height.max(1);
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);
        let tiles = tiles_x * tiles_y;
        let blocks = tiles.div_ceil(1024);
        // Counts, starts, fill cursors, block sums, active list, then two counters, then the
        // farthest opaque depth per tile.
        let tile_words = tiles as u64 * 5 + blocks as u64 + 2;
        // (Room for what the latest frame read back wanted, and some: a near head of hair
        // wants many references for each point, a far coat fewer.)
        let ref_want = (drawn_points * 4)
            .max(self.refs_wanted + self.refs_wanted / 4)
            .clamp(MIN_REFS, MAX_REFS);

        let b = &mut self.buffers;
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let mut grown = false;
        let points = out as u64;
        if b.posed.is_none() || b.point_capacity < points {
            let cap = points.max(4096).next_power_of_two();
            let make = |label: &'static str, stride: u64| {
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: cap * stride,
                    usage: storage,
                    mapped_at_creation: false,
                })
            };
            b.posed = Some(make("strand posed points", 16));
            b.history = Some(make("strand history", 16));
            b.lit = Some(make("strand lit points", 32));
            b.point_capacity = cap;
            grown = true;
        }
        let voxel_bytes = instances.len() as u64 * VOXELS.pow(3) as u64 * 4;
        grown |= ensure(
            gpu,
            &mut b.voxels,
            &mut b.voxel_capacity,
            voxel_bytes,
            "strand density",
            storage,
        );
        grown |= ensure(
            gpu,
            &mut b.tiles,
            &mut b.tile_capacity,
            tile_words * 4,
            "strand tiles",
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        if b.refs.is_none() || b.ref_capacity < ref_want {
            let cap = ref_want.next_power_of_two().min(MAX_REFS);
            b.refs = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand tile references"),
                size: cap * 4,
                usage: storage,
                mapped_at_creation: false,
            }));
            b.ref_capacity = cap;
            grown = true;
        }
        // The measured bodies' samples: sent once each, again when the buffer is made anew.
        if ensure(
            gpu,
            &mut b.bodies,
            &mut b.body_capacity,
            (self.body_used.max(1) as u64) * 4,
            "strand bodies",
            storage,
        ) {
            grown = true;
            for slot in self.bodies.values_mut() {
                slot.sent = false;
            }
        }
        for slot in self.bodies.values_mut().filter(|s| !s.sent) {
            let cells: Vec<u32> = slot
                .distance
                .iter()
                .zip(slot.weight.iter())
                .map(|(d, w)| (*d as u16 as u32) | ((*w as u32) << 16))
                .collect();
            gpu.queue.write_buffer(
                b.bodies.as_ref().expect("made"),
                slot.start as u64 * 4,
                bytemuck::cast_slice(&cells),
            );
            slot.sent = true;
        }
        let guide_bytes = (offsets.len().max(1) * 16) as u64;
        grown |= ensure(
            gpu,
            &mut b.guide_offsets,
            &mut b.guide_capacity,
            guide_bytes,
            "strand guide offsets",
            storage,
        );
        if b.params.is_none() {
            b.params = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand params"),
                size: std::mem::size_of::<GpuParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            b.instances = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand instances"),
                size: (MAX_INSTANCES * std::mem::size_of::<GpuInstance>()) as u64,
                usage: storage,
                mapped_at_creation: false,
            }));
            b.args = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("strand indirect"),
                size: 32,
                usage: storage | wgpu::BufferUsages::INDIRECT,
                mapped_at_creation: false,
            }));
            grown = true;
        }
        if b.size != (width, height) || b.color.is_none() {
            let make = |label: &'static str| {
                let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let v = t.create_view(&Default::default());
                (t, v)
            };
            b.color = Some(make("strand color"));
            b.motion = Some(make("strand motion"));
            b.size = (width, height);
            grown = true;
        }

        let params = GpuParams {
            size: [
                tiles_x,
                tiles_y,
                b.ref_capacity as u32,
                instances.len() as u32,
            ],
            misc: [tiles, kept, self.frame_index, VOXELS],
            tiles: [tiles, tiles * 2, tiles * 3, tiles * 3 + blocks],
            screen: [width as f32, height as f32, OPACITY[0], OPACITY[1]],
        };
        let queue = &gpu.queue;
        queue.write_buffer(
            b.params.as_ref().expect("made"),
            0,
            bytemuck::bytes_of(&params),
        );
        queue.write_buffer(
            b.instances.as_ref().expect("made"),
            0,
            bytemuck::cast_slice(&instances),
        );
        if !offsets.is_empty() {
            queue.write_buffer(
                b.guide_offsets.as_ref().expect("made"),
                0,
                bytemuck::cast_slice(&offsets),
            );
        }
        if grown {
            self.groups = None;
        }
        if self.groups.is_none() {
            let p = self.pipelines.as_ref().expect("made");
            let b = &self.buffers;
            fn buf(o: &Option<wgpu::Buffer>) -> wgpu::BindingResource<'_> {
                o.as_ref().expect("made").as_entire_binding()
            }
            let entry = |binding: u32, resource| wgpu::BindGroupEntry { binding, resource };
            let pose = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands pose"),
                layout: &p.pose_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.instances)),
                    entry(2, buf(&b.rest)),
                    entry(3, buf(&b.info)),
                    entry(4, buf(&b.guide_offsets)),
                    entry(5, buf(&b.posed)),
                    entry(6, buf(&b.voxels)),
                    entry(7, buf(&b.bodies)),
                ],
            });
            let color = &b.color.as_ref().expect("made").1;
            let motion = &b.motion.as_ref().expect("made").1;
            let raster = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands raster"),
                layout: &p.raster_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.instances)),
                    entry(2, buf(&b.info)),
                    entry(3, buf(&b.posed)),
                    entry(4, buf(&b.voxels)),
                    entry(5, buf(&b.history)),
                    entry(6, buf(&b.lit)),
                    entry(7, buf(&b.tiles)),
                    entry(8, buf(&b.refs)),
                    entry(10, wgpu::BindingResource::TextureView(color)),
                    entry(11, wgpu::BindingResource::TextureView(motion)),
                ],
            });
            let args = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands args"),
                layout: &p.args_layout,
                entries: &[entry(0, buf(&b.args))],
            });
            let composite = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands composite"),
                layout: &p.composite_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.tiles)),
                    entry(2, wgpu::BindingResource::TextureView(color)),
                    entry(3, wgpu::BindingResource::TextureView(motion)),
                ],
            });
            let shadow = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("strands shadow"),
                layout: &p.shadow_layout,
                entries: &[
                    entry(0, buf(&b.params)),
                    entry(1, buf(&b.instances)),
                    entry(2, buf(&b.posed)),
                ],
            });
            self.groups = Some(Groups {
                pose,
                raster,
                args,
                composite,
                shadow,
            });
        }
        self.stats.draw_calls = 1;
        self.frame = Some(FrameDraws {
            instances,
            widest,
            widest_pose,
            tiles,
            tiles_x,
            tiles_y,
            points: drawn_points,
            strands: strand_base as u64,
            groom_strands,
        });
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        let voxels = self.buffers.voxels.as_ref().expect("made");
        encoder.clear_buffer(
            voxels,
            0,
            Some(f.instances.len() as u64 * VOXELS.pow(3) as u64 * 4),
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("strands pose"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&p.pose);
        pass.set_bind_group(0, &g.pose, &[]);
        pass.dispatch_workgroups(
            f.widest_pose.div_ceil(WORKGROUP),
            f.instances.len() as u32,
            1,
        );
    }

    fn shadow_moving(&self, map: usize, ctx: &FrameContext) -> crate::shadows::ShadowTiles {
        let mut tiles = crate::shadows::ShadowTiles::NONE;
        if map >= crate::shadows::MAX_CASCADES || ctx.view.extra_view {
            return tiles;
        }
        let (Some(f), Some(_), Some(_)) = (&self.frame, &self.pipelines, &self.groups) else {
            return tiles;
        };
        for inst in &f.instances {
            let s = inst.sphere;
            let center = Vec3::new(s[0], s[1], s[2]);
            if ctx.view.cascades.sphere_in_cascade(map, center, s[3]) {
                tiles.add_sphere(ctx.view, map, center, s[3]);
            }
        }
        tiles
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, ctx: &FrameContext) {
        if cascade >= crate::shadows::MAX_CASCADES || ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        let mut bound = false;
        for inst in &f.instances {
            let s = inst.sphere;
            if !ctx
                .view
                .cascades
                .sphere_in_cascade(cascade, Vec3::new(s[0], s[1], s[2]), s[3])
            {
                continue;
            }
            if !bound {
                pass.set_pipeline(&p.shadow_cascade);
                pass.set_bind_group(1, &g.shadow, &[]);
                bound = true;
            }
            let first = inst.more[0];
            let n = inst.first[3].div_ceil(CASCADE_SHARE);
            pass.draw(0..(inst.first[2] - 1) * 6, first..first + n);
        }
    }

    fn draw_inset_shadow(&self, pass: &mut wgpu::RenderPass<'_>, inset: usize, ctx: &FrameContext) {
        if ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g), Some(i)) = (
            &self.frame,
            &self.pipelines,
            &self.groups,
            ctx.view.shadow_insets.get(inset),
        ) else {
            return;
        };
        let mut bound = false;
        for inst in &f.instances {
            let s = inst.sphere;
            if !i.casts(Vec3::new(s[0], s[1], s[2]), s[3]) {
                continue;
            }
            if !bound {
                pass.set_pipeline(&p.shadow_inset);
                pass.set_bind_group(1, &g.shadow, &[]);
                bound = true;
            }
            let first = inst.more[0];
            pass.draw(0..(inst.first[2] - 1) * 6, first..first + inst.first[3]);
        }
    }

    fn before_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        motion: &wgpu::TextureView,
        ctx: &FrameContext,
    ) {
        if ctx.view.extra_view {
            return;
        }
        let (Some(f), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        let b = &self.buffers;
        encoder.clear_buffer(b.tiles.as_ref().expect("made"), 0, None);
        let args = b.args.as_ref().expect("made");
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("strands"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &g.raster, &[]);
            let points = (f.widest.div_ceil(WORKGROUP), f.instances.len() as u32, 1);
            let tiles = f.tiles.div_ceil(WORKGROUP);
            pass.set_pipeline(&p.tile_depth);
            pass.dispatch_workgroups(f.tiles_x.div_ceil(8), f.tiles_y.div_ceil(8), 1);
            pass.set_pipeline(&p.project);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            pass.set_pipeline(&p.shade);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            pass.set_pipeline(&p.count);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            pass.set_pipeline(&p.scan_tiles);
            pass.dispatch_workgroups(f.tiles.div_ceil(1024), 1, 1);
            pass.set_pipeline(&p.scan_blocks);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&p.scan_finish);
            pass.dispatch_workgroups(tiles, 1, 1);
            pass.set_pipeline(&p.args);
            pass.set_bind_group(2, &g.args, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&p.fill);
            pass.dispatch_workgroups(points.0, points.1, points.2);
            if ctx.settings.debug_skip & shade_skip::STRAND_RASTER == 0 {
                pass.set_pipeline(&p.raster);
                pass.dispatch_workgroups_indirect(args, 0);
            }
        }
        copy_refs_wanted(
            ctx.gpu,
            &mut self.ref_readbacks,
            b.tiles.as_ref().expect("made"),
            encoder,
            f.tiles,
        );
        // The hair's motion where it covers a fifth of a pixel or more.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("strands motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: motion,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&p.motion);
        pass.set_bind_group(0, ctx.frame_group, &[]);
        pass.set_bind_group(1, &g.composite, &[]);
        pass.draw_indirect(args, 12);
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {
        if phase != RenderPhase::Transparent || ctx.view.extra_view {
            return;
        }
        let (Some(_), Some(p), Some(g)) = (&self.frame, &self.pipelines, &self.groups) else {
            return;
        };
        pass.set_pipeline(&p.composite);
        pass.set_bind_group(1, &g.composite, &[]);
        pass.draw_indirect(self.buffers.args.as_ref().expect("made"), 12);
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        let f = self.frame.as_ref();
        serde_json::json!({
            // On the GPU: grooms, the megabytes their arrays take and what of that is in
            // use, and how many were sent, dropped, and how often the arrays were made again.
            "grooms": self.grooms.len(),
            "groom_mb": (self.point_room.capacity as u64 * POINT_BYTES
                + self.strand_room.capacity as u64 * STRAND_BYTES) as f64
                / (1 << 20) as f64,
            "groom_mb_used": (self.point_room.used() as u64 * POINT_BYTES
                + self.strand_room.used() as u64 * STRAND_BYTES) as f64
                / (1 << 20) as f64,
            "groom_uploads": self.groom_uploads,
            // Of those, packed by a worker (in play), and the grooms workers have now.
            "groom_worker_packs": self.groom_worker_packs,
            "grooms_with_workers": self.packing.len(),
            "groom_evictions": self.groom_evictions,
            "groom_repacks": self.groom_repacks,
            "instances": f.map_or(0, |f| f.instances.len()),
            // Drawn this frame, and the strands their grooms hold.
            "points": f.map_or(0, |f| f.points),
            "strands": f.map_or(0, |f| f.strands),
            "strands_in_grooms": f.map_or(0, |f| f.groom_strands),
            "refs_wanted": self.refs_wanted,
            "strands_cut_for_refs": self.ref_cut,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strands_are_cut_until_their_tile_references_fit() {
        let room = MAX_REFS;
        // Fits: nothing left out.
        assert_eq!(next_ref_cut(0.0, room / 2, room), 0.0);
        // Twice what fits: about half left out, with a margin.
        let cut = next_ref_cut(0.0, room * 2, room);
        assert!((0.55..0.6).contains(&cut), "{cut}");
        // Drawn with that cut it fits, and the cut comes back by degrees, never over the room.
        let wanted = (room as f32 * 2.0 * (1.0 - cut)) as u64;
        let next = next_ref_cut(cut, wanted, room);
        assert!(next <= cut && next > cut - 0.05, "{cut} -> {next}");
        let mut c = cut;
        for _ in 0..200 {
            c = next_ref_cut(
                c,
                (room as f32 * 0.3 * (1.0 - c) / (1.0 - cut)) as u64,
                room,
            );
        }
        assert_eq!(c, 0.0);
        // Never more than nine in ten.
        assert_eq!(next_ref_cut(0.0, room * 100, room), 0.9);
    }

    #[test]
    fn strand_shaders_validate() {
        for (label, code) in composed_shaders() {
            crate::features::validate_wgsl(label, &code).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn gpu_layouts_match_wgsl() {
        assert_eq!(std::mem::size_of::<GpuParams>(), 64);
        assert_eq!(std::mem::size_of::<GpuInstance>(), 192);
        assert_eq!(std::mem::size_of::<GpuInfo>(), 32);
    }

    #[test]
    fn half_floats_round_trip() {
        for v in [0.0f32, 1.0, 0.5, 0.123, 3.75, 1000.0] {
            let b = f16_bits(v) as u16;
            let back = f32::from(HalfBits(b));
            assert!((back - v).abs() <= v * 1e-3, "{v} -> {back}");
        }
        assert_eq!(f16_bits(-2.0), 0xc000);
    }

    struct HalfBits(u16);
    impl From<HalfBits> for f32 {
        fn from(h: HalfBits) -> f32 {
            let h = h.0 as u32;
            let exp = (h >> 10) & 0x1f;
            let mant = h & 0x3ff;
            let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
            if exp == 0 {
                sign * mant as f32 * 2f32.powi(-24)
            } else {
                sign * (1.0 + mant as f32 / 1024.0) * 2f32.powi(exp as i32 - 15)
            }
        }
    }

    #[test]
    fn far_grooms_draw_a_share_of_their_strands() {
        assert_eq!(strand_share(1000.0), 1.0);
        assert_eq!(strand_share(FULL_PX), 1.0);
        assert_eq!(strand_share(1.0), 0.0);
        let mid = strand_share(100.0);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(strand_share(10.0) >= MIN_SHARE);
        // In steps of 1/16.
        assert_eq!((mid * 16.0).fract(), 0.0);
    }

    #[test]
    fn a_dense_coat_far_away_draws_by_its_pixels() {
        // A deer's coat: 200 000 hairs 0.31 mm thick at the root, bounds 2 m across.
        let (all, radius, size) = (200_000u32, 0.00031f32, 2.0f32);
        let at = |px: f32| strands_wanted(all, px, radius * px / size, STRANDS_PER_PX2);
        // Near: the near rule's sixteenths, as before.
        assert_eq!(at(600.0), (200_000.0, true));
        assert_eq!(at(450.0), ((all as f32 * strand_share(450.0)).ceil(), true));
        // Twenty pixels tall: four strands a pixel of its square, not the near rule's 12 500
        // (and with the sparse switch as few as keep the widest strand within a pixel).
        let (far, stepped) = at(20.0);
        assert!(!stepped);
        assert_eq!(far, 1600.0);
        assert_eq!((all as f32 * strand_share(20.0)).ceil(), 12_500.0);
        let sparse = strands_wanted(all, 20.0, radius * 20.0 / size, SPARSE_PER_PX2).0;
        assert!((1239.0..1241.0).contains(&sparse), "{sparse}");
        // A hundred and fifty: the near rule's again.
        assert_eq!(at(150.0), (62_500.0, true));
        // Under the smallest size nothing, as before.
        assert_eq!(at(MIN_PX * 0.9).0, 0.0);
        let mut before = 0.0;
        for px in [
            3.0f32, 4.5, 6.0, 12.0, 20.0, 45.0, 90.0, 150.0, 300.0, 380.0, 500.0,
        ] {
            let (n, stepped) = at(px);
            // Never more than the near rule asks, never fewer than the floor, and more the
            // larger the coat is on screen.
            assert!(n <= (all as f32 * strand_share(px)).ceil(), "{px}: {n}");
            assert!(n >= MIN_STRANDS && n >= before, "{px}: {n} after {before}");
            before = n;
            if !stepped {
                // By its pixels: no strand widened past a pixel, and the pixels' worth.
                let widened = radius * px / size * all as f32 / n;
                assert!(widened <= MAX_WIDE_HALF_PX * 1.001, "{px}: {widened} px");
                assert!(n >= STRANDS_PER_PX2 * px * px, "{px}: {n}");
            }
        }
        // A head of hair (20 000 strands) is the near rule's until it is small.
        assert!(strands_wanted(20_000, 200.0, 0.01, STRANDS_PER_PX2).1);
        assert!(strands_wanted(20_000, 60.0, 0.003, STRANDS_PER_PX2).1);
        assert!(strands_wanted(20_000, 20.0, 0.001, STRANDS_PER_PX2).1);
        assert!(!strands_wanted(20_000, 12.0, 0.0006, STRANDS_PER_PX2).1);
        // A groom of a few strands draws them all.
        assert_eq!(strands_wanted(10, 5.0, 0.0, STRANDS_PER_PX2), (1.0, true));
        assert_eq!(strands_wanted(0, 50.0, 0.0, STRANDS_PER_PX2).0, 0.0);
    }

    #[test]
    fn any_first_strands_spread_over_the_groom() {
        // Roots on a 64 x 64 grid (stored row by row), two points a strand.
        let groom_of = |roots: &[[f32; 2]]| {
            let mut g = StrandGroom {
                points_per: 2,
                ..Default::default()
            };
            for r in roots {
                g.points.push([r[0], r[1], 0.0, 1e-4]);
                g.points.push([r[0], r[1], 0.01, 1e-4]);
                g.roots.push([0.5, 0.5, 0.5, 0.0]);
            }
            g
        };
        let grid: Vec<[f32; 2]> = (0..4096)
            .map(|i| [(i % 64) as f32 / 63.0, (i / 64) as f32 / 63.0])
            .collect();
        let order = spread_order(&groom_of(&grid));
        let mut seen = order.clone();
        seen.sort_unstable();
        assert!(seen.iter().copied().eq(0..4096), "every strand once");
        // The first 16 strands: one in each of the 16 squares 16 roots across; the first 64:
        // one in each of the 64 squares 8 across; the first 100: one or two in each of those.
        for (first, across) in [(16usize, 16u32), (64, 8), (100, 8)] {
            let side = 64 / across;
            let mut held = vec![0u32; (side * side) as usize];
            for &s in &order[..first] {
                held[((s % 64) / across + (s / 64) / across * side) as usize] += 1;
            }
            let (least, most) = (held.iter().min().unwrap(), held.iter().max().unwrap());
            assert!(
                *least >= (first as u32) / (side * side)
                    && *most <= (first as u32).div_ceil(side * side),
                "the first {first}: {least} to {most} a square"
            );
        }
        // Any number of strands, wherever their roots: each once.
        let mut state = 12345u32;
        let mut rand = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32
        };
        let scattered: Vec<[f32; 2]> = (0..3001).map(|_| [rand(), rand() * 0.3]).collect();
        let mut seen = spread_order(&groom_of(&scattered));
        seen.sort_unstable();
        assert!(seen.iter().copied().eq(0..3001));
        assert!(spread_order(&groom_of(&[])).is_empty());
        assert_eq!(spread_order(&groom_of(&[[0.3, 0.1]])), [0]);
    }

    #[test]
    fn a_long_line_sorts_on_threads_as_a_short_one_does_on_one() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        // Places of 63 bits; most of them crowded into a few of the 64 runs, some runs empty,
        // and many places twice (told apart by their strand).
        let line: Vec<(u64, u32)> = (0..100_000u32)
            .map(|s| {
                let r = rand();
                let run = [0u64, 1, 1, 1, 17, 17, 40, 63][(r % 8) as usize];
                let place = (run << 57) | ((r >> 8) & ((1 << 57) - 1));
                (if s % 5 == 0 { place & !0xffff } else { place }, s)
            })
            .collect();
        let mut plain = line.clone();
        plain.sort_unstable();
        let mut runs = line.clone();
        sort_line(&mut runs);
        assert!(
            runs == plain,
            "the runs, each sorted, are the whole line sorted"
        );
        // A short line is sorted as it always was.
        let mut short = line[..1000].to_vec();
        let mut short_plain = short.clone();
        sort_line(&mut short);
        short_plain.sort_unstable();
        assert_eq!(short, short_plain);
        sort_line(&mut Vec::new());
    }

    #[test]
    fn a_packed_groom_holds_every_strand_once_in_spread_order() {
        // More strands than a thread packs at a time, three points each, with tips and guides.
        const STRANDS: usize = 2 * PACK_CHUNK + 777;
        let mut state = 4242u32;
        let mut rand = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32
        };
        let mut groom = StrandGroom {
            points_per: 3,
            fiber: StrandFiber::Fur,
            guides: vec![[0.0, 0.0, 0.0], [0.0, 0.1, 0.0], [0.0, 0.2, 0.0]],
            ..Default::default()
        };
        for s in 0..STRANDS {
            let (x, y) = (rand(), rand() * 0.5);
            for j in 0..3 {
                groom
                    .points
                    .push([x, y, j as f32 * 0.01, 1e-4 + s as f32 * 1e-9]);
            }
            groom
                .roots
                .push([0.25, 0.5, 0.75, s as f32 / STRANDS as f32]);
            groom.tips.push([1.0, 0.5, 0.25, 0.4]);
            groom.follow.push(crate::strands::StrandFollow {
                guides: [0, u32::MAX, u32::MAX],
                weights: [1.0, 0.0, 0.0],
            });
        }
        assert!(groom.is_valid());
        let facts = GroomFacts::of(&groom, 7);
        assert!(facts.valid && facts.version == 7);
        assert!(facts.lo.x >= -1e-3 && facts.hi.x <= 1.001 && facts.hi.z > 0.019);
        assert!((facts.max_radius - (1e-4 + (STRANDS - 1) as f32 * 1e-9)).abs() < 1e-9);
        let packed = pack_groom(&groom, facts.lo, facts.hi);
        let order = spread_order(&groom);
        assert_eq!(packed.rest.len(), STRANDS * 3);
        assert_eq!(packed.info.len(), STRANDS);
        assert_eq!(packed.guides.len(), 3);
        for (k, &s) in order.iter().enumerate() {
            let s = s as usize;
            assert_eq!(
                packed.rest[k * 3..k * 3 + 3],
                groom.points[s * 3..s * 3 + 3],
                "strand {s} at place {k}"
            );
            let random = f16_bits(s as f32 / STRANDS as f32);
            assert_eq!(
                packed.info[k].color,
                [
                    f16_bits(0.25) | (f16_bits(0.5) << 16),
                    f16_bits(0.75) | (random << 16),
                    f16_bits(1.0) | (f16_bits(0.5) << 16),
                    f16_bits(0.25) | (f16_bits(0.4) << 16),
                ]
            );
            // The guide's number, and how far along it the strand reaches (2 of its 20 cm).
            let reach = packed.info[k].follow[0] >> FOLLOW_GUIDE_BITS;
            assert_eq!(packed.info[k].follow[0] & ((1 << FOLLOW_GUIDE_BITS) - 1), 0);
            assert!((408..=411).contains(&reach), "reach {reach}");
            assert_eq!(packed.info[k].follow[1..], [u32::MAX, u32::MAX, 65535]);
        }
        // A groom whose arrays don't agree is known as such, and has no bounds.
        groom.roots.pop();
        let facts = GroomFacts::of(&groom, 8);
        assert!(!facts.valid && facts.lo.x > facts.hi.x);
    }

    #[test]
    fn room_in_the_groom_arrays_is_taken_first_fit_and_joins_when_given_back() {
        let mut room = Spans::new(100, 0);
        assert_eq!(room.take(30), Some(0));
        assert_eq!(room.take(30), Some(30));
        assert_eq!(room.take(30), Some(60));
        assert_eq!(room.take(30), None, "ten are left");
        assert_eq!(room.used(), 90);
        // The middle one leaves: its room is found again, by a smaller one first.
        room.give(30, 30);
        assert_eq!(room.free, [(30, 30), (90, 10)]);
        assert_eq!(room.take(20), Some(30));
        assert_eq!(room.take(10), Some(50));
        assert_eq!(room.take(10), Some(90));
        assert_eq!(room.used(), 100);
        assert!(room.free.is_empty());
        // Given back in any order, neighbours join: one span when all is free.
        room.give(60, 30);
        room.give(0, 30);
        assert_eq!(room.free, [(0, 30), (60, 30)]);
        room.give(50, 10);
        assert_eq!(room.free, [(0, 30), (50, 40)]);
        room.give(90, 10);
        room.give(30, 20);
        assert_eq!(room.free, [(0, 100)]);
        assert_eq!(room.used(), 0);
        // Nothing asked takes nothing; arrays partly full from the start.
        assert_eq!(room.take(0), Some(0));
        assert_eq!(room.free, [(0, 100)]);
        let mut room = Spans::new(64, 40);
        assert_eq!(
            (room.used(), room.take(24), room.take(1)),
            (40, Some(40), None)
        );
        assert_eq!(Spans::new(8, 8).free, []);
    }

    /// A groom of `strands` strands, four points each, held by `f` as last drawn in frame
    /// `seen`.
    fn hold(f: &mut StrandFeature, index: u32, strands: u32, seen: u64) {
        let first_point = f.point_room.take(strands * 4).expect("room for its points");
        let first_strand = f.strand_room.take(strands).expect("room for its strands");
        f.grooms.insert(
            index,
            GroomSlot {
                version: 1,
                guides: Vec::new(),
                points_per: 4,
                strands,
                fiber: StrandFiber::Fur,
                thin: false,
                lo: Vec3::ZERO,
                hi: Vec3::ONE,
                first_point,
                first_strand,
                last_seen: seen,
            },
        );
    }

    #[test]
    fn grooms_leave_the_gpu_only_when_too_much_is_held_and_the_longest_unseen_first() {
        // (Grooms of two million strands here, 8 M points each, ten coats' worth: so that a
        // few of them are more than is kept.)
        const STRANDS: u32 = 2_000_000;
        const POINTS: u32 = STRANDS * 4;
        let mut f = StrandFeature {
            point_room: Spans::new(1 << 30, 0),
            strand_room: Spans::new(1 << 28, 0),
            ..Default::default()
        };
        // Three of them, 24 M points: under the 32 M kept.
        for index in 0..3 {
            hold(&mut f, index, STRANDS, 10 + index as u64);
        }
        f.frames = 100_000;
        assert_eq!(
            f.grooms_to_drop(),
            [] as [u32; 0],
            "little is held: all stay"
        );
        // Seven in all, 56 M points: over what is kept. Nothing leaves while all were drawn
        // lately...
        for index in 3..7 {
            hold(&mut f, index, STRANDS, 10 + index as u64);
        }
        f.frames = 500;
        assert_eq!(f.grooms_to_drop(), [] as [u32; 0], "all were drawn lately");
        // ...then those unseen for 600 frames go, and only those...
        f.frames = 10 + 1 + EVICT_FRAMES + 1;
        assert_eq!(f.grooms_to_drop(), [0, 1], "the rest were drawn since");
        // ...the longest unseen first, until 32 M are held: three of the seven.
        f.frames = 100_000;
        assert_eq!(f.grooms_to_drop(), [0, 1, 2], "no more than it takes");
        // One drawn again stays, and the next unseen goes in its place.
        f.grooms.get_mut(&0).unwrap().last_seen = 99_999;
        assert_eq!(f.grooms_to_drop(), [1, 2, 3]);
        for index in f.grooms_to_drop() {
            f.drop_groom(index);
        }
        assert_eq!(f.grooms.len(), 4);
        assert_eq!(f.point_room.used(), 4 * POINTS);
        assert_eq!(f.strand_room.used(), 4 * STRANDS);
        // Their room is one span again (they lay side by side), ahead of the free end.
        assert_eq!(f.point_room.free[0], (POINTS, 3 * POINTS));
        assert_eq!(f.grooms_to_drop(), [] as [u32; 0]);
        // What the frame draws is not counted. Seven are held again, four of them drawn now:
        // 24 M points wait undrawn, and all stay however long they were unseen...
        for index in 1..4 {
            hold(&mut f, index, STRANDS, 10 + index as u64);
        }
        for index in [0, 4, 5, 6] {
            f.grooms.get_mut(&index).unwrap().last_seen = f.frames;
        }
        assert_eq!(f.point_room.used(), 7 * POINTS);
        assert_eq!(
            f.grooms_to_drop(),
            [] as [u32; 0],
            "a herd in view sends nobody away"
        );
        // ...and with two of the herd out of view for long, 40 M wait: the longest unseen
        // goes, and no more.
        for (index, seen) in [(4, 14), (5, 15)] {
            f.grooms.get_mut(&index).unwrap().last_seen = seen;
        }
        assert_eq!(f.grooms_to_drop(), [1]);
        // The showcase's thirty coats of 200 000 hairs, none in view: all are kept.
        let mut f = StrandFeature {
            point_room: Spans::new(1 << 30, 0),
            strand_room: Spans::new(1 << 28, 0),
            frames: 100_000,
            ..Default::default()
        };
        for index in 0..30 {
            hold(&mut f, index, 200_000, index as u64);
        }
        assert_eq!(f.point_room.used(), 24_000_000);
        assert_eq!(f.grooms_to_drop(), [] as [u32; 0]);
    }

    #[test]
    fn grooms_nobody_draws_leave_for_one_in_view_where_the_arrays_can_grow_no_more() {
        // A device whose largest buffer is 16 000 bytes: a thousand points, 500 strands.
        let most = 16_000;
        let mut f = StrandFeature {
            point_room: Spans::new(1000, 0),
            strand_room: Spans::new(500, 0),
            frames: 50,
            ..Default::default()
        };
        // Four grooms of 50 strands, 200 points: two drawn this frame, two not.
        hold(&mut f, 0, 50, 50);
        hold(&mut f, 1, 50, 20);
        hold(&mut f, 2, 50, 50);
        hold(&mut f, 3, 50, 10);
        // One more has room as things are: nobody leaves.
        assert_eq!(f.grooms_in_the_way((200, 50), most), [] as [u32; 0]);
        // Two more make 1200 points: the longest unseen leaves, which is enough...
        assert_eq!(f.grooms_in_the_way((400, 100), most), [3]);
        // ...three more take both that nobody draws...
        assert_eq!(f.grooms_in_the_way((600, 150), most), [3, 1]);
        // ...and those in view never leave, whatever is asked.
        assert_eq!(f.grooms_in_the_way((5000, 150), most), [3, 1]);
        // The strands can be what is short: 200 held and 350 coming, where 500 fit.
        assert_eq!(f.grooms_in_the_way((0, 350), most), [3]);
        // A device whose arrays may still grow sends nobody away.
        assert_eq!(f.grooms_in_the_way((600, 150), 1 << 30), [] as [u32; 0]);
    }

    #[test]
    fn the_groom_arrays_are_made_again_only_where_that_makes_room() {
        // Free room in two pieces of 15 points, of 100; strands to spare.
        let points = Spans {
            capacity: 100,
            free: vec![(10, 15), (60, 15)],
        };
        let strands = Spans::new(50, 10);
        // A groom of 30 points fits neither piece, and the two together hold it...
        assert_eq!(placed(points.clone(), strands.clone(), &[(30, 5)]), 0);
        assert!(worth_making_again(&points, &strands, &[(30, 5)], (100, 50)));
        // ...one of 40 does not fit even then: arrays as large as they may be stay as they
        // are (they were made again, every groom copied, each time it asked)...
        assert!(!worth_making_again(
            &points,
            &strands,
            &[(40, 5)],
            (100, 50)
        ));
        // ...and are made again where they may grow.
        assert!(worth_making_again(&points, &strands, &[(40, 5)], (128, 50)));
        // The strands can be what is short: a groom with room for its points only takes none.
        let few = Spans::new(50, 48);
        assert_eq!(placed(points.clone(), few.clone(), &[(10, 5), (10, 2)]), 1);
        assert!(!worth_making_again(&points, &few, &[(10, 5)], (100, 50)));
        assert!(worth_making_again(&points, &few, &[(10, 5)], (100, 64)));
        // What fits as the room lies needs nothing made again.
        assert_eq!(
            placed(points.clone(), strands.clone(), &[(15, 5), (15, 5)]),
            2
        );
        assert!(!worth_making_again(
            &points,
            &strands,
            &[(15, 5), (15, 5)],
            (100, 50)
        ));
        // An array's size: a power of two, the first size at least, the device's most at most.
        assert_eq!(capacity_for(0, 64, 16, 1 << 30), 64);
        assert_eq!(capacity_for(65, 64, 16, 1 << 30), 128);
        assert_eq!(capacity_for(1 << 40, 64, 16, 16 * 1000), 1000);
        assert_eq!(capacity_for(1 << 40, 64, 16, u64::MAX), 1 << 31);
        // Past 64 MB (4 M points, 2 M strands) it grows by as much, not to twice its size:
        // thirty coats' 24 M points get room for 24 M and no more.
        assert_eq!(capacity_for(4 << 20, 64, 16, u64::MAX), 4 << 20);
        assert_eq!(capacity_for((4 << 20) + 1, 64, 16, u64::MAX), 8 << 20);
        assert_eq!(capacity_for((8 << 20) + 1, 64, 16, u64::MAX), 12 << 20);
        assert_eq!(capacity_for(24_000_000, 64, 16, u64::MAX), 24 << 20);
        assert_eq!(capacity_for(6_000_000, 64, 32, u64::MAX), 6 << 20);
        assert_eq!(capacity_for(24_000_000, 64, 16, 128 << 20), 8 << 20);
    }

    #[test]
    fn strand_slots_stay_while_they_fit() {
        // New: the strands drawn and some room.
        assert_eq!(strand_slots(1000, 200_000, None), 1250);
        // The number drifts: the slots stay.
        assert_eq!(strand_slots(1100, 200_000, Some(1250)), 1250);
        assert_eq!(strand_slots(1250, 200_000, Some(1250)), 1250);
        assert_eq!(strand_slots(800, 200_000, Some(1250)), 1250);
        // More strands than slots, or far fewer: made again.
        assert_eq!(strand_slots(1251, 200_000, Some(1250)), 1564);
        assert_eq!(strand_slots(700, 200_000, Some(1250)), 875);
        // Never more than the groom has, never fewer than drawn.
        assert_eq!(strand_slots(190_000, 200_000, None), 200_000);
        assert_eq!(strand_slots(16, 16, Some(40)), 16);
        assert_eq!(strand_slots(1, 1, None), 1);
        // Without room the number goes in steps, sixteen to a doubling, and never up.
        assert_eq!(stepped_count(1000.0), 992.0);
        assert_eq!(stepped_count(1023.9), 992.0);
        assert_eq!(stepped_count(1024.0), 1024.0);
        assert_eq!(stepped_count(87_500.0), 86_016.0);
        assert_eq!(stepped_count(31.9), 31.0);
        assert_eq!(stepped_count(1.0), 1.0);
    }
}
