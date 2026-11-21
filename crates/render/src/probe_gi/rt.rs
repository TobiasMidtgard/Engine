//! Probe GI's ray scene on ray-tracing hardware ([`GpuShared::ray_queries`]): each mesh's
//! triangles in a bottom-level acceleration structure the GPU builds, and one top-level
//! structure over the instances near the grid, which probe_gi_trace_hw.wgsl's ray queries walk
//! on the RT cores. The triangles are the ones the software hierarchy is built from
//! (`bvh::build_mesh`): the mesh's positions, and every triangle whose corners are all in
//! range, corners in index order. On such a GPU this replaces the software store, its builds
//! on workers and on the prepare threads, the instance hierarchy built and refit on the CPU,
//! and their uploads.
//!
//! [`GpuShared::ray_queries`]: crate::features::GpuShared::ray_queries

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use wgpu::util::DeviceExt;

use crate::mesh::Mesh;
use crate::ray_scene::stress_offset;

/// Triangles whose structures may be built a frame; a mesh with more builds alone in its
/// frame. A zone's arrival builds over a few frames, each mesh in the rays' scene once built.
const BUILD_TRIANGLES_PER_FRAME: usize = 200_000;
/// Structures that may be made a frame (each is a few driver objects: the structure, its
/// memory and its geometry's two uploads), so a zone of small props arrives over frames too.
const BUILDS_PER_FRAME: u32 = 256;
/// Meshes with more triangles than this have their structure compacted once built (a copy
/// that frees what the build reserved; not worth it for a few hundred triangles).
const COMPACT_ABOVE_TRIANGLES: u32 = 1_000;
/// Instances the top-level structure has room for at first; it doubles as needed.
const FIRST_CAPACITY: u32 = 1024;

/// A mesh structure's compaction (`HwMesh::compaction`).
const COMPACT_NOT_ASKED: u8 = 0;
const COMPACT_ASKED: u8 = 1;
const COMPACT_READY: u8 = 2;
/// Compacted, failed, or too small to be worth it: nothing more to do.
const COMPACT_DONE: u8 = 3;

/// What a read finds of a mesh's structure ([`HwScene::want`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Want {
    /// Built for the mesh's version (this frame's, at the latest).
    Ready,
    /// An older version's serves while the mesh's waits (edited in place, or the frame's
    /// budget spent): read the entity again next frame.
    Older,
    /// None yet (the frame's budget is spent): read the entity again next frame.
    Waiting,
    /// No triangle to hit.
    Empty,
}

/// A mesh's bottom-level structure.
struct HwMesh {
    version: u64,
    /// The frame it was built (`REBUILD_AFTER_FRAMES` counts from it).
    built: u64,
    last_used: u64,
    triangles: u32,
    blas: wgpu::Blas,
    /// Its compaction: one of the `COMPACT_` states, the callback sets ready or done.
    compaction: Arc<AtomicU8>,
}

/// A structure out of the scene (its mesh built again, emptied, compacted or forgotten), kept
/// until the GPU has finished every frame that may still trace it: a top-level structure built
/// before it left points at it, and wgpu frees a structure as soon as its last handle goes,
/// whatever the GPU is still doing.
struct Retired {
    _blas: wgpu::Blas,
    /// The submission whose top-level build first left it out (0: none yet).
    gone_after: u64,
}

/// A structure this frame builds: its geometry, uploaded for the build (wgpu keeps the buffers
/// alive until the GPU is done with them).
struct PendingBuild {
    key: u32,
    size: wgpu::BlasTriangleGeometrySizeDescriptor,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
}

/// A structure of one triangle that nothing points at, built in a frame that swapped in
/// compacted copies and builds no other ([`HwScene::encode`]). wgpu records a compaction's copy
/// with no barrier after it (`Queue::compact_blas`), and puts one after a call's bottom-level
/// builds, before its top-level build, only when the call has any: built with the fence, the
/// top-level build reads the compacted copies (for their bounds) after they are written.
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
            label: Some("probe gi rt fence vertices"),
            contents: bytemuck::cast_slice(&corners),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("probe gi rt fence indices"),
            contents: bytemuck::cast_slice(&[0u32, 1, 2]),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("probe gi rt fence"),
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

/// `render.gi.*` numbers of the hardware scene.
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct HwStats {
    /// The triangles of the meshes with a structure.
    pub triangles: u64,
    /// Structures built and their triangles, this frame.
    pub built: u32,
    pub built_triangles: u32,
    /// Instances in the top-level structure (slots holding one).
    pub instances: u32,
    /// The top-level structure was built this frame.
    pub tlas_built: bool,
}

