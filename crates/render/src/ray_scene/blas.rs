//! The ray scene's geometries: a bottom-level acceleration structure each, built by the GPU
//! from the feature's own buffers where they carry `BLAS_INPUT` (no copy is made) or from a
//! copy of the triangles a feature holds on the CPU. A frame builds what its allowance
//! holds; large structures are compacted once built; a structure that leaves the scene is
//! kept until the GPU is past every frame that may still trace it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use wgpu::util::DeviceExt;

use super::hook::{RayGeometryState, RayKey, RaySource};
use super::tris::{Job, Records};

/// Triangles whose structures may be built a frame. A geometry with more builds as the first
/// of its frame, or, once it had to wait a frame, in the next whatever was built before it
/// (so nothing can keep it waiting). A zone's arrival builds over a few frames.
const BUILD_TRIANGLES_PER_FRAME: usize = 200_000;
/// Structures that may be made a frame (each is a few driver objects).
const BUILDS_PER_FRAME: u32 = 256;
/// Structures out of the scene that may be freed a frame (each is a few driver calls on the
/// render thread): what a zone left behind goes over a few frames, as it came.
const FREES_PER_FRAME: u32 = 64;
/// Geometries with more triangles than this have their structure compacted once it has
/// served a while (a copy that frees what the build reserved): each that may be compacted
/// carries a query and a readback for its size, so a patch of ground or a small prop is left
/// as built.
const COMPACT_ABOVE_TRIANGLES: u32 = 4_000;
/// Frames a structure serves before it is compacted: one built again soon after (a mesh's
/// coarse level after the whole mesh it streamed in as, an edit) is not worth the copy.
const COMPACT_AFTER_FRAMES: u64 = 120;
/// A geometry built from a feature's own buffers with more triangles than this is counted
/// (`render.ray.over_cap`): the most the plan gives a mesh's ray level. Nothing holds a
/// geometry to it until the stops have been counted.
const CAP_TRIANGLES: u32 = 60_000;
/// The label of the scene's bottom-level structures (wgpu-core labels a compacted copy
/// "<label> (compacted)"): their memory is read from the allocator's report by it.
const BLAS_LABEL: &str = "ray scene blas";
/// Frames the structures must have been left alone before their memory is read, and the
/// most frames a change waits for its reading while they keep changing (a reading walks every
/// allocation of the device).
const MEMORY_SETTLE_FRAMES: u32 = 30;
const MEMORY_MOST_FRAMES: u32 = 600;
/// Frames a geometry whose version changed keeps its last structure before it is built
/// again: a mesh edited every frame is built twice a second at 60 fps, and the rays see its
/// shape from a moment ago.
const REBUILD_AFTER_FRAMES: u64 = 30;
/// Frames a geometry stays after its last instance left.
const KEEP_UNUSED_FRAMES: u64 = 600;
/// Triangles of [`RaySource::UnitDisc`].
const DISC_TRIANGLES: u32 = 24;

/// A geometry's compaction (`Geometry::compaction`).
const COMPACT_NOT_ASKED: u8 = 0;
const COMPACT_ASKED: u8 = 1;
const COMPACT_READY: u8 = 2;
/// Compacted, failed, or too small to be worth it: nothing more to do.
const COMPACT_DONE: u8 = 3;

/// The top-level structures a bottom-level one may be in: the bits of [`Geometry::slots`]'
/// places and of [`Retired::held_by`].
pub(super) const STILL: usize = 0;
pub(super) const MOVING: usize = 1;

/// A feature's geometry: the feature's place among the renderer's, and its own name.
pub(super) type Name = (u16, RayKey);

/// Where a geometry's triangles came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    /// A feature's own buffers (a mesh's ranges of the pool).
    Buffers,
    /// A copy a feature made on the CPU (a mesh drawn only as clusters).
    Copy,
    /// One of the scene's own shapes.
    Shape,
}

/// Standing counts of the geometries held.
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct Held {
    pub triangles: u64,
    /// From a feature's buffers: their triangles.
    pub buffer_triangles: u64,
    /// Of those, the geometries with more than [`CAP_TRIANGLES`], and their triangles.
    pub over_cap: u32,
    pub over_cap_triangles: u64,
    /// From a copy on the CPU, and their triangles.
    pub copied: u32,
    pub copied_triangles: u64,
}

