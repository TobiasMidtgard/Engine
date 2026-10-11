//! Scene and prefab documents: plain JSON that spawns entities through the [`TypeRegistry`].
//!
//! ```json
//! {
//!   "entities": [
//!     {
//!       "name": "Campfire",
//!       "components": {
//!         "Transform": { "translation": [4, 0, -2] },
//!         "MeshRenderer": { "mesh": { "shape": { "Cylinder": { "radius": 0.6, "height": 0.2 } } } }
//!       },
//!       "children": [
//!         { "name": "Flame", "components": { "Transform": { "translation": [0, 0.3, 0] } } }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! Because the format is just registered component names plus JSON values, AI agents can write
//! scenes directly, and `scene.save` round-trips whatever is in the world.

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::registry::TypeRegistry;

/// A list of root entities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneDoc {
    #[serde(default)]
    pub entities: Vec<EntityDoc>,
}

/// One entity: optional name, registered components by name, and children.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub components: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<EntityDoc>,
}

impl SceneDoc {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("scene JSON error: {e}"))
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}"))?;
        Self::from_json(&text)
    }

    pub fn save(&self, path: &str) -> Result<(), String> {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, self.to_json_pretty()).map_err(|e| format!("writing {path}: {e}"))
    }
}

/// Spawn one entity document (and its children). On error nothing is left behind.
pub fn spawn_entity_doc(
    world: &mut World,
    doc: &EntityDoc,
    parent: Option<Entity>,
) -> Result<Entity, String> {
    let entity = world.spawn_empty().id();
    match fill_entity(world, entity, doc) {
        Ok(()) => {
            if let Some(p) = parent {
                world.entity_mut(entity).insert(ChildOf(p));
            }
            for child in &doc.children {
                if let Err(e) = spawn_entity_doc(world, child, Some(entity)) {
                    world.despawn(entity);
                    return Err(e);
                }
            }
            Ok(entity)
        }
        Err(e) => {
            world.despawn(entity);
            Err(match &doc.name {
                Some(n) => format!("entity '{n}': {e}"),
                None => e,
            })
        }
    }
}

fn fill_entity(world: &mut World, entity: Entity, doc: &EntityDoc) -> Result<(), String> {
    if let Some(name) = &doc.name {
        world.entity_mut(entity).insert(Name::new(name.clone()));
    }
    for (component, value) in &doc.components {
        let entry = world
            .resource::<TypeRegistry>()
            .component_or_err(component)?
            .clone();
        (entry.insert)(world, entity, value.clone())?;
    }
    Ok(())
}

/// Spawn every root entity of a scene, optionally under `parent`.
pub fn spawn_scene(
    world: &mut World,
    scene: &SceneDoc,
    parent: Option<Entity>,
) -> Result<Vec<Entity>, String> {
    let mut spawned = Vec::new();
    for doc in &scene.entities {
        match spawn_entity_doc(world, doc, parent) {
            Ok(e) => spawned.push(e),
            Err(err) => {
                for e in spawned {
                    world.despawn(e);
                }
                return Err(err);
            }
        }
    }
    Ok(spawned)
}

/// Serialize an entity's registered components (and optionally its children).
pub fn snapshot_entity(world: &World, entity: Entity, recursive: bool) -> EntityDoc {
    let registry = world.resource::<TypeRegistry>();
    let mut components = Map::new();
    for entry in registry.components() {
        if entry.name == "Name" || !(entry.has)(world, entity) {
            continue;
        }
        if let Some(v) = (entry.get)(world, entity) {
            components.insert(entry.name.clone(), v);
        }
    }
    let children = if recursive {
        world
            .get::<Children>(entity)
            .map(|c| c.iter().map(|e| snapshot_entity(world, e, true)).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    EntityDoc {
        name: world.get::<Name>(entity).map(|n| n.as_str().to_string()),
        components,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn round_trip_with_children() {
        let mut app = App::new();
        let scene = SceneDoc::from_json(
            r#"{"entities":[{"name":"Root","components":{"Transform":{"translation":[1,0,0]}},
                "children":[{"name":"Kid","components":{"Transform":{"translation":[0,1,0]}}}]}]}"#,
        )
        .unwrap();
        let ids = spawn_scene(&mut app.world, &scene, None).unwrap();
        app.update();
        let snap = snapshot_entity(&app.world, ids[0], true);
        assert_eq!(snap.name.as_deref(), Some("Root"));
        assert_eq!(snap.children.len(), 1);
        assert_eq!(snap.children[0].name.as_deref(), Some("Kid"));
    }

    #[test]
    fn failed_spawn_leaves_nothing() {
        let mut app = App::new();
        let before = app.world.entities().count_spawned();
        let scene = SceneDoc::from_json(
            r#"{"entities":[{"name":"Ok"},{"name":"Bad","components":{"Nope":{}}}]}"#,
        )
        .unwrap();
        assert!(spawn_scene(&mut app.world, &scene, None).is_err());
        assert_eq!(app.world.entities().count_spawned(), before);
    }
}
