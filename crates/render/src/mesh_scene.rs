//! The mesh feature's persistent instance store, in the spirit of Unreal's GPUScene and Unity's
//! GPU Resident Drawer: what each drawn entity needs is kept from frame to frame in dense arrays
//! and rewritten only when its transform, renderer or mesh components change (by the ECS's
//! change ticks), so a frame's gather culls compact data instead of walking every component of
//! every entity (Aaltonen and Haar, "GPU-Driven Rendering Pipelines", SIGGRAPH 2015; Unreal's
//! cached mesh draw commands, GDC 2019).
//!
//! Each frame [`MeshScene::sync`] walks the entities that draw meshes, reading only their
//! components' change ticks, and rewrites the slots of those that changed; slots of entities
//! that no longer draw (despawned, a component removed) are dropped. A slot's hot part (`Hot`:
//! world bounds, mesh, material, flags) is what the cull reads; its cold part (`Cold`:
//! transforms, normal matrix) is read only for the instances drawn. World positions stay `f64`
//! and the view origin is subtracted when culling, so moving the origin dirties nothing.

use bevy_ecs::change_detection::Tick;
use bevy_ecs::lifecycle::RemovedComponentEntity;
use bevy_ecs::message::MessageCursor;
use engine_core::prelude::*;

use std::sync::Arc;

use crate::components::{
    DitherFade, InstanceRow, MaxDrawDistance, MeshInstances, MeshRenderer, NoMeshLod,
    NoVirtualGeometry, ResolvedMesh, ShadowOnly, ShadowTexelRange,
};
use crate::mesh_feature::max_scale;
use crate::virtual_geometry::Displacement;

/// `Hot::flags`: drawn (`MeshRenderer::visible`).
pub(crate) const VISIBLE: u8 = 1;
/// Casts shadows (`MeshRenderer::cast_shadows`).
pub(crate) const CASTS: u8 = 2;
/// A negative determinant: mirrored, front faces turn clockwise.
pub(crate) const MIRRORED: u8 = 4;
/// Has `NoVirtualGeometry`.
pub(crate) const INSTANCED_ONLY: u8 = 8;
/// Has a `Displacement`.
pub(crate) const DISPLACED: u8 = 16;
/// Has `NoMeshLod`: always drawn at full detail.
pub(crate) const FULL_DETAIL: u8 = 32;
/// Has `ShadowOnly`: drawn into shadow maps, never in a view.
pub(crate) const SHADOW_ONLY: u8 = 64;
/// Has `MeshInstances`: its mesh is drawn once per row (`MeshScene::rows`).
pub(crate) const ROWS: u8 = 128;
/// The flags that come from marker components (the entity's archetype).
const KINDS: u8 = INSTANCED_ONLY | DISPLACED | FULL_DETAIL | SHADOW_ONLY | ROWS;

/// What the cull reads of a slot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hot {
    /// World-space center of the mesh's bounds (follows `mesh_version`).
    pub center: DVec3,
    /// Bounding sphere radius, world space (follows `mesh_version`).
    pub radius: f32,
    /// Largest scale of the transform.
    pub scale: f32,
    /// `MaxDrawDistance` (`f32::MAX` without one).
    pub max_distance: f32,
    /// `DitherFade` (0 without one).
    pub dither: f32,
    /// `ShadowTexelRange` (any texel without one).
    pub cast_texels: ShadowTexelRange,
    /// Mesh and material handle indices, as the entity names them (not canonical).
    pub mesh: u32,
    pub material: u32,
    /// The mesh version `center` and `radius` were computed for (`u64::MAX`: not yet).
    pub mesh_version: u64,
    pub flags: u8,
    /// The camera's level of detail last frame (`LOD_UNSET`: none yet), kept with hysteresis.
    pub lod: u8,
    /// The level it cross-fades from, and the frame (low 32 bits) the fade started.
    pub lod_from: u8,
    pub lod_frame: u32,
}

/// `Hot::lod` before the slot has been seen by the camera: its first level is taken as it is.
pub(crate) const LOD_UNSET: u8 = u8::MAX - 1;

/// What only the drawn instances read of a slot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cold {
    pub entity: Entity,
    pub transform: GlobalTransform,
    /// The transform before the latest change (last frame's, when `moved` is this frame).
    pub previous: GlobalTransform,
    /// The frame the transform last changed.
    pub moved: u64,
    /// The frame the slot was last written: added, moved, or any of its components changed.
    /// Static shadow images hold slots unchanged for a while ([`Cold::still_from`]).
    pub changed: u64,
    /// The frame the slot was added.
    pub born: u64,
    /// The normal matrix's rows (`MeshInstance::n`).
    pub normal: [[f32; 4]; 3],
    /// The rows of the transform's linear part, `f32` (translations are added per view, from
    /// `f64`), now and before the latest change.
    rows: [[f32; 4]; 3],
    previous_rows: [[f32; 4]; 3],
}