impl Held {
    /// A geometry of `triangles` from `origin` came (`sign` 1) or went (-1).
    fn count(&mut self, triangles: u32, origin: Origin, sign: i32) {
        let t = sign as i64 * triangles as i64;
        self.triangles = self.triangles.wrapping_add_signed(t);
        match origin {
            Origin::Buffers => {
                self.buffer_triangles = self.buffer_triangles.wrapping_add_signed(t);
                if triangles > CAP_TRIANGLES {
                    self.over_cap = self.over_cap.wrapping_add_signed(sign);
                    self.over_cap_triangles = self.over_cap_triangles.wrapping_add_signed(t);
                }
            }
            Origin::Copy => {
                self.copied = self.copied.wrapping_add_signed(sign);
                self.copied_triangles = self.copied_triangles.wrapping_add_signed(t);
            }
            Origin::Shape => {}
        }
    }
}

pub(super) struct Geometry {
    name: Name,
    pub version: u64,
    /// The frame its structure was built.
    built: u64,
    /// The frame it was last asked for or had an instance.
    last_used: u64,
    pub triangles: u32,
    origin: Origin,
    opaque: bool,
    pub blas: wgpu::Blas,
    /// One of the `COMPACT_` states; the callback sets ready or done.
    compaction: Arc<AtomicU8>,
    /// Instances placed with it.
    pub users: u32,
    /// Slots of each top-level structure that hold a structure of its: set with one, or
    /// set with one when that top-level structure was last built (which then still holds
    /// it, whatever the slot holds now).
    pub slots: [u32; 2],
    /// Its triangle records while `blas` is its structure: the header's record, and how
    /// many (`triangles + 1`). `None`: none (not the pool's vertices, or no room).
    pub records: Option<(u32, u32)>,
    /// The records it was to have and was refused (`triangles + 1`; 0: it has them, or is
    /// not built from the pool's vertices): counted in [`Store::missing_records`].
    missing: u32,
}

/// A structure out of the scene (its geometry built again, compacted or forgotten), kept
/// until no top-level structure holds it and the GPU has finished every frame that may
/// still trace it: wgpu frees a structure with its last handle, whatever the GPU is doing.
struct Retired {
    _blas: wgpu::Blas,
    /// Bits (`1 << STILL`, `1 << MOVING`) of the top-level structures that may hold it
    /// ([`Geometry::slots`] when it left): each lets go when it is next built.
    held_by: u8,
    /// The submission after which nothing traces it (0: not known yet).
    gone_after: u64,
    /// The triangle records written for it (rows that hold it read them): released when it
    /// is freed.
    records: Option<(u32, u32)>,
}

impl Retired {
    /// `slots`: how many slots of each top-level structure hold it now.
    fn new(blas: wgpu::Blas, slots: [u32; 2], records: Option<(u32, u32)>) -> Self {
        Self {
            _blas: blas,
            held_by: (slots[STILL] > 0) as u8 | ((slots[MOVING] > 0) as u8) << MOVING,
            gone_after: 0,
            records,
        }
    }
}

/// A structure this frame builds, and where its triangles are.
struct Pending {
    geometry: u32,
    size: wgpu::BlasTriangleGeometrySizeDescriptor,
    vertex: wgpu::Buffer,
    first_vertex: u32,
    vertex_stride: u64,
    index: wgpu::Buffer,
    first_index: u32,
}

/// A structure of one triangle that nothing points at, built in a call that builds a
/// top-level structure after compacted copies were swapped in and builds no other. wgpu
/// records a compaction's copy with no barrier after it (`Queue::compact_blas`), and puts
/// one after a call's bottom-level builds, before its top-level builds, only when the call
/// has any: built with the fence, the top-level build reads the compacted copies (for their
/// bounds) after they are written.
struct Fence {
    blas: wgpu::Blas,
    size: wgpu::BlasTriangleGeometrySizeDescriptor,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
}

impl Fence {
    fn new(device: &wgpu::Device) -> Self {
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: 3,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(3),
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let corners: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ray scene fence vertices"),
            contents: bytemuck::cast_slice(&corners),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ray scene fence indices"),
            contents: bytemuck::cast_slice(&[0u32, 1, 2]),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("ray scene fence"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        Self {
            blas,
            size,
            vertices,
            indices,
        }
    }

    fn entry(&self) -> wgpu::BlasBuildEntry<'_> {
        wgpu::BlasBuildEntry {
            blas: &self.blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                size: &self.size,
                vertex_buffer: &self.vertices,
                first_vertex: 0,
                vertex_stride: 12,
                index_buffer: Some(&self.indices),
                first_index: Some(0),
                transform_buffer: None,
                transform_buffer_offset: None,
            }]),
        }
    }
}

