//! The ray scene's instances and the two top-level structures that hold them: one for what
//! holds still, built again at most every [`STILL_EVERY`] frames, and a small one for what
//! moves, built again whenever a member changed. wgpu can only build a top-level structure
//! whole, and what a build costs the render thread grows with its instances (0.06 ms a
//! thousand to record and about half that again at submit on the RTX 5080), so the many that
//! never move are kept out of the structure that is built every frame.
//!
//! An instance answers rays with its row ([`RayRow`], the structures' `custom_data`): a still
//! one's row is its slot, a moving one's [`STILL_CAPACITY`] plus its slot.

use std::collections::HashMap;

use engine_core::prelude::*;

use super::blas::{MOVING, Name, STILL, Store};
use super::hook::{RayClass, RayKey, RayLook, RaySphere};

/// Instances the still structure holds at most.
pub(super) const STILL_CAPACITY: u32 = 16_384;
/// Instances the moving structure holds at most.
pub(super) const MOVING_CAPACITY: u32 = 2_048;
/// Frames between two builds of the still structure, at least (an anchor move builds it at
/// once).
const STILL_EVERY: u64 = 4;
/// How close a placement must be to the one an instance has to count as the same: its
/// translation in meters, and each element of its matrix as a share of the largest (so a
/// scaled instance is held to the same angle as one that is not). A world shift and the
/// transform propagation round differently: the scene turns a placement in `f64`, the game
/// turns each root's `f32` rotation and places its children from it, which differs by a few
/// `f32` steps of the rotation, times the instance's scale and a child's distance from its
/// root. A thing that creeps is still seen to move once it is this far from where the
/// scene has it.
const UNMOVED_M: f64 = 1e-3;
const UNMOVED_TURN: f64 = 4e-6;
/// No instance (a slot), no geometry (what a slot was set with).
const NONE: u32 = u32::MAX;
/// At a full structure, what waits for room takes the slot of the farthest member only when
/// it is this much nearer the camera (meters), so that two at much the same distance do not
/// trade places as the camera sways (each trade sets two slots).
const TRADE_M: f32 = 16.0;
/// Trades at a build, at most.
const TRADES_MOST: usize = 1_024;
/// How far the camera moves (meters) before a full structure whose waiters found no trade is
/// ranked again (unless more wait since).
const TRADE_AGAIN_M: f64 = 1.0;
/// The feature number of the measuring fill's copies ([`Instances::stress`]): no feature has it.
pub(super) const STRESS: u16 = u16::MAX;

/// [`RayRow::flags`]: lit from both sides.
const ROW_DOUBLE_SIDED: u32 = 1 << 8;
/// [`RayRow::flags`]: `tris` names the triangle records of the structure the slot holds.
const ROW_HAS_TRIS: u32 = 1 << 9;
/// [`RayRow::flags`]: `tile` names a layer of the ray tiles.
const ROW_HAS_TILE: u32 = 1 << 10;

/// What a hit reads of its instance (`RayRow` in lib_ray.wgsl).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct RayRow {
    /// Bits 0-7: the class ([`RayClass::bit`]); 8: lit from both sides; 9: `tris` names its
    /// triangle records; 10: `tile` names its layer; 16-23: the alpha cutoff, 24-31: the
    /// texture's coverage (both in 255ths).
    pub flags: u32,
    /// Its first triangle record.
    pub tris: u32,
    /// Its layer in the ray tiles.
    pub tile: u32,
    /// Reserved (a tint over the tile).
    pub tint: u32,
    /// Mean diffuse colour, and roughness.
    pub albedo: [f32; 4],
    /// Its own glow (cd/m²), and metallic.
    pub emissive: [f32; 4],
    /// The texture's scale (xy) and offset (zw).
    pub uv: [f32; 4],
}

impl RayRow {
    pub(super) fn new(class: RayClass, look: &RayLook) -> Self {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        let mut flags = class.bit() as u32 | byte(look.cutoff) << 16 | byte(look.coverage) << 24;
        if look.double_sided {
            flags |= ROW_DOUBLE_SIDED;
        }
        if look.tile.is_some() {
            flags |= ROW_HAS_TILE;
        }
        let [r, g, b] = look.albedo;
        let [er, eg, eb] = look.emissive;
        Self {
            flags,
            tris: 0,
            tile: look.tile.map_or(0, u32::from),
            tint: u32::MAX,
            albedo: [r, g, b, look.roughness],
            emissive: [er, eg, eb, look.metallic],
            uv: look.uv,
        }
    }
}

/// Where an instance is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum At {
    /// In neither structure: the one it belongs in is full. It is tried again every frame.
    /// Its place in the list of the parked.
    Parked(u32),
    /// In a slot of the still ([`STILL`]) or the moving ([`MOVING`]) structure.
    In(usize, u32),
}

struct Instance {
    name: Name,
    /// Its geometry in the [`Store`], and its place in that geometry's list of instances.
    geometry: u32,
    of_geometry: u32,
    /// Mesh to world.
    to_world: DAffine3,
    /// Its bounding sphere in the world (moved with `to_world` by a shift): what ranks it at
    /// a full structure.
    sphere: RaySphere,
    class: RayClass,
    /// The frame from which it holds still.
    still_from: u64,
    row: RayRow,
    at: At,
    /// The frame it took its slot.
    entered: u64,
    /// The frame it was last placed (as it stood, or anew).
    seen: u64,
}

/// Mesh to anchor-relative world as the rows of a 3x4, what a top-level instance takes.
fn rows_3x4(a: &DAffine3, anchor: DVec3) -> [f32; 12] {
    let m = a.matrix3;
    let t = a.translation - anchor;
    [
        m.x_axis.x as f32,
        m.y_axis.x as f32,
        m.z_axis.x as f32,
        t.x as f32,
        m.x_axis.y as f32,
        m.y_axis.y as f32,
        m.z_axis.y as f32,
        t.y as f32,
        m.x_axis.z as f32,
        m.y_axis.z as f32,
        m.z_axis.z as f32,
        t.z as f32,
    ]
}

