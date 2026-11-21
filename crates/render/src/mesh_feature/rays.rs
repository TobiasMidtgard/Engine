//! The meshes' part of the frame's ray scene (`crate::ray_scene`): which drawn slots the
//! frame's rays see, the geometry each one's mesh gives, and how the scene keeps up with
//! them from the render thread ([`MeshFeature::fill_ray_scene`], the feature's
//! `RenderFeature::ray_scene`).
//!
//! A slot is in the scene when it is drawn at all (visible, its mesh loaded, not skinned,
//! not a row of instances), its material stops light (not blended, not unlit, not a swaying
//! leaf card the canopy map holds), its bounding sphere has a radius of 0.3 m or more
//! (smaller casters wait for the rays that see `RayClass::Small`) and it is within the
//! scene's reach of the camera. A stand-in drawn into the shadow maps only (`ShadowOnly`) is
//! in the scene for the shadow rays alone, if it casts into the finest maps: the thing it
//! casts for casts no shadow itself, so the light at a hit is shadowed as the frame draws
//! that shadow (a shadow ray is stopped where it goes into a stand-in, not where it leaves
//! one: `RayClass::ShadowOnly`). A slot's geometry is its mesh at full detail, built by the GPU straight from
//! the mesh's ranges of the pool; a mesh drawn only as clusters has no such ranges, so the
//! main thread copies its triangles when the scene asks. A geometry is named by its mesh's
//! handle index and that index's generation: an index a streamed-out mesh left to another
//! is another geometry.
//!
//! The scene is not walked every frame. The mesh scene logs the entities whose slot a sync
//! wrote, moved or dropped (`mesh_scene::SlotLog`), and a frame looks at those, at the
//! meshes whose geometry was not ready, and at a share of all slots in turn (the camera
//! brings slots into reach, and a material may change its look). After the camera jumped, or
//! when more happened than the log keeps, it looks at every slot over a few frames and then
//! drops what it did not find.

use super::*;
use crate::ray_scene::look::{material_light, texture_mean};
use crate::ray_scene::{
    RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RayView,
};

/// Slots a frame looks at in turn, at least; every slot comes up within [`ROLL_FRAMES`].
const ROLL_SLOTS: usize = 2_048;
const ROLL_FRAMES: usize = 64;
/// Slots a frame looks at while it looks at every slot.
const SWEEP_SLOTS: usize = 10_000;
/// The smallest thing the rays see (its bounding sphere's radius, meters), as probe GI's own
/// scene has it.
const MIN_RADIUS_M: f32 = crate::probe_gi::MIN_INSTANCE_RADIUS_M as f32;
/// Meters past a reach a slot in the scene stays in it, so that one at the edge does not
/// come and go as the camera sways (each change builds a structure again).
const KEEP_M: f32 = 16.0;
/// Triangles of meshes drawn only as clusters the main thread copies a frame for the scene
/// (a larger mesh alone in its frame).
const COPY_TRIANGLES_PER_FRAME: usize = 300_000;
/// Frames a copy is kept after the scene last asked for it.
const COPY_KEEP_FRAMES: u64 = 120;
/// The radius (meters, times the slot's scale) a mesh drawn as displaced clusters is taken
/// to have while nothing has measured it: larger than such a mesh is likely to be, so that
/// one in reach by its true size is asked for its copy, which brings its bounds.
const UNMEASURED_RADIUS_M: f32 = 64.0;
/// A shadow stand-in is in the scene when it casts into shadow maps with texels this wide
/// (meters), which the finest cascades have: of the stand-ins a thing may have for maps of
/// different coarseness (`ShadowTexelRange`), the one for the finest.
const STAND_IN_TEXEL_M: f32 = 0.05;

/// The triangles of a mesh the pool does not hold, for the scene to build from.
struct RayCopy {
    version: u64,
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    /// The frame it was made, or the scene last asked for it and could not build it yet.
    asked: u64,
}