/// `render.ray.*` numbers of the geometries.
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct StoreStats {
    /// Geometries with a structure, and their triangles by where they came from.
    pub geometries: u32,
    pub held: Held,
    /// Bytes of GPU memory under the structures, as last read (`None`: not read yet, or the
    /// backend has no report).
    pub blas_bytes: Option<u64>,
    /// Structures built and their triangles, this frame.
    pub built: u32,
    pub built_triangles: u32,
    /// Structures compacted so far, and times the fence was built.
    pub compacted: u32,
    pub fences: u32,
    /// Structures out of the scene that wait for the GPU.
    pub retired: u32,
}

/// The indices of the triangles whose corners are all among `positions` vertices, in order (a
/// trailing partial triangle is dropped).
pub(crate) fn kept_indices(positions: usize, indices: &[u32]) -> Vec<u32> {
    let mut kept = Vec::with_capacity(indices.len() - indices.len() % 3);
    for t in indices.as_chunks::<3>().0 {
        if t.iter().all(|&i| (i as usize) < positions) {
            kept.extend_from_slice(t);
        }
    }
    kept
}

/// [`RaySource::UnitSquare`]: corners and indices, facing up (counter-clockwise from above).
fn unit_square() -> (Vec<[f32; 3]>, Vec<u32>) {
    (
        vec![
            [-1.0, 0.0, -1.0],
            [-1.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, -1.0],
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

/// [`RaySource::UnitDisc`]: a fan around the origin, facing up.
fn unit_disc() -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = vec![[0.0, 0.0, 0.0]];
    let mut indices = Vec::with_capacity(DISC_TRIANGLES as usize * 3);
    for i in 0..DISC_TRIANGLES {
        let a = std::f32::consts::TAU * i as f32 / DISC_TRIANGLES as f32;
        // Around the up axis from +z toward +x: counter-clockwise seen from above.
        positions.push([a.sin(), 0.0, a.cos()]);
        indices.extend([0, 1 + i, 1 + (i + 1) % DISC_TRIANGLES]);
    }
    (positions, indices)
}

/// The geometries and their structures.
pub(super) struct Store {
    device: wgpu::Device,
    list: Vec<Option<Geometry>>,
    free: Vec<u32>,
    names: HashMap<Name, u32>,
    /// Geometries that will never have a structure, by version.
    refused: HashMap<Name, u64>,
    /// Geometries whose structure became another since [`take_changed`](Self::take_changed):
    /// the slots of their instances point at the one before.
    changed: Vec<u32>,
    pending: Vec<Pending>,
    /// Structures built in the frame being recorded (wgpu can't prepare a compaction before
    /// the build is submitted).
    built_now: Vec<u32>,
    /// Large structures built and submitted, by the frame they were built: asked to compact
    /// once they have served [`COMPACT_AFTER_FRAMES`] (those built again since are dropped).
    to_compact: std::collections::VecDeque<(u32, u64)>,
    /// Geometries asked to compact whose answer is awaited: all that
    /// [`swap_compacted`](Self::swap_compacted) looks at.
    compacting: Vec<u32>,
    /// Triangles built this frame, and structures made.
    budget_used: usize,
    builds_now: u32,
    /// A geometry with more triangles than a frame's allowance that had to wait last frame
    /// because others were built before it: built this frame when it is asked for, whatever
    /// was built before. And the one that waited this frame, for the next.
    large_turn: Option<Name>,
    large_next: Option<Name>,
    /// Standing counts of the geometries held.
    held: Held,
    /// The records the geometries held were refused ([`Geometry::missing`], summed).
    missing_records: u64,
    /// A structure was made, swapped for its compacted copy or freed since the last
    /// [`read_memory`](Self::read_memory); frames since the last such change; frames the
    /// first change no reading has seen yet has waited (0: none).
    memory_changed: bool,
    memory_quiet: u32,
    memory_unread: u32,
    blas_bytes: Option<u64>,
    /// Compacted copies were swapped in and no call has built a bottom-level structure
    /// since: the next call that builds a top-level one builds the [`Fence`] first.
    swapped: bool,
    fence: Option<Fence>,
    retired: Vec<Retired>,
    /// Submissions so far (`after_submit`), and the last whose work the GPU has finished
    /// (the queue's callback sets it).
    submitted: u64,
    done: Arc<AtomicU64>,
    /// This frame recorded builds (`encode`).
    encoded: bool,
    /// A submission holding builds failed (`after_submit`): what the GPU has of the scene is
    /// unknown, so it starts over.
    lost: bool,
    stats: StoreStats,
}

impl Store {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            list: Vec::new(),
            free: Vec::new(),
            names: HashMap::new(),
            refused: HashMap::new(),
            changed: Vec::new(),
            pending: Vec::new(),
            built_now: Vec::new(),
            to_compact: std::collections::VecDeque::new(),
            compacting: Vec::new(),
            budget_used: 0,
            builds_now: 0,
            large_turn: None,
            large_next: None,
            held: Held::default(),
            missing_records: 0,
            memory_changed: false,
            memory_quiet: 0,
            memory_unread: 0,
            blas_bytes: None,
            swapped: false,
            fence: None,
            retired: Vec::new(),
            submitted: 0,
            done: Arc::new(AtomicU64::new(0)),
            encoded: false,
            lost: false,
            stats: StoreStats::default(),
        }
    }

    pub(super) fn lost(&self) -> bool {
        self.lost
    }

    /// The triangle records the geometries held were to have and were refused: what more
    /// room the records need (`tris::Records::stats`).
    pub(super) fn missing_records(&self) -> u64 {
        self.missing_records
    }

    pub(super) fn stats(&self) -> StoreStats {
        StoreStats {
            geometries: self.names.len() as u32,
            held: self.held,
            blas_bytes: self.blas_bytes,
            retired: self.retired.len() as u32,
            ..self.stats
        }
    }

    /// Start a frame: the build allowance is whole again, and the structures out of the scene
    /// that the GPU is past go, [`FREES_PER_FRAME`] at most, with their triangle records.
    pub(super) fn begin_frame(&mut self, records: &mut Records) {
        self.budget_used = 0;
        self.builds_now = 0;
        self.large_turn = self.large_next.take();
        self.stats.built = 0;
        self.stats.built_triangles = 0;
        let done = self.done.load(Ordering::Acquire);
        let mut freed = 0;
        self.retired.retain(|r| {
            let past = r.gone_after != 0 && r.gone_after <= done;
            if past && freed < FREES_PER_FRAME {
                freed += 1;
                if let Some((first, len)) = r.records {
                    records.release(first, len);
                }
                return false;
            }
            true
        });
        self.memory_changed |= freed > 0;
    }

    pub(super) fn get(&self, index: u32) -> Option<&Geometry> {
        self.list.get(index as usize)?.as_ref()
    }

    pub(super) fn get_mut(&mut self, index: u32) -> Option<&mut Geometry> {
        self.list.get_mut(index as usize)?.as_mut()
    }

    pub(super) fn index_of(&self, name: Name) -> Option<u32> {
        self.names.get(&name).copied()
    }

    /// Whether `name` has a structure for `version` (asked for: it stays a while longer).
    /// One for another version is not kept by the asking: a geometry whose new version is
    /// refused would otherwise keep its last structure for good.
    pub(super) fn has(&mut self, name: Name, version: u64, frame: u64) -> bool {
        let Some(g) = self
            .names
            .get(&name)
            .and_then(|&i| self.list[i as usize].as_mut())
        else {
            return false;
        };
        if g.version != version {
            return false;
        }
        g.last_used = frame;
        true
    }

    /// Instances placed with geometry `name`.
    pub(super) fn users(&self, name: Name) -> u32 {
        self.index_of(name)
            .and_then(|i| self.get(i))
            .map_or(0, |g| g.users)
    }

    /// Keep `blas` and the triangle records written for it until no top-level structure
    /// holds it and the GPU is past the frames that may trace it. `slots`: how many slots of
    /// each top-level structure hold it now.
    fn retire(&mut self, blas: wgpu::Blas, slots: [u32; 2], records: Option<(u32, u32)>) {
        self.retired.push(Retired::new(blas, slots, records));
    }

    /// Geometry `name` at `version` for an instance read at `frame`: built now if the
    /// frame's allowance holds it (see [`super::RaySceneOut::geometry`]). `records`: its
    /// triangle records are written too, from the pool's vertices `source` names.
    pub(super) fn want(
        &mut self,
        name: Name,
        version: u64,
        opaque: bool,
        source: RaySource<'_>,
        frame: u64,
        records: Option<&mut Records>,
    ) -> RayGeometryState {
        if self.refused.get(&name) == Some(&version) {
            return RayGeometryState::Refused;
        }
        let existing = self.names.get(&name).copied();
        if let Some(g) = existing.and_then(|i| self.list[i as usize].as_mut()) {
            g.last_used = frame;
            debug_assert_eq!(
                g.opaque, opaque,
                "a geometry's triangles stop rays or are asked, for good: a feature names the \
                 two ways apart"
            );
            if g.version == version {
                return RayGeometryState::Ready;
            }
            if frame < g.built + REBUILD_AFTER_FRAMES {
                return RayGeometryState::Older;
            }
        }
        let wait = if existing.is_some() {
            RayGeometryState::Older
        } else {
            RayGeometryState::Building
        };
        // The allowance first, by the most triangles the source can have: a geometry that
        // waits is asked for every frame until it's built.
        let (most, origin) = match &source {
            RaySource::Buffers { index_count, .. } => (*index_count / 3, Origin::Buffers),
            RaySource::Cpu { indices, .. } => ((indices.len() / 3) as u32, Origin::Copy),
            RaySource::UnitSquare => (2, Origin::Shape),
            RaySource::UnitDisc => (DISC_TRIANGLES, Origin::Shape),
        };
        let large = most as usize > BUILD_TRIANGLES_PER_FRAME;
        let its_turn = large && self.large_turn == Some(name);
        if self.budget_used > 0
            && !its_turn
            && (self.budget_used + most as usize > BUILD_TRIANGLES_PER_FRAME
                || self.builds_now >= BUILDS_PER_FRAME)
        {
            // One too large to follow anything in a frame: the next frame builds it whatever
            // came before it (one such a frame).
            if large && self.large_next.is_none() {
                self.large_next = Some(name);
            }
            return wait;
        }
        if its_turn {
            self.large_turn = None;
        }
        let limits = self.device.limits();
        let blas_input = |label, contents: &[u8]| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::BLAS_INPUT,
                })
        };
        let from_cpu = |positions: &[[f32; 3]], indices: &[u32]| {
            (
                blas_input("ray scene vertices", bytemuck::cast_slice(positions)),
                0u32,
                positions.len() as u32,
                12u64,
                blas_input("ray scene indices", bytemuck::cast_slice(indices)),
                wgpu::IndexFormat::Uint32,
                0u32,
                indices.len() as u32,
            )
        };
        let (
            vertex,
            first_vertex,
            vertex_count,
            vertex_stride,
            index,
            index_format,
            first_index,
            index_count,
        ) = match source {
            RaySource::Buffers {
                vertex,
                first_vertex,
                vertex_count,
                vertex_stride,
                index,
                index_format,
                first_index,
                index_count,
            } => (
                vertex.clone(),
                first_vertex,
                vertex_count,
                vertex_stride as u64,
                index.clone(),
                index_format,
                first_index,
                index_count - index_count % 3,
            ),
            RaySource::Cpu { positions, indices } => {
                // (Empty buffers can't be made: an empty geometry is refused below.)
                let kept = kept_indices(positions.len(), indices);
                if kept.is_empty() {
                    self.refused.insert(name, version);
                    return RayGeometryState::Refused;
                }
                from_cpu(positions, &kept)
            }
            RaySource::UnitSquare => {
                let (positions, indices) = unit_square();
                from_cpu(&positions, &indices)
            }
            RaySource::UnitDisc => {
                let (positions, indices) = unit_disc();
                from_cpu(&positions, &indices)
            }
        };
        let triangles = index_count / 3;
        // (wgpu holds the vertex count to the same limit as the triangles.)
        if triangles == 0
            || vertex_count == 0
            || triangles > limits.max_blas_primitive_count
            || vertex_count > limits.max_blas_primitive_count
            || !vertex_stride.is_multiple_of(4)
            || vertex_stride < 12
        {
            self.refused.insert(name, version);
            return RayGeometryState::Refused;
        }
        self.budget_used += triangles as usize;
        self.builds_now += 1;
        // Its records, from the pool's vertices and `u32` indices only.
        let (range, missing) = match records {
            Some(records)
                if vertex_stride == crate::mesh_pool::VERTEX_BYTES
                    && index_format == wgpu::IndexFormat::Uint32 =>
            {
                match records.take(triangles, &vertex, &index) {
                    Some(range) => (Some(range), 0),
                    None => (None, triangles + 1),
                }
            }
            _ => (None, 0),
        };
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count,
            index_format: Some(index_format),
            index_count: Some(index_count),
            flags: if opaque {
                wgpu::AccelerationStructureGeometryFlags::OPAQUE
            } else {
                wgpu::AccelerationStructureGeometryFlags::empty()
            },
        };
        // Compacted once it has served a while (`after_submit`): a large geometry's.
        let compact = triangles > COMPACT_ABOVE_TRIANGLES;
        // The rays read the hit triangle's corners (`getCommittedHitVertexPositions`).
        let mut flags = wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
            | wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN;
        if compact {
            flags |= wgpu::AccelerationStructureFlags::ALLOW_COMPACTION;
        }
        let blas = self.device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some(BLAS_LABEL),
                flags,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        let compaction = Arc::new(AtomicU8::new(if compact {
            COMPACT_NOT_ASKED
        } else {
            COMPACT_DONE
        }));
        let index_of = match existing {
            Some(i) => {
                let g = self.list[i as usize].as_mut().expect("named geometry");
                let old = std::mem::replace(&mut g.blas, blas);
                // (The old records go with the old structure: rows that hold it read them.)
                let old_range = std::mem::replace(&mut g.records, range);
                self.missing_records -= g.missing as u64;
                self.missing_records += missing as u64;
                g.missing = missing;
                g.version = version;
                g.built = frame;
                self.held.count(g.triangles, g.origin, -1);
                self.held.count(triangles, origin, 1);
                g.triangles = triangles;
                g.origin = origin;
                g.compaction = compaction;
                let slots = g.slots;
                self.retire(old, slots, old_range);
                self.changed.push(i);
                // One still pending for it gives way to this one.
                self.pending.retain(|p| p.geometry != i);
                i
            }
            None => {
                let g = Geometry {
                    name,
                    version,
                    built: frame,
                    last_used: frame,
                    triangles,
                    origin,
                    opaque,
                    blas,
                    compaction,
                    users: 0,
                    slots: [0; 2],
                    records: range,
                    missing,
                };
                let i = match self.free.pop() {
                    Some(i) => {
                        self.list[i as usize] = Some(g);
                        i
                    }
                    None => {
                        self.list.push(Some(g));
                        self.list.len() as u32 - 1
                    }
                };
                self.names.insert(name, i);
                self.held.count(triangles, origin, 1);
                self.missing_records += missing as u64;
                i
            }
        };
        debug_assert_eq!(
            self.get(index_of).map(|g| g.opaque),
            Some(opaque),
            "a geometry's triangles stop rays or are asked, for good"
        );
        self.refused.remove(&name);
        self.memory_changed = true;
        self.pending.push(Pending {
            geometry: index_of,
            size,
            vertex,
            first_vertex,
            vertex_stride,
            index,
            first_index,
        });
        self.stats.built += 1;
        self.stats.built_triangles += triangles;
        RayGeometryState::Ready
    }

    /// Swap the structures whose compaction is ready for their compacted copies; the slots
    /// that hold them are set again when their top-level structure is next built
    /// ([`take_changed`](Self::take_changed)). True when any was swapped. Only while no
    /// other thread may submit: wgpu-core's `compact_blas` holds the queue's pending writes
    /// while it takes the device's command indices, and `submit` takes them the other way
    /// round, so the two on different threads can deadlock.
    pub(super) fn swap_compacted(&mut self, queue: &wgpu::Queue) -> bool {
        if self.lost {
            return false;
        }
        let mut any = false;
        // Only those asked and not answered yet (a geometry built again or forgotten since
        // is neither: its state is done, or its place is another's).
        let mut compacting = std::mem::take(&mut self.compacting);
        compacting.retain(|&i| {
            let Some(g) = self.list.get_mut(i as usize).and_then(Option::as_mut) else {
                return false;
            };
            match g.compaction.load(Ordering::Acquire) {
                COMPACT_ASKED => return true,
                COMPACT_READY => {}
                _ => return false,
            }
            let compacted = queue.compact_blas(&g.blas);
            let full = std::mem::replace(&mut g.blas, compacted);
            g.compaction.store(COMPACT_DONE, Ordering::Release);
            // (The copy has the same triangles in the same order: the records stay.)
            self.retired.push(Retired::new(full, g.slots, None));
            self.changed.push(i);
            self.stats.compacted += 1;
            any = true;
            false
        });
        self.memory_changed |= any;
        self.compacting = compacting;
        if any {
            self.swapped = true;
            if self.fence.is_none() {
                self.fence = Some(Fence::new(&self.device));
            }
        }
        any
    }

    /// Record this frame's builds: the geometries' new structures, then the top-level
    /// structures in `tlas` (wgpu builds structures and those pointing at them in one call),
    /// then the triangle records of what was built. `built`: which of the two top-level
    /// structures `tlas` holds (bits). True when the records moved to another buffer.
    pub(super) fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        tlas: &[&wgpu::Tlas],
        built: u8,
        records: &mut Records,
    ) -> bool {
        // The top-level structures built here let go of the retired structures their slots
        // held: once the GPU finishes this frame's submission, nothing traces those that no
        // other holds (in a frame that builds nothing, those no structure held).
        let at = self.submitted + 1;
        for r in &mut self.retired {
            r.held_by &= !built;
            if r.held_by == 0 && r.gone_after == 0 {
                r.gone_after = at;
            }
        }
        if self.pending.is_empty() && tlas.is_empty() {
            return records.encode(encoder, &mut []);
        }
        let pending = std::mem::take(&mut self.pending);
        let mut entries: Vec<wgpu::BlasBuildEntry> = pending
            .iter()
            .filter_map(|p| {
                let g = self.list.get(p.geometry as usize)?.as_ref()?;
                Some(wgpu::BlasBuildEntry {
                    blas: &g.blas,
                    geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                        wgpu::BlasTriangleGeometry {
                            size: &p.size,
                            vertex_buffer: &p.vertex,
                            first_vertex: p.first_vertex,
                            vertex_stride: p.vertex_stride,
                            index_buffer: Some(&p.index),
                            first_index: Some(p.first_index),
                            transform_buffer: None,
                            transform_buffer_offset: None,
                        },
                    ]),
                })
            })
            .collect();
        if self.swapped
            && entries.is_empty()
            && !tlas.is_empty()
            && let Some(fence) = &self.fence
        {
            entries.push(fence.entry());
            self.stats.fences += 1;
        }
        if !entries.is_empty() {
            self.swapped = false;
        }
        encoder.build_acceleration_structures(entries.iter(), tlas.iter().copied());
        drop(entries);
        // The records of what was just built, from the ranges its structure was given (a
        // pending build is always for its geometry's structure as it stands).
        let mut jobs: Vec<Job<'_>> = pending
            .iter()
            .filter_map(|p| {
                let g = self.list.get(p.geometry as usize)?.as_ref()?;
                let (header, _) = g.records?;
                Some(Job {
                    vertex: &p.vertex,
                    index: &p.index,
                    first_vertex: p.first_vertex,
                    first_index: p.first_index,
                    triangles: g.triangles,
                    header,
                })
            })
            .collect();
        let regroup = records.encode(encoder, &mut jobs);
        drop(jobs);
        self.built_now.extend(pending.iter().map(|p| p.geometry));
        self.encoded = true;
        regroup
    }

    /// After the submit of frame `frame` (`failed`: wgpu rejected it, so none of its work
    /// runs): count it, so the structures retired before it are freed once the GPU is past
    /// it, and ask for the compaction of the large structures that have served
    /// [`COMPACT_AFTER_FRAMES`] (wgpu answers once the GPU finished building them;
    /// [`swap_compacted`](Self::swap_compacted) swaps them then). If it held builds and
    /// failed, what the GPU has of the scene is unknown: the scene is lost.
    pub(super) fn after_submit(&mut self, queue: &wgpu::Queue, failed: bool, frame: u64) {
        self.submitted += 1;
        let (done, at) = (self.done.clone(), self.submitted);
        queue.on_submitted_work_done(move || {
            done.fetch_max(at, Ordering::AcqRel);
        });
        if std::mem::take(&mut self.encoded) && failed {
            self.lost = true;
            self.built_now.clear();
            return;
        }
        for i in self.built_now.drain(..) {
            if let Some(g) = self.list.get(i as usize).and_then(Option::as_ref)
                && g.compaction.load(Ordering::Acquire) == COMPACT_NOT_ASKED
            {
                self.to_compact.push_back((i, g.built));
            }
        }
        // Oldest first: those built `COMPACT_AFTER_FRAMES` ago and not built again since.
        while let Some(&(i, built)) = self.to_compact.front() {
            if frame < built + COMPACT_AFTER_FRAMES {
                break;
            }
            self.to_compact.pop_front();
            let Some(g) = self
                .list
                .get(i as usize)
                .and_then(Option::as_ref)
                .filter(|g| g.built == built)
            else {
                continue;
            };
            let state = &g.compaction;
            if state
                .compare_exchange(
                    COMPACT_NOT_ASKED,
                    COMPACT_ASKED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
            {
                continue;
            }
            self.compacting.push(i);
            let state = state.clone();
            g.blas.prepare_compaction_async(move |done| {
                let now = if done.is_ok() {
                    COMPACT_READY
                } else {
                    COMPACT_DONE
                };
                state.store(now, Ordering::Release);
            });
        }
        self.read_memory();
    }

    /// After a frame's submit: read the memory under the scene's structures from the
    /// allocator's report when a change is due its reading ([`MEMORY_SETTLE_FRAMES`] after the
    /// last change, or [`MEMORY_MOST_FRAMES`] after the first unread one while they keep
    /// changing). The reading holds the structures as built, their compacted copies, and
    /// those out of the scene the GPU may still trace.
    fn read_memory(&mut self) {
        if std::mem::take(&mut self.memory_changed) {
            self.memory_quiet = 0;
            self.memory_unread = self.memory_unread.max(1);
        } else {
            self.memory_quiet = self.memory_quiet.saturating_add(1);
        }
        if self.memory_unread == 0 {
            return;
        }
        if self.memory_quiet < MEMORY_SETTLE_FRAMES && self.memory_unread < MEMORY_MOST_FRAMES {
            self.memory_unread += 1;
            return;
        }
        self.memory_unread = 0;
        if let Some(report) = self.device.generate_allocator_report() {
            self.blas_bytes = Some(
                report
                    .allocations
                    .iter()
                    .filter(|a| a.name.starts_with(BLAS_LABEL))
                    .map(|a| a.size)
                    .sum(),
            );
        }
    }

    /// Forget the geometries no instance used for [`KEEP_UNUSED_FRAMES`] and no top-level
    /// structure holds.
    pub(super) fn forget_unused(&mut self, frame: u64) {
        for i in 0..self.list.len() {
            let gone = self.list[i].as_ref().is_some_and(|g| {
                g.users == 0 && g.slots == [0; 2] && frame > g.last_used + KEEP_UNUSED_FRAMES
            });
            if !gone {
                continue;
            }
            let g = self.list[i].take().expect("checked above");
            self.held.count(g.triangles, g.origin, -1);
            self.missing_records -= g.missing as u64;
            self.names.remove(&g.name);
            self.pending.retain(|p| p.geometry != i as u32);
            self.free.push(i as u32);
            self.retire(g.blas, [0; 2], g.records);
        }
        if self.refused.len() > 4096 {
            self.refused.clear();
        }
    }

    /// The geometries whose structure became another since the last call.
    pub(super) fn take_changed(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.changed)
    }

    /// A slot of top-level structure `level` was set with geometry `index`'s structure.
    pub(super) fn hold_slot(&mut self, index: u32, level: usize) {
        if let Some(g) = self.get_mut(index) {
            g.slots[level] += 1;
        }
    }

    /// Top-level structure `level` was built: the slots that held the geometries in
    /// `released` (one entry a slot) before they were set again or emptied are let go.
    pub(super) fn release_slots(&mut self, level: usize, released: &mut Vec<u32>) {
        for index in released.drain(..) {
            if let Some(g) = self.get_mut(index) {
                g.slots[level] = g.slots[level].saturating_sub(1);
            }
        }
    }

    /// An instance of geometry `index` came (`delta` 1) or went (-1).
    pub(super) fn count_user(&mut self, index: u32, delta: i32, frame: u64) {
        if let Some(g) = self.get_mut(index) {
            g.users = g.users.saturating_add_signed(delta);
            g.last_used = frame;
        }
    }
}

