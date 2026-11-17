//! Inline materials nothing draws any more are freed.
//!
//! [`crate::resolve_mesh_renderers`] shares inline materials by value: each distinct value is a
//! material keyed `mat:` and its formatted value. Nothing counted who drew them, so an inline
//! material changed every frame (a color tweened through `entity.set`, a glow) left one
//! material behind per value it went through, for good: an hour of one tween is about 200 000
//! materials and their keys of a few hundred bytes each. Unreal frees what no live object
//! references any more; streamed cells here already free the `mat:` materials their entities
//! drew once they are gone (`engine::cell_assets`), as the renderer makes them again from the
//! key whenever something asks.
//!
//! Here [`ResolvedMesh`]'s hooks count the entities drawing each inline material: one inserted
//! for a [`MaterialRef::Inline`] renderer takes its material, one replaced or removed (the
//! entity despawned) gives it back. [`release_inline_materials`] frees the materials no entity
//! has drawn for two of its runs, in batches (`Assets::remove_many` walks every key once).

use std::collections::HashMap;

use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::world::DeferredWorld;
use engine_assets::{Assets, Handle};
use engine_core::prelude::*;

use crate::components::{MaterialRef, MeshRenderer, ResolvedMesh};
use crate::material::StandardMaterial;

/// Unused materials freed together once there are this many.
const RELEASE_BATCH: usize = 1024;
/// Or every this many runs (frames), however few.
const RELEASE_EVERY: u64 = 600;

/// Who draws each inline material (see the module docs).
#[derive(Resource, Default, Debug)]
pub struct InlineMaterialUsers {
    /// The inline material each entity draws.
    of: HashMap<Entity, Handle<StandardMaterial>>,
    /// How many entities draw each.
    users: HashMap<Handle<StandardMaterial>, u32>,
    /// Drawn by none since this run of `release_inline_materials`.
    unused: HashMap<Handle<StandardMaterial>, u64>,
    runs: u64,
    /// Materials freed so far.
    released: u64,
}

impl InlineMaterialUsers {
    fn take(&mut self, entity: Entity, material: Handle<StandardMaterial>) {
        self.of.insert(entity, material);
        *self.users.entry(material).or_default() += 1;
        self.unused.remove(&material);
    }

    fn give_back(&mut self, entity: Entity) {
        let Some(material) = self.of.remove(&entity) else {
            return;
        };
        if let Some(n) = self.users.get_mut(&material) {
            *n -= 1;
            if *n == 0 {
                self.users.remove(&material);
                self.unused.insert(material, self.runs);
            }
        }
    }

    /// Inline materials some entity draws.
    pub fn drawn(&self) -> usize {
        self.users.len()
    }

    /// Inline materials freed so far.
    pub fn released(&self) -> u64 {
        self.released
    }
}

/// `ResolvedMesh`'s `on_insert` hook: an inline material's renderer takes its material.
pub(crate) fn resolved_inserted(mut world: DeferredWorld, ctx: HookContext) {
    let e = ctx.entity;
    let Some(material) = world.get::<ResolvedMesh>(e).map(|r| r.material) else {
        return;
    };
    let inline = world
        .get::<MeshRenderer>(e)
        .is_some_and(|r| matches!(r.material, MaterialRef::Inline(_)));
    if inline && let Some(mut users) = world.get_resource_mut::<InlineMaterialUsers>() {
        users.take(e, material);
    }
}

/// `ResolvedMesh`'s `on_discard` hook (replaced or removed): its material is given back.
pub(crate) fn resolved_discarded(mut world: DeferredWorld, ctx: HookContext) {
    if let Some(mut users) = world.get_resource_mut::<InlineMaterialUsers>() {
        users.give_back(ctx.entity);
    }
}