/// The rays' scene in acceleration structures.
pub(super) struct HwScene {
    device: wgpu::Device,
    meshes: HashMap<u32, HwMesh>,
    /// Meshes with no triangle to hit, by version.
    empty: HashMap<u32, u64>,
    pending: Vec<PendingBuild>,
    /// Structures built in the frame being recorded, asked to compact once it's submitted
    /// (wgpu can't prepare a compaction before the build is submitted).
    built_now: Vec<u32>,
    tlas: wgpu::Tlas,
    capacity: u32,
    /// The top-level structure's slots that may hold an instance (the last placement's).
    slots: usize,
    /// The copies the last placement added for a measurement (`place`'s `stress`).
    stress: u32,
    /// Build the top-level structure this frame (its instances changed, or a structure they
    /// point at). Set at first, so the empty scene is built before any ray reads it.
    tlas_dirty: bool,
    /// Triangles built this frame, and structures made.
    budget_used: usize,
    builds_now: u32,
    compacted: u32,
    /// Compacted copies were swapped in since the last [`encode`](Self::encode), which builds
    /// the [`Fence`] if it builds no other structure (made at the first swap).
    swapped: bool,
    fence: Option<Fence>,
    /// Times the fence was built.
    fences: u32,
    stats: HwStats,
    retired: Vec<Retired>,
    /// Submissions so far (`after_submit`), and the last whose work the GPU has finished (the
    /// queue's callback sets it).
    submitted: u64,
    done: Arc<AtomicU64>,
    /// This frame recorded builds (`encode`).
    encoded: bool,
    /// A submission holding this scene's builds failed (`after_submit`): what the GPU has of the
    /// scene is unknown, so it starts over ([`Self::lost`]).
    lost: bool,
}

fn create_tlas(device: &wgpu::Device, capacity: u32) -> wgpu::Tlas {
    device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("probe gi rt tlas"),
        max_instances: capacity,
        // The rays read the hit triangle's corners (`getCommittedHitVertexPositions`): the
        // top-level structure must allow it as well as each mesh's.
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
            | wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    })
}

/// The indices of `mesh`'s triangles that `bvh::build_mesh` keeps: those whose corners are all
/// in range, in order (a trailing partial triangle is dropped).
pub(super) fn kept_indices(positions: usize, indices: &[u32]) -> Vec<u32> {
    let mut kept = Vec::with_capacity(indices.len() - indices.len() % 3);
    for t in indices.as_chunks::<3>().0 {
        if t.iter().all(|&i| (i as usize) < positions) {
            kept.extend_from_slice(t);
        }
    }
    kept
}