/// Whether every index of `mesh` names one of its vertices (`GpuMesh::ray_ok`).
pub(super) fn indices_in_range(mesh: &Mesh) -> bool {
    mesh.indices
        .iter()
        .copied()
        .max()
        .is_some_and(|most| (most as usize) < mesh.vertices.len())
}

/// What a frame's fill did, for `render.meshes.ray_*`.
#[derive(Default, Clone, Copy)]
struct FillStats {
    /// Slots looked at.
    looked: u32,
    /// Of those, the ones in the scene after.
    placed: u32,
    /// Meshes whose geometry is not there yet.
    waiting: u32,
    /// A walk of every slot is under way.
    sweeping: bool,
    ms: f32,
}

/// The mesh feature's state for the frame's ray scene.
#[derive(Default)]
pub(super) struct MeshRays {
    /// The scene is wanted (`ray_scene::wanted` by the main view's settings).
    on: bool,
    /// It was wanted before: `render.meshes.ray_*` read zero while it is not.
    was_on: bool,
    /// The scene's epoch the slots were last walked for.
    epoch: Option<u32>,
    /// A walk of every slot: the next slot, and the frame it began.
    sweep: Option<(usize, u64)>,
    /// Another walk of every slot follows the one under way (the log was lost, or the
    /// camera jumped, after it began: the slots it passed already may have changed).
    sweep_again: bool,
    /// The next slot of the walk in turn.
    cursor: usize,
    /// Meshes (by the handle index slots name) with a slot in reach whose geometry was not
    /// there: asked again every frame.
    waiting: FastSet<u32>,
    /// Meshes (the handle index holding them, their version) the scene wants a copy of.
    wanted: Vec<(u32, u64)>,
    copies: FastMap<u32, RayCopy>,
    /// Copies made so far.
    copied: u32,
    /// Bounds in mesh space, by version (center, radius), of meshes the pool does not hold
    /// (any more): measured for a copy, or kept when the pool's copy was freed.
    bounds: FastMap<u32, (u64, Vec3, f32)>,
    /// Meshes (the handle index holding them) edited in place whose geometry in the scene
    /// is of the version before: asked again every frame while an instance uses it.
    edited: FastSet<u32>,
    /// The generation of the handle indices that held another mesh before (those freed and
    /// given out again; all others are at 0), and the asset store's allocation count they
    /// were read at.
    generations: FastMap<u32, u32>,
    allocations: u64,
    /// The mesh scene's log, as the last fill took it (kept for the allocations).
    touched: Vec<Entity>,
    gone: Vec<Entity>,
    /// Base color images' mean colors, by image and version.
    means: HashMap<u32, (u64, Option<Vec3>)>,
    stats: FillStats,
}

impl MeshRays {
    pub(super) fn on(&self) -> bool {
        self.on
    }

    /// The scene is wanted from now on, or not. True when that changed.
    pub(super) fn set_on(&mut self, on: bool) -> bool {
        if on == self.on {
            return false;
        }
        *self = Self {
            on,
            was_on: true,
            ..Default::default()
        };
        true
    }

    /// Mesh `index` was uploaded again in place.
    pub(super) fn edited(&mut self, index: u32) {
        if self.on {
            self.edited.insert(index);
        }
    }

    /// The mesh behind handle index `raw` (the one slots name) loaded, or changed what it
    /// is to the tables: its slots, passed over while it was not there, are looked at again.
    pub(super) fn mesh_changed(&mut self, raw: u32) {
        if self.on {
            self.waiting.insert(raw);
        }
    }

    /// The pool's copy of mesh `index` is freed (it is drawn only as clusters now): the
    /// bounds it had go on telling the rays its size.
    pub(super) fn keep_bounds(&mut self, index: u32, version: u64, center: Vec3, radius: f32) {
        if self.on {
            self.bounds.insert(index, (version, center, radius));
        }
    }

    /// The scene's name for the geometry of the mesh at handle index `index`.
    fn key(&self, index: u32) -> RayKey {
        let generation = self.generations.get(&index).copied().unwrap_or(0);
        RayKey(index as u64, generation as u64)
    }

