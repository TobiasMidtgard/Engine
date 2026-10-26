//! What belongs to the edited scene, and how parts of it are saved, spawned and removed.
//!
//! Scene files are the engine's own [`SceneDoc`] JSON (registered component names plus JSON
//! values), so anything the editor saves the AI bridge and games can read too. Entities in the
//! scene carry [`InScene`]; everything else in the world (the editor camera, entities modules
//! spawn at runtime) is left out of saves.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::core::scene::{EntityDoc, SceneDoc};
use engine::prelude::*;

/// Marks an entity that belongs to the edited scene (roots and their children).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct InScene;

/// Where a scene entity sits in the outliner. Entity ids get reused after a respawn, so the
/// order is kept here (and in [`SavedEntity`]) rather than read from ids.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneOrder(pub u64);

impl SceneOrder {
    /// After everything spawned so far.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Marks the editor's own entities (its camera); never shown, saved or picked.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct EditorOnly;

/// An entity saved with its old id and its children, so it can be put back and every
/// reference to the old id (undo history, selection) can be remapped to the new one.
#[derive(Debug, Clone)]
pub struct SavedEntity {
    pub old: Entity,
    pub doc: EntityDoc,
    pub order: Option<SceneOrder>,
    pub children: Vec<SavedEntity>,
}

/// Old entity id → the id it was respawned as.
pub type Remap = HashMap<Entity, Entity>;

/// Save `entity` and its scene children (registered components only).
pub fn save_entity(world: &World, entity: Entity) -> SavedEntity {
    let doc = engine::core::scene::snapshot_entity(world, entity, false);
    let children = world
        .get::<Children>(entity)
        .map(|c| {
            c.iter()
                .filter(|&child| world.get::<InScene>(child).is_some())
                .map(|child| save_entity(world, child))
                .collect()
        })
        .unwrap_or_default();
    SavedEntity {
        old: entity,
        doc,
        order: world.get::<SceneOrder>(entity).copied(),
        children,
    }
}

/// Spawn a saved entity (and its children) into the scene under `parent`, recording old → new
/// ids in `remap`. Components that no longer deserialize are skipped with a warning, so one
/// stale field never loses the whole entity.
pub fn restore_entity(
    world: &mut World,
    saved: &SavedEntity,
    parent: Option<Entity>,
    remap: &mut Remap,
) -> Entity {
    let entity = spawn_doc_lenient(world, &saved.doc, parent);
    if let Some(order) = saved.order {
        world.entity_mut(entity).insert(order);
    }
    remap.insert(saved.old, entity);
    for child in &saved.children {
        restore_entity(world, child, Some(entity), remap);
    }
    entity
}

/// Spawn one entity document (not its children) as a scene entity.
pub fn spawn_doc_lenient(world: &mut World, doc: &EntityDoc, parent: Option<Entity>) -> Entity {
    let entity = world
        .spawn((InScene, SceneOrder::next(), Transform::default()))
        .id();
    if let Some(name) = &doc.name {
        world.entity_mut(entity).insert(Name::new(name.clone()));
    }
    for (component, value) in &doc.components {
        let entry = world
            .resource::<TypeRegistry>()
            .component_or_err(component)
            .cloned();
        let result = entry.and_then(|entry| (entry.insert)(world, entity, value.clone()));
        if let Err(e) = result {
            engine::core::log::warn!("{}: skipped {component}: {e}", label(world, entity));
        }
    }
    if let Some(p) = parent {
        world.entity_mut(entity).insert(ChildOf(p));
    }
    entity
}

/// Spawn a whole document's entities as scene entities, children included.
pub fn spawn_doc_tree(world: &mut World, doc: &EntityDoc, parent: Option<Entity>) -> Entity {
    let entity = spawn_doc_lenient(world, doc, parent);
    for child in &doc.children {
        spawn_doc_tree(world, child, Some(entity));
    }
    entity
}

/// Root entities of the scene, in outliner order.
pub fn scene_roots(world: &mut World) -> Vec<Entity> {
    let mut q =
        world.query_filtered::<(Entity, Option<&ChildOf>, Option<&SceneOrder>), With<InScene>>();
    let mut roots: Vec<_> = q
        .iter(world)
        .filter(|(_, parent, _)| parent.is_none_or(|p| world.get::<InScene>(p.parent()).is_none()))
        .map(|(e, _, order)| (order.map_or(u64::MAX, |o| o.0), e.index(), e))
        .collect();
    roots.sort_unstable_by_key(|&(order, index, _)| (order, index));
    roots.into_iter().map(|(_, _, e)| e).collect()
}

/// Every scene entity as a document, children nested.
pub fn snapshot_scene(world: &mut World) -> SceneDoc {
    let roots = scene_roots(world);
    SceneDoc {
        entities: roots
            .into_iter()
            .map(|e| to_doc(&save_entity(world, e)))
            .collect(),
    }
}

fn to_doc(saved: &SavedEntity) -> EntityDoc {
    EntityDoc {
        children: saved.children.iter().map(to_doc).collect(),
        ..saved.doc.clone()
    }
}

/// Despawn every scene entity (and anything parented under them).
pub fn clear_scene(world: &mut World) {
    for root in scene_roots(world) {
        if world.get_entity(root).is_ok() {
            world.entity_mut(root).despawn();
        }
    }
}

/// Spawn a scene document; returns the new roots.
pub fn load_scene(world: &mut World, scene: &SceneDoc) -> Vec<Entity> {
    scene
        .entities
        .iter()
        .map(|doc| spawn_doc_tree(world, doc, None))
        .collect()
}

/// "Name" or "Entity 12v0" for logs and the outliner.
pub fn label(world: &World, entity: Entity) -> String {
    match world.get::<Name>(entity) {
        Some(name) => name.as_str().to_string(),
        None => format!("Entity {entity}"),
    }
}

/// The topmost ancestor of `entity` (itself when it has no parent).
pub fn root_of(world: &World, mut entity: Entity) -> Entity {
    while let Some(parent) = world.get::<ChildOf>(entity) {
        entity = parent.parent();
    }
    entity
}

/// Whether `ancestor` is `entity` or one of its parents.
pub fn is_ancestor(world: &World, ancestor: Entity, mut entity: Entity) -> bool {
    loop {
        if entity == ancestor {
            return true;
        }
        match world.get::<ChildOf>(entity) {
            Some(p) => entity = p.parent(),
            None => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.update();
        app
    }

    #[test]
    fn save_and_restore_keep_children_and_remap_ids() {
        let mut app = app();
        let world = &mut app.world;
        let root = world
            .spawn((
                InScene,
                Name::new("Root"),
                Transform::from_xyz(1.0, 2.0, 3.0),
            ))
            .id();
        let kid = world
            .spawn((
                InScene,
                Name::new("Kid"),
                Transform::from_xyz(0.0, 1.0, 0.0),
            ))
            .insert(ChildOf(root))
            .id();
        let saved = save_entity(world, root);
        world.entity_mut(root).despawn();
        assert!(world.get_entity(kid).is_err());

        let mut remap = Remap::new();
        let new_root = restore_entity(world, &saved, None, &mut remap);
        let new_kid = remap[&kid];
        assert_eq!(remap[&root], new_root);
        assert_eq!(world.get::<ChildOf>(new_kid).unwrap().parent(), new_root);
        assert_eq!(world.get::<Name>(new_kid).unwrap().as_str(), "Kid");
        assert_eq!(
            world.get::<Transform>(new_root).unwrap().translation,
            DVec3::new(1.0, 2.0, 3.0)
        );
    }

    #[test]
    fn snapshot_skips_entities_outside_the_scene() {
        let mut app = app();
        let world = &mut app.world;
        world.spawn((InScene, Name::new("A"), Transform::default()));
        world.spawn((Name::new("Runtime"), Transform::default()));
        world.spawn((EditorOnly, Name::new("Editor camera"), Transform::default()));
        let scene = snapshot_scene(world);
        assert_eq!(scene.entities.len(), 1);
        assert_eq!(scene.entities[0].name.as_deref(), Some("A"));

        clear_scene(world);
        assert!(snapshot_scene(world).entities.is_empty());
        let roots = load_scene(world, &scene);
        assert_eq!(roots.len(), 1);
        assert_eq!(label(world, roots[0]), "A");
    }

    #[test]
    fn unknown_components_are_skipped_not_fatal() {
        let mut app = app();
        let world = &mut app.world;
        let scene = SceneDoc::from_json(
            r#"{"entities":[{"name":"Odd","components":{"Nope":{},"Transform":{"translation":[0,5,0]}}}]}"#,
        )
        .unwrap();
        let roots = load_scene(world, &scene);
        assert_eq!(world.get::<Transform>(roots[0]).unwrap().translation.y, 5.0);
    }
}