impl Drop for Store {
    /// The frames the GPU may still be running trace these structures: a top-level structure
    /// in flight points at structures it may no longer hold, and wgpu frees a structure with
    /// its last handle. So wait for the GPU first. If the wait fails (wgpu panics on a lost
    /// device; a GPU stuck past the timeout), or while a panic unwinds, the structures are
    /// leaked instead.
    fn drop(&mut self) {
        if !self.lost && self.done.load(Ordering::Acquire) >= self.submitted {
            return;
        }
        let waited = !std::thread::panicking()
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(5)),
                })
            }))
            .is_ok_and(|polled| polled.is_ok());
        if !waited {
            std::mem::forget(std::mem::take(&mut self.list));
            std::mem::forget(std::mem::take(&mut self.retired));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_triangles_have_all_their_corners() {
        // Three corners in range, one out, and a trailing partial triangle.
        let indices = [0, 1, 2, 2, 1, 9, 3, 2, 1, 0, 1];
        assert_eq!(kept_indices(4, &indices), vec![0, 1, 2, 3, 2, 1]);
        assert!(kept_indices(2, &[0, 1, 2]).is_empty());
        assert!(kept_indices(3, &[]).is_empty());
    }

    /// The area a triangle list covers seen from above, counter-clockwise positive.
    fn area_from_above(positions: &[[f32; 3]], indices: &[u32]) -> f32 {
        indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| positions[i as usize]);
                // Seen from +y, x runs right and z runs toward the viewer's bottom: a
                // triangle is counter-clockwise from above when this is positive.
                0.5 * ((b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]))
            })
            .sum()
    }

    #[test]
    fn the_unit_shapes_face_up_and_cover_their_area() {
        let (positions, indices) = unit_square();
        assert!((area_from_above(&positions, &indices) - 4.0).abs() < 1e-5);
        let (positions, indices) = unit_disc();
        assert_eq!(indices.len() as u32, DISC_TRIANGLES * 3);
        let area = area_from_above(&positions, &indices);
        // A fan of 24 triangles inside the circle: a little under pi.
        assert!(area > 3.1 && area < std::f32::consts::PI, "{area}");
        // Every triangle faces up on its own.
        for t in indices.as_chunks::<3>().0 {
            assert!(area_from_above(&positions, t) > 0.0);
        }
    }
}