    /// On the main thread, once a frame: note the handle indices that were given to another
    /// mesh since the last call (a zone streamed out, another in), so that the scene takes
    /// the newcomer for a geometry of its own and not for an edit of the mesh before, whose
    /// triangles it would trace in its place meanwhile.
    pub(super) fn follow_assets(&mut self, meshes: &Assets<Mesh>) {
        if !self.on {
            return;
        }
        let allocations = meshes.allocations();
        if allocations == self.allocations {
            return;
        }
        // (Only an index that was freed before holds another mesh than it did.)
        if meshes.removals() > 0 {
            for handle in meshes.added_between(self.allocations, allocations) {
                if let Some(generation) = handle.generation().filter(|&g| g != 0) {
                    self.generations.insert(handle.index(), generation);
                }
            }
        }
        self.allocations = allocations;
    }

    /// What a ray's hit reads of `material`: its mean diffuse colour and its glow. Only
    /// worked out while the scene is on.
    pub(super) fn light(
        &mut self,
        gpu: &GpuShared,
        material: &StandardMaterial,
        images: Option<&Assets<Image>>,
    ) -> ([f32; 3], [f32; 3]) {
        if !self.on {
            return ([0.5; 3], [0.0; 3]);
        }
        if self.means.len() > 8192 {
            self.means.clear();
        }
        let mean = match (material.base_color_texture, images) {
            (Some(image), Some(images)) => texture_mean(&mut self.means, gpu, images, image),
            _ => None,
        };
        let (albedo, emissive) = material_light(material, mean);
        (albedo.to_array(), emissive.to_array())
    }

    /// On the main thread, once a frame: copy the triangles of the meshes the last fill
    /// asked for, within the frame's allowance (those left out are asked for again).
    pub(super) fn copy_wanted(&mut self, meshes: &Assets<Mesh>, frame: u64) {
        if !self.on {
            return;
        }
        self.copies
            .retain(|_, copy| copy.asked + COPY_KEEP_FRAMES >= frame);
        let mut triangles = 0;
        for (index, version) in std::mem::take(&mut self.wanted) {
            if self
                .copies
                .get(&index)
                .is_some_and(|c| c.version == version)
            {
                continue;
            }
            let handle = Handle::<Mesh>::from_index(index);
            let Some(mesh) = meshes
                .get(handle)
                .filter(|_| meshes.version(handle) == version)
            else {
                continue;
            };
            let count = mesh.indices.len() / 3;
            if triangles > 0 && triangles + count > COPY_TRIANGLES_PER_FRAME {
                continue;
            }
            triangles += count;
            let _span = crate::perf::HitchStep::new("meshes: copy a mesh for the ray scene");
            let aabb = mesh.aabb();
            self.bounds.insert(
                index,
                (version, aabb.center(), aabb.half_extents().length()),
            );
            self.copied += 1;
            self.copies.insert(
                index,
                RayCopy {
                    version,
                    positions: mesh.vertices.iter().map(|v| v.position).collect(),
                    indices: mesh.indices.clone(),
                    asked: frame,
                },
            );
        }
    }

    /// `render.meshes.ray_*`: what the last fill did. Zeros once the scene is no longer
    /// wanted (the last fill's numbers would stand otherwise); nothing if it never was.
    pub(super) fn publish(&self, d: &mut Diagnostics) {
        if !self.on && !self.was_on {
            return;
        }
        let s = &self.stats;
        d.set("render.meshes.ray_looked", s.looked as f64);
        d.set("render.meshes.ray_placed", s.placed as f64);
        d.set("render.meshes.ray_waiting", s.waiting as f64);
        d.set("render.meshes.ray_sweeping", s.sweeping as u8 as f64);
        d.set("render.meshes.ray_copies", self.copies.len() as f64);
        d.set("render.meshes.ray_copied", self.copied as f64);
        d.set("render.meshes.ray_ms", s.ms as f64);
    }
}

