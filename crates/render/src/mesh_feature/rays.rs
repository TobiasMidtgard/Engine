//! The meshes' part of the frame's ray scene (`crate::ray_scene`): which drawn slots the
//! frame's rays see, the geometry each one's mesh gives, and how the scene keeps up with
//! them from the render thread ([`MeshFeature::fill_ray_scene`], the feature's
//! `RenderFeature::ray_scene`).
//!
//! A slot is in the scene when it is drawn at all (visible, its mesh loaded, not skinned),
//! its material stops light (not blended, not unlit, not cut out everywhere) and it is within
//! the scene's reach of the camera ([`in_reach`]), as its class ([`class_of`]): what casts a
//! shadow is a `Leaf` where it is a swaying card cut out by its texture (the canopy map holds
//! such cards for the probes), `Sway` where it moves in the wind otherwise, `Small` under 0.3 m
//! and `Solid` else; what casts none is `NoCast`, and out under 0.3 m or as a swaying card (a
//! crown beside its stand-in). A cut-out that holds still (a fence, a lattice) keeps the class
//! of its size, and its triangles are asked like a leaf's ([`cut_out`]): reflections see
//! through its holes, probes see it whole as their own scene does. A stand-in drawn into the
//! shadow maps only (`ShadowOnly`) is in the scene if it casts into the finest maps: a crown's
//! (its swaying cards) as a `Leaf`, the crown's one place in the rays; any other as
//! `ShadowOnly`, for the shadow rays alone (the thing it casts for casts no shadow itself, so
//! the light at a hit is shadowed as the frame draws that shadow; a shadow ray is stopped where
//! it goes into a stand-in, not where it leaves one). Each row of a `MeshInstances` slot is an
//! instance of its own (`RayKey(entity, row + 1)`) of the one geometry the rows share, its
//! class by its own size and its look tinted by its row's tint.
//!
//! A slot's geometry is its mesh at its ray level, built by the GPU straight from the mesh's
//! ranges of the pool: the coarsest of its levels of detail that strays at most 3 cm from it
//! at the largest scale it is placed with (the whole mesh while its levels are on their way,
//! and when none is that close). A mesh drawn only as clusters has no such ranges, so the
//! main thread copies its triangles when the scene asks. A geometry is named by its mesh's
//! handle index and that index's generation (an index a streamed-out mesh left to another is
//! another geometry), and by whether its triangles stop a ray at once or are asked one by one
//! (whether its instances' material cuts holes in it, [`cut_out`]): a mesh placed both ways
//! is two geometries.
//!
//! The scene is not walked every frame. The mesh scene logs the entities whose slot a sync
//! wrote, moved or dropped (`mesh_scene::SlotLog`), and a frame looks at those, at the
//! meshes whose geometry was not ready, and at a share of all slots in turn (the camera
//! brings slots into reach, and a material may change its look), counted in looks (a slot of
//! rows takes a look a row) so that a round of every slot takes about [`ROLL_FRAMES`] frames
//! however many rows there are. After the camera jumped, or when more happened than the log
//! keeps, it looks at every slot over a few frames and then drops what it did not find.

use super::*;
use crate::ray_scene::look::{material_light, texture_mean};
use crate::ray_scene::{
    RayClass, RayGeometryState, RayKey, RayLook, RaySceneOut, RaySource, RaySphere, RayTileSource,
    RayView,
};