impl Cold {
    /// The frame from which static shadow images may hold the slot: `STILL_FRAMES` after it
    /// last changed, or `SPAWN_STILL_FRAMES` for a young slot that never moved (a tree streamed
    /// in, its mesh resolved or its material set after it was added), so new casters join the
    /// shadows at once instead of popping in seconds later.
    pub fn still_from(&self) -> u64 {
        use crate::mesh_feature::{SPAWN_STILL_FRAMES, STILL_FRAMES};
        if self.moved == 0 && self.changed < self.born + STILL_FRAMES {
            self.changed + SPAWN_STILL_FRAMES
        } else {
            self.changed + STILL_FRAMES
        }
    }

    /// Last frame's transform.
    pub fn previous_at(&self, frame: u64) -> &GlobalTransform {
        if self.moved == frame {
            &self.previous
        } else {
            &self.transform
        }
    }

    /// The model matrix's rows relative to `origin` (`MeshInstance::m`).
    pub fn rows(&self, origin: DVec3) -> [[f32; 4]; 3] {
        with_translation(self.rows, self.transform.0.translation - origin)
    }

    /// Last frame's model matrix rows, relative to last frame's origin.
    pub fn previous_rows(&self, frame: u64, origin: DVec3) -> [[f32; 4]; 3] {
        if self.moved == frame {
            with_translation(self.previous_rows, self.previous.0.translation - origin)
        } else {
            self.rows(origin)
        }
    }
}

/// A [`MeshInstances`] row's own model matrix: the entity's transform `transform` applied to
/// the row's placement (`R_yaw · s` about its position), relative to `origin`; and its normal
/// matrix's columns (`MeshInstance::n`).
pub(crate) fn row_matrices(
    transform: &GlobalTransform,
    row: &InstanceRow,
    origin: DVec3,
) -> ([[f32; 4]; 3], [[f32; 4]; 3]) {
    let entity = transform.0.matrix3.as_mat3();
    let turn = Mat3::from_rotation_y(row.yaw);
    let scale = row.scale.abs().max(1e-6);
    let linear = entity * turn * scale;
    let t = (transform.0.transform_point3(row.position.as_dvec3()) - origin).as_vec3();
    let m = [
        linear.row(0).extend(t.x).to_array(),
        linear.row(1).extend(t.y).to_array(),
        linear.row(2).extend(t.z).to_array(),
    ];
    // The inverse transpose of `E · R · s` is `E⁻ᵀ · R / s` (R is a rotation).
    let normal = entity.inverse().transpose() * turn * (1.0 / scale);
    let n = [
        normal.x_axis.extend(0.0).to_array(),
        normal.y_axis.extend(0.0).to_array(),
        normal.z_axis.extend(0.0).to_array(),
    ];
    (m, n)
}

fn with_translation(mut rows: [[f32; 4]; 3], t: DVec3) -> [[f32; 4]; 3] {
    let t = t.as_vec3();
    rows[0][3] = t.x;
    rows[1][3] = t.y;
    rows[2][3] = t.z;
    rows
}

/// What a change to which [`MeshScene::sync`] rewrites a slot (removals come as messages).
type SceneChanged = Or<(
    Changed<GlobalTransform>,
    Changed<MeshRenderer>,
    Changed<ResolvedMesh>,
    Changed<MaxDrawDistance>,
    Changed<DitherFade>,
    Changed<ShadowTexelRange>,
    Changed<MeshInstances>,
    Added<NoVirtualGeometry>,
    Added<Displacement>,
    Added<NoMeshLod>,
    Added<ShadowOnly>,
)>;

type SceneItem<'w> = (
    Entity,
    Ref<'w, GlobalTransform>,
    Ref<'w, MeshRenderer>,
    Ref<'w, ResolvedMesh>,
    Option<Ref<'w, MaxDrawDistance>>,
    Option<Ref<'w, DitherFade>>,
    Option<Ref<'w, ShadowTexelRange>>,
    Option<Ref<'w, MeshInstances>>,
    bool,
    bool,
    bool,
    bool,
);

type SceneQuery = (
    Entity,
    Ref<'static, GlobalTransform>,
    Ref<'static, MeshRenderer>,
    Ref<'static, ResolvedMesh>,
    Option<Ref<'static, MaxDrawDistance>>,
    Option<Ref<'static, DitherFade>>,
    Option<Ref<'static, ShadowTexelRange>>,
    Option<Ref<'static, MeshInstances>>,
    Has<NoVirtualGeometry>,
    Has<Displacement>,
    Has<NoMeshLod>,
    Has<ShadowOnly>,
);

/// Entities' slots, by entity index (an array lookup instead of a hash).
const NO_SLOT: u32 = u32::MAX;

#[derive(Default)]
pub(crate) struct MeshScene {
    pub hot: Vec<Hot>,
    pub cold: Vec<Cold>,
    /// Each slot's `MeshInstances` rows (`None`: the entity draws its mesh once).
    pub rows: Vec<Option<Arc<[InstanceRow]>>>,
    /// The frame each slot's entity was last found.
    seen: Vec<u64>,
    /// Slot by entity index, with the entity (a reused index belongs to another entity).
    slot_of: Vec<(Entity, u32)>,
    /// Slots using each mesh and material handle index.
    mesh_users: Vec<u32>,
    material_users: Vec<u32>,
    query: Option<QueryState<SceneQuery>>,
    /// The same, only for the entities changed since the last sync.
    changed_query: Option<QueryState<SceneQuery, SceneChanged>>,
    /// Where the removal messages of the query's components were last read (in
    /// `read_removals`' order).
    removal_cursors: [MessageCursor<RemovedComponentEntity>; 11],
    /// The world's change tick at the last sync (changes after it are new).
    tick: Option<Tick>,
    /// Slots rewritten by the latest sync.
    pub dirty: u32,
    /// Entities the latest sync found changed whose slot already held exactly what they
    /// draw: a transform or renderer written without changing (left alone, so static shadow
    /// images keep them and nothing is uploaded again).
    pub unchanged: u32,
    /// Slots rewritten or dropped by the latest sync, as they were before (static shadow
    /// images that hold them must be drawn again).
    pub changes: Vec<SlotChange>,
}

