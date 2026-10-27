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
use engine_core::prelude::*;

use crate::components::{
    MaxDrawDistance, MeshRenderer, NoMeshLod, NoVirtualGeometry, ResolvedMesh,
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
    /// Mesh and material handle indices, as the entity names them (not canonical).
    pub mesh: u32,
    pub material: u32,
    /// The mesh version `center` and `radius` were computed for (`u64::MAX`: not yet).
    pub mesh_version: u64,
    pub flags: u8,
}

/// What only the drawn instances read of a slot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cold {
    pub entity: Entity,
    pub transform: GlobalTransform,
    /// The transform before the latest change (last frame's, when `moved` is this frame).
    pub previous: GlobalTransform,
    /// The frame the transform last changed.
    pub moved: u64,
    /// The normal matrix's rows (`MeshInstance::n`).
    pub normal: [[f32; 4]; 3],
    /// The rows of the transform's linear part, `f32` (translations are added per view, from
    /// `f64`), now and before the latest change.
    rows: [[f32; 4]; 3],
    previous_rows: [[f32; 4]; 3],
}

impl Cold {
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

fn with_translation(mut rows: [[f32; 4]; 3], t: DVec3) -> [[f32; 4]; 3] {
    let t = t.as_vec3();
    rows[0][3] = t.x;
    rows[1][3] = t.y;
    rows[2][3] = t.z;
    rows
}

type SceneQuery = (
    Entity,
    Ref<'static, GlobalTransform>,
    Ref<'static, MeshRenderer>,
    Ref<'static, ResolvedMesh>,
    Option<Ref<'static, MaxDrawDistance>>,
    Has<NoVirtualGeometry>,
    Has<Displacement>,
    Has<NoMeshLod>,
);

/// Entities' slots, by entity index (an array lookup instead of a hash).
const NO_SLOT: u32 = u32::MAX;

#[derive(Default)]
pub(crate) struct MeshScene {
    pub hot: Vec<Hot>,
    pub cold: Vec<Cold>,
    /// The frame each slot's entity was last found.
    seen: Vec<u64>,
    /// Slot by entity index, with the entity (a reused index belongs to another entity).
    slot_of: Vec<(Entity, u32)>,
    /// Slots using each mesh and material handle index.
    mesh_users: Vec<u32>,
    material_users: Vec<u32>,
    query: Option<QueryState<SceneQuery>>,
    /// The world's change tick at the last sync (changes after it are new).
    tick: Option<Tick>,
    /// Slots rewritten by the latest sync.
    pub dirty: u32,
}

impl MeshScene {
    /// Bring the slots up to date with the world: rewrite those whose components changed since
    /// the last sync, add new entities, drop the ones gone.
    pub fn sync(&mut self, world: &mut World, frame: u64) {
        let mut query = self
            .query
            .take()
            .unwrap_or_else(|| world.query::<SceneQuery>());
        self.dirty = 0;
        match self.tick {
            Some(tick) => {
                world.last_change_tick_scope(tick, |w| self.sweep(&mut query, w, frame, false))
            }
            None => self.sweep(&mut query, world, frame, true),
        }
        self.query = Some(query);
        // Changes from here on (even later in this system) get a newer tick than the one the
        // sweep saw, so the next sync finds them.
        self.tick = Some(world.increment_change_tick());
        // Entities no longer found: their slots go (the last slot moves into the gap).
        let mut s = 0;
        while s < self.hot.len() {
            if self.seen[s] == frame {
                s += 1;
                continue;
            }
            self.release(s);
            let entity = self.cold[s].entity;
            if let Some(e) = self.slot_of.get_mut(entity.index_u32() as usize)
                && e.0 == entity
            {
                *e = (Entity::PLACEHOLDER, NO_SLOT);
            }
            self.hot.swap_remove(s);
            self.cold.swap_remove(s);
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
    }

    fn sweep(&mut self, query: &mut QueryState<SceneQuery>, world: &World, frame: u64, all: bool) {
        for (
            entity,
            transform,
            renderer,
            resolved,
            max_distance,
            instanced_only,
            displaced,
            full_detail,
        ) in query.iter(world)
        {
            let index = entity.index_u32() as usize;
            if index >= self.slot_of.len() {
                self.slot_of
                    .resize(index + 1, (Entity::PLACEHOLDER, NO_SLOT));
            }
            let existing = match self.slot_of[index] {
                (e, s) if e == entity && s != NO_SLOT => Some(s as usize),
                _ => None,
            };
            // Archetype flags: no per-entity reads.
            let kind = if instanced_only { INSTANCED_ONLY } else { 0 }
                | if displaced { DISPLACED } else { 0 }
                | if full_detail { FULL_DETAIL } else { 0 };
            let s = match existing {
                Some(s) => {
                    self.seen[s] = frame;
                    // Unchanged entities read only change ticks.
                    let hot = &self.hot[s];
                    let changed = all
                        || transform.is_changed()
                        || renderer.is_changed()
                        || resolved.is_changed()
                        || hot.flags & (INSTANCED_ONLY | DISPLACED | FULL_DETAIL) != kind
                        || match &max_distance {
                            Some(m) => m.is_changed(),
                            None => hot.max_distance != f32::MAX,
                        };
                    if !changed {
                        continue;
                    }
                    self.release(s);
                    let cold = &mut self.cold[s];
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
                        normal: [[0.0; 4]; 3],
                        rows: [[0.0; 4]; 3],
                        previous_rows: [[0.0; 4]; 3],
                    };
                    set_transform(&mut cold, *transform);
                    self.cold.push(cold);
                    self.seen.push(frame);
                    self.hot.push(Hot {
                        center: DVec3::ZERO,
                        radius: 0.0,
                        scale: 1.0,
                        max_distance: f32::MAX,
                        mesh: 0,
                        material: 0,
                        mesh_version: u64::MAX,
                        flags: 0,
                    });
                    s
                }
            };
            self.dirty += 1;
            let max = max_distance.as_ref().map_or(f32::MAX, |m| m.0);
            let flags = if renderer.visible { VISIBLE } else { 0 }
                | if renderer.cast_shadows { CASTS } else { 0 }
                | kind;
            let mirrored = transform.0.matrix3.determinant() < 0.0;
            self.hot[s] = Hot {
                scale: max_scale(&transform),
                max_distance: max,
                mesh: resolved.mesh.index(),
                material: resolved.material.index(),
                // Bounds again, from the mesh, when next culled.
                mesh_version: u64::MAX,
                flags: flags | if mirrored { MIRRORED } else { 0 },
                ..self.hot[s]
            };
            count(&mut self.mesh_users, resolved.mesh.index(), 1);
            count(&mut self.material_users, resolved.material.index(), 1);
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
        scene.sync(&mut world, 1);
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
        scene.sync(&mut world, 2);
        scene.sync(&mut world, 3);
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
        scene.sync(&mut world, 1);
        assert_eq!((scene.hot.len(), scene.dirty), (2, 2));
        scene.sync(&mut world, 2);
        assert_eq!(scene.dirty, 0, "nothing changed");

        // A move: rewritten, and last frame's transform kept for motion vectors.
        world.get_mut::<GlobalTransform>(a).unwrap().0.translation.x = 5.0;
        scene.sync(&mut world, 3);
        assert_eq!(scene.dirty, 1);
        let cold = &scene.cold[slot(&scene, a)];
        assert_eq!(cold.previous_at(3).0.translation.x, 1.0);
        assert_eq!(cold.previous_at(4).0.translation.x, 5.0);
        assert_eq!(cold.rows(DVec3::new(1.0, 0.0, 0.0))[0][3], 4.0);
        assert_eq!(cold.previous_rows(3, DVec3::ZERO)[0][3], 1.0);

        // Visibility, an added marker and a removed draw distance each dirty their slot.
        world.get_mut::<MeshRenderer>(b).unwrap().visible = false;
        scene.sync(&mut world, 4);
        assert_eq!(scene.dirty, 1);
        assert_eq!(scene.hot[slot(&scene, b)].flags & VISIBLE, 0);
        world
            .entity_mut(a)
            .insert((NoVirtualGeometry, NoMeshLod, MaxDrawDistance(50.0)));
        scene.sync(&mut world, 5);
        let hot = scene.hot[slot(&scene, a)];
        assert_eq!((scene.dirty, hot.max_distance), (1, 50.0));
        assert_ne!(hot.flags & INSTANCED_ONLY, 0);
        assert_ne!(hot.flags & FULL_DETAIL, 0);
        world.entity_mut(a).remove::<MaxDrawDistance>();
        scene.sync(&mut world, 6);
        assert_eq!(scene.dirty, 1);
        assert_eq!(scene.hot[slot(&scene, a)].max_distance, f32::MAX);
    }

    #[test]
    fn drops_despawned_entities_and_counts_handle_users() {
        let mut world = World::new();
        let entities: Vec<Entity> = (0..4)
            .map(|i| spawn(&mut world, i as f64, i % 2, 7))
            .collect();
        let mut scene = MeshScene::default();
        scene.sync(&mut world, 1);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(scene.materials_used().collect::<Vec<_>>(), [7]);

        // The first slot goes; the last moves into it and keeps its own data.
        world.despawn(entities[0]);
        world.despawn(entities[2]);
        scene.sync(&mut world, 2);
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
        scene.sync(&mut world, 3);
        assert_eq!((scene.hot.len(), scene.dirty), (3, 1));
        assert_eq!(scene.cold[slot(&scene, c)].transform.0.translation.x, 9.0);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [1, 3]);

        // A changed mesh moves its use.
        world.get_mut::<ResolvedMesh>(c).unwrap().mesh = Handle::from_index(1);
        scene.sync(&mut world, 4);
        assert_eq!(scene.meshes_used().collect::<Vec<_>>(), [1]);
    }
}