/// What a slot is to the frame's rays.
enum Seen {
    /// Not in the scene.
    No,
    /// Its bounds are not known yet (its mesh is neither uploaded nor built as clusters).
    NotYet,
    /// In the scene, as this class (one past a reach, within [`KEEP_M`] of it, only if it
    /// is there already).
    Yes(RayClass),
}

/// Whether something of radius `radius` whose nearest point is `distance` from the camera
/// is within the scene's reach grown by `slack` meters: every such thing up to the casters'
/// reach, past it up to the far reach those at least `cut` times their distance across.
fn in_reach(view: &RayView, distance: f32, radius: f32, slack: f32) -> bool {
    distance <= view.caster_reach + slack
        || (distance <= view.far_reach + slack && radius >= view.cut * (distance - slack))
}

/// What a fill reads of the feature.
struct Fill<'a> {
    scene: &'a MeshScene,
    mesh_frames: &'a [MeshFrame],
    material_frames: &'a [MaterialFrame],
    meshes: &'a FastMap<u32, GpuMesh>,
    pool: &'a MeshPool,
    clusters: Option<&'a ClusterRenderer>,
    clustered: &'a FastSet<u32>,
    unusable: &'a FastMap<u32, u64>,
    skins: &'a crate::skinning::Skins,
    /// Swaying leaf cards are in the canopy map (`RenderSettings::canopy`).
    canopy: bool,
    view: &'a RayView,
    rays: &'a mut MeshRays,
}