/// Whether two placements are the same to within [`UNMOVED_M`] and [`UNMOVED_TURN`].
fn unmoved(a: &DAffine3, b: &DAffine3) -> bool {
    let largest = a
        .matrix3
        .x_axis
        .abs()
        .max(a.matrix3.y_axis.abs())
        .max(a.matrix3.z_axis.abs())
        .max_element();
    a.translation.abs_diff_eq(b.translation, UNMOVED_M)
        && a.matrix3.abs_diff_eq(b.matrix3, UNMOVED_TURN * largest)
}

/// One of the two top-level structures.
struct Level {
    tlas: wgpu::Tlas,
    capacity: u32,
    /// Each slot's instance ([`NONE`]: empty).
    slots: Vec<u32>,
    /// The geometry each slot of `tlas` was last set with ([`NONE`]: with nothing).
    set_with: Vec<u32>,
    /// The first triangle record of the structure each slot was last set with ([`NONE`]:
    /// that structure has no records). Changed only where a slot is set ([`Instances::set_slots`]):
    /// the built structure holds that structure, whatever the instance's geometry became since.
    tris: Vec<u32>,
    /// Empty slots an instance may take (the last is taken first).
    free: Vec<u32>,
    /// Slots emptied since the last build: the built structure still holds what was in them
    /// (and rays read their rows), so they are free only once it is built without them.
    freed: Vec<u32>,
    /// Slots to set again at the next build, and whether each is among them.
    dirty: Vec<u32>,
    is_dirty: Vec<bool>,
    /// Geometries whose structure a slot held until it was set again or emptied; the built
    /// structure holds them until the next build ([`Store::release_slots`]).
    released: Vec<u32>,
    /// Something changed since the last build.
    wanted: bool,
    /// The frame it was last built (0: never), and the instances that build held.
    built: u64,
    live: u32,
    /// Slots that hold an instance.
    used: u32,
    /// Which slots hold a shadow stand-in ([`RayClass::ShadowOnly`]), and how many do.
    is_stand_in: Vec<bool>,
    stand_ins: u32,
    /// Which slots hold a copy of the measuring fill, and how many do.
    is_copy: Vec<bool>,
    copies: u32,
}

impl Level {
    fn new(device: &wgpu::Device, label: &str, capacity: u32) -> Self {
        let capacity = capacity.min(device.limits().max_tlas_instance_count);
        let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some(label),
            max_instances: capacity,
            // The rays read the hit triangle's corners (`getCommittedHitVertexPositions`): the
            // top-level structure must allow it as well as each geometry's.
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
                | wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        Self {
            tlas,
            capacity,
            slots: vec![NONE; capacity as usize],
            set_with: vec![NONE; capacity as usize],
            tris: vec![NONE; capacity as usize],
            free: (0..capacity).rev().collect(),
            freed: Vec::new(),
            dirty: Vec::new(),
            is_dirty: vec![false; capacity as usize],
            released: Vec::new(),
            // Built at once, empty: a submit that binds a structure never built fails.
            wanted: true,
            built: 0,
            live: 0,
            used: 0,
            is_stand_in: vec![false; capacity as usize],
            stand_ins: 0,
            is_copy: vec![false; capacity as usize],
            copies: 0,
        }
    }

    /// `slot` holds a shadow stand-in from now on, or not.
    fn set_stand_in(&mut self, slot: u32, stand_in: bool) {
        if std::mem::replace(&mut self.is_stand_in[slot as usize], stand_in) != stand_in {
            if stand_in {
                self.stand_ins += 1;
            } else {
                self.stand_ins -= 1;
            }
        }
    }

    /// `slot` holds a copy of the measuring fill from now on, or not.
    fn set_copy(&mut self, slot: u32, copy: bool) {
        if std::mem::replace(&mut self.is_copy[slot as usize], copy) != copy {
            if copy {
                self.copies += 1;
            } else {
                self.copies -= 1;
            }
        }
    }

    /// Set `slot` again at the next build.
    fn mark(&mut self, slot: u32) {
        self.wanted = true;
        if !std::mem::replace(&mut self.is_dirty[slot as usize], true) {
            self.dirty.push(slot);
        }
    }
}

/// `render.ray.*` numbers of the instances.
#[derive(Default, Clone, Copy, Debug)]
pub(super) struct InstanceStats {
    /// Instances in the still and in the moving structure, as last built.
    pub still: u32,
    pub moving: u32,
    /// Instances that wait for room.
    pub parked: u32,
    /// Placements that found the structure they belong in full this frame: parked, to wait
    /// for room or for a farther member to trade places with.
    pub refused: u32,
    /// Members of a full structure that gave their slot to a nearer instance, since the
    /// scene was made.
    pub evicted: u32,
    /// What belongs in the still structure (holds still by now), with a slot or waiting for
    /// one: past its cap, the valve raises the cut.
    pub still_set: u32,
    /// At the last ranking of a full still structure: its farthest member's nearest point and
    /// the nearest of what waits for it (meters; -1 for none).
    pub still_farthest_m: f32,
    pub parked_nearest_m: f32,
    /// Instances of each class ([`RayClass::ALL`]), the measuring fill's copies left out.
    pub classes: [u32; 8],
    /// The structures built this frame (bits `1 << STILL`, `1 << MOVING`).
    pub built: u8,
    /// Slots set this frame.
    pub set: u32,
}

/// The instances and their two structures.
pub(super) struct Instances {
    list: Vec<Option<Instance>>,
    free: Vec<u32>,
    names: HashMap<Name, u32>,
    /// Each geometry's instances (by the geometry's index in the [`Store`]).
    by_geometry: Vec<Vec<u32>>,
    levels: [Level; 2],
    parked: Vec<u32>,
    /// The rows as the GPU has them, and the range of each level's that changed.
    rows: Vec<RayRow>,
    rows_changed: [Option<(u32, u32)>; 2],
    /// World position the structures' instances are placed around.
    anchor: DVec3,
    /// Build both structures in this frame, whatever their turn (the anchor moved).
    rebuild_all: bool,
    /// The frame of the last world shift (0: none). An instance not placed since is taken
    /// where the feature has it, if that is where the shift left it to within rounding.
    shifted: u64,
    /// The structures this frame builds (bits), set by [`settle`](Self::settle).
    building: u8,
    /// Rows of instances (in a structure or parked) that name each layer of the ray tiles:
    /// a layer goes to another texture only once none has for a while.
    tile_users: Vec<u32>,
    /// Instances parked since the scene was made; the camera, that count and the movers
    /// that held still when the still structure's waiters last found no trade (its ranking
    /// waits until one of them changes).
    parked_ever: u64,
    quiet_trade: Option<(DVec3, u64, usize)>,
    /// The measuring fill's copies placed, and whether both structures are built every frame
    /// for it.
    copies: u32,
    every_frame: bool,
    stats: InstanceStats,
}