/// A slot as it was before the latest sync rewrote or dropped it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SlotChange {
    /// World-space bounds.
    pub center: DVec3,
    pub radius: f32,
    /// `Cold::still_from` before.
    pub still_from: u64,
}

impl MeshScene {
    /// Make the query [`sync`](Self::sync) reads with (registering its components).
    pub fn ensure_query(&mut self, world: &mut World) {
        if self.query.is_none() {
            self.query = Some(world.query::<SceneQuery>());
        }
        if self.changed_query.is_none() {
            self.changed_query = Some(world.query_filtered::<SceneQuery, SceneChanged>());
        }
    }

    /// Bring the slots up to date with the world: rewrite those whose components changed since
    /// the last sync, add new entities, drop the ones gone.
    ///
    /// After the first sync only what changed is visited, as Bevy's retained render world
    /// extracts only changed meshes: the entities a change-filtered query finds (moved,
    /// renderer or mesh changed, added, a marker added) and those removal messages name (a
    /// component removed, despawned). A full sweep of every entity, dropping the slots of
    /// those no longer found, runs first, when removal messages were missed (the renderer
    /// paused for a while) and whenever the slot count disagrees with the query's.
    ///
    /// Reads the world shared (beside other features' prepares); make the query first with
    /// [`ensure_query`](Self::ensure_query).
    pub fn sync(&mut self, world: &World, frame: u64) {
        self.dirty = 0;
        self.unchanged = 0;
        self.changes.clear();
        if self.query.is_none() {
            // `None` while a component of the query was never registered.
            self.query = world.try_query::<SceneQuery>();
        }
        if self.changed_query.is_none() {
            self.changed_query = world.try_query_filtered::<SceneQuery, SceneChanged>();
        }
        let (removed, missed) = self.read_removals(world);
        let mut full = self.tick.is_none() || missed || self.changed_query.is_none();
        if !full
            && let (Some(mut query), Some(mut changed)) =
                (self.query.take(), self.changed_query.take())
        {
            // Removals first: a despawned entity's index may already hold a new entity.
            query.update_archetypes(world);
            for entity in removed {
                let Some(s) = self.slot_of_entity(entity) else {
                    continue;
                };
                match query.get_manual(world, entity) {
                    // Still drawn (a marker or an optional component went): rewritten.
                    Ok(item) => self.write(item, frame, true),
                    Err(_) => self.drop_slot(s),
                }
            }
            let since = self.tick.expect("not a full sync");
            for item in crate::features::iter_since(&mut changed, world, since) {
                self.write(item, frame, true);
            }
            // Anything missed shows as a count that disagrees: sweep everything.
            let matched: usize = query
                .matched_archetypes()
                .filter_map(|id| world.archetypes().get(id))
                .map(|a| a.len() as usize)
                .sum();
            full = matched != self.hot.len();
            if full {
                tracing::debug!(
                    "mesh scene: {} slots for {matched} entities, sweeping them all",
                    self.hot.len()
                );
            }
            self.query = Some(query);
            self.changed_query = Some(changed);
        }
        if full {
            if let Some(mut query) = self.query.take() {
                self.sweep(&mut query, world, self.tick, frame);
                self.query = Some(query);
            }
            // Entities no longer found: their slots go (the last slot moves into the gap).
            let mut s = 0;
            while s < self.hot.len() {
                if self.seen[s] == frame {
                    s += 1;
                } else {
                    self.drop_slot(s);
                }
            }
        }
        // Changes from here on (even later in this system) get a newer tick than the one the
        // sweep saw, so the next sync finds them.
        self.tick = Some(crate::features::take_change_tick(world));
    }

    /// The entities removal messages named since the last sync (a component of the query
    /// removed, or despawned), and whether some were missed (their messages already dropped).
    fn read_removals(&mut self, world: &World) -> (Vec<Entity>, bool) {
        let components = world.components();
        let ids = [
            components.valid_component_id::<GlobalTransform>(),
            components.valid_component_id::<MeshRenderer>(),
            components.valid_component_id::<ResolvedMesh>(),
            components.valid_component_id::<MaxDrawDistance>(),
            components.valid_component_id::<DitherFade>(),
            components.valid_component_id::<ShadowTexelRange>(),
            components.valid_component_id::<NoVirtualGeometry>(),
            components.valid_component_id::<Displacement>(),
            components.valid_component_id::<NoMeshLod>(),
            components.valid_component_id::<ShadowOnly>(),
            components.valid_component_id::<MeshInstances>(),
        ];
        let mut removed = Vec::new();
        let mut missed = false;
        for (id, cursor) in ids.into_iter().zip(&mut self.removal_cursors) {
            let Some(messages) = id.and_then(|id| world.removed_components().get(id)) else {
                continue;
            };
            missed |= cursor.missed_messages(messages) > 0;
            removed.extend(cursor.read(messages).cloned().map(Entity::from));
        }
        (removed, missed)
    }