impl Fill<'_> {
    /// The mesh and material tables' entries of a slot (`None`: the tables don't hold them,
    /// a slot written after they were built).
    fn tables(&self, hot: &Hot) -> Option<(&MeshFrame, &MaterialFrame)> {
        Some((
            self.mesh_frames.get(hot.mesh as usize)?,
            self.material_frames.get(hot.material as usize)?,
        ))
    }

    /// Slot `s`'s bounding sphere in the world (`None`: not known yet).
    fn bounds(&self, hot: &Hot, cold: &Cold, mesh: &MeshFrame) -> Option<(DVec3, f32)> {
        if mesh.ready && hot.mesh_version == mesh.version {
            // As the cull left them.
            return Some((hot.center, hot.radius));
        }
        let (center, radius) = if mesh.ready {
            (mesh.center, mesh.radius)
        } else if let Some(sphere) = self.clusters.and_then(|c| c.bounds(mesh.index as u64)) {
            (sphere.center, sphere.radius)
        } else if let Some(&(version, center, radius)) = self.rays.bounds.get(&mesh.index)
            && version == mesh.version
        {
            (center, radius)
        } else if self.clustered.contains(&mesh.index) {
            // Drawn as clusters of another shape (displaced) and never measured: taken by
            // its origin and larger than it is likely to be, so that it is asked for its
            // copy, which brings its bounds.
            (Vec3::ZERO, UNMEASURED_RADIUS_M)
        } else {
            return None;
        };
        Some((
            cold.transform.transform_point(center.as_dvec3()),
            radius * hot.scale,
        ))
    }

    /// What slot `s` is to the rays. `placed`: whether it is in the scene now (asked only
    /// for a slot past a reach).
    fn seen(&self, s: usize, placed: impl FnOnce() -> bool) -> Seen {
        let hot = &self.scene.hot[s];
        if hot.flags & mesh_scene::VISIBLE == 0
            || hot.flags & mesh_scene::ROWS != 0
            || hot.dither.abs() >= 1.0
        {
            return Seen::No;
        }
        // A stand-in drawn into the shadow maps only: there for the shadow rays, if it is
        // the one that casts into the finest maps (a thing may have another for the coarse
        // ones, which would cast its shadow a second time).
        let stand_in = hot.flags & mesh_scene::SHADOW_ONLY != 0;
        if stand_in
            && (hot.flags & mesh_scene::CASTS == 0 || !hot.cast_texels.casts_at(STAND_IN_TEXEL_M))
        {
            return Seen::No;
        }
        let Some((mesh, mat)) = self.tables(hot) else {
            return Seen::No;
        };
        if mesh.version == 0
            || mesh.skinned
            || self.skins.skins.contains_key(&mesh.index)
            || self.unusable.get(&mesh.index) == Some(&mesh.version)
        {
            return Seen::No;
        }
        // Swaying leaf cards: as solid triangles they would roof the crowns (the canopy map
        // holds them for the rays).
        if mat.blend || mat.unlit || (self.canopy && mat.masked && mat.sways) {
            return Seen::No;
        }
        let Some((center, radius)) = self.bounds(hot, &self.scene.cold[s], mesh) else {
            return Seen::NotYet;
        };
        // (Also a caster: the rays that see `RayClass::Small` are not there yet.)
        if radius < MIN_RADIUS_M {
            return Seen::No;
        }
        let distance = ((center - self.view.eye).length() as f32 - radius).max(0.0);
        let class = if stand_in {
            RayClass::ShadowOnly
        } else if hot.flags & mesh_scene::CASTS != 0 {
            RayClass::Solid
        } else {
            RayClass::NoCast
        };
        let within = distance <= hot.max_distance && in_reach(self.view, distance, radius, 0.0);
        // One past a reach, within `KEEP_M` of it, stays if it is in the scene already.
        let kept = || {
            distance <= hot.max_distance + KEEP_M
                && in_reach(self.view, distance, radius, KEEP_M)
                && placed()
        };
        if within || kept() {
            Seen::Yes(class)
        } else {
            Seen::No
        }
    }

    /// The scene's geometry of `mesh` at its version: there already, built now from its
    /// ranges of the pool or from its copy, or waiting (its copy is asked for).
    fn geometry(&mut self, mesh: &MeshFrame, out: &mut RaySceneOut<'_>) -> RayGeometryState {
        let key = self.rays.key(mesh.index);
        if out.has_geometry(key, mesh.version) {
            return RayGeometryState::Ready;
        }
        let wait = if out.geometry_version(key).is_some() {
            RayGeometryState::Older
        } else {
            RayGeometryState::Building
        };
        if let Some(gpu_mesh) = self.meshes.get(&mesh.index)
            && gpu_mesh.version == mesh.version
        {
            if !gpu_mesh.ray_ok {
                return RayGeometryState::Refused;
            }
            let Some((vertex, index)) = self.pool.buffers(gpu_mesh.pool.page) else {
                return wait;
            };
            // The whole mesh (level 0): its vertices first, its indices count from them.
            return out.geometry(
                key,
                mesh.version,
                true,
                RaySource::Buffers {
                    vertex,
                    first_vertex: gpu_mesh.pool.base_vertex,
                    vertex_count: gpu_mesh.pool.vertex_count,
                    vertex_stride: crate::mesh_pool::VERTEX_BYTES as u32,
                    index,
                    index_format: wgpu::IndexFormat::Uint32,
                    first_index: gpu_mesh.pool.first_index,
                    index_count: gpu_mesh.pool.index_count,
                },
            );
        }
        if let Some(copy) = self.rays.copies.get_mut(&mesh.index)
            && copy.version == mesh.version
        {
            let state = out.geometry(
                key,
                mesh.version,
                true,
                RaySource::Cpu {
                    positions: &copy.positions,
                    indices: &copy.indices,
                },
            );
            if matches!(state, RayGeometryState::Ready | RayGeometryState::Refused) {
                self.rays.copies.remove(&mesh.index);
            } else {
                // Still wanted: it stays while its mesh waits for its turn to build.
                copy.asked = self.view.frame;
            }
            return state;
        }
        // Drawn only as clusters: the pool will not hold it, so the main thread copies its
        // triangles (next frame at the earliest). Else its upload is on its way.
        if self.clustered.contains(&mesh.index)
            && !self.rays.wanted.contains(&(mesh.index, mesh.version))
        {
            self.rays.wanted.push((mesh.index, mesh.version));
        }
        wait
    }

    /// Bring slot `s` up to date in the scene: placed as it stands, or removed.
    fn slot(&mut self, s: usize, out: &mut RaySceneOut<'_>) {
        let cold = &self.scene.cold[s];
        let id = RayKey(cold.entity.to_bits(), 0);
        self.rays.stats.looked += 1;
        let class = match self.seen(s, || out.has(id)) {
            Seen::No => {
                out.remove(id);
                return;
            }
            Seen::NotYet => {
                self.rays.waiting.insert(self.scene.hot[s].mesh);
                return;
            }
            Seen::Yes(class) => class,
        };
        let hot = &self.scene.hot[s];
        let Some((&mesh, &mat)) = self.tables(hot) else {
            return;
        };
        match self.geometry(&mesh, out) {
            RayGeometryState::Ready => {}
            // Placed with the shape before, and asked again every frame until its own is
            // built.
            RayGeometryState::Older => {
                self.rays.waiting.insert(hot.mesh);
            }
            RayGeometryState::Building => {
                self.rays.waiting.insert(hot.mesh);
                return;
            }
            RayGeometryState::Refused => {
                out.remove(id);
                return;
            }
        }
        // From when it holds still: 90 frames after it last moved. What never moved does
        // already; so does what last moved with the world (a world shift moves every slot,
        // and the scene moved its instances itself) unless the scene has it, which then
        // tells a move of its own from the shift's.
        let still_from = if cold.moved == 0 || (cold.moved == self.view.shifted && !out.has(id)) {
            0
        } else {
            cold.moved + STILL_FRAMES
        };
        let look = RayLook {
            albedo: mat.ray_albedo,
            roughness: mat.ray_surface[0],
            emissive: mat.ray_emissive,
            metallic: mat.ray_surface[1],
            cutoff: mat.ray_surface[2],
            coverage: mat.coverage,
            double_sided: mat.double_sided,
            ..Default::default()
        };
        out.place(
            id,
            self.rays.key(mesh.index),
            &cold.transform.0,
            class,
            still_from,
            &look,
        );
        self.rays.stats.placed += 1;
    }

    /// A mesh whose geometry was not there when a slot of its was looked at: ask again
    /// while a slot of its is in reach, and place its slots once it is.
    fn waiting_mesh(&mut self, raw: u32, out: &mut RaySceneOut<'_>) {
        let Some(&mesh) = self.mesh_frames.get(raw as usize) else {
            return;
        };
        let slots = self.scene.slots_of_mesh(raw);
        // (A slot at the edge counts only once it is placed, which none of these is.)
        let mut unknown = false;
        let wanted = slots
            .iter()
            .any(|&s| match self.seen(s as usize, || false) {
                Seen::Yes(_) => true,
                Seen::NotYet => {
                    unknown = true;
                    false
                }
                Seen::No => false,
            });
        if !wanted {
            // None in reach: the walk in turn finds it when one comes. Bounds not known
            // yet: its upload is on its way, so it keeps waiting.
            if unknown {
                self.rays.waiting.insert(raw);
            }
            return;
        }
        match self.geometry(&mesh, out) {
            RayGeometryState::Ready => {
                for &s in slots {
                    self.slot(s as usize, out);
                }
            }
            // (Its slots were placed with the shape before when they were looked at.)
            RayGeometryState::Older | RayGeometryState::Building => {
                self.rays.waiting.insert(raw);
            }
            RayGeometryState::Refused => {}
        }
    }
}