/// Looks a frame takes in turn (a slot of rows: one a row): the last round's looks spread
/// over [`ROLL_FRAMES`] frames, at least this many, so that every slot comes up within about
/// that many frames. A slot of more rows than a frame's share is looked at whole, and the
/// frames after it take that much less.
const ROLL_LOOKS: usize = 2_048;
const ROLL_FRAMES: usize = 64;
/// Looks a frame takes while it looks at every slot.
const SWEEP_LOOKS: usize = 10_000;
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
/// Key bit of a geometry whose triangles a ray asks one by one (a mesh's handle index is
/// below 2^32): `opaque` cannot change for a key.
const ASKED: u64 = 1 << 32;

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
    /// Looks taken: a slot that draws its mesh once is one, a slot of rows one a row (one
    /// for a region wholly out of reach).
    looked: u32,
    /// Instances placed by them.
    placed: u32,
    /// Meshes whose geometry is not there yet.
    waiting: u32,
    /// Rows of `MeshInstances` slots placed, as of each slot's last look.
    rows: u32,
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
    /// The walk in turn: looks the frames to come may take (below 0 after a slot of more
    /// rows than a frame's share), the looks of its last round of every slot, and of the
    /// round under way so far.
    credit: i64,
    round_looks: usize,
    round_so_far: usize,
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
    /// What the scene holds of each `MeshInstances` slot's rows, by entity
    /// (`Entity::to_bits`).
    rows: FastMap<u64, HeldRows>,
    /// (A look's list of the rows in reach, kept for its allocation.)
    near_rows: Vec<NearRow>,
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

    /// The scene's name for the geometry of the mesh at handle index `index`, built `opaque`
    /// (its instances' material cuts no holes, [`cut_out`]) or to be asked triangle by
    /// triangle: a mesh placed both ways is two geometries.
    fn key(&self, index: u32, opaque: bool) -> RayKey {
        let generation = self.generations.get(&index).copied().unwrap_or(0);
        RayKey(
            index as u64 | if opaque { 0 } else { ASKED },
            generation as u64,
        )
    }

    /// The rows of entity `bits` (`Entity::to_bits`) are out of the scene: it is gone, draws
    /// its mesh once, or no row of it is in reach. Returns how many rows the scene may have
    /// held (each one taken out).
    fn drop_rows(&mut self, bits: u64, out: &mut RaySceneOut<'_>) -> usize {
        let Some(held) = self.rows.remove(&bits) else {
            return 0;
        };
        for k in 0..held.had {
            out.remove(RayKey(bits, k as u64 + 1));
        }
        held.had as usize
    }

    /// Note what the scene holds of entity `bits`'s rows after a look in frame `frame`
    /// ([`HeldRows`]; `had` 0: none), and take back the look's list.
    fn hold_rows(&mut self, bits: u64, had: u32, placed: u32, frame: u64, near: Vec<NearRow>) {
        if had == 0 {
            self.rows.remove(&bits);
        } else {
            let seen = frame;
            self.rows.insert(bits, HeldRows { had, placed, seen });
        }
        self.near_rows = near;
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
        d.set("render.meshes.ray_rows", s.rows as f64);
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
    /// In the scene, as `class` (one past a reach, within [`KEEP_M`] of it, only if it is
    /// there already), within `sphere`, its nearest point `distance` meters from the camera.
    Yes {
        class: RayClass,
        sphere: RaySphere,
        distance: f32,
    },
}

/// A row of a `MeshInstances` slot in reach, as a look of its slot found it.
#[derive(Clone, Copy)]
struct NearRow {
    /// Its place in the slot's rows.
    row: u32,
    class: RayClass,
    sphere: RaySphere,
    distance: f32,
    /// Mesh to world ([`mesh_scene::row_affine`]).
    to_world: DAffine3,
}

/// What the scene holds of a `MeshInstances` slot's rows (`MeshRays::rows`).
#[derive(Clone, Copy, Default)]
struct HeldRows {
    /// Rows `0..had` may be in the scene (as `RayKey(entity, row + 1)`); no other is.
    had: u32,
    /// Of those, the ones in the scene after the last look (counted again when a walk of
    /// every slot ends).
    placed: u32,
    /// The frame of that look.
    seen: u64,
}

/// The largest scale slot `s` places its mesh with: its own, times its rows' largest.
fn slot_scale(scene: &MeshScene, s: usize) -> f32 {
    let Some(hot) = scene.hot.get(s) else {
        return 0.0;
    };
    match scene.rows.get(s).and_then(Option::as_deref) {
        Some(rows) if hot.flags & mesh_scene::ROWS != 0 => {
            hot.scale * rows.iter().fold(0.0f32, |m, r| m.max(r.scale.abs()))
        }
        _ => hot.scale,
    }
}

/// Whether something of radius `radius` whose nearest point is `distance` from the camera
/// is within the scene's reach grown by `slack` meters: every such thing up to the casters'
/// reach, past it up to the far reach those at least `cut` times their distance across.
fn in_reach(view: &RayView, distance: f32, radius: f32, slack: f32) -> bool {
    distance <= view.caster_reach + slack
        || (distance <= view.far_reach + slack && radius >= view.cut * (distance - slack))
}

/// Whether a thing moves in the wind, by the two tests the shadow lists use: the mesh of a
/// bone plant (`MeshFrame::wind`), or a material that sways.
fn plant_sways(mesh: &MeshFrame, mat: &MaterialFrame) -> bool {
    mesh.wind != 0 || mat.sways
}

/// Whether material `mat` cuts holes in what it covers, so that the triangles a ray meets of it
/// are asked one by one (`RULE_LEAF_TEXTURE` in lib_ray.wgsl): masked, and its texture clear
/// somewhere (its share kept by an alpha test at 0.5, measured once the image was uploaded; a
/// mask without a texture, or over one with no clear texel, is drawn whole).
fn cut_out(mat: &MaterialFrame) -> bool {
    mat.masked && mat.coverage < 1.0
}

