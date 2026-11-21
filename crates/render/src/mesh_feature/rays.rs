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
//! one: `RayClass::ShadowOnly`). A slot's geometry is its mesh at its ray level, built by
//! the GPU straight from the mesh's ranges of the pool: the coarsest of its levels of detail
//! that strays at most 3 cm from it at the largest scale it is placed with (the whole mesh
//! while its levels are on their way, and when none is that close). A mesh drawn only as
//! clusters has no such ranges, so the main thread copies its triangles when the scene asks.
//! A geometry is named by its mesh's handle index and that index's generation: an index a
//! streamed-out mesh left to another is another geometry.
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
    RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RayTileSource, RayView,
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

/// The texture a material's tile in the ray tiles is filled from (`MaterialFrame::ray_tile`).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(super) enum RayTileName {
    /// None: its hits take their row's colour.
    #[default]
    None,
    /// Its base colour image, RGBA8: the canonical handle index, the generation of that index
    /// (a zone streamed in may be given an index another image held), and the image's version
    /// (only a GPU copy of these pixels fills its tile); `srgb`: read as sRGB, as a colour slot
    /// reads an sRGB image.
    Image {
        index: u32,
        generation: u32,
        srgb: bool,
        version: u64,
    },
    /// Its preset's generated base colour at `size` texels.
    Preset { preset: MaterialPreset, size: u32 },
}

/// Key bit of a preset's tile (an image's key is its handle index, below 2^32).
const PRESET_TILE: u64 = 1 << 63;

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

/// A mesh's ray level is the coarsest of its levels of detail whose surface strays at most
/// this far from the mesh, in meters at the largest scale the mesh is placed with: one
/// structure serves every instance, and a ray's hit is within this of the drawn surface.
pub(super) const LEVEL_ERROR_M: f32 = 0.03;

/// The level the rays trace a mesh at (0: the mesh itself). `levels`: each level's error in
/// mesh space and whether the scene may build from it (`GpuLod::ray_ok`), finest first
/// (errors never fall along them). `scale`: the largest scale the mesh is placed with (0: not
/// known yet, so the mesh itself).
pub(super) fn level_for(levels: impl Iterator<Item = (f32, bool)>, scale: f32) -> u8 {
    if scale <= 0.0 {
        return 0;
    }
    let mut level = 0;
    for (i, (error, ok)) in levels.enumerate() {
        if error * scale > LEVEL_ERROR_M {
            break;
        }
        if ok {
            level = i as u8 + 1;
        }
    }
    level
}

/// Which levels of `mesh` have every index name one of the mesh's vertices or one of those
/// the chain adds (bit `l`: `levels.levels[l]`), for `GpuLod::ray_ok`. Run where the levels
/// are built, on the worker.
pub(super) fn levels_in_range(mesh: &Mesh, levels: &crate::mesh_lod::Lods) -> u8 {
    let most = mesh.vertices.len() + levels.vertices.len();
    let mut ok = 0u8;
    for (l, level) in levels.levels.iter().enumerate().take(8) {
        if level.indices.iter().all(|&i| (i as usize) < most) {
            ok |= 1 << l;
        }
    }
    ok
}

/// The scene's version of a mesh's geometry. The mesh itself (level 0, from the pool or from
/// a copy) is named by the asset's version alone: its triangles are the asset's wherever the
/// pool keeps them. A level of detail is named by the asset's version, the level and the
/// stamp of the pool ranges it is read from (`GpuMesh::ranges`), so that no structure is
/// built from ranges read before they were given to another level or mesh.
pub(super) fn ray_version(asset: u64, ranges: u32, level: u8) -> u64 {
    if level == 0 {
        return asset << 24;
    }
    (asset << 24) | ((ranges as u64 & 0x1f_ffff) << 3) | (level as u64 & 7)
}