/// The layer of the ray tiles `row` names.
fn tile_of(row: &RayRow) -> Option<usize> {
    (row.flags & ROW_HAS_TILE != 0).then_some(row.tile as usize)
}

impl Instances {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let levels = [
            Level::new(device, "ray scene still", STILL_CAPACITY),
            Level::new(device, "ray scene moving", MOVING_CAPACITY),
        ];
        Self {
            list: Vec::new(),
            free: Vec::new(),
            names: HashMap::new(),
            by_geometry: Vec::new(),
            levels,
            parked: Vec::new(),
            rows: vec![bytemuck::Zeroable::zeroed(); (STILL_CAPACITY + MOVING_CAPACITY) as usize],
            rows_changed: [None; 2],
            anchor: DVec3::ZERO,
            rebuild_all: false,
            shifted: 0,
            building: 0,
            tile_users: vec![0; super::tiles::LAYERS as usize],
            parked_ever: 0,
            quiet_trade: None,
            copies: 0,
            every_frame: false,
            stats: InstanceStats {
                still_farthest_m: -1.0,
                parked_nearest_m: -1.0,
                ..Default::default()
            },
        }
    }

    pub(super) fn tlas(&self, level: usize) -> &wgpu::Tlas {
        &self.levels[level].tlas
    }

    /// Rows that name layer `layer` of the ray tiles now.
    pub(super) fn tile_users(&self, layer: u16) -> u32 {
        self.tile_users.get(layer as usize).copied().unwrap_or(0)
    }

    /// An instance's row named layer `was` of the ray tiles, and names `now`.
    fn count_tile(&mut self, was: Option<usize>, now: Option<usize>) {
        if was == now {
            return;
        }
        if let Some(n) = was.and_then(|t| self.tile_users.get_mut(t)) {
            *n -= 1;
        }
        if let Some(n) = now.and_then(|t| self.tile_users.get_mut(t)) {
            *n += 1;
        }
    }

    pub(super) fn anchor(&self) -> DVec3 {
        self.anchor
    }

    pub(super) fn stats(&self) -> InstanceStats {
        InstanceStats {
            still: self.levels[STILL].live,
            moving: self.levels[MOVING].live,
            parked: self.parked.len() as u32,
            ..self.stats
        }
    }

    /// The instances each structure holds once this frame's builds ran (after
    /// [`settle`](Self::settle)): still, moving.
    pub(super) fn counts(&self) -> [u32; 2] {
        [STILL, MOVING].map(|level| {
            let l = &self.levels[level];
            if self.building & (1 << level) != 0 {
                l.used
            } else {
                l.live
            }
        })
    }

    /// The shadow stand-ins that hold a slot of the still and of the moving structure. One
    /// that took its slot since the structure was last built is counted before a ray can
    /// meet it, and one that left is not though rays meet it until the next build: a count
    /// only says whether asking the structure for stand-ins is worth a ray's while.
    pub(super) fn stand_ins(&self) -> [u32; 2] {
        [STILL, MOVING].map(|level| self.levels[level].stand_ins)
    }

    /// The measuring fill's copies placed.
    pub(super) fn copies(&self) -> u32 {
        self.copies
    }

    /// Instances the scene knows (placed or parked).
    pub(super) fn len(&self) -> usize {
        self.names.len()
    }

    /// Start a frame's placements.
    pub(super) fn begin_frame(&mut self) {
        self.stats.refused = 0;
        self.stats.built = 0;
        self.stats.set = 0;
    }

    /// The structures' instances are placed around `anchor` from now on: both are built again
    /// this frame.
    pub(super) fn set_anchor(&mut self, anchor: DVec3) {
        if anchor == self.anchor {
            return;
        }
        self.anchor = anchor;
        self.rebuild_all = true;
        for level in &mut self.levels {
            for slot in 0..level.capacity {
                if level.slots[slot as usize] != NONE {
                    level.mark(slot);
                }
            }
        }
    }

    /// The world moved to a new frame by `shift` (mesh to world: `shift * to_world`), in
    /// frame `frame`. A move without a turn carries the anchor along and leaves both
    /// structures as they are.
    pub(super) fn shift(&mut self, shift: &DAffine3, translation_only: bool, frame: u64) {
        self.shifted = frame;
        self.quiet_trade = None;
        for i in self.list.iter_mut().flatten() {
            if translation_only {
                i.to_world.translation += shift.translation;
                i.sphere.center += shift.translation;
            } else {
                i.to_world = *shift * i.to_world;
                i.sphere.center = shift.transform_point3(i.sphere.center);
            }
        }
        if translation_only {
            self.anchor += shift.translation;
        } else {
            let anchor = shift.transform_point3(self.anchor);
            // (Not through `set_anchor`: every instance turned, whether or not the anchor
            // lands where it was.)
            self.anchor = anchor;
            self.rebuild_all = true;
            for level in &mut self.levels {
                for slot in 0..level.capacity {
                    if level.slots[slot as usize] != NONE {
                        level.mark(slot);
                    }
                }
            }
        }
    }

    fn row_index(level: usize, slot: u32) -> u32 {
        if level == STILL {
            slot
        } else {
            STILL_CAPACITY + slot
        }
    }

    /// Write `row` (an instance's look) as the row of `level`'s `slot`, with the triangle
    /// records of the structure the slot holds as built.
    fn write_row(&mut self, level: usize, slot: u32, mut row: RayRow) {
        let tris = self.levels[level].tris[slot as usize];
        if tris != NONE {
            row.flags |= ROW_HAS_TRIS;
            row.tris = tris;
        }
        let at = Self::row_index(level, slot);
        if self.rows[at as usize] == row {
            return;
        }
        self.rows[at as usize] = row;
        let range = &mut self.rows_changed[level];
        *range = Some(match *range {
            Some((lo, hi)) => (lo.min(at), hi.max(at + 1)),
            None => (at, at + 1),
        });
    }

    /// Give instance `i`, which has no slot and is not among the parked, a slot of the
    /// structure it belongs in at `frame`, or park it. True when it has a slot.
    fn enter(&mut self, i: u32, frame: u64) -> bool {
        let level = match &self.list[i as usize] {
            Some(inst) if frame >= inst.still_from => STILL,
            _ => MOVING,
        };
        let Some(slot) = self.levels[level].free.pop() else {
            self.park(i);
            return false;
        };
        self.take_slot(i, level, slot, frame);
        true
    }

    /// Instance `i` holds `slot` of `level` from now on (an empty one, or one whose member
    /// just left it for the parked), from `frame`.
    fn take_slot(&mut self, i: u32, level: usize, slot: u32, frame: u64) {
        let inst = self.list[i as usize].as_mut().expect("a listed instance");
        inst.at = At::In(level, slot);
        inst.entered = frame;
        let row = inst.row;
        let stand_in = inst.class == RayClass::ShadowOnly;
        let copy = inst.name.0 == STRESS;
        let l = &mut self.levels[level];
        if l.slots[slot as usize] == NONE {
            l.used += 1;
        }
        l.slots[slot as usize] = i;
        l.set_stand_in(slot, stand_in);
        l.set_copy(slot, copy);
        l.mark(slot);
        self.write_row(level, slot, row);
    }

    /// Instance `i`, which holds no slot, waits for room from now on.
    fn park(&mut self, i: u32) {
        if let Some(inst) = self.list[i as usize].as_mut() {
            inst.at = At::Parked(self.parked.len() as u32);
        }
        self.parked.push(i);
        self.parked_ever += 1;
    }

    /// Take the instance out of `slot` of `level`: the slot is empty at the next build and
    /// free after it.
    fn leave(&mut self, level: usize, slot: u32) {
        let l = &mut self.levels[level];
        l.slots[slot as usize] = NONE;
        l.used -= 1;
        l.set_stand_in(slot, false);
        l.set_copy(slot, false);
        l.freed.push(slot);
        l.mark(slot);
    }

    fn list_under(&mut self, geometry: u32, i: u32) -> u32 {
        if self.by_geometry.len() <= geometry as usize {
            self.by_geometry
                .resize_with(geometry as usize + 1, Vec::new);
        }
        let list = &mut self.by_geometry[geometry as usize];
        list.push(i);
        list.len() as u32 - 1
    }

    fn unlist_under(&mut self, geometry: u32, at: u32) {
        let list = &mut self.by_geometry[geometry as usize];
        list.swap_remove(at as usize);
        if let Some(&moved) = list.get(at as usize)
            && let Some(m) = self.list[moved as usize].as_mut()
        {
            m.of_geometry = at;
        }
    }

    /// Instance `name` of geometry `geometry` (its index in `store`) stands at `to_world`
    /// (see [`super::RaySceneOut::place`]). True when it has a slot.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn place(
        &mut self,
        store: &mut Store,
        name: Name,
        geometry: u32,
        to_world: &DAffine3,
        sphere: RaySphere,
        class: RayClass,
        still_from: u64,
        row: RayRow,
        frame: u64,
    ) -> bool {
        let copy = name.0 == STRESS;
        let Some(&i) = self.names.get(&name) else {
            let of_geometry = NONE;
            let inst = Instance {
                name,
                geometry,
                of_geometry,
                to_world: *to_world,
                sphere,
                class,
                still_from,
                row,
                // (`enter` below gives it its place.)
                at: At::Parked(NONE),
                entered: 0,
                seen: frame,
            };
            let i = match self.free.pop() {
                Some(i) => {
                    self.list[i as usize] = Some(inst);
                    i
                }
                None => {
                    self.list.push(Some(inst));
                    self.list.len() as u32 - 1
                }
            };
            self.names.insert(name, i);
            self.count_tile(None, tile_of(&row));
            if !copy {
                self.stats.classes[class.index()] += 1;
            }
            let at = self.list_under(geometry, i);
            self.list[i as usize]
                .as_mut()
                .expect("just listed")
                .of_geometry = at;
            store.count_user(geometry, 1, frame);
            let placed = self.enter(i, frame);
            self.stats.refused += !placed as u32;
            return placed;
        };
        let inst = self.list[i as usize].as_mut().expect("a named instance");
        let first_since_shift = inst.seen < self.shifted;
        inst.seen = frame;
        // (Its bounds may change without a move: a mesh of another size.)
        inst.sphere = sphere;
        let same_place = unmoved(&inst.to_world, to_world);
        if same_place && first_since_shift {
            // Where the shift left it, to within rounding: the feature's placement is the
            // one later placements are held against (else the rounding of shift after shift
            // would add up to a move). Its structure shows it where it stood, well within
            // what a ray can tell.
            inst.to_world = *to_world;
        }
        let same_shape = inst.geometry == geometry && inst.class == class;
        if same_place && same_shape && inst.row == row {
            // As it stands (a world shift, or a writer that sets what is there already).
            return matches!(inst.at, At::In(..));
        }
        let (was_geometry, was_at) = (inst.geometry, inst.of_geometry);
        let tiles = (tile_of(&inst.row), tile_of(&row));
        let was_class = inst.class;
        inst.to_world = *to_world;
        inst.class = class;
        inst.row = row;
        inst.geometry = geometry;
        if !same_place {
            inst.still_from = still_from;
        }
        let at = inst.at;
        self.count_tile(tiles.0, tiles.1);
        if was_class != class && !copy {
            self.stats.classes[was_class.index()] -= 1;
            self.stats.classes[class.index()] += 1;
        }
        if was_geometry != geometry {
            self.unlist_under(was_geometry, was_at);
            let at = self.list_under(geometry, i);
            self.list[i as usize]
                .as_mut()
                .expect("a named instance")
                .of_geometry = at;
            store.count_user(was_geometry, -1, frame);
            store.count_user(geometry, 1, frame);
        }
        match at {
            At::In(level, slot) => {
                if level == STILL && !same_place && still_from > frame {
                    // It moves: into the moving structure at once (the still one shows it
                    // where it stood until its next build).
                    self.leave(STILL, slot);
                    let placed = self.enter(i, frame);
                    self.stats.refused += !placed as u32;
                    return placed;
                }
                self.write_row(level, slot, row);
                if !same_place || !same_shape {
                    self.levels[level].set_stand_in(slot, class == RayClass::ShadowOnly);
                    self.levels[level].mark(slot);
                }
                true
            }
            // (Tried again with the others when the frame's placements end.)
            At::Parked(_) => false,
        }
    }

    /// Instance `name` is gone. Its geometry's index when it was known.
    pub(super) fn remove(&mut self, store: &mut Store, name: Name, frame: u64) -> Option<u32> {
        let i = self.names.remove(&name)?;
        let inst = self.list[i as usize].take().expect("a named instance");
        self.count_tile(tile_of(&inst.row), None);
        if name.0 != STRESS {
            self.stats.classes[inst.class.index()] -= 1;
        }
        self.free.push(i);
        self.unlist_under(inst.geometry, inst.of_geometry);
        store.count_user(inst.geometry, -1, frame);
        match inst.at {
            At::In(level, slot) => self.leave(level, slot),
            At::Parked(place) => {
                // The last of the parked takes its place in their list.
                self.parked.swap_remove(place as usize);
                if let Some(&moved) = self.parked.get(place as usize)
                    && let Some(m) = self.list[moved as usize].as_mut()
                {
                    m.at = At::Parked(place);
                }
            }
        }
        Some(inst.geometry)
    }

    /// The measuring fill (`RenderSettings::ray_scene_stress`): `copies` more instances,
    /// copy `k` of the `k % n`-th of the `n` others moved by `offset(k)` (meters), named
    /// `(STRESS, k)`. As many as the still structure has room for hold still, the rest move.
    /// Placed every frame while it is on, and both structures are built every frame (each a
    /// full build, which is what the measurement prices); at 0 the copies go. They take no
    /// part in the ranking at a full structure, the still set or the class counts.
    pub(super) fn stress(
        &mut self,
        store: &mut Store,
        copies: u32,
        frame: u64,
        offset: impl Fn(u32) -> DVec3,
    ) {
        self.every_frame = copies > 0;
        let sources: Vec<(u32, DAffine3, RaySphere, RayClass, RayRow)> = if copies > 0 {
            self.list
                .iter()
                .flatten()
                .filter(|inst| inst.name.0 != STRESS)
                .map(|inst| {
                    (
                        inst.geometry,
                        inst.to_world,
                        inst.sphere,
                        inst.class,
                        inst.row,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let placing = if sources.is_empty() { 0 } else { copies };
        let l = &self.levels[STILL];
        let still_room = l.capacity.saturating_sub(l.used - l.copies);
        for k in 0..placing {
            let (geometry, mut to_world, mut sphere, class, row) =
                sources[k as usize % sources.len()];
            let by = offset(k);
            to_world.translation += by;
            sphere.center += by;
            let still_from = if k < still_room { 0 } else { u64::MAX };
            self.place(
                store,
                (STRESS, RayKey(k as u64, 0)),
                geometry,
                &to_world,
                sphere,
                class,
                still_from,
                row,
                frame,
            );
        }
        for k in placing..self.copies {
            self.remove(store, (STRESS, RayKey(k as u64, 0)), frame);
        }
        self.copies = placing;
    }

    /// Whether instance `name` is known: in a structure, or waiting for room.
    pub(super) fn has(&self, name: Name) -> bool {
        self.names.contains_key(&name)
    }

    /// Remove the instances of feature `feature` that were not placed since frame `since`
    /// (it looked at everything it has since then, and they were not among it). How many
    /// went.
    pub(super) fn remove_unseen(
        &mut self,
        store: &mut Store,
        feature: u16,
        since: u64,
        frame: u64,
    ) -> u32 {
        let gone: Vec<Name> = self
            .list
            .iter()
            .flatten()
            .filter(|inst| inst.name.0 == feature && inst.seen < since)
            .map(|inst| inst.name)
            .collect();
        for &name in &gone {
            self.remove(store, name, frame);
        }
        gone.len() as u32
    }

    /// Whether instance `name` is in a structure as last built, in the slot it has now (see
    /// [`super::RaySceneOut::is_live`]).
    pub(super) fn is_live(&self, name: Name) -> bool {
        let Some(inst) = self
            .names
            .get(&name)
            .and_then(|&i| self.list[i as usize].as_ref())
        else {
            return false;
        };
        match inst.at {
            At::In(level, _) => inst.entered <= self.levels[level].built,
            At::Parked(_) => false,
        }
    }

    /// Geometry `geometry`'s structure became another (built again, compacted): the slots of
    /// its instances are set again at their structures' next builds.
    pub(super) fn geometry_changed(&mut self, geometry: u32) {
        let Some(list) = self.by_geometry.get(geometry as usize) else {
            return;
        };
        for &i in list {
            if let Some(Instance {
                at: At::In(level, slot),
                ..
            }) = self.list[i as usize]
            {
                self.levels[level].mark(slot);
            }
        }
    }

    /// End a frame's placements: move what holds still by now into the still structure when
    /// that is built this frame, give parked instances the room that came free (the nearest
    /// to `eye` first), let what waits for a full structure trade places with its farthest
    /// members, and set the changed slots of the structures this frame builds. Returns those
    /// structures (bits `1 << STILL`, `1 << MOVING`).
    pub(super) fn settle(&mut self, store: &mut Store, frame: u64, eye: DVec3) -> u8 {
        // What moved until now and holds still from this frame on asks for the still
        // structure's next build.
        let rested: Vec<u32> = (0..self.levels[MOVING].capacity)
            .filter(|&slot| {
                let i = self.levels[MOVING].slots[slot as usize];
                i != NONE
                    && self.list[i as usize]
                        .as_ref()
                        .is_some_and(|inst| frame >= inst.still_from)
            })
            .collect();
        let still_room = !self.levels[STILL].free.is_empty();
        // (Only when one of them can move in: with the still structure full they stay where
        // they are unless they trade places, and a build would change nothing.)
        if !rested.is_empty() && still_room {
            self.levels[STILL].wanted = true;
        }
        if still_room {
            // (What a ranking left holds only while the structure is full.)
            self.stats.still_farthest_m = -1.0;
            self.stats.parked_nearest_m = -1.0;
        }
        let still_due = self.levels[STILL].built == 0
            || self.rebuild_all
            || self.every_frame
            || frame >= self.levels[STILL].built + STILL_EVERY;
        // A full still structure, in a frame it may be built: what belongs in it and waits
        // (parked, or resting in the moving structure) takes the slots of its farthest
        // members where it is nearer. Ranked again only once the camera moved or more wait.
        if still_due && !still_room {
            let quiet = self.quiet_trade.is_some_and(|(at, parked, movers)| {
                at.distance(eye) < TRADE_AGAIN_M
                    && parked == self.parked_ever
                    && movers == rested.len()
            });
            if !quiet {
                let traded = self.trade(STILL, &rested, frame, eye);
                self.quiet_trade = (traded == 0).then_some((eye, self.parked_ever, rested.len()));
                if traded > 0 {
                    self.levels[STILL].wanted = true;
                }
            }
        }
        // (The measuring fill builds it every frame, whether or not a member changed.)
        let build_still = (self.levels[STILL].wanted && still_due) || self.every_frame;
        if build_still && !rested.is_empty() {
            // Nearest first, while the still structure has room (they stay where they are
            // otherwise).
            let mut movers: Vec<(f32, u32)> = rested
                .iter()
                .filter_map(|&slot| {
                    let i = self.levels[MOVING].slots[slot as usize];
                    let inst = self.list.get(i as usize)?.as_ref()?;
                    (inst.at == At::In(MOVING, slot)).then(|| (inst.sphere.distance(eye), slot))
                })
                .collect();
            movers.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            for (_, slot) in movers {
                if self.levels[STILL].free.is_empty() {
                    break;
                }
                let i = self.levels[MOVING].slots[slot as usize];
                self.leave(MOVING, slot);
                self.enter(i, frame);
            }
        }
        // Parked instances take what room there is, the nearest first (a slot emptied this
        // frame is free after the build, so next frame). Those whose structure has none wait
        // where they are.
        let room = [STILL, MOVING].map(|level| self.levels[level].free.len());
        if !self.parked.is_empty() && room.iter().any(|&r| r > 0) {
            let parked = std::mem::take(&mut self.parked);
            let mut entering = Vec::new();
            for i in parked {
                let Some(inst) = self.list[i as usize].as_ref() else {
                    continue;
                };
                let level = if frame >= inst.still_from {
                    STILL
                } else {
                    MOVING
                };
                if room[level] > 0 {
                    entering.push((inst.sphere.distance(eye), i));
                } else {
                    let place = self.parked.len() as u32;
                    self.parked.push(i);
                    if let Some(inst) = self.list[i as usize].as_mut() {
                        inst.at = At::Parked(place);
                    }
                }
            }
            entering.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            for (_, i) in entering {
                // (`enter` parks it again when there is no room left.)
                self.enter(i, frame);
            }
        }
        // A full moving structure: movers that wait take the slots of the farthest (those
        // that hold still by now first).
        if self.levels[MOVING].free.is_empty() && !self.parked.is_empty() {
            let rested: Vec<u32> = (0..self.levels[MOVING].capacity)
                .filter(|&slot| {
                    let i = self.levels[MOVING].slots[slot as usize];
                    i != NONE
                        && self.list[i as usize]
                            .as_ref()
                            .is_some_and(|inst| frame >= inst.still_from)
                })
                .collect();
            if self.trade(MOVING, &rested, frame, eye) > 0 {
                self.levels[MOVING].wanted = true;
            }
        }
        let build_moving = self.levels[MOVING].wanted
            || self.levels[MOVING].built == 0
            || self.rebuild_all
            || self.every_frame;
        let mut building = 0u8;
        if build_still {
            building |= 1 << STILL;
        }
        if build_moving {
            building |= 1 << MOVING;
        }
        self.rebuild_all = false;
        for level in [STILL, MOVING] {
            if building & (1 << level) != 0 {
                self.set_slots(store, level);
            }
        }
        self.building = building;
        self.stats.still_set = self.still_set(frame);
        building
    }

    /// In a frame that builds `level`, which is full: the nearest of what belongs in it and
    /// waits for room (parked; for the still structure also the movers in the moving one that
    /// hold still by now, in `rested` slots of it) take the slots of its farthest members, a
    /// pair at a time while the newcomer is [`TRADE_M`] nearer `eye`, and those members are
    /// parked. A slot's instance and its row change in the build that sets the slot, so no
    /// ray reads a row that is not its slot's (the rule a slot emptied by `leave` keeps by
    /// waiting for a build). For the moving structure, `rested` are its members that hold
    /// still by now: they give way first. The measuring fill's copies take no part. How many
    /// traded.
    fn trade(&mut self, level: usize, rested: &[u32], frame: u64, eye: DVec3) -> u32 {
        let list = &self.list;
        let belongs = |inst: &Instance| (frame >= inst.still_from) == (level == STILL);
        let mut waiting: Vec<(f32, u32)> = self
            .parked
            .iter()
            .filter_map(|&i| {
                let inst = list[i as usize].as_ref()?;
                (inst.name.0 != STRESS && belongs(inst)).then(|| (inst.sphere.distance(eye), i))
            })
            .collect();
        if level == STILL {
            for &slot in rested {
                let i = self.levels[MOVING].slots[slot as usize];
                if let Some(inst) = list.get(i as usize).and_then(Option::as_ref)
                    && inst.name.0 != STRESS
                {
                    waiting.push((inst.sphere.distance(eye), i));
                }
            }
        }
        let l = &self.levels[level];
        let mut gives_way = vec![
            false;
            if level == MOVING {
                l.capacity as usize
            } else {
                0
            }
        ];
        if level == MOVING {
            for &slot in rested {
                gives_way[slot as usize] = true;
            }
        }
        let mut members: Vec<(f32, u32)> = (0..l.capacity)
            .filter_map(|slot| {
                let i = l.slots[slot as usize];
                let inst = list.get(i as usize)?.as_ref()?;
                if inst.name.0 == STRESS {
                    return None;
                }
                // (In the moving structure, one that holds still by now gives way first.)
                let distance = if level == MOVING && gives_way[slot as usize] {
                    f32::INFINITY
                } else {
                    inst.sphere.distance(eye)
                };
                Some((distance, slot))
            })
            .collect();
        let k = waiting.len().min(members.len()).min(TRADES_MOST);
        if k == 0 {
            if level == STILL {
                self.stats.still_farthest_m = -1.0;
                self.stats.parked_nearest_m = -1.0;
            }
            return 0;
        }
        // The k nearest that wait, nearest first, and the k farthest members, farthest first.
        if k < waiting.len() {
            waiting.select_nth_unstable_by(k - 1, |a, b| a.0.total_cmp(&b.0));
        }
        let nearest_left = waiting[k..]
            .iter()
            .map(|w| w.0)
            .fold(f32::INFINITY, f32::min);
        waiting.truncate(k);
        waiting.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        if k < members.len() {
            members.select_nth_unstable_by(k - 1, |a, b| b.0.total_cmp(&a.0));
        }
        let farthest_left = members[k..].iter().map(|m| m.0).fold(-1.0, f32::max);
        members.truncate(k);
        members.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        let pairs: Vec<(u32, u32)> = waiting
            .iter()
            .zip(&members)
            .take_while(|(near, far)| near.0 + TRADE_M < far.0)
            .map(|(&(_, i), &(_, slot))| (i, slot))
            .collect();
        let traded = pairs.len();
        let mut left = Vec::with_capacity(traded);
        for &(i, slot) in &pairs {
            // A mover that holds still leaves its slot of the moving structure (empty at its
            // build this frame, which the leaving asks for).
            if let Some(At::In(from, from_slot)) = self.list[i as usize].as_ref().map(|x| x.at) {
                self.leave(from, from_slot);
            }
            let member = self.levels[level].slots[slot as usize];
            if let Some(m) = self.list[member as usize].as_mut() {
                m.at = At::Parked(NONE);
            }
            left.push(member);
            self.take_slot(i, level, slot, frame);
        }
        if level == STILL {
            // What the ranking leaves: the farthest member (of those it did not trade, the
            // newcomers, those past the k farthest) and the nearest that waits (of those it
            // did not trade, the members it parked, those past the k nearest).
            let far = members[traded..]
                .iter()
                .map(|m| m.0)
                .chain(waiting[..traded].iter().map(|w| w.0))
                .chain(std::iter::once(farthest_left))
                .fold(-1.0, f32::max);
            let near = waiting[traded..]
                .iter()
                .map(|w| w.0)
                .chain(members[..traded].iter().map(|m| m.0))
                .chain(std::iter::once(nearest_left))
                .fold(f32::INFINITY, f32::min);
            self.stats.still_farthest_m = far;
            self.stats.parked_nearest_m = if near.is_finite() { near } else { -1.0 };
        }
        if traded > 0 {
            // The parked list: those that took a slot leave it, the members join it.
            let list = &self.list;
            self.parked.retain(|&p| {
                list[p as usize]
                    .as_ref()
                    .is_some_and(|inst| matches!(inst.at, At::Parked(_)))
            });
            self.parked.extend(left);
            for (place, &p) in self.parked.iter().enumerate() {
                if let Some(inst) = self.list[p as usize].as_mut() {
                    inst.at = At::Parked(place as u32);
                }
            }
            self.parked_ever += traded as u64;
            self.stats.evicted += traded as u32;
        }
        traded as u32
    }

    /// What belongs in the still structure at `frame` (holds still by now), with a slot or
    /// waiting for one, the measuring fill's copies left out.
    fn still_set(&self, frame: u64) -> u32 {
        let l = &self.levels[STILL];
        let rested = |i: u32| {
            self.list
                .get(i as usize)
                .and_then(Option::as_ref)
                .is_some_and(|inst| inst.name.0 != STRESS && frame >= inst.still_from)
        };
        let moving = self.levels[MOVING]
            .slots
            .iter()
            .filter(|&&i| i != NONE && rested(i))
            .count();
        let parked = self.parked.iter().filter(|&&i| rested(i)).count();
        // (A still member that began to move this frame is in the moving structure already.)
        l.used - l.copies + (moving + parked) as u32
    }

    /// Set the changed slots of `level`'s structure for its build.
    fn set_slots(&mut self, store: &mut Store, level: usize) {
        let anchor = self.anchor;
        let l = &mut self.levels[level];
        let dirty = std::mem::take(&mut l.dirty);
        // The rows of the slots set, written once the level is let go: their records follow
        // the structure each slot is set with.
        let mut looks = Vec::with_capacity(dirty.len());
        for &slot in &dirty {
            l.is_dirty[slot as usize] = false;
            let s = slot as usize;
            let i = l.slots[s];
            let now = (i != NONE)
                .then(|| self.list[i as usize].as_ref())
                .flatten()
                .and_then(|inst| {
                    let g = store.get(inst.geometry)?;
                    let row = Self::row_index(level, slot);
                    Some((
                        inst.geometry,
                        g.records.map_or(NONE, |(header, _)| header + 1),
                        inst.row,
                        wgpu::TlasInstance::new(
                            &g.blas,
                            rows_3x4(&inst.to_world, anchor),
                            row,
                            inst.class.bit(),
                        ),
                    ))
                });
            let was = l.set_with[s];
            if was != NONE {
                l.released.push(was);
            }
            match now {
                Some((geometry, tris, look, instance)) => {
                    store.hold_slot(geometry, level);
                    l.set_with[s] = geometry;
                    l.tris[s] = tris;
                    l.tlas[s] = Some(instance);
                    looks.push((slot, look));
                }
                None => {
                    l.set_with[s] = NONE;
                    l.tris[s] = NONE;
                    if was != NONE || l.tlas[s].is_some() {
                        l.tlas[s] = None;
                    }
                }
            }
            self.stats.set += 1;
        }
        // (Keep the list's room for the next frame.)
        let mut dirty = dirty;
        dirty.clear();
        l.dirty = dirty;
        for (slot, look) in looks {
            self.write_row(level, slot, look);
        }
    }

    /// The structures [`settle`](Self::settle) chose were recorded for building in `frame`'s
    /// commands: their emptied slots are free, and what they held before is let go.
    pub(super) fn built(&mut self, store: &mut Store, frame: u64) {
        let building = std::mem::take(&mut self.building);
        for level in [STILL, MOVING] {
            if building & (1 << level) == 0 {
                continue;
            }
            let l = &mut self.levels[level];
            l.built = frame;
            l.wanted = false;
            l.live = l.used;
            l.free.append(&mut l.freed);
            store.release_slots(level, &mut l.released);
        }
        self.stats.built = building;
    }

    /// The rows that changed since the last call, as (first row, rows) to write.
    pub(super) fn take_changed_rows(&mut self) -> impl Iterator<Item = (u32, &[RayRow])> {
        let changed = std::mem::take(&mut self.rows_changed);
        let rows = &self.rows;
        changed
            .into_iter()
            .flatten()
            .map(move |(lo, hi)| (lo, &rows[lo as usize..hi as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_packs_its_class_and_look() {
        let look = RayLook {
            albedo: [0.1, 0.2, 0.3],
            roughness: 0.4,
            emissive: [1.0, 2.0, 3.0],
            metallic: 0.5,
            tile: Some(7),
            uv: [2.0, 2.0, 0.5, 0.5],
            cutoff: 0.5,
            coverage: 1.0,
            double_sided: true,
        };
        let row = RayRow::new(RayClass::Leaf, &look);
        assert_eq!(row.flags & 0x7f, RayClass::Leaf.bit() as u32);
        assert_ne!(row.flags & ROW_DOUBLE_SIDED, 0);
        assert_ne!(row.flags & ROW_HAS_TILE, 0);
        assert_eq!((row.flags >> 16) & 0xff, 128);
        assert_eq!(row.flags >> 24, 255);
        assert_eq!(row.tile, 7);
        assert_eq!(row.albedo, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(row.emissive, [1.0, 2.0, 3.0, 0.5]);
        assert_eq!(std::mem::size_of::<RayRow>(), 64);
        let plain = RayRow::new(RayClass::Solid, &RayLook::default());
        assert_eq!(plain.flags & 0xffff, RayClass::Solid.bit() as u32);
    }

    #[test]
    fn a_placement_is_rows_of_a_3x4_around_the_anchor() {
        let a = DAffine3::from_scale_rotation_translation(
            DVec3::new(2.0, 3.0, 4.0),
            DQuat::from_rotation_y(0.5),
            DVec3::new(1000.0, 20.0, -3000.0),
        );
        let anchor = DVec3::new(960.0, 0.0, -3008.0);
        let rows = rows_3x4(&a, anchor);
        assert_eq!(rows[3], 40.0, "row-major 3x4, translation last");
        assert_eq!(rows[7], 20.0);
        assert_eq!(rows[11], 8.0);
        assert_eq!(rows[4], a.matrix3.row(1).x as f32);
        // A point of the mesh lands where the transform puts it, less the anchor.
        let p = DVec3::new(0.5, -1.0, 2.0);
        let want = a.transform_point3(p) - anchor;
        for axis in 0..3 {
            let r = &rows[axis * 4..axis * 4 + 4];
            let got = r[0] as f64 * p.x + r[1] as f64 * p.y + r[2] as f64 * p.z + r[3] as f64;
            assert!((got - want[axis]).abs() < 1e-4, "{got} {}", want[axis]);
        }
    }

    #[test]
    fn a_shifted_placement_counts_as_the_same() {
        let a = DAffine3::from_translation(DVec3::new(5.0, 0.0, 7.0));
        let mut b = a;
        b.translation.x += 4e-4;
        assert!(unmoved(&a, &b));
        b.translation.x += 1e-3;
        assert!(!unmoved(&a, &b));
    }

    /// What a world shift does to a root's rotation: through `f64` and back to `f32`
    /// (`WorldShift::orientation`), where the scene turns the placement's matrix in `f64`.
    /// The two differ by a few `f32` steps times the scale: the same placement whatever
    /// the scale. A turn a ray could tell (a millimeter at ten meters) is a move.
    #[test]
    fn rounding_of_a_turned_and_scaled_placement_is_no_move() {
        let turn = DQuat::from_euler(EulerRot::YXZ, 0.3, -0.2, 0.1);
        for scale in [1.0, 4.0, 16.0, 64.0] {
            let mut worst = 0.0f64;
            for k in 0..200 {
                let k = k as f32;
                let q = Quat::from_euler(EulerRot::YXZ, 0.37 * k, 0.11 * k, -0.23 * k);
                let place = DVec3::new(100.0 + k as f64, 5.0, -40.0);
                let was = DAffine3::from_scale_rotation_translation(
                    DVec3::splat(scale),
                    q.as_dquat(),
                    place,
                );
                // The scene's own: the placement turned in f64.
                let scene = DAffine3::from_quat(turn) * was;
                // The game's: the root's rotation turned, rounded to f32, placed again.
                let rounded = (turn * q.as_dquat()).normalize().as_quat();
                let game = DAffine3::from_scale_rotation_translation(
                    DVec3::splat(scale),
                    rounded.as_dquat(),
                    turn * place,
                );
                assert!(unmoved(&scene, &game), "scale {scale}, rotation {k}");
                worst = worst.max(
                    (scene.matrix3.x_axis - game.matrix3.x_axis)
                        .abs()
                        .max_element(),
                );
            }
            // (The absolute difference grows with the scale: why the bound is a share.)
            assert!(worst < UNMOVED_TURN * scale, "{worst} at scale {scale}");
        }
        let a = DAffine3::from_scale(DVec3::splat(16.0));
        let b = DAffine3::from_rotation_y(1e-4) * a;
        assert!(!unmoved(&a, &b));
    }
}