    /// The slot of `entity` (not of another entity on its index).
    fn slot_of_entity(&self, entity: Entity) -> Option<usize> {
        match self.slot_of.get(entity.index_u32() as usize) {
            Some(&(e, s)) if e == entity && s != NO_SLOT => Some(s as usize),
            _ => None,
        }
    }

    /// Drop slot `s` (its entity draws no more): the last slot moves into the gap.
    fn drop_slot(&mut self, s: usize) {
        self.release(s);
        self.note_change(s);
        let entity = self.cold[s].entity;
        if let Some(e) = self.slot_of.get_mut(entity.index_u32() as usize)
            && e.0 == entity
        {
            *e = (Entity::PLACEHOLDER, NO_SLOT);
        }
        self.hot.swap_remove(s);
        self.cold.swap_remove(s);
        self.rows.swap_remove(s);
        self.seen.swap_remove(s);
        if s < self.hot.len() {
            // The moved slot may be another gone entity's whose index a new one holds now.
            let moved = self.cold[s].entity;
            if let Some(e) = self.slot_of.get_mut(moved.index_u32() as usize)
                && e.0 == moved
            {
                e.1 = s as u32;
            }
        }
    }

    /// Every entity of the query: unchanged ones are only marked seen (the slots of those not
    /// seen are dropped after). `since`: the tick of the last sync (`None`: everything is new).
    fn sweep(
        &mut self,
        query: &mut QueryState<SceneQuery>,
        world: &World,
        since: Option<Tick>,
        frame: u64,
    ) {
        let all = since.is_none();
        let last_run = since.unwrap_or_else(|| world.last_change_tick());
        for item in crate::features::iter_since(query, world, last_run) {
            self.write(item, frame, all);
        }
    }