/// The class a slot (or a row of one) takes by what it is; `None`: no ray sees it.
/// `stand_in`: it is drawn into the shadow maps only (and casts into the finest: the meshes
/// leave out the others); `casts`: it casts a shadow; `cut_out`: its material cuts holes in it
/// ([`cut_out`]); `sways`: it moves in the wind ([`plant_sways`]); `radius`: its bounding
/// sphere's, meters.
fn class_of(
    stand_in: bool,
    casts: bool,
    cut_out: bool,
    sways: bool,
    radius: f32,
) -> Option<RayClass> {
    // A swaying card: the leaves that the canopy map holds for the probes while it is on.
    let leaf = cut_out && sways;
    Some(if stand_in {
        // A crown's stand-in is cards: the crown is in the rays once, through the holes of
        // its texture. Any other casts as a closed shape.
        if leaf {
            RayClass::Leaf
        } else {
            RayClass::ShadowOnly
        }
    } else if !casts {
        // A swaying card that casts no shadow is a crown beside its stand-in, and a small
        // thing that casts none no ray needs.
        if leaf || radius < MIN_RADIUS_M {
            return None;
        }
        RayClass::NoCast
    } else if leaf {
        RayClass::Leaf
    } else if sways {
        // (Before `Small`: what moves in the wind is `Sway` at any size.)
        RayClass::Sway
    } else if radius < MIN_RADIUS_M {
        RayClass::Small
    } else {
        RayClass::Solid
    })
}

