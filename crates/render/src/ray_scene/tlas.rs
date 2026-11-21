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
use super::hook::{RayClass, RayLook};

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

/// [`RayRow::flags`]: lit from both sides.
const ROW_DOUBLE_SIDED: u32 = 1 << 8;
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
    /// Placements the scene had no room for, this frame.
    pub refused: u32,
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
    stats: InstanceStats,
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
            stats: InstanceStats::default(),
        }
    }

    pub(super) fn tlas(&self, level: usize) -> &wgpu::Tlas {
        &self.levels[level].tlas
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
        for i in self.list.iter_mut().flatten() {
            if translation_only {
                i.to_world.translation += shift.translation;
            } else {
                i.to_world = *shift * i.to_world;
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

    fn write_row(&mut self, level: usize, slot: u32, row: RayRow) {
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
        let inst = self.list[i as usize].as_mut().expect("a listed instance");
        let level = if frame >= inst.still_from {
            STILL
        } else {
            MOVING
        };
        let Some(slot) = self.levels[level].free.pop() else {
            inst.at = At::Parked(self.parked.len() as u32);
            self.parked.push(i);
            return false;
        };
        inst.at = At::In(level, slot);
        inst.entered = frame;
        let row = inst.row;
        let stand_in = inst.class == RayClass::ShadowOnly;
        let l = &mut self.levels[level];
        l.slots[slot as usize] = i;
        l.used += 1;
        l.set_stand_in(slot, stand_in);
        l.mark(slot);
        self.write_row(level, slot, row);
        true
    }

    /// Take the instance out of `slot` of `level`: the slot is empty at the next build and
    /// free after it.
    fn leave(&mut self, level: usize, slot: u32) {
        let l = &mut self.levels[level];
        l.slots[slot as usize] = NONE;
        l.used -= 1;
        l.set_stand_in(slot, false);
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
        class: RayClass,
        still_from: u64,
        row: RayRow,
        frame: u64,
    ) -> bool {
        let Some(&i) = self.names.get(&name) else {
            let of_geometry = NONE;
            let inst = Instance {
                name,
                geometry,
                of_geometry,
                to_world: *to_world,
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
        inst.to_world = *to_world;
        inst.class = class;
        inst.row = row;
        inst.geometry = geometry;
        if !same_place {
            inst.still_from = still_from;
        }
        let at = inst.at;
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
    /// that is built this frame, give parked instances the room that came free, and set the
    /// changed slots of the structures this frame builds. Returns those structures (bits
    /// `1 << STILL`, `1 << MOVING`).
    pub(super) fn settle(&mut self, store: &mut Store, frame: u64) -> u8 {
        // What moved until now and holds still from this frame on asks for the still
        // structure's next build.
        let promote = |list: &[Option<Instance>], i: u32| {
            list[i as usize]
                .as_ref()
                .is_some_and(|inst| frame >= inst.still_from)
        };
        let waiting = self.levels[MOVING]
            .slots
            .iter()
            .any(|&i| i != NONE && promote(&self.list, i));
        // (Only when one of them can move in: with the still structure full they stay where
        // they are, and a build would change nothing.)
        if waiting && !self.levels[STILL].free.is_empty() {
            self.levels[STILL].wanted = true;
        }
        let still_due = self.levels[STILL].built == 0
            || self.rebuild_all
            || frame >= self.levels[STILL].built + STILL_EVERY;
        let build_still = self.levels[STILL].wanted && still_due;
        if build_still && waiting {
            for slot in 0..self.levels[MOVING].capacity {
                let i = self.levels[MOVING].slots[slot as usize];
                if i == NONE || !promote(&self.list, i) {
                    continue;
                }
                // Only where the still structure has room: it stays where it is otherwise.
                if self.levels[STILL].free.is_empty() {
                    break;
                }
                self.leave(MOVING, slot);
                self.enter(i, frame);
            }
        }
        // Parked instances take what room there is (a slot emptied this frame is free after
        // the build, so next frame).
        if !self.parked.is_empty() {
            let parked = std::mem::take(&mut self.parked);
            for i in parked {
                // (`enter` parks it again when there is still no room.)
                self.enter(i, frame);
            }
        }
        let build_moving =
            self.levels[MOVING].wanted || self.levels[MOVING].built == 0 || self.rebuild_all;
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
        building
    }

    /// Set the changed slots of `level`'s structure for its build.
    fn set_slots(&mut self, store: &mut Store, level: usize) {
        let anchor = self.anchor;
        let l = &mut self.levels[level];
        let dirty = std::mem::take(&mut l.dirty);
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
                Some((geometry, instance)) => {
                    store.hold_slot(geometry, level);
                    l.set_with[s] = geometry;
                    l.tlas[s] = Some(instance);
                }
                None => {
                    l.set_with[s] = NONE;
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