    /// Bring one entity's slot up to date (a new slot for a new entity), when it changed since
    /// the last sync or `force`; marks it seen this frame either way.
    fn write(&mut self, item: SceneItem<'_>, frame: u64, force: bool) {
        let (
            entity,
            transform,
            renderer,
            resolved,
            max_distance,
            dither,
            texels,
            instances,
            instanced_only,
            displaced,
            full_detail,
            shadow_only,
        ) = item;
        let index = entity.index_u32() as usize;
        if index >= self.slot_of.len() {
            self.slot_of
                .resize(index + 1, (Entity::PLACEHOLDER, NO_SLOT));
        }
        let existing = match self.slot_of[index] {
            (e, s) if e == entity && s != NO_SLOT => Some(s as usize),
            _ => None,
        };
        // Archetype flags: no per-entity reads. A texel range keeps a caster instanced (the
        // cluster path draws its shadows without one), and so do rows.
        let kind = if instanced_only || texels.is_some() || instances.is_some() {
            INSTANCED_ONLY
        } else {
            0
        } | if displaced { DISPLACED } else { 0 }
            | if full_detail { FULL_DETAIL } else { 0 }
            | if shadow_only { SHADOW_ONLY } else { 0 }
            | if instances.is_some() { ROWS } else { 0 };
        let rows = instances.as_ref().map(|i| i.rows.clone());
        let max = max_distance.as_ref().map_or(f32::MAX, |m| m.0);
        let cast_texels = texels.as_deref().copied().unwrap_or_default();
        let fade = dither.as_ref().map_or(0.0, |d| {
            if d.0.is_finite() {
                d.0.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        });
        let flags = if renderer.visible { VISIBLE } else { 0 }
            | if renderer.cast_shadows { CASTS } else { 0 }
            | kind;
        let s = match existing {
            Some(s) => {
                self.seen[s] = frame;
                // Unchanged entities read only change ticks.
                let hot = &self.hot[s];
                let changed = force
                    || transform.is_changed()
                    || renderer.is_changed()
                    || resolved.is_changed()
                    || hot.flags & KINDS != kind
                    || match &max_distance {
                        Some(m) => m.is_changed(),
                        None => hot.max_distance != f32::MAX,
                    }
                    || match &dither {
                        Some(d) => d.is_changed(),
                        None => hot.dither != 0.0,
                    }
                    || match &texels {
                        Some(t) => t.is_changed(),
                        None => hot.cast_texels != ShadowTexelRange::default(),
                    }
                    || instances.as_ref().is_some_and(|i| i.is_changed());
                if !changed {
                    return;
                }
                let same_rows = match (&self.rows[s], &rows) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                // Moved without turning or scaling, the rest as it was (a world shift moves
                // every slot this way, and so do most movers): its bounds move with it, and
                // its mesh's users and levels stay.
                let hot = self.hot[s];
                if hot.mesh == resolved.mesh.index()
                    && hot.material == resolved.material.index()
                    && hot.max_distance == max
                    && hot.dither == fade
                    && hot.cast_texels == cast_texels
                    && hot.flags & !MIRRORED == flags
                    && same_rows
                    && self.cold[s].transform.0.matrix3 == transform.0.matrix3
                {
                    let delta = transform.0.translation - self.cold[s].transform.0.translation;
                    if delta == DVec3::ZERO {
                        // Written but not changed (systems that set a transform or a renderer
                        // every frame): nothing to do. Marking it changed would keep it out of
                        // the static shadow images and draw those again (`changes`), as Unreal
                        // updates a primitive's render state only when its transform moved.
                        self.unchanged += 1;
                        return;
                    }
                    self.note_change(s);
                    let cold = &mut self.cold[s];
                    cold.changed = frame;
                    cold.previous = cold.transform;
                    cold.previous_rows = cold.rows;
                    cold.moved = frame;
                    cold.transform = *transform;
                    self.hot[s].center += delta;
                    self.dirty += 1;
                    return;
                }
                self.release(s);
                self.note_change(s);
                let cold = &mut self.cold[s];
                cold.changed = frame;
                if cold.transform != *transform {
                    cold.previous = cold.transform;
                    cold.previous_rows = cold.rows;
                    cold.moved = frame;
                    set_transform(cold, *transform);
                }
                s
            }
            None => {
                let s = self.hot.len();
                self.slot_of[index] = (entity, s as u32);
                let mut cold = Cold {
                    entity,
                    transform: *transform,
                    previous: *transform,
                    moved: 0,
                    changed: frame,
                    born: frame,
                    normal: [[0.0; 4]; 3],
                    rows: [[0.0; 4]; 3],
                    previous_rows: [[0.0; 4]; 3],
                };
                set_transform(&mut cold, *transform);
                self.cold.push(cold);
                self.rows.push(None);
                self.seen.push(frame);
                self.hot.push(Hot {
                    center: DVec3::ZERO,
                    radius: 0.0,
                    scale: 1.0,
                    max_distance: f32::MAX,
                    dither: 0.0,
                    cast_texels: ShadowTexelRange::default(),
                    mesh: 0,
                    material: 0,
                    mesh_version: u64::MAX,
                    flags: 0,
                    lod: LOD_UNSET,
                    lod_from: LOD_UNSET,
                    lod_frame: 0,
                });
                s
            }
        };
        self.dirty += 1;
        self.rows[s] = rows;
        let mirrored = transform.0.matrix3.determinant() < 0.0;
        self.hot[s] = Hot {
            scale: max_scale(&transform),
            max_distance: max,
            dither: fade,
            cast_texels,
            mesh: resolved.mesh.index(),
            material: resolved.material.index(),
            // Bounds again, from the mesh, when next culled.
            mesh_version: u64::MAX,
            flags: flags | if mirrored { MIRRORED } else { 0 },
            // Another mesh has other levels: start over without a fade.
            lod: if self.hot[s].mesh == resolved.mesh.index() {
                self.hot[s].lod
            } else {
                LOD_UNSET
            },
            ..self.hot[s]
        };
        count(&mut self.mesh_users, resolved.mesh.index(), 1);
        count(&mut self.material_users, resolved.material.index(), 1);
    }

    /// Remember slot `s` as it is, before it is rewritten or dropped (if it was ever placed).
    fn note_change(&mut self, s: usize) {
        let hot = &self.hot[s];
        if hot.mesh_version != u64::MAX {
            self.changes.push(SlotChange {
                center: hot.center,
                radius: hot.radius,
                still_from: self.cold[s].still_from(),
            });
        }
    }

    /// A slot stops using its mesh and material (before it is rewritten or dropped; every
    /// slot counted its users when written).
    fn release(&mut self, s: usize) {
        let hot = self.hot[s];
        count(&mut self.mesh_users, hot.mesh, -1);
        count(&mut self.material_users, hot.material, -1);
    }

    /// Mesh handle indices some slot uses.
    pub fn meshes_used(&self) -> impl Iterator<Item = u32> + '_ {
        used(&self.mesh_users)
    }

    /// Material handle indices some slot uses.
    pub fn materials_used(&self) -> impl Iterator<Item = u32> + '_ {
        used(&self.material_users)
    }
}

fn used(users: &[u32]) -> impl Iterator<Item = u32> + '_ {
    users
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(i, _)| i as u32)
}

fn count(users: &mut Vec<u32>, index: u32, delta: i32) {
    let i = index as usize;
    if i >= users.len() {
        users.resize(i + 1, 0);
    }
    users[i] = users[i].saturating_add_signed(delta);
}

fn set_transform(cold: &mut Cold, transform: GlobalTransform) {
    let linear = transform.0.matrix3.as_mat3();
    let normal = linear.inverse().transpose();
    cold.transform = transform;
    cold.rows = [
        linear.row(0).extend(0.0).to_array(),
        linear.row(1).extend(0.0).to_array(),
        linear.row(2).extend(0.0).to_array(),
    ];
    cold.normal = [
        normal.x_axis.extend(0.0).to_array(),
        normal.y_axis.extend(0.0).to_array(),
        normal.z_axis.extend(0.0).to_array(),
    ];
}

#[cfg(test)]
mod tests {
    use engine_assets::Handle;

    use super::*;