/// The asset version a geometry's version names ([`ray_version`]).
pub(super) fn asset_of(version: u64) -> u64 {
    version >> 24
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
    /// The same for image handle indices (the tiles' names), and the image store's count.
    image_generations: FastMap<u32, u32>,
    image_allocations: u64,
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

    /// [`Self::follow_assets`] for images: a tile is named by its image's index and that
    /// index's generation, so an image given a freed index never takes the layer that holds
    /// the image before it.
    pub(super) fn follow_images(&mut self, images: &Assets<Image>) {
        if !self.on {
            return;
        }
        let allocations = images.allocations();
        if allocations == self.image_allocations {
            return;
        }
        if images.removals() > 0 {
            for handle in images.added_between(self.image_allocations, allocations) {
                if let Some(generation) = handle.generation().filter(|&g| g != 0) {
                    self.image_generations.insert(handle.index(), generation);
                }
            }
        }
        self.image_allocations = allocations;
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

    /// Which texture a hit of `material` reads as its tile, and the texture's scale (xy) and
    /// offset (zw) over the mesh's uv: its base colour image (RGBA8, as its mean is), else its
    /// preset's generated map, which is what the raster draws in that slot
    /// (`texture_set_for`); none where the texture is not laid by the mesh's uv (a hit knows
    /// no other). Field reads, only while the scene is on.
    pub(super) fn tile_name(
        &self,
        material: &StandardMaterial,
        images: Option<&Assets<Image>>,
        preset_size: u32,
    ) -> (RayTileName, [f32; 4]) {
        let none = (RayTileName::None, [1.0, 1.0, 0.0, 0.0]);
        if !self.on || material.mapping.projection() != crate::TextureProjection::Uv {
            return none;
        }
        let name = match (material.base_color_texture, material.preset) {
            (Some(handle), _) => {
                let Some(images) = images else {
                    return none;
                };
                let handle = images.canonical(handle);
                match images.get(handle) {
                    Some(image) if image.format == crate::PixelFormat::Rgba8 => {
                        let index = handle.index();
                        RayTileName::Image {
                            index,
                            generation: self.image_generations.get(&index).copied().unwrap_or(0),
                            srgb: image.srgb,
                            version: images.version(handle),
                        }
                    }
                    _ => return none,
                }
            }
            (None, Some(preset)) => RayTileName::Preset {
                preset,
                size: preset_size,
            },
            (None, None) => return none,
        };
        // As mesh.wgsl lays it: uv x `uv_scale` / `texture_scale` (a material has no offset).
        let per_m = 1.0 / material.texture_scale.max(1e-4);
        (
            name,
            [
                material.uv_scale[0] * per_m,
                material.uv_scale[1] * per_m,
                0.0,
                0.0,
            ],
        )
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
    /// is there already), its nearest point this far from the camera (meters).
    Yes(RayClass, f32),
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
    /// (Mutable for the scale the rays see each at, and a level the scene refused.)
    meshes: &'a mut FastMap<u32, GpuMesh>,
    pool: &'a MeshPool,
    clusters: Option<&'a ClusterRenderer>,
    clustered: &'a FastSet<u32>,
    unusable: &'a FastMap<u32, u64>,
    skins: &'a crate::skinning::Skins,
    /// Swaying leaf cards are in the canopy map (`RenderSettings::canopy`).
    canopy: bool,
    view: &'a RayView,
    rays: &'a mut MeshRays,
    /// Where the ray tiles' pixels are looked up: the image cache, the generated presets,
    /// and the virtual texture cache's root tiles of streamed base colours.
    gpu: &'a GpuShared,
    vt: Option<&'a VirtualTextureSystem>,
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
            Seen::Yes(class, distance)
        } else {
            Seen::No
        }
    }

    /// The scene's geometry of `mesh` at its version: there already, built now from its
    /// ranges of the pool at its ray level or from its copy, or waiting (its copy is asked
    /// for). `raw`, `scale`: the handle index slots name, and the largest scale of the slot
    /// that asks (0: no slot).
    fn geometry(
        &mut self,
        mesh: &MeshFrame,
        raw: u32,
        scale: f32,
        out: &mut RaySceneOut<'_>,
    ) -> RayGeometryState {
        let key = self.rays.key(mesh.index);
        if let Some(gpu_mesh) = self.meshes.get_mut(&mesh.index)
            && gpu_mesh.version == mesh.version
        {
            // The largest scale it is placed with. The first look takes every slot that names
            // its handle, so that a scene switched on where the levels are there already
            // builds each mesh once, at its level.
            if gpu_mesh.ray_scale <= 0.0 {
                for &s in self.scene.slots_of_mesh(raw) {
                    if let Some(hot) = self.scene.hot.get(s as usize) {
                        gpu_mesh.ray_scale = gpu_mesh.ray_scale.max(hot.scale);
                    }
                }
            }
            gpu_mesh.ray_scale = gpu_mesh.ray_scale.max(scale);
            let wanted = ray_version(mesh.version, gpu_mesh.ranges, gpu_mesh.ray_level());
            if out.has_geometry(key, wanted) {
                return RayGeometryState::Ready;
            }
            // This very mesh at another level (its levels came since, or it is placed larger
            // than it was): that serves, and the walk of the edited meshes asks for this
            // level, once a frame (its instances are never out of the rays meanwhile).
            if out
                .geometry_version(key)
                .is_some_and(|v| asset_of(v) == mesh.version)
            {
                self.rays.edited.insert(mesh.index);
                return RayGeometryState::Ready;
            }
            if !gpu_mesh.ray_ok {
                return RayGeometryState::Refused;
            }
            return Self::from_pool(self.pool, key, gpu_mesh, out);
        }
        // Not in the pool (any more): what the scene has of this version of the mesh serves,
        // at whatever level it was built while the pool held it.
        let whole = ray_version(mesh.version, 0, 0);
        if out.has_geometry(key, whole) {
            return RayGeometryState::Ready;
        }
        let held = out.geometry_version(key);
        if let Some(v) = held
            && asset_of(v) == mesh.version
            && out.has_geometry(key, v)
        {
            return RayGeometryState::Ready;
        }
        let wait = if held.is_some() {
            RayGeometryState::Older
        } else {
            RayGeometryState::Building
        };
        if let Some(copy) = self.rays.copies.get_mut(&mesh.index)
            && copy.version == mesh.version
        {
            let state = out.geometry(
                key,
                whole,
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

    /// Ask the scene for `gpu_mesh` at its ray level, from its ranges of the pool as they are
    /// now. A level the scene refuses is passed over for the next finer one (so a refusal
    /// never takes out of the rays what the whole mesh would have put in).
    fn from_pool(
        pool: &MeshPool,
        key: RayKey,
        gpu_mesh: &mut GpuMesh,
        out: &mut RaySceneOut<'_>,
    ) -> RayGeometryState {
        let Some((vertex, index)) = pool.buffers(gpu_mesh.pool.page) else {
            return if out.geometry_version(key).is_some() {
                RayGeometryState::Older
            } else {
                RayGeometryState::Building
            };
        };
        loop {
            let level = gpu_mesh.ray_level();
            // Indices count from the first vertex named: the mesh's own for the whole mesh,
            // the level's base (the lower of its two ranges) for a level.
            let lod = level
                .checked_sub(1)
                .and_then(|l| gpu_mesh.lods.get(l as usize));
            let (first_vertex, vertex_count, first_index, index_count) = match lod {
                None => (
                    gpu_mesh.pool.base_vertex,
                    gpu_mesh.pool.vertex_count,
                    gpu_mesh.pool.first_index,
                    gpu_mesh.pool.index_count,
                ),
                Some(lod) => (
                    lod.base_vertex,
                    gpu_mesh.lod_span(lod),
                    lod.first_index,
                    lod.index_count,
                ),
            };
            let state = out.pool_geometry(
                key,
                ray_version(gpu_mesh.version, gpu_mesh.ranges, level),
                true,
                RaySource::Buffers {
                    vertex,
                    first_vertex,
                    vertex_count,
                    vertex_stride: crate::mesh_pool::VERTEX_BYTES as u32,
                    index,
                    index_format: wgpu::IndexFormat::Uint32,
                    first_index,
                    index_count,
                },
            );
            if state == RayGeometryState::Refused && level > 0 {
                gpu_mesh.lods[level as usize - 1].ray_ok = false;
                continue;
            }
            return state;
        }
    }

    /// The layer of the ray tiles that holds texture `name`, for a slot whose nearest point is
    /// `distance` meters away (`None`: no tile, or waiting for one). Its pixels are looked up
    /// only when the scene asks for them, where the raster has them (`texture_set_for`): the
    /// virtual texture cache's root tile, the image cache's upload, the preset's maps as
    /// generated. Nothing is uploaded or generated for the rays.
    fn tile(&self, name: RayTileName, distance: f32, out: &mut RaySceneOut<'_>) -> Option<u16> {
        let (gpu, vt) = (self.gpu, self.vt);
        match name {
            RayTileName::None => None,
            RayTileName::Image {
                index,
                generation,
                srgb,
                version,
            } => {
                let key = RayKey(index as u64, generation as u64);
                out.tile(key, distance, || {
                    // Only a copy of the pixels the image has now (a cache may still hold the
                    // pixels before an edit, or those of the image the index held before).
                    if let Some((view, rect, version)) =
                        vt.and_then(|vt| vt.root_tile(index, srgb, version))
                    {
                        // (A place in the cache's atlas.)
                        return Some(RayTileSource {
                            view,
                            rect,
                            version,
                            lasting: false,
                        });
                    }
                    let texture = gpu.images.uploaded_color(index, srgb, version)?;
                    Some(RayTileSource {
                        view: texture.view.clone(),
                        rect: [0.0, 0.0, 1.0, 1.0],
                        version,
                        lasting: true,
                    })
                })
            }
            RayTileName::Preset { preset, size } => {
                let key = RayKey(PRESET_TILE | preset as u64, size as u64);
                out.tile(key, distance, || {
                    let set = gpu.materials.existing(preset, size)?;
                    Some(RayTileSource {
                        view: set.base_color.clone(),
                        rect: [0.0, 0.0, 1.0, 1.0],
                        // (A preset at a size has one look; its compressed copy shows it too.)
                        version: 0,
                        lasting: true,
                    })
                })
            }
        }
    }

    /// Bring slot `s` up to date in the scene: placed as it stands, or removed.
    fn slot(&mut self, s: usize, out: &mut RaySceneOut<'_>) {
        let cold = &self.scene.cold[s];
        let id = RayKey(cold.entity.to_bits(), 0);
        self.rays.stats.looked += 1;
        let (class, distance) = match self.seen(s, || out.has(id)) {
            Seen::No => {
                out.remove(id);
                return;
            }
            Seen::NotYet => {
                self.rays.waiting.insert(self.scene.hot[s].mesh);
                return;
            }
            Seen::Yes(class, distance) => (class, distance),
        };
        let hot = &self.scene.hot[s];
        let Some((&mesh, &mat)) = self.tables(hot) else {
            return;
        };
        match self.geometry(&mesh, hot.mesh, hot.scale, out) {
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
        // Its texture as a tile, where a hit has a uv to read it at: triangles with records
        // (a geometry built from the pool's ranges; one drawn only as clusters is built from
        // a copy, which has none). A shadow stand-in shows no colour.
        let pooled = self
            .meshes
            .get(&mesh.index)
            .is_some_and(|g| g.version == mesh.version);
        let tile = if class != RayClass::ShadowOnly && pooled {
            self.tile(mat.ray_tile, distance, out)
        } else {
            None
        };
        let look = RayLook {
            albedo: mat.ray_albedo,
            roughness: mat.ray_surface[0],
            emissive: mat.ray_emissive,
            metallic: mat.ray_surface[1],
            tile,
            uv: mat.ray_uv,
            cutoff: mat.ray_surface[2],
            coverage: mat.coverage,
            double_sided: mat.double_sided,
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
                Seen::Yes(..) => true,
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
        match self.geometry(&mesh, raw, 0.0, out) {
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
    pub(super) fn fill_ray_scene(
        &mut self,
        gpu: &GpuShared,
        view: &RayView,
        out: &mut RaySceneOut<'_>,
    ) {
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
            meshes: &mut self.meshes,
            pool: &self.pool,
            clusters: self.clusters.as_ref(),
            clustered: &self.clustered,
            unusable: &self.unusable,
            skins: &self.skins,
            canopy: self.class_canopy,
            view,
            rays: &mut rays,
            gpu,
            vt: self.vt.as_ref(),
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
        // Meshes edited in place, and meshes whose ray level changed (their levels came, or
        // they are placed larger than they were): the scene's geometry of them is built again
        // while an instance uses it (their instances stay as they are, with the shape before
        // until then; one nothing uses is left to be forgotten).
        for index in edited {
            let key = fill.rays.key(index);
            let Some(gpu_mesh) = fill.meshes.get_mut(&index) else {
                continue;
            };
            if !gpu_mesh.ray_ok {
                continue;
            }
            let wanted = ray_version(gpu_mesh.version, gpu_mesh.ranges, gpu_mesh.ray_level());
            if out.geometry_version(key).is_some_and(|v| v != wanted)
                && out.geometry_users(key) > 0
                && Fill::from_pool(fill.pool, key, gpu_mesh, out) == RayGeometryState::Older
            {
                // Put off (built lately, or the frame's allowance is spent).
                fill.rays.edited.insert(index);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ray_level_is_the_coarsest_within_three_centimeters_at_the_largest_scale() {
        let checked = [(0.004, true), (0.012, true), (0.04, true)];
        assert_eq!(level_for(checked.into_iter(), 1.0), 2);
        assert_eq!(level_for(checked.into_iter(), 3.0), 1);
        assert_eq!(level_for(checked.into_iter(), 10.0), 0);
        // No scale known yet: the mesh itself.
        assert_eq!(level_for(checked.into_iter(), 0.0), 0);
        // A level whose indices were not checked is passed over, not a stop.
        let second_unchecked = [(0.004, true), (0.012, false), (0.02, true)];
        assert_eq!(level_for(second_unchecked.into_iter(), 1.0), 3);
        let only_first = [(0.004, true), (0.012, false), (0.04, true)];
        assert_eq!(level_for(only_first.into_iter(), 1.0), 1);
        assert_eq!(level_for(std::iter::empty(), 1.0), 0);
    }

    #[test]
    fn a_levels_version_names_its_asset_level_and_ranges() {
        let asset = 1234;
        for (ranges, level) in [(0, 0), (7, 0), (7, 1), (8, 1), (7, 6), (0x1f_ffff, 3)] {
            assert_eq!(asset_of(ray_version(asset, ranges, level)), asset);
        }
        // The mesh itself is the same geometry wherever the pool keeps it.
        assert_eq!(ray_version(asset, 3, 0), ray_version(asset, 9, 0));
        assert_eq!(ray_version(asset, 3, 0), asset << 24);
        // Two levels, two ranges, and a level against the whole mesh all differ.
        assert_ne!(ray_version(asset, 3, 1), ray_version(asset, 3, 2));
        assert_ne!(ray_version(asset, 3, 1), ray_version(asset, 4, 1));
        assert_ne!(ray_version(asset, 3, 1), ray_version(asset, 3, 0));
        assert_ne!(ray_version(asset, 3, 1), ray_version(asset + 1, 3, 1));
    }

    #[test]
    fn a_level_with_an_index_past_the_vertices_is_not_built_from() {
        let mesh = crate::mesh::cuboid(Vec3::ONE);
        let own = mesh.vertices.len() as u32;
        let added = vec![mesh.vertices[0]; 2];
        let level = |indices: Vec<u32>| crate::mesh_lod::LodIndices {
            indices,
            error: 0.01,
        };
        let lods = crate::mesh_lod::Lods {
            levels: vec![
                level(vec![0, 1, 2]),
                // The last vertex the chain adds: still in range.
                level(vec![0, own, own + 1]),
                // One past it.
                level(vec![0, 1, own + 2]),
                level(vec![2, 1, 0]),
            ],
            vertices: added,
        };
        assert_eq!(levels_in_range(&mesh, &lods), 0b1011);
    }

    /// An image given the index of one removed before is a tile of its own (its name holds the
    /// index's generation), so it never reads the layer with the image before's pixels; an
    /// edit keeps the name's generation and moves its version.
    #[test]
    fn an_image_at_a_freed_index_is_a_tile_apart_from_the_one_before() {
        let mut images = Assets::<Image>::default();
        let mut rays = MeshRays::default();
        rays.set_on(true);
        let checker = |v: u8| Image::checker(8, 2, [v, v, v, 255], [0, 0, 0, 255]);
        let material = |image| StandardMaterial {
            base_color_texture: Some(image),
            ..Default::default()
        };
        let parts = |name: RayTileName| match name {
            RayTileName::Image {
                index,
                generation,
                version,
                ..
            } => (index, generation, version),
            other => panic!("{other:?}"),
        };
        let a = images.add(checker(200));
        rays.follow_images(&images);
        let (index, first, version) = parts(rays.tile_name(&material(a), Some(&images), 64).0);
        images.get_mut(a).expect("there").data[0] = 10;
        let (_, same, edited) = parts(rays.tile_name(&material(a), Some(&images), 64).0);
        assert_eq!(same, first);
        assert_ne!(edited, version);
        images.remove(a);
        let b = images.add(checker(50));
        assert_eq!(b.index(), index, "the freed index is given out again");
        rays.follow_images(&images);
        let (_, second, _) = parts(rays.tile_name(&material(b), Some(&images), 64).0);
        assert_ne!(second, first);
    }
}