/// Frees the `mat:` materials no entity has drawn for two runs (so a renderer resolved to one
/// again this frame, its insert still a command, keeps it), once a batch is due.
/// Publishes `render.inline_materials` (drawn now) and `render.inline_materials_freed` (so far).
pub fn release_inline_materials(
    mut users: ResMut<InlineMaterialUsers>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    metrics: Option<Res<Metrics>>,
) {
    if let Some(m) = &metrics {
        m.set("render.inline_materials", users.drawn() as f64);
        m.set("render.inline_materials_freed", users.released() as f64);
    }
    users.runs += 1;
    let now = users.runs;
    if users.unused.len() < RELEASE_BATCH && !now.is_multiple_of(RELEASE_EVERY) {
        return;
    }
    let Some(mut materials) = materials else {
        return;
    };
    let due: Vec<_> = users
        .unused
        .iter()
        .filter(|&(_, &since)| since + 2 <= now)
        .map(|(&h, _)| h)
        .collect();
    if due.is_empty() {
        return;
    }
    for h in &due {
        users.unused.remove(h);
    }
    // Only materials the renderer makes again from their key.
    let remade: std::collections::HashSet<u32> = materials
        .keys()
        .filter(|(k, _)| k.starts_with("mat:"))
        .map(|(_, h)| h.index())
        .collect();
    let freed = materials.remove_many(due.into_iter().filter(|h| remade.contains(&h.index())));
    users.released += freed.len() as u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;
    use crate::{Shape, resolve_mesh_renderers};
    use bevy_ecs::schedule::Schedule;

    fn world() -> (World, Schedule) {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.init_resource::<InlineMaterialUsers>();
        let mut schedule = Schedule::default();
        schedule.add_systems((resolve_mesh_renderers, release_inline_materials).chain());
        (world, schedule)
    }

    fn renderer(red: f32) -> MeshRenderer {
        MeshRenderer::new(
            Shape::Cuboid {
                size: [1.0, 1.0, 1.0],
            },
            StandardMaterial::color(red, 0.5, 0.5),
        )
    }

    fn materials(world: &World) -> usize {
        world.resource::<Assets<StandardMaterial>>().len()
    }

    /// A color tweened every frame for a minute keeps the materials to a batch's worth, and the
    /// entity always draws one that is there.
    #[test]
    fn a_tweened_inline_material_leaves_no_trail() {
        let (mut world, mut schedule) = world();
        let still = world.spawn(renderer(0.9)).id();
        let tweened = world.spawn(renderer(0.0)).id();
        for frame in 0..3600 {
            world.get_mut::<MeshRenderer>(tweened).unwrap().material =
                MaterialRef::Inline(StandardMaterial::color(frame as f32 / 3600.0, 0.5, 0.5));
            schedule.run(&mut world);
            let assets = world.resource::<Assets<StandardMaterial>>();
            for e in [still, tweened] {
                let r = world.get::<ResolvedMesh>(e).expect("resolved");
                assert!(
                    assets.get(r.material).is_some(),
                    "frame {frame}: material gone"
                );
            }
        }
        let users = world.resource::<InlineMaterialUsers>();
        eprintln!(
            "{} materials, {} drawn, {} freed",
            materials(&world),
            users.drawn(),
            users.released()
        );
        assert!(
            materials(&world) <= RELEASE_BATCH + 2,
            "{}",
            materials(&world)
        );
        assert!(users.released() >= 3600 - RELEASE_BATCH as u64 - 2);
        assert_eq!(users.drawn(), 2);
    }

    /// A material two entities share stays while one draws it, and goes once neither does;
    /// one made with `Assets::add` and drawn through its handle is never freed.
    #[test]
    fn a_shared_material_goes_with_its_last_user() {
        let (mut world, mut schedule) = world();
        let a = world.spawn(renderer(0.3)).id();
        let b = world.spawn(renderer(0.3)).id();
        let own = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::color(0.1, 0.2, 0.3));
        let c = world
            .spawn(MeshRenderer {
                material: MaterialRef::Handle(own),
                ..renderer(0.0)
            })
            .id();
        schedule.run(&mut world);
        let shared = world.get::<ResolvedMesh>(a).unwrap().material;
        assert_eq!(world.get::<ResolvedMesh>(b).unwrap().material, shared);

        world.despawn(a);
        for _ in 0..RELEASE_EVERY {
            schedule.run(&mut world);
        }
        let assets = world.resource::<Assets<StandardMaterial>>();
        assert!(assets.get(shared).is_some(), "freed while b draws it");

        world.despawn(b);
        world.despawn(c);
        for _ in 0..RELEASE_EVERY {
            schedule.run(&mut world);
        }
        let assets = world.resource::<Assets<StandardMaterial>>();
        assert!(assets.get(shared).is_none(), "kept with nothing drawing it");
        assert!(assets.get(own).is_some(), "a material of its own was freed");
    }
}