impl MeshFeature {
    /// The meshes' part of the frame's ray scene, on the render thread after the main view's
    /// `prepare_frame` (see the module).
    pub(super) fn fill_ray_scene(&mut self, view: &RayView, out: &mut RaySceneOut<'_>) {
        if self.shader.is_none() || !self.rays.on() {
            return;
        }
        let started = std::time::Instant::now();
        let mut rays = std::mem::take(&mut self.rays);
        rays.stats = FillStats::default();
        let (mut touched, mut gone) = (
            std::mem::take(&mut rays.touched),
            std::mem::take(&mut rays.gone),
        );
        let lost = self.scene.log.take(&mut touched, &mut gone);
        if lost || rays.epoch != Some(view.epoch) {
            // The camera jumped, the scene is new, or more happened than the log kept: look
            // at every slot. A walk under way goes on (starting over each time, one that
            // takes several frames would never end while the log is lost every frame) and
            // another follows it.
            rays.epoch = Some(view.epoch);
            if rays.sweep.is_some() {
                rays.sweep_again = true;
            } else {
                rays.sweep = Some((0, view.frame));
                rays.waiting.clear();
            }
        }
        let edited: Vec<u32> = rays.edited.drain().collect();
        let waiting: Vec<u32> = rays.waiting.drain().collect();
        let mut fill = Fill {
            scene: &self.scene,
            mesh_frames: &self.mesh_frames,
            material_frames: &self.material_frames,
            meshes: &self.meshes,
            pool: &self.pool,
            clusters: self.clusters.as_ref(),
            clustered: &self.clustered,
            unusable: &self.unusable,
            skins: &self.skins,
            canopy: self.class_canopy,
            view,
            rays: &mut rays,
        };
        // Gone first: an entity in both lists is gone unless it has a slot now.
        for entity in &gone {
            out.remove(RayKey(entity.to_bits(), 0));
        }
        for &entity in &touched {
            match fill.scene.slot_of_entity(entity) {
                Some(s) => fill.slot(s, out),
                None => out.remove(RayKey(entity.to_bits(), 0)),
            }
        }
        // Meshes edited in place: the scene's geometry of them is built again while an
        // instance uses it (their instances stay as they are, with the older shape until
        // then; one nothing uses is left to be forgotten).
        for index in edited {
            let key = fill.rays.key(index);
            if let Some(gpu_mesh) = fill.meshes.get(&index)
                && out
                    .geometry_version(key)
                    .is_some_and(|v| v != gpu_mesh.version)
                && out.geometry_users(key) > 0
            {
                let mesh = MeshFrame {
                    index,
                    version: gpu_mesh.version,
                    ..Default::default()
                };
                if fill.geometry(&mesh, out) == RayGeometryState::Older {
                    // Put off (built lately, or the frame's allowance is spent).
                    fill.rays.edited.insert(index);
                }
            }
        }
        for raw in waiting {
            fill.waiting_mesh(raw, out);
        }
        let slots = fill.scene.hot.len();
        match fill.rays.sweep {
            Some((next, began)) => {
                let end = (next + SWEEP_SLOTS).min(slots);
                for s in next.min(slots)..end {
                    fill.slot(s, out);
                }
                if end >= slots {
                    // Every slot was looked at since the walk began (one whose number
                    // changed meanwhile is in the log): what the scene still has of this
                    // feature and no slot placed is gone. Not when the log was lost
                    // meanwhile: a slot that took a number the walk had passed was then
                    // looked at by no one, so the walk that follows drops what is gone.
                    let again = std::mem::take(&mut fill.rays.sweep_again);
                    if !again {
                        out.remove_unplaced_since(began);
                    }
                    fill.rays.sweep = again.then_some((0, view.frame + 1));
                    fill.rays.cursor = 0;
                } else {
                    fill.rays.sweep = Some((end, began));
                }
            }
            None if slots > 0 => {
                let count = (slots / ROLL_FRAMES).max(ROLL_SLOTS).min(slots);
                let first = fill.rays.cursor % slots;
                for k in 0..count {
                    fill.slot((first + k) % slots, out);
                }
                fill.rays.cursor = (first + count) % slots;
            }
            None => {}
        }
        rays.stats.waiting = rays.waiting.len() as u32;
        rays.stats.sweeping = rays.sweep.is_some();
        rays.stats.ms = started.elapsed().as_secs_f32() * 1e3;
        rays.touched = touched;
        rays.gone = gone;
        self.rays = rays;
    }
}