/// What a hit of an instance with material `mat` reads: class `class`, a shadow stand-in
/// (`stand_in`), its tile, its colour times `tint` (a row's).
fn look_of(mat: &MaterialFrame, stand_in: bool, tile: Option<u16>, tint: [f32; 3]) -> RayLook {
    // (A tint above 1 brightens the colour no further than a surface may be.)
    let albedo = [0, 1, 2].map(|k| {
        let a = mat.ray_albedo[k];
        (a * tint[k]).min(a.max(0.95))
    });
    RayLook {
        albedo,
        roughness: mat.ray_surface[0],
        emissive: mat.ray_emissive,
        metallic: mat.ray_surface[1],
        tile,
        uv: mat.ray_uv,
        cutoff: mat.ray_surface[2],
        coverage: mat.coverage,
        // A crown's stand-in is a one-sided copy of its cards: a ray that meets the back of
        // one has met a leaf, not the inside of something.
        double_sided: mat.double_sided || stand_in,
    }
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

    /// The tables' entries of a slot a ray may see, and whether it is a shadow stand-in:
    /// drawn and not faded out, a stand-in only if it casts into the finest maps (a thing may
    /// have another for the coarse ones, which would cast its shadow a second time), its mesh
    /// loaded and not skinned, its material one that stops light (not blended, not unlit).
    fn usable(&self, hot: &Hot) -> Option<(MeshFrame, MaterialFrame, bool)> {
        if hot.flags & mesh_scene::VISIBLE == 0 || hot.dither.abs() >= 1.0 {
            return None;
        }
        let stand_in = hot.flags & mesh_scene::SHADOW_ONLY != 0;
        if stand_in
            && (hot.flags & mesh_scene::CASTS == 0 || !hot.cast_texels.casts_at(STAND_IN_TEXEL_M))
        {
            return None;
        }
        let (mesh, mat) = self.tables(hot)?;
        // (A mask over a colour whose alpha is below its cutoff is drawn nowhere: the raster
        // tests the texel's alpha times the colour's.)
        if mesh.version == 0
            || mesh.skinned
            || self.skins.skins.contains_key(&mesh.index)
            || self.unusable.get(&mesh.index) == Some(&mesh.version)
            || mat.blend
            || mat.unlit
            || (mat.masked && mat.ray_surface[2] > 1.0)
        {
            return None;
        }
        Some((*mesh, *mat, stand_in))
    }

    /// A mesh's bounding sphere in its own space (`None`: not known yet).
    fn local_bounds(&self, mesh: &MeshFrame) -> Option<(Vec3, f32)> {
        if mesh.ready {
            return Some((mesh.center, mesh.radius));
        }
        if let Some(sphere) = self.clusters.and_then(|c| c.bounds(mesh.index as u64)) {
            return Some((sphere.center, sphere.radius));
        }
        if let Some(&(version, center, radius)) = self.rays.bounds.get(&mesh.index)
            && version == mesh.version
        {
            return Some((center, radius));
        }
        // Drawn as clusters of another shape (displaced) and never measured: taken by its
        // origin and larger than it is likely to be, so that it is asked for its copy, which
        // brings its bounds.
        self.clustered
            .contains(&mesh.index)
            .then_some((Vec3::ZERO, UNMEASURED_RADIUS_M))
    }

    /// Slot `s`'s bounding sphere in the world (`None`: not known yet).
    fn bounds(&self, hot: &Hot, cold: &Cold, mesh: &MeshFrame) -> Option<(DVec3, f32)> {
        if mesh.ready && hot.mesh_version == mesh.version {
            // As the cull left them.
            return Some((hot.center, hot.radius));
        }
        let (center, radius) = self.local_bounds(mesh)?;
        Some((
            cold.transform.transform_point(center.as_dvec3()),
            radius * hot.scale,
        ))
    }

    /// Whether something of `class` within `sphere` (and `max_distance`, its own draw
    /// distance) is in the scene's reach; one past a reach, within `KEEP_M` of it, stays if
    /// it is there already (`placed`, asked only then).
    fn reach(
        &self,
        class: RayClass,
        sphere: RaySphere,
        max_distance: f32,
        placed: impl FnOnce() -> bool,
    ) -> Seen {
        let distance = sphere.distance(self.view.eye);
        let within = distance <= max_distance && in_reach(self.view, distance, sphere.radius, 0.0);
        let kept = || {
            distance <= max_distance + KEEP_M
                && in_reach(self.view, distance, sphere.radius, KEEP_M)
                && placed()
        };
        if within || kept() {
            Seen::Yes {
                class,
                sphere,
                distance,
            }
        } else {
            Seen::No
        }
    }

    /// What slot `s` (not one of rows) is to the rays. `placed`: whether it is in the scene
    /// now (asked only for a slot past a reach).
    fn seen(&self, s: usize, placed: impl FnOnce() -> bool) -> Seen {
        let hot = &self.scene.hot[s];
        if hot.flags & mesh_scene::ROWS != 0 {
            return Seen::No;
        }
        let Some((mesh, mat, stand_in)) = self.usable(hot) else {
            return Seen::No;
        };
        let Some((center, radius)) = self.bounds(hot, &self.scene.cold[s], &mesh) else {
            return Seen::NotYet;
        };
        let casts = hot.flags & mesh_scene::CASTS != 0;
        let sways = plant_sways(&mesh, &mat);
        let Some(class) = class_of(stand_in, casts, cut_out(&mat), sways, radius) else {
            return Seen::No;
        };
        self.reach(
            class,
            RaySphere { center, radius },
            hot.max_distance,
            placed,
        )
    }

    /// For a mesh that waits: whether a row of `MeshInstances` slot `s` may be in reach (by
    /// its region's sphere, where the cull measured it), and then whether the geometry its
    /// rows take is opaque.
    fn rows_may_be_seen(&self, s: usize) -> Option<bool> {
        let hot = &self.scene.hot[s];
        let (mesh, mat, _) = self.usable(hot)?;
        if self.region_out_of_reach(hot, &mesh) {
            return None;
        }
        Some(!cut_out(&mat))
    }

    /// Whether a `MeshInstances` slot's region (all its rows, as the cull measured it) lies
    /// wholly past every reach, so that none of its rows is in the scene; false while the cull
    /// has not measured it.
    fn region_out_of_reach(&self, hot: &Hot, mesh: &MeshFrame) -> bool {
        if !mesh.ready || hot.mesh_version != mesh.version {
            return false;
        }
        let distance = ((hot.center - self.view.eye).length() as f32 - hot.radius).max(0.0);
        distance > hot.max_distance.min(self.view.far_reach) + KEEP_M
    }

    /// The frame from which an instance of a slot holds still (0: it never moves): 90 frames
    /// after it last moved. What never moved does already; so does what last moved with the
    /// world (a world shift moves every slot, and the scene moved its instances itself) unless
    /// the scene has it (`placed`), which then tells a move of its own from the shift's.
    fn still_from(&self, cold: &Cold, placed: bool) -> u64 {
        if cold.moved == 0 || (cold.moved == self.view.shifted && !placed) {
            0
        } else {
            cold.moved + STILL_FRAMES
        }
    }

    /// The scene's geometry of `mesh` at its version, built `opaque` or to be asked triangle
    /// by triangle (its instances' material cuts holes, [`cut_out`]): there already, built now
    /// from its ranges of the pool at its ray level or from its copy, or waiting (its copy is
    /// asked for). `raw`, `scale`: the handle index slots name, and the largest scale of the
    /// slot that asks (0: no slot).
    fn geometry(
        &mut self,
        mesh: &MeshFrame,
        raw: u32,
        scale: f32,
        opaque: bool,
        out: &mut RaySceneOut<'_>,
    ) -> RayGeometryState {
        let key = self.rays.key(mesh.index, opaque);
        let scene = self.scene;
        if let Some(gpu_mesh) = self.meshes.get_mut(&mesh.index)
            && gpu_mesh.version == mesh.version
        {
            // The largest scale it is placed with. The first look takes every slot that names
            // its handle (rows at their largest), so that a scene switched on where the levels
            // are there already builds each mesh once, at its level.
            if gpu_mesh.ray_scale <= 0.0 {
                for &s in scene.slots_of_mesh(raw) {
                    gpu_mesh.ray_scale = gpu_mesh.ray_scale.max(slot_scale(scene, s as usize));
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
            return Self::from_pool(self.pool, key, opaque, gpu_mesh, out);
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
                opaque,
                RaySource::Cpu {
                    positions: &copy.positions,
                    indices: &copy.indices,
                },
            );
            if matches!(state, RayGeometryState::Ready | RayGeometryState::Refused) {
                // (A mesh placed both opaque and asked is copied again for its other key.)
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
    /// now, built `opaque` or to be asked. A level the scene refuses is passed over for the
    /// next finer one (so a refusal never takes out of the rays what the whole mesh would have
    /// put in).
    fn from_pool(
        pool: &MeshPool,
        key: RayKey,
        opaque: bool,
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
                opaque,
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

    /// The tile a hit of an instance of `mesh` reads, by its material's texture: only where a
    /// hit has a uv to read it at, triangles with records (a geometry built from the pool's
    /// ranges; one drawn only as clusters is built from a copy, which has none). A stand-in
    /// that casts as a closed shape shows no colour; a crown's (a `Leaf`) is cut out by it.
    fn tile_for(
        &self,
        class: RayClass,
        mesh: &MeshFrame,
        mat: &MaterialFrame,
        distance: f32,
        out: &mut RaySceneOut<'_>,
    ) -> Option<u16> {
        let pooled = self
            .meshes
            .get(&mesh.index)
            .is_some_and(|g| g.version == mesh.version);
        if class == RayClass::ShadowOnly || !pooled {
            return None;
        }
        self.tile(mat.ray_tile, distance, out)
    }

    /// Bring slot `s` up to date in the scene: placed as it stands, or removed. Returns how
    /// many instances it looked at (a slot of rows: one a row, or one for a region it passes
    /// over whole), which `render.meshes.ray_looked` counts.
    fn slot(&mut self, s: usize, out: &mut RaySceneOut<'_>) -> usize {
        let looks = if self.scene.hot[s].flags & mesh_scene::ROWS != 0 {
            self.rows(s, out)
        } else {
            self.single(s, out)
        };
        self.rays.stats.looked += looks as u32;
        looks
    }

    /// [`slot`](Self::slot) for a slot that draws its mesh once.
    fn single(&mut self, s: usize, out: &mut RaySceneOut<'_>) -> usize {
        let scene = self.scene;
        let (hot, cold) = (&scene.hot[s], &scene.cold[s]);
        let bits = cold.entity.to_bits();
        // (Its entity may have drawn rows before.)
        self.rays.drop_rows(bits, out);
        let id = RayKey(bits, 0);
        let (class, sphere, distance) = match self.seen(s, || out.has(id)) {
            Seen::No => {
                out.remove(id);
                return 1;
            }
            Seen::NotYet => {
                self.rays.waiting.insert(hot.mesh);
                return 1;
            }
            Seen::Yes {
                class,
                sphere,
                distance,
            } => (class, sphere, distance),
        };
        let Some((&mesh, &mat)) = self.tables(hot) else {
            return 1;
        };
        let opaque = !cut_out(&mat);
        match self.geometry(&mesh, hot.mesh, hot.scale, opaque, out) {
            RayGeometryState::Ready => {}
            // Placed with the shape before, and asked again every frame until its own is
            // built.
            RayGeometryState::Older => {
                self.rays.waiting.insert(hot.mesh);
            }
            RayGeometryState::Building => {
                self.rays.waiting.insert(hot.mesh);
                return 1;
            }
            RayGeometryState::Refused => {
                out.remove(id);
                return 1;
            }
        }
        let still_from = self.still_from(cold, out.has(id));
        let tile = self.tile_for(class, &mesh, &mat, distance, out);
        let stand_in = hot.flags & mesh_scene::SHADOW_ONLY != 0;
        let look = look_of(&mat, stand_in, tile, [1.0; 3]);
        out.place(
            id,
            self.rays.key(mesh.index, opaque),
            &cold.transform.0,
            sphere,
            class,
            still_from,
            &look,
        );
        self.rays.stats.placed += 1;
        1
    }

    /// Bring the rows of `MeshInstances` slot `s` up to date in the scene, each an instance of
    /// its own (`RayKey(entity, row + 1)`) of the one geometry they share, its class by its
    /// own size and its look tinted by its row's tint. Returns how many rows it looked at: one
    /// for a region it passes over whole (it cannot be used, it is wholly out of reach, its
    /// bounds are not known yet), and one for each row it took out of the scene.
    fn rows(&mut self, s: usize, out: &mut RaySceneOut<'_>) -> usize {
        let scene = self.scene;
        let (hot, cold) = (&scene.hot[s], &scene.cold[s]);
        let rows = scene.rows[s].as_deref().unwrap_or(&[]);
        let bits = cold.entity.to_bits();
        let looks = rows.len().max(1);
        // (Its entity may have drawn its mesh once before.)
        out.remove(RayKey(bits, 0));
        let Some((mesh, mat, stand_in)) = self.usable(hot) else {
            return 1 + self.rays.drop_rows(bits, out);
        };
        if self.region_out_of_reach(hot, &mesh) {
            return 1 + self.rays.drop_rows(bits, out);
        }
        let Some((center, radius)) = self.local_bounds(&mesh) else {
            self.rays.waiting.insert(hot.mesh);
            return 1;
        };
        let held = self.rays.rows.get(&bits).copied().unwrap_or_default();
        let casts = hot.flags & mesh_scene::CASTS != 0;
        let sways = plant_sways(&mesh, &mat);
        let cut = cut_out(&mat);
        // Which rows are in reach (those that may be in the scene and are not go).
        let mut near = std::mem::take(&mut self.rays.near_rows);
        near.clear();
        let mut largest = 0.0f32;
        for (k, row) in rows.iter().enumerate() {
            let id = RayKey(bits, k as u64 + 1);
            let to_world = mesh_scene::row_affine(&cold.transform, row);
            let sphere = RaySphere {
                center: to_world.transform_point3(center.as_dvec3()),
                radius: radius * row.scale.abs() * hot.scale,
            };
            let seen = match class_of(stand_in, casts, cut, sways, sphere.radius) {
                Some(class) => self.reach(class, sphere, hot.max_distance, || {
                    held.had > k as u32 && out.has(id)
                }),
                None => Seen::No,
            };
            match seen {
                Seen::Yes {
                    class,
                    sphere,
                    distance,
                } => {
                    largest = largest.max(row.scale.abs() * hot.scale);
                    near.push(NearRow {
                        row: k as u32,
                        class,
                        sphere,
                        distance,
                        to_world,
                    });
                }
                _ if held.had > k as u32 => out.remove(id),
                _ => {}
            }
        }
        // Rows it had and has no more.
        for k in rows.len() as u32..held.had {
            out.remove(RayKey(bits, k as u64 + 1));
        }
        // Rows that may be in the scene now, whatever the geometry's state.
        let had = (rows.len() as u32).min(held.had);
        let frame = self.view.frame;
        if near.is_empty() {
            // (Every row it had is out: those in the list removed above, the rest before.)
            self.rays.hold_rows(bits, 0, 0, frame, near);
            return looks;
        }
        let opaque = !cut;
        match self.geometry(&mesh, hot.mesh, largest, opaque, out) {
            RayGeometryState::Ready => {}
            RayGeometryState::Older => {
                self.rays.waiting.insert(hot.mesh);
            }
            RayGeometryState::Building => {
                // (Rows placed before and still in reach stay as they are; those this look
                // took out are out.)
                self.rays.waiting.insert(hot.mesh);
                let placed = near
                    .iter()
                    .filter(|r| r.row < had && out.has(RayKey(bits, r.row as u64 + 1)))
                    .count() as u32;
                self.rays.hold_rows(bits, had, placed, frame, near);
                return looks;
            }
            RayGeometryState::Refused => {
                for k in 0..had {
                    out.remove(RayKey(bits, k as u64 + 1));
                }
                self.rays.hold_rows(bits, 0, 0, frame, near);
                return looks;
            }
        }
        // One tile for every row (they share the material), asked for by the nearest.
        let nearest = near.iter().fold(f32::MAX, |m, r| m.min(r.distance));
        let tile = self.tile_for(near[0].class, &mesh, &mat, nearest, out);
        let key = self.rays.key(mesh.index, opaque);
        for r in &near {
            let id = RayKey(bits, r.row as u64 + 1);
            let still_from = self.still_from(cold, out.has(id));
            let look = look_of(&mat, stand_in, tile, rows[r.row as usize].tint);
            out.place(id, key, &r.to_world, r.sphere, r.class, still_from, &look);
        }
        let placed = near.len() as u32;
        self.rays.stats.placed += placed;
        self.rays
            .hold_rows(bits, rows.len() as u32, placed, frame, near);
        looks
    }

    /// A mesh whose geometry was not there when a slot of its was looked at: ask again
    /// while a slot of its is in reach, and place its slots once it is.
    fn waiting_mesh(&mut self, raw: u32, out: &mut RaySceneOut<'_>) {
        let Some(&mesh) = self.mesh_frames.get(raw as usize) else {
            return;
        };
        let scene = self.scene;
        let slots = scene.slots_of_mesh(raw);
        // The geometry the first slot in reach takes (a slot at the edge counts only once it
        // is placed, which none of these is). Slots of the other kind ask for theirs when they
        // are placed.
        let mut unknown = false;
        let mut wanted = None;
        for &s in slots {
            let s = s as usize;
            let opaque = if scene.hot[s].flags & mesh_scene::ROWS != 0 {
                self.rows_may_be_seen(s)
            } else {
                match self.seen(s, || false) {
                    Seen::Yes { .. } => self.tables(&scene.hot[s]).map(|(_, mat)| !cut_out(mat)),
                    Seen::NotYet => {
                        unknown = true;
                        None
                    }
                    Seen::No => None,
                }
            };
            if opaque.is_some() {
                wanted = opaque;
                break;
            }
        }
        let Some(opaque) = wanted else {
            // None in reach: the walk in turn finds it when one comes. Bounds not known
            // yet: its upload is on its way, so it keeps waiting.
            if unknown {
                self.rays.waiting.insert(raw);
            }
            return;
        };
        match self.geometry(&mesh, raw, 0.0, opaque, out) {
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
            view,
            rays: &mut rays,
            gpu,
            vt: self.vt.as_ref(),
        };
        // Gone first: an entity in both lists is gone unless it has a slot now.
        for entity in &gone {
            out.remove(RayKey(entity.to_bits(), 0));
            fill.rays.drop_rows(entity.to_bits(), out);
        }
        for &entity in &touched {
            match fill.scene.slot_of_entity(entity) {
                Some(s) => {
                    fill.slot(s, out);
                }
                None => {
                    out.remove(RayKey(entity.to_bits(), 0));
                    fill.rays.drop_rows(entity.to_bits(), out);
                }
            }
        }
        // Meshes edited in place, and meshes whose ray level changed (their levels came, or
        // they are placed larger than they were): the scene's geometry of them is built again
        // while an instance uses it (their instances stay as they are, with the shape before
        // until then; one nothing uses is left to be forgotten).
        // (A mesh placed both opaque and asked is two geometries, each built again.)
        for index in edited {
            let Some(gpu_mesh) = fill.meshes.get_mut(&index) else {
                continue;
            };
            if !gpu_mesh.ray_ok {
                continue;
            }
            for opaque in [true, false] {
                let key = fill.rays.key(index, opaque);
                let wanted = ray_version(gpu_mesh.version, gpu_mesh.ranges, gpu_mesh.ray_level());
                if out.geometry_version(key).is_some_and(|v| v != wanted)
                    && out.geometry_users(key) > 0
                    && Fill::from_pool(fill.pool, key, opaque, gpu_mesh, out)
                        == RayGeometryState::Older
                {
                    // Put off (built lately, or the frame's allowance is spent).
                    fill.rays.edited.insert(index);
                }
            }
        }
        for raw in waiting {
            fill.waiting_mesh(raw, out);
        }
        // The walks count looks, not slots: a slot of rows is a look a row.
        let slots = fill.scene.hot.len();
        match fill.rays.sweep {
            Some((next, began)) => {
                let (mut end, mut looks) = (next.min(slots), 0);
                while end < slots && looks < SWEEP_LOOKS {
                    looks += fill.slot(end, out);
                    end += 1;
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
                        // (Rows kept while their geometry was building, placed before the walk
                        // began, are gone now: what is left of each slot is counted again.)
                        fill.rays.rows.retain(|&bits, held| {
                            held.placed = (0..held.had)
                                .filter(|&k| out.has(RayKey(bits, k as u64 + 1)))
                                .count() as u32;
                            held.seen >= began
                        });
                    }
                    fill.rays.sweep = again.then_some((0, view.frame + 1));
                    fill.rays.cursor = 0;
                    fill.rays.round_so_far = 0;
                } else {
                    fill.rays.sweep = Some((end, began));
                }
            }
            None if slots > 0 => {
                // The last round's looks over `ROLL_FRAMES` frames: what a slot of many rows
                // takes past a frame's share, the frames after it take less (at most one
                // round a frame).
                let share = (fill.rays.round_looks.max(slots) / ROLL_FRAMES).max(ROLL_LOOKS) as i64;
                fill.rays.credit = (fill.rays.credit + share).min(share);
                let first = fill.rays.cursor % slots;
                let mut k = 0;
                while k < slots && fill.rays.credit > 0 {
                    let at = (first + k) % slots;
                    let looks = fill.slot(at, out);
                    fill.rays.credit -= looks as i64;
                    fill.rays.round_so_far += looks;
                    if at + 1 == slots {
                        fill.rays.round_looks = std::mem::take(&mut fill.rays.round_so_far);
                    }
                    k += 1;
                }
                fill.rays.cursor = (first + k) % slots;
            }
            None => {}
        }
        rays.stats.waiting = rays.waiting.len() as u32;
        rays.stats.rows = rays.rows.values().map(|held| held.placed).sum();
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

    /// What each kind of slot is to the rays.
    #[test]
    fn a_slot_takes_its_class_by_what_it_is() {
        let (small, big) = (0.1, 1.0);
        // (stand-in, casts, cut out, sways, radius)
        let cases = [
            // A crown's stand-in: its swaying cards; another stand-in casts as a closed shape.
            ((true, true, true, true, big), Some(RayClass::Leaf)),
            (
                (true, true, false, false, small),
                Some(RayClass::ShadowOnly),
            ),
            ((true, true, true, false, big), Some(RayClass::ShadowOnly)),
            // The crown beside its stand-in, and small things that cast nothing.
            ((false, false, true, true, big), None),
            ((false, false, false, false, small), None),
            ((false, false, false, false, big), Some(RayClass::NoCast)),
            ((false, false, true, false, big), Some(RayClass::NoCast)),
            ((false, false, false, true, big), Some(RayClass::NoCast)),
            // Swaying cards that cast, at any size; saplings; fences, pebbles, rocks.
            ((false, true, true, true, small), Some(RayClass::Leaf)),
            ((false, true, true, true, big), Some(RayClass::Leaf)),
            ((false, true, false, true, small), Some(RayClass::Sway)),
            ((false, true, true, false, big), Some(RayClass::Solid)),
            ((false, true, true, false, small), Some(RayClass::Small)),
            ((false, true, false, false, small), Some(RayClass::Small)),
            ((false, true, false, false, big), Some(RayClass::Solid)),
        ];
        for ((stand_in, casts, cut, sways, radius), class) in cases {
            assert_eq!(
                class_of(stand_in, casts, cut, sways, radius),
                class,
                "{:?}",
                (stand_in, casts, cut, sways, radius)
            );
        }
        // A `Leaf` is a swaying card (what the canopy map holds), and a caster the probes see
        // never sways (what casts none they see whole, as their own scene does).
        for bits in 0..32u32 {
            let [stand_in, casts, cut, sways, large] = [0, 1, 2, 3, 4].map(|b| bits >> b & 1 != 0);
            let radius = if large { big } else { small };
            match class_of(stand_in, casts, cut, sways, radius) {
                Some(RayClass::Leaf) => assert!(cut && sways, "{bits:05b}"),
                Some(RayClass::Solid) => assert!(!sways, "{bits:05b}"),
                _ => {}
            }
        }
    }

    /// A mask cuts holes where its texture is clear somewhere; over no texture, or one with
    /// no clear texel, it is drawn whole.
    #[test]
    fn a_mask_cuts_holes_only_where_its_texture_is_clear() {
        let mask = |coverage| MaterialFrame {
            masked: true,
            coverage,
            ..MaterialFrame::default()
        };
        assert!(cut_out(&mask(0.4)));
        assert!(!cut_out(&mask(1.0)));
        assert!(!cut_out(&MaterialFrame {
            coverage: 0.4,
            ..MaterialFrame::default()
        }));
    }

    /// A mesh placed both opaque and asked is two geometries of one mesh.
    #[test]
    fn the_two_ways_of_a_mesh_are_two_keys() {
        let mut rays = MeshRays::default();
        rays.set_on(true);
        let (opaque, asked) = (rays.key(7, true), rays.key(7, false));
        assert_ne!(opaque, asked);
        assert_eq!(opaque.0 | ASKED, asked.0);
        assert_eq!(opaque.1, asked.1);
        assert_ne!(rays.key(7, true), rays.key(8, true));
    }

    /// Everything up to the casters' reach; past it, up to the far reach, what is at least
    /// the cut times its distance across; a thing at an edge stays within `KEEP_M` of it.
    #[test]
    fn far_things_are_in_reach_by_their_size() {
        let view = RayView {
            eye: DVec3::ZERO,
            frame: 0,
            epoch: 0,
            shifted: 0,
            caster_reach: 160.0,
            far_reach: 384.0,
            cut: 0.0175,
            ground_patches: 0,
        };
        assert!(in_reach(&view, 150.0, 0.01, 0.0));
        assert!(!in_reach(&view, 200.0, 3.0, 0.0));
        assert!(in_reach(&view, 200.0, 4.0, 0.0));
        assert!(!in_reach(&view, 400.0, 100.0, 0.0));
        assert!(in_reach(&view, 170.0, 0.01, KEEP_M));
        assert!(in_reach(&view, 390.0, 7.0, KEEP_M));
        assert!(!in_reach(&view, 401.0, 100.0, KEEP_M));
    }

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