    fn spawn(world: &mut World, x: f64, mesh: u32, material: u32) -> Entity {
        world
            .spawn((
                GlobalTransform::from(Transform::from_xyz(x, 0.0, 0.0)),
                MeshRenderer::default(),
                ResolvedMesh {
                    mesh: Handle::from_index(mesh),
                    material: Handle::from_index(material),
                },
            ))
            .id()
    }

    fn slot(scene: &MeshScene, entity: Entity) -> usize {
        scene
            .cold
            .iter()
            .position(|c| c.entity == entity)
            .expect("has a slot")
    }

    #[test]
    fn a_reused_index_keeps_its_own_slot() {
        // Three go, and a new entity takes the last one's index: removing the first two moves
        // the third's (stale) slot into a gap, which must not touch the new entity's slot.
        let mut world = World::new();
        let gone: Vec<Entity> = (0..3).map(|i| spawn(&mut world, i as f64, i, 0)).collect();
        let mut scene = MeshScene::default();
        scene.ensure_query(&mut world);
        scene.sync(&world, 1);
        world.despawn(gone[0]);
        world.despawn(gone[1]);
        // The next generation of the last one's index, as the allocator hands it out later.
        let reused = world.despawn_no_free(gone[2]).expect("despawned");
        let new = world
            .spawn_at(
                reused,
                (
                    GlobalTransform::from(Transform::from_xyz(9.0, 0.0, 0.0)),
                    MeshRenderer::default(),
                    ResolvedMesh {
                        mesh: Handle::from_index(7),
                        material: Handle::from_index(0),
                    },
                ),
            )
            .expect("spawned")
            .id();
        assert_eq!(new.index(), gone[2].index());
        assert_ne!(new, gone[2]);
        scene.sync(&world, 2);
        scene.sync(&world, 3);
        assert_eq!(scene.hot.len(), 1);
        assert_eq!(scene.cold[0].entity, new);
        assert_eq!(scene.slot_of[new.index_u32() as usize], (new, 0));
    }

    #[test]
    fn rewrites_only_what_changed() {
        let mut world = World::new();
        let a = spawn(&mut world, 1.0, 0, 0);
        let b = spawn(&mut world, 2.0, 1, 0);
        let mut scene = MeshScene::default();
        scene.ensure_query(&mut world);
        scene.sync(&world, 1);
        assert_eq!((scene.hot.len(), scene.dirty), (2, 2));
        scene.sync(&world, 2);
        assert_eq!(scene.dirty, 0, "nothing changed");
        // Written without changing (change ticks moved, values the same): left alone, so
        // static shadow images keep them.
        world.get_mut::<GlobalTransform>(a).unwrap().set_changed();
        world.get_mut::<MeshRenderer>(b).unwrap().set_changed();
        scene.sync(&world, 3);
        assert_eq!((scene.dirty, scene.unchanged), (0, 2));
        assert!(scene.changes.is_empty());
        assert_eq!(scene.cold[slot(&scene, a)].changed, 1);
        assert_eq!(scene.cold[slot(&scene, b)].changed, 1);

        // A move: rewritten, and last frame's transform kept for motion vectors.
        world.get_mut::<GlobalTransform>(a).unwrap().0.translation.x = 5.0;
        scene.sync(&world, 3);
        assert_eq!(scene.dirty, 1);
        let cold = &scene.cold[slot(&scene, a)];
        assert_eq!(cold.previous_at(3).0.translation.x, 1.0);
        assert_eq!(cold.previous_at(4).0.translation.x, 5.0);
        assert_eq!(cold.rows(DVec3::new(1.0, 0.0, 0.0))[0][3], 4.0);
        assert_eq!(cold.previous_rows(3, DVec3::ZERO)[0][3], 1.0);

        // Visibility, an added marker and a removed draw distance each dirty their slot.
        world.get_mut::<MeshRenderer>(b).unwrap().visible = false;
        scene.sync(&world, 4);
        assert_eq!(scene.dirty, 1);
        assert_eq!(scene.hot[slot(&scene, b)].flags & VISIBLE, 0);
        world.entity_mut(a).insert((
            NoVirtualGeometry,
            NoMeshLod,
            ShadowOnly,
            MaxDrawDistance(50.0),
        ));
        scene.sync(&world, 5);
        let hot = scene.hot[slot(&scene, a)];
        assert_eq!((scene.dirty, hot.max_distance), (1, 50.0));
        assert_ne!(hot.flags & INSTANCED_ONLY, 0);
        assert_ne!(hot.flags & FULL_DETAIL, 0);
        assert_ne!(hot.flags & SHADOW_ONLY, 0);
        world.entity_mut(a).remove::<MaxDrawDistance>();
        scene.sync(&world, 6);
        assert_eq!(scene.dirty, 1);
        assert_eq!(scene.hot[slot(&scene, a)].max_distance, f32::MAX);
        // A marker removed (a removal message, no change tick) rewrites it too.
        world.entity_mut(a).remove::<NoMeshLod>();
        scene.sync(&world, 7);
        assert_eq!(scene.dirty, 1);
        assert_eq!(scene.hot[slot(&scene, a)].flags & FULL_DETAIL, 0);
        // A shadow texel range: added, changed and removed, each rewrites the slot; with one
        // the slot stays on the instanced path.
        world
            .entity_mut(b)
            .insert(ShadowTexelRange::finer_than(0.1));
        scene.sync(&world, 9);
        let hot = scene.hot[slot(&scene, b)];
        assert_eq!((scene.dirty, hot.cast_texels.max), (1, 0.1));
        assert_ne!(hot.flags & INSTANCED_ONLY, 0);
        *world.get_mut::<ShadowTexelRange>(b).unwrap() = ShadowTexelRange::from_texels(0.1);
        scene.sync(&world, 10);
        let hot = scene.hot[slot(&scene, b)];
        assert_eq!((scene.dirty, hot.cast_texels.min), (1, 0.1));
        assert!(hot.cast_texels.casts_at(0.1) && !hot.cast_texels.casts_at(0.05));
        world.entity_mut(b).remove::<ShadowTexelRange>();
        scene.sync(&world, 11);
        let hot = scene.hot[slot(&scene, b)];
        assert_eq!(scene.dirty, 1);
        assert_eq!(hot.cast_texels, ShadowTexelRange::default());
        assert_eq!(hot.flags & INSTANCED_ONLY, 0);
        // And a renderer removed drops the slot, as despawning does.
        world.entity_mut(b).remove::<MeshRenderer>();
        scene.sync(&world, 12);
        assert_eq!(scene.hot.len(), 1);
        assert_eq!(scene.cold[0].entity, a);
    }