impl HwScene {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            meshes: HashMap::new(),
            empty: HashMap::new(),
            pending: Vec::new(),
            built_now: Vec::new(),
            tlas: create_tlas(device, FIRST_CAPACITY),
            capacity: FIRST_CAPACITY,
            slots: 0,
            stress: 0,
            tlas_dirty: true,
            budget_used: 0,
            builds_now: 0,
            compacted: 0,
            swapped: false,
            fence: None,
            fences: 0,
            stats: HwStats::default(),
            retired: Vec::new(),
            submitted: 0,
            done: Arc::new(AtomicU64::new(0)),
            encoded: false,
            lost: false,
        }
    }

    /// Keep `blas` until the GPU is past every frame that may trace it, and build the top-level
    /// structure again without it.
    fn retire(&mut self, blas: wgpu::Blas) {
        self.retired.push(Retired {
            _blas: blas,
            gone_after: 0,
        });
        self.tlas_dirty = true;
    }

    /// A submission holding this scene's builds failed: start the scene over (on a new one).
    pub(super) fn lost(&self) -> bool {
        self.lost
    }

    /// The top-level structure, as the trace's binding 1.
    pub(super) fn binding(&self) -> wgpu::BindingResource<'_> {
        self.tlas.as_binding()
    }

    /// Whether the top-level structure is to be built again this frame (its instances must be
    /// placed again first: a structure they point at was built again or compacted).
    pub(super) fn tlas_dirty(&self) -> bool {
        self.tlas_dirty
    }

    pub(super) fn stats(&self) -> HwStats {
        self.stats
    }

    /// Start a frame: the build budget is whole again, and the structures out of the scene that
    /// the GPU is past go.
    pub(super) fn begin_frame(&mut self) {
        self.budget_used = 0;
        self.builds_now = 0;
        self.stats.built = 0;
        self.stats.built_triangles = 0;
        self.stats.tlas_built = false;
        // Structures left out of a top-level build the GPU has finished are traced by nothing.
        let done = self.done.load(Ordering::Acquire);
        self.retired
            .retain(|r| r.gone_after == 0 || r.gone_after > done);
    }

    /// Swap the structures whose compaction is ready for their compacted copies (their
    /// instances are placed again this frame, before the top-level build). Only while no other
    /// thread may submit: wgpu-core's `compact_blas` holds the queue's pending writes while it
    /// takes the device's command indices, and `submit` takes them the other way round, so the
    /// two on different threads can deadlock. The renderer calls this before the prepare
    /// threads start (they submit buffer copies, and the gather runs on one of them).
    pub(super) fn swap_compacted(&mut self, queue: &wgpu::Queue) {
        if self.lost {
            return;
        }
        for mesh in self.meshes.values_mut() {
            if mesh.compaction.load(Ordering::Acquire) != COMPACT_READY {
                continue;
            }
            let compacted = queue.compact_blas(&mesh.blas);
            let full = std::mem::replace(&mut mesh.blas, compacted);
            mesh.compaction.store(COMPACT_DONE, Ordering::Release);
            self.retired.push(Retired {
                _blas: full,
                gone_after: 0,
            });
            self.compacted += 1;
            self.tlas_dirty = true;
            self.swapped = true;
        }
        if self.swapped && self.fence.is_none() {
            self.fence = Some(Fence::new(&self.device));
        }
    }

    /// Mesh `key` at `version` (`mesh`: its asset, `None` while not loaded) for an entity read
    /// at `frame`: built now if the frame's budget allows. A mesh edited in place keeps its last
    /// structure for `REBUILD_AFTER_FRAMES` (a mesh edited every frame is built twice a second
    /// at 60 fps, as the software path does).
    pub(super) fn want(
        &mut self,
        device: &wgpu::Device,
        key: u32,
        version: u64,
        mesh: Option<&Mesh>,
        frame: u64,
    ) -> Want {
        if self.empty.get(&key) == Some(&version) {
            return Want::Empty;
        }
        let older = match self.meshes.get_mut(&key) {
            Some(m) => {
                m.last_used = frame;
                if m.version == version {
                    return Want::Ready;
                }
                if frame < m.built + super::REBUILD_AFTER_FRAMES {
                    return Want::Older;
                }
                true
            }
            None => false,
        };
        let wait = if older { Want::Older } else { Want::Waiting };
        let Some(mesh) = mesh else {
            return wait;
        };
        // The budget first, by the most triangles the mesh can have: an entity whose mesh waits
        // is read every frame until it's built.
        if self.budget_used > 0
            && (self.budget_used + mesh.indices.len() / 3 > BUILD_TRIANGLES_PER_FRAME
                || self.builds_now >= BUILDS_PER_FRAME)
        {
            return wait;
        }
        let indices = kept_indices(mesh.vertices.len(), &mesh.indices);
        if indices.is_empty() {
            self.empty.insert(key, version);
            if let Some(old) = self.meshes.remove(&key) {
                // Its instances lose their structure.
                self.retire(old.blas);
            }
            return Want::Empty;
        }
        let triangles = indices.len() / 3;
        self.budget_used += triangles;
        self.builds_now += 1;
        let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("probe gi rt vertices"),
            contents: bytemuck::cast_slice(&positions),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("probe gi rt indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: positions.len() as u32,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(indices.len() as u32),
            // Every triangle stops a ray (the trace forces it besides): no candidate is ever
            // left for the query's loop to confirm.
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        // Compacted once built: a large mesh's first structure (one built again because the mesh
        // was edited in place is likely to be built again soon). Only those ask for it, as each
        // that may be compacted carries a query and a readback for its size.
        let compact = triangles as u32 > COMPACT_ABOVE_TRIANGLES && !older;
        let mut flags = wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
            | wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN;
        if compact {
            flags |= wgpu::AccelerationStructureFlags::ALLOW_COMPACTION;
        }
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("probe gi rt blas"),
                flags,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        // A structure built again this frame replaces any still pending for the mesh.
        self.pending.retain(|p| p.key != key);
        self.pending.push(PendingBuild {
            key,
            size,
            vertices,
            indices: index_buffer,
        });
        let compaction = if compact {
            COMPACT_NOT_ASKED
        } else {
            COMPACT_DONE
        };
        let old = self.meshes.insert(
            key,
            HwMesh {
                version,
                built: frame,
                last_used: frame,
                triangles: triangles as u32,
                blas,
                compaction: Arc::new(AtomicU8::new(compaction)),
            },
        );
        if let Some(old) = old {
            self.retire(old.blas);
        }
        self.empty.remove(&key);
        self.stats.built += 1;
        self.stats.built_triangles += triangles as u32;
        // The instances of the mesh point at its new structure.
        self.tlas_dirty = true;
        Want::Ready
    }

    /// Put the instances in the top-level structure: slot `i` holds mesh `placements[i].0`'s
    /// structure moved by `placements[i].1` (anchor-relative mesh -> world, rows of a 3x4), and
    /// answers rays with `i` (the instance row `instances[i]`). A slot whose mesh has no
    /// structure holds nothing. True when the structure was made again with more or less room
    /// (bind it again).
    ///
    /// `stress` (`RenderSettings::ray_scene_stress`, a measurement): that many more slots after
    /// them, each a copy of one of the instances moved by [`stress_offset`], answering rays
    /// with the row of the instance it copies. The caller places every frame while it is not
    /// 0, so the structure is built again every frame.
    pub(super) fn place(
        &mut self,
        device: &wgpu::Device,
        placements: &[(u32, [f32; 12])],
        stress: u32,
    ) -> bool {
        let mut grown = false;
        let most = device.limits().max_tlas_instance_count as usize;
        let copies = if placements.is_empty() {
            0
        } else {
            (stress as usize).min(most.saturating_sub(placements.len()))
        };
        let total = placements.len() + copies;
        let needed = total as u32;
        // (Back to a structure of the usual size once the measurement's copies are gone.)
        let fit = needed.next_power_of_two().max(FIRST_CAPACITY);
        if needed > self.capacity || (copies == 0 && self.stress != 0 && fit < self.capacity) {
            self.capacity = fit;
            self.tlas = create_tlas(device, self.capacity);
            self.slots = 0;
            grown = true;
        }
        let mut instances = 0;
        // The slots of this list and the last (those past both are empty).
        for i in 0..total.max(self.slots) {
            let now = if i < placements.len() {
                let (key, to_world) = placements[i];
                self.meshes
                    .get(&key)
                    .map(|m| wgpu::TlasInstance::new(&m.blas, to_world, i as u32, 0xff))
            } else if i < total {
                let copy = i - placements.len();
                let of = copy % placements.len();
                let (key, mut to_world) = placements[of];
                let by = stress_offset(copy as u32);
                for (axis, by) in by.into_iter().enumerate() {
                    to_world[axis * 4 + 3] += by;
                }
                self.meshes
                    .get(&key)
                    .map(|m| wgpu::TlasInstance::new(&m.blas, to_world, of as u32, 0xff))
            } else {
                None
            };
            instances += now.is_some() as u32;
            if now.is_none() && self.tlas[i].is_none() {
                continue;
            }
            self.tlas[i] = now;
        }
        self.slots = total;
        self.stress = stress;
        self.stats.instances = instances;
        self.tlas_dirty = true;
        grown
    }

    /// The copies asked of the last [`place`](Self::place) (its `stress`).
    pub(super) fn stress(&self) -> u32 {
        self.stress
    }

    /// Record this frame's builds: the meshes' new structures, then the top-level one when its
    /// instances changed (wgpu builds a structure and one pointing at it in one call).
    pub(super) fn encode(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.pending.is_empty() && !self.tlas_dirty {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        let swapped = std::mem::take(&mut self.swapped);
        let mut entries: Vec<wgpu::BlasBuildEntry> = pending
            .iter()
            .filter_map(|p| {
                let mesh = self.meshes.get(&p.key)?;
                Some(wgpu::BlasBuildEntry {
                    blas: &mesh.blas,
                    geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                        wgpu::BlasTriangleGeometry {
                            size: &p.size,
                            vertex_buffer: &p.vertices,
                            first_vertex: 0,
                            vertex_stride: 12,
                            index_buffer: Some(&p.indices),
                            first_index: Some(0),
                            transform_buffer: None,
                            transform_buffer_offset: None,
                        },
                    ]),
                })
            })
            .collect();
        // The top-level build reads the compacted copies swapped in: with no other structure
        // built, the fence puts wgpu's barrier between their copies and it.
        if swapped
            && entries.is_empty()
            && self.tlas_dirty
            && let Some(fence) = &self.fence
        {
            entries.push(fence.entry());
            self.fences += 1;
        }
        let tlas = self.tlas_dirty.then_some(&self.tlas);
        encoder.build_acceleration_structures(entries.iter(), tlas);
        self.built_now.extend(pending.iter().map(|p| p.key));
        if self.tlas_dirty {
            // The structures retired so far are out of this top-level build: once the GPU
            // finishes this submission, nothing traces them.
            let at = self.submitted + 1;
            for r in self.retired.iter_mut().filter(|r| r.gone_after == 0) {
                r.gone_after = at;
            }
        }
        self.encoded = true;
        self.stats.tlas_built = self.tlas_dirty;
        self.tlas_dirty = false;
        self.stats.triangles = self.meshes.values().map(|m| m.triangles as u64).sum();
    }

    /// After a submit (`failed`: wgpu rejected it, so none of its work runs): count it, so the
    /// structures retired before it are freed once the GPU is past it, and ask for the
    /// compaction of the large structures it built (wgpu answers once the GPU finished building
    /// them; [`swap_compacted`](Self::swap_compacted) swaps them then). If it held this scene's builds and failed, what the
    /// GPU has of the scene is unknown: the scene is [lost](Self::lost).
    pub(super) fn after_submit(&mut self, queue: &wgpu::Queue, failed: bool) {
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
        for key in self.built_now.drain(..) {
            let Some(mesh) = self.meshes.get(&key) else {
                continue;
            };
            let state = &mesh.compaction;
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
            let state = state.clone();
            mesh.blas.prepare_compaction_async(move |done| {
                let now = if done.is_ok() {
                    COMPACT_READY
                } else {
                    COMPACT_DONE
                };
                state.store(now, Ordering::Release);
            });
        }
    }

    /// Forget the structures of meshes no instance used for `KEEP_UNUSED_FRAMES`, but those
    /// in `used` (an instance placed holds them).
    pub(super) fn forget_unused(&mut self, frame: u64, used: &HashSet<u32>) {
        let gone: Vec<u32> = self
            .meshes
            .iter()
            .filter(|(key, m)| {
                frame > m.last_used + super::KEEP_UNUSED_FRAMES && !used.contains(key)
            })
            .map(|(&key, _)| key)
            .collect();
        for key in gone {
            if let Some(m) = self.meshes.remove(&key) {
                self.retire(m.blas);
            }
        }
        if self.empty.len() > 4096 {
            self.empty.clear();
        }
    }

    /// Meshes with a structure.
    pub(super) fn len(&self) -> usize {
        self.meshes.len()
    }

    /// Whether mesh `key` has a structure (or is known to have no triangle).
    pub(super) fn has(&self, key: u32) -> bool {
        self.meshes.contains_key(&key) || self.empty.contains_key(&key)
    }

    /// Structures compacted so far.
    pub(super) fn compacted(&self) -> u32 {
        self.compacted
    }

    /// Times the [`Fence`] was built.
    pub(super) fn fences(&self) -> u32 {
        self.fences
    }

    /// Structures out of the scene, waiting for the GPU to finish the frames that may trace
    /// them.
    pub(super) fn retired(&self) -> usize {
        self.retired.len()
    }
}

impl Drop for HwScene {
    /// The frames the GPU may still be running trace these structures: a top-level structure
    /// in flight points at meshes' structures it may no longer hold (one built again since lets
    /// go of those it left out), and wgpu frees a structure with its last handle. So wait for
    /// the GPU first. If the wait fails (wgpu panics on a lost device; a GPU stuck past the
    /// timeout), or while a panic unwinds, the structures are leaked instead.
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
            std::mem::forget(std::mem::take(&mut self.meshes));
            std::mem::forget(std::mem::take(&mut self.retired));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_triangles_are_the_software_hierarchys() {
        // Three corners in range, one out, and a trailing partial triangle.
        let indices = [0, 1, 2, 2, 1, 9, 3, 2, 1, 0, 1];
        assert_eq!(kept_indices(4, &indices), vec![0, 1, 2, 3, 2, 1]);
        assert!(kept_indices(2, &[0, 1, 2]).is_empty());
        assert!(kept_indices(3, &[]).is_empty());
    }
}