    /// A slot that only moved (no turn, no scale, nothing else changed: what a world shift does
    /// to every slot) keeps its bounds, moved by the same offset, and its level of detail; one
    /// that turned or changed mesh has them computed again.
    #[test]
    fn a_slot_that_only_moved_keeps_its_bounds_and_level() {
        let mut world = World::new();
        let a = spawn(&mut world, 1.0, 0, 0);
        let b = spawn(&mut world, 2.0, 1, 0);
        let mut scene = MeshScene::default();
        scene.ensure_query(&mut world);
        scene.sync(&world, 1);
        // As a cull leaves them: bounds for the mesh's version, a level.
        for hot in &mut scene.hot {
            hot.mesh_version = 3;
            hot.center = DVec3::new(hot.center.x + 10.0, 0.5, 0.0);
            hot.lod = 2;
        }
        let shift = DVec3::new(-3000.0, 0.0, 250.0);
        for e in [a, b] {
            world.get_mut::<GlobalTransform>(e).unwrap().0.translation += shift;
        }
        world.get_mut::<GlobalTransform>(b).unwrap().0.matrix3 = DMat3::from_rotation_y(0.5);
        scene.sync(&world, 2);
        assert_eq!(scene.dirty, 2);
        let (sa, sb) = (slot(&scene, a), slot(&scene, b));
        let moved = scene.hot[sa];
        assert_eq!(
            (moved.mesh_version, moved.lod, moved.center),
            (3, 2, DVec3::new(10.0, 0.5, 0.0) + shift),
            "moved only"
        );
        let cold = &scene.cold[sa];
        assert_eq!(cold.previous_at(2).0.translation.x, 1.0);
        assert_eq!(
            cold.transform.0.translation,
            DVec3::new(1.0, 0.0, 0.0) + shift
        );
        assert_eq!((cold.moved, cold.changed), (2, 2));
        assert_eq!(cold.rows(cold.transform.0.translation)[0][3], 0.0);
        // Turned: computed again at the next cull.
        assert_eq!(scene.hot[sb].mesh_version, u64::MAX);
        // A new mesh on a slot that didn't move goes the full way too.
        world.get_mut::<ResolvedMesh>(a).unwrap().mesh = Handle::from_index(5);
        scene.sync(&world, 3);
        let hot = scene.hot[slot(&scene, a)];
        assert_eq!(
            (hot.mesh, hot.mesh_version, hot.lod),
            (5, u64::MAX, LOD_UNSET)
        );
    }

    #[test]
    fn missed_removal_messages_fall_back_to_a_full_sweep() {
        // The renderer paused over two updates: the despawn's message is dropped before the
        // next sync reads it, and the sweep still finds the entity gone.
        let mut world = World::new();
        let a = spawn(&mut world, 1.0, 0, 0);
        let b = spawn(&mut world, 2.0, 1, 0);
        let mut scene = MeshScene::default();
        scene.ensure_query(&mut world);
        scene.sync(&world, 1);
        world.despawn(a);
        world.clear_trackers();
        world.clear_trackers();
        scene.sync(&world, 2);
        assert_eq!(scene.hot.len(), 1);
        assert_eq!(scene.cold[0].entity, b);
    }

    #[test]
    fn drops_despawned_entities_and_counts_handle_users() {
        let mut world = World::new();
        let entities: Vec<Entity> = (0..4)
            .map(|i| spawn(&mut world, i as f64, i % 2, 7))
            .collect();
        let mut scene = MeshScene::default();
        scene.ensure_query(&mut world);
        scene.sync(&world, 1);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(scene.materials_used().collect::<Vec<_>>(), [7]);

        // The first slot goes; the last moves into it and keeps its own data.
        world.despawn(entities[0]);
        world.despawn(entities[2]);
        scene.sync(&world, 2);
        assert_eq!(scene.hot.len(), 2);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [1]);
        for &e in &[entities[1], entities[3]] {
            let s = slot(&scene, e);
            let x = world.get::<GlobalTransform>(e).unwrap().0.translation.x;
            assert_eq!(scene.cold[s].transform.0.translation.x, x);
            assert_eq!(scene.slot_of[e.index_u32() as usize], (e, s as u32));
        }

        // A new entity, possibly on a freed index, gets a fresh slot.
        let c = spawn(&mut world, 9.0, 3, 7);
        scene.sync(&world, 3);
        assert_eq!((scene.hot.len(), scene.dirty), (3, 1));
        assert_eq!(scene.cold[slot(&scene, c)].transform.0.translation.x, 9.0);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [1, 3]);

        // A changed mesh moves its use.
        world.get_mut::<ResolvedMesh>(c).unwrap().mesh = Handle::from_index(1);
        scene.sync(&world, 4);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_region_of_rows_is_one_slot_rewritten_once() {
        let mut world = World::new();
        let rows: Vec<InstanceRow> = (0..1000)
            .map(|i| InstanceRow::at(Vec3::new(i as f32, 0.0, 0.0)))
            .collect();
        let region = spawn(&mut world, 0.0, 0, 0);
        world
            .entity_mut(region)
            .insert(MeshInstances::new(rows.clone()));
        let mut scene = MeshScene::default();
        scene.ensure_query(&mut world);
        scene.sync(&world, 1);
        assert_eq!((scene.hot.len(), scene.dirty), (1, 1));
        let s = slot(&scene, region);
        assert_ne!(scene.hot[s].flags & ROWS, 0);
        assert_ne!(
            scene.hot[s].flags & INSTANCED_ONLY,
            0,
            "rows stay off the cluster path"
        );
        assert_eq!(scene.rows[s].as_deref().map(<[_]>::len), Some(1000));

        // Touched with the same rows: left alone.
        world
            .get_mut::<MeshInstances>(region)
            .unwrap()
            .set_changed();
        scene.sync(&world, 2);
        assert_eq!(scene.dirty, 0);

        // New rows: the one slot is rewritten (bounds again when next culled).
        world.get_mut::<MeshInstances>(region).unwrap().rows = rows[..10].into();
        scene.sync(&world, 3);
        assert_eq!(scene.dirty, 1);
        assert_eq!(scene.rows[s].as_deref().map(<[_]>::len), Some(10));
        assert_eq!(scene.hot[s].mesh_version, u64::MAX);
        assert_eq!(scene.cold[s].changed, 3);

        // Rows gone: a plain entity again.
        world.entity_mut(region).remove::<MeshInstances>();
        scene.sync(&world, 4);
        let s = slot(&scene, region);
        assert!(scene.rows[s].is_none());
        assert_eq!(scene.hot[s].flags & ROWS, 0);

        // Slots dropped in the middle keep each slot's rows with it.
        let a = spawn(&mut world, 5.0, 1, 0);
        world
            .entity_mut(a)
            .insert(MeshInstances::new(rows[..3].to_vec()));
        scene.sync(&world, 5);
        world.despawn(region);
        scene.sync(&world, 6);
        assert_eq!(scene.hot.len(), 1);
        assert_eq!(
            scene.rows[slot(&scene, a)].as_deref().map(<[_]>::len),
            Some(3)
        );
    }

    #[test]
    fn a_row_is_placed_by_the_entity_then_its_own_turn_and_scale() {
        let transform = GlobalTransform::from(
            Transform::from_xyz(100.0, 2.0, 0.0).with_rotation(Quat::from_rotation_y(0.5)),
        );
        let row = InstanceRow {
            position: Vec3::new(1.0, 0.0, 2.0),
            yaw: 0.25,
            scale: 2.0,
            ..Default::default()
        };
        let origin = DVec3::new(100.0, 0.0, 0.0);
        let (m, n) = row_matrices(&transform, &row, origin);
        let point = Vec3::new(0.3, 1.0, -0.2);
        let placed = Vec3::new(
            Vec4::from(m[0]).dot(point.extend(1.0)),
            Vec4::from(m[1]).dot(point.extend(1.0)),
            Vec4::from(m[2]).dot(point.extend(1.0)),
        );
        let local = row.position + Quat::from_rotation_y(row.yaw) * point * row.scale;
        let expect = (transform.transform_point(local.as_dvec3()) - origin).as_vec3();
        assert!(placed.distance(expect) < 1e-4, "{placed} vs {expect}");
        // Normals turn with the row (both yaws) and keep their length up to the scale.
        let normal = Mat3::from_cols(
            Vec4::from(n[0]).truncate(),
            Vec4::from(n[1]).truncate(),
            Vec4::from(n[2]).truncate(),
        ) * Vec3::X;
        let turned = Quat::from_rotation_y(0.75) * Vec3::X;
        assert!(
            normal.normalize().distance(turned) < 1e-5,
            "{normal} vs {turned}"
        );
        assert_eq!(row.packed_tint(), 0.0);
        let tinted = InstanceRow {
            tint: [0.5, 1.0, 2.0],
            ..row
        };
        let v = tinted.packed_tint() as u32 - 1;
        assert_eq!((v & 255, v >> 8 & 255, v >> 16), (64, 128, 255));
    }
}
