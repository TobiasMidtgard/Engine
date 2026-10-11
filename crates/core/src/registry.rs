//! Name-based reflection over serde/JSON, plus the [`ApiRegistry`] of named engine functions.
//!
//! This one registry is what scenes, the AI bridge, scripting and the debug inspector all use,
//! so registering a component once makes it available everywhere:
//!
//! ```
//! # use engine_core::prelude::*;
//! #[derive(Component, Serialize, Deserialize, Default)]
//! #[serde(default)]
//! struct Health { current: f32, max: f32 }
//!
//! let mut app = App::new();
//! app.register_component::<Health>("Hit points. The entity dies at 0.");
//! ```
//!
//! Components are addressed by their short type name (`"Health"`). Values are JSON.
//! Patching merges JSON objects field by field, so `{"current": 5}` changes one field.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::component::Mutable;
use bevy_ecs::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::app::App;
use crate::scene::{self, EntityDoc, SceneDoc};

/// Reflection data for one component type.
#[derive(Clone)]
pub struct ComponentEntry {
    pub name: String,
    pub type_name: &'static str,
    pub doc: String,
    pub get: fn(&World, Entity) -> Option<Value>,
    pub insert: fn(&mut World, Entity, Value) -> Result<(), String>,
    pub patch: fn(&mut World, Entity, Value) -> Result<(), String>,
    pub remove: fn(&mut World, Entity) -> bool,
    pub has: fn(&World, Entity) -> bool,
    pub component_id: fn(&World) -> Option<bevy_ecs::component::ComponentId>,
}

/// Reflection data for one resource type.
#[derive(Clone)]
pub struct ResourceEntry {
    pub name: String,
    pub type_name: &'static str,
    pub doc: String,
    pub get: fn(&World) -> Option<Value>,
    pub patch: fn(&mut World, Value) -> Result<(), String>,
}

/// All reflected components and resources, by short name.
#[derive(Resource, Default, Clone)]
pub struct TypeRegistry {
    components: BTreeMap<String, ComponentEntry>,
    by_type_name: BTreeMap<&'static str, String>,
    resources: BTreeMap<String, ResourceEntry>,
}

/// `"my_game::combat::Health"` → `"Health"`; generics keep their arguments' short names.
pub fn short_type_name(full: &str) -> String {
    let mut out = String::new();
    let mut segment = String::new();
    for ch in full.chars() {
        match ch {
            ':' => segment.clear(),
            '<' | '>' | ',' | ' ' | '(' | ')' | '[' | ']' | ';' | '&' => {
                out.push_str(&segment);
                segment.clear();
                out.push(ch);
            }
            _ => segment.push(ch),
        }
    }
    out.push_str(&segment);
    out
}

fn merge_json(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(&k) {
                    Some(existing) if existing.is_object() && v.is_object() => {
                        merge_json(existing, v)
                    }
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, p) => *b = p,
    }
}

fn get_component<T: Component + Serialize>(world: &World, e: Entity) -> Option<Value> {
    world.get::<T>(e).and_then(|c| serde_json::to_value(c).ok())
}

fn insert_component<T: Component + DeserializeOwned>(
    world: &mut World,
    e: Entity,
    v: Value,
) -> Result<(), String> {
    let c: T = serde_json::from_value(v).map_err(|err| {
        format!(
            "invalid value for {}: {err}",
            short_type_name(std::any::type_name::<T>())
        )
    })?;
    world
        .get_entity_mut(e)
        .map_err(|_| format!("entity {e} does not exist"))?
        .insert(c);
    Ok(())
}

fn patch_component<T>(world: &mut World, e: Entity, patch: Value) -> Result<(), String>
where
    T: Component<Mutability = Mutable> + Serialize + DeserializeOwned,
{
    let Some(existing) = world.get::<T>(e) else {
        return insert_component::<T>(world, e, patch);
    };
    let mut base = serde_json::to_value(existing).map_err(|e| e.to_string())?;
    merge_json(&mut base, patch);
    let new: T = serde_json::from_value(base).map_err(|err| {
        format!(
            "invalid value for {}: {err}",
            short_type_name(std::any::type_name::<T>())
        )
    })?;
    if let Some(mut slot) = world.get_mut::<T>(e) {
        *slot = new;
    }
    Ok(())
}

fn remove_component<T: Component>(world: &mut World, e: Entity) -> bool {
    match world.get_entity_mut(e) {
        Ok(mut ent) => ent.take::<T>().is_some(),
        Err(_) => false,
    }
}

fn has_component<T: Component>(world: &World, e: Entity) -> bool {
    world.get::<T>(e).is_some()
}

fn get_resource<T: Resource + Serialize>(world: &World) -> Option<Value> {
    world
        .get_resource::<T>()
        .and_then(|r| serde_json::to_value(r).ok())
}

fn patch_resource<T: Resource + Serialize + DeserializeOwned>(
    world: &mut World,
    patch: Value,
) -> Result<(), String> {
    let mut base = match world.get_resource::<T>() {
        Some(r) => serde_json::to_value(r).map_err(|e| e.to_string())?,
        None => Value::Object(Default::default()),
    };
    merge_json(&mut base, patch);
    let new: T = serde_json::from_value(base).map_err(|err| {
        format!(
            "invalid value for {}: {err}",
            short_type_name(std::any::type_name::<T>())
        )
    })?;
    world.insert_resource(new);
    Ok(())
}

impl TypeRegistry {
    pub fn register_component<T>(&mut self, doc: &str)
    where
        T: Component<Mutability = Mutable> + Serialize + DeserializeOwned,
    {
        let type_name = std::any::type_name::<T>();
        let name = short_type_name(type_name);
        if let Some(existing) = self.components.get(&name)
            && existing.type_name != type_name
        {
            tracing::warn!(
                "component name {name} is used by both {} and {type_name}; the latter wins",
                existing.type_name
            );
        }
        self.by_type_name.insert(type_name, name.clone());
        self.components.insert(
            name.clone(),
            ComponentEntry {
                name,
                type_name,
                doc: doc.to_string(),
                get: get_component::<T>,
                insert: insert_component::<T>,
                patch: patch_component::<T>,
                remove: remove_component::<T>,
                has: has_component::<T>,
                component_id: |w: &World| w.component_id::<T>(),
            },
        );
    }

    pub fn register_resource<T>(&mut self, doc: &str)
    where
        T: Resource + Serialize + DeserializeOwned,
    {
        let type_name = std::any::type_name::<T>();
        let name = short_type_name(type_name);
        self.resources.insert(
            name.clone(),
            ResourceEntry {
                name,
                type_name,
                doc: doc.to_string(),
                get: get_resource::<T>,
                patch: patch_resource::<T>,
            },
        );
    }

    pub fn component(&self, name: &str) -> Option<&ComponentEntry> {
        self.components.get(name)
    }

    pub fn resource(&self, name: &str) -> Option<&ResourceEntry> {
        self.resources.get(name)
    }

    pub fn components(&self) -> impl Iterator<Item = &ComponentEntry> {
        self.components.values()
    }

    pub fn resources(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.resources.values()
    }

    /// Short name for a full Rust type name, if that type is registered.
    pub fn name_for_type(&self, type_name: &str) -> Option<&str> {
        self.by_type_name.get(type_name).map(|s| s.as_str())
    }

    /// Lookup with a helpful "did you mean" error.
    pub fn component_or_err(&self, name: &str) -> Result<&ComponentEntry, String> {
        self.components.get(name).ok_or_else(|| {
            unknown_name_error("component", name, self.components.keys().map(|s| s.as_str()))
        })
    }

    pub fn resource_or_err(&self, name: &str) -> Result<&ResourceEntry, String> {
        self.resources.get(name).ok_or_else(|| {
            unknown_name_error("resource", name, self.resources.keys().map(|s| s.as_str()))
        })
    }
}

/// "unknown component 'Helth'. Did you mean 'Health'?" style errors.
pub fn unknown_name_error<'a>(
    kind: &str,
    name: &str,
    known: impl Iterator<Item = &'a str>,
) -> String {
    let known: Vec<&str> = known.collect();
    let mut best: Option<(usize, &str)> = None;
    for k in &known {
        let d = edit_distance(&name.to_lowercase(), &k.to_lowercase());
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, k));
        }
    }
    match best {
        Some((d, k)) if d <= 3 || k.to_lowercase().contains(&name.to_lowercase()) => {
            format!("unknown {kind} '{name}'. Did you mean '{k}'?")
        }
        _ => format!(
            "unknown {kind} '{name}'. Known {kind}s: {}",
            known.join(", ")
        ),
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == *cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

// ------------------------------------------------------------------------- API registry

/// Signature of an engine API function: JSON in, JSON out.
pub type ApiFn = Arc<dyn Fn(&mut World, Value) -> Result<Value, String> + Send + Sync>;

/// One named engine function.
#[derive(Clone)]
pub struct ApiEntry {
    pub name: String,
    pub summary: String,
    pub args_example: String,
    pub func: ApiFn,
}

/// Named functions any module can expose (`"weather.set"`, `"entity.spawn"`, …). The AI bridge
/// lists these as tools; the debug console and scripts call them too.
#[derive(Resource, Default, Clone)]
pub struct ApiRegistry {
    entries: BTreeMap<String, ApiEntry>,
}

impl ApiRegistry {
    pub fn register(
        &mut self,
        name: &str,
        summary: &str,
        args_example: &str,
        f: impl Fn(&mut World, Value) -> Result<Value, String> + Send + Sync + 'static,
    ) {
        self.entries.insert(
            name.to_string(),
            ApiEntry {
                name: name.to_string(),
                summary: summary.to_string(),
                args_example: args_example.to_string(),
                func: Arc::new(f),
            },
        );
    }

    pub fn get(&self, name: &str) -> Option<&ApiEntry> {
        self.entries.get(name)
    }

    pub fn list(&self) -> impl Iterator<Item = &ApiEntry> {
        self.entries.values()
    }
}

/// Call a registered API function by name.
pub fn call_api(world: &mut World, name: &str, args: Value) -> Result<Value, String> {
    let entry = {
        let registry = world.resource::<ApiRegistry>();
        registry.get(name).cloned().ok_or_else(|| {
            unknown_name_error("api function", name, registry.entries.keys().map(|s| s.as_str()))
        })?
    };
    (entry.func)(world, args)
}

// ------------------------------------------------------------------------- entity helpers

/// Entities are addressed in JSON either by id (`{"id": 4294967296}`) or by `Name`
/// (`{"name": "Player"}`), or a bare number/string.
pub fn resolve_entity(world: &mut World, selector: &Value) -> Result<Entity, String> {
    let by_id = |bits: u64| {
        Entity::try_from_bits(bits).ok_or_else(|| format!("{bits} is not a valid entity id"))
    };
    let entity = match selector {
        Value::Number(n) => by_id(n.as_u64().ok_or("entity id must be a positive integer")?)?,
        Value::String(name) => find_by_name(world, name)?,
        Value::Object(map) => {
            if let Some(id) = map.get("id").and_then(|v| v.as_u64()) {
                by_id(id)?
            } else if let Some(name) = map.get("name").and_then(|v| v.as_str()) {
                find_by_name(world, name)?
            } else if let Some(inner) = map.get("entity") {
                return resolve_entity(world, inner);
            } else {
                return Err("expected {\"id\": <number>} or {\"name\": <string>}".into());
            }
        }
        _ => return Err("expected an entity id or name".into()),
    };
    if world.get_entity(entity).is_err() {
        return Err(format!("entity {entity} does not exist"));
    }
    Ok(entity)
}

/// First entity with this `Name`.
pub fn find_by_name(world: &mut World, name: &str) -> Result<Entity, String> {
    let mut q = world.query::<(Entity, &Name)>();
    q.iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
        .ok_or_else(|| format!("no entity named '{name}'"))
}

/// Short names of every component on an entity (registered or not).
pub fn component_names(world: &World, entity: Entity) -> Vec<String> {
    let Ok(components) = world.inspect_entity(entity) else {
        return Vec::new();
    };
    let registry = world.resource::<TypeRegistry>();
    let by_id: std::collections::HashMap<_, &str> = registry
        .components()
        .filter_map(|c| (c.component_id)(world).map(|id| (id, c.name.as_str())))
        .collect();
    components
        .map(|(id, info)| match by_id.get(&id) {
            Some(name) => name.to_string(),
            None => short_type_name(&info.name().to_string()),
        })
        .collect()
}

/// JSON description of one entity: id, name, and every registered component's value.
pub fn describe_entity(world: &World, entity: Entity) -> Value {
    let registry = world.resource::<TypeRegistry>();
    let mut components = serde_json::Map::new();
    let mut opaque = Vec::new();
    for name in component_names(world, entity) {
        match registry.component(&name) {
            Some(entry) => {
                if let Some(v) = (entry.get)(world, entity) {
                    components.insert(name, v);
                }
            }
            None => opaque.push(name),
        }
    }
    json!({
        "id": entity.to_bits(),
        "name": world.get::<Name>(entity).map(|n| n.as_str().to_string()),
        "components": components,
        "other_components": opaque,
        "parent": world.get::<ChildOf>(entity).map(|c| c.0.to_bits()),
        "children": world
            .get::<Children>(entity)
            .map(|c| c.iter().map(|e| e.to_bits()).collect::<Vec<_>>())
            .unwrap_or_default(),
    })
}

/// The built-in `engine.*`, `entity.*`, `resource.*` and `scene.*` API functions.
pub(crate) fn register_core_api(app: &mut App) {
    app.register_api(
        "engine.describe",
        "Everything an agent needs to start: registered components and resources (with docs) \
         and every API function with an argument example.",
        "{}",
        |world, _| {
            let reg = world.resource::<TypeRegistry>();
            let api = world.resource::<ApiRegistry>();
            Ok(json!({
                "components": reg.components().map(|c| json!({"name": c.name, "doc": c.doc})).collect::<Vec<_>>(),
                "resources": reg.resources().map(|r| json!({"name": r.name, "doc": r.doc})).collect::<Vec<_>>(),
                "api": api.list().map(|a| json!({"name": a.name, "summary": a.summary, "args_example": a.args_example})).collect::<Vec<_>>(),
            }))
        },
    )
    .register_api(
        "entity.list",
        "List entities. Optional filters: name_contains, with (component names). Default limit 200.",
        r#"{"name_contains": "Tree", "with": ["Transform"], "limit": 50}"#,
        |world, args| {
            let limit = args["limit"].as_u64().unwrap_or(200) as usize;
            let needle = args["name_contains"].as_str().map(|s| s.to_lowercase());
            let with: Vec<String> = args["with"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let entities: Vec<Entity> = world.query::<Entity>().iter(world).collect();
            let mut out = Vec::new();
            let mut total = 0usize;
            for e in entities {
                let name = world.get::<Name>(e).map(|n| n.as_str().to_string());
                if let Some(n) = &needle
                    && !name.as_deref().unwrap_or("").to_lowercase().contains(n)
                {
                    continue;
                }
                let comps = component_names(world, e);
                if !with.iter().all(|w| comps.contains(w)) {
                    continue;
                }
                total += 1;
                if out.len() < limit {
                    out.push(json!({"id": e.to_bits(), "name": name, "components": comps}));
                }
            }
            Ok(json!({"total": total, "entities": out}))
        },
    )
    .register_api(
        "entity.get",
        "Full JSON of one entity's registered components.",
        r#"{"name": "Player"}"#,
        |world, args| {
            let e = resolve_entity(world, &args)?;
            Ok(describe_entity(world, e))
        },
    )
    .register_api(
        "entity.spawn",
        "Spawn an entity (with optional children) from component JSON. Returns its id.",
        r#"{"name": "Crate", "components": {"Transform": {"translation": [0, 2, 0]}}, "parent": null}"#,
        |world, args| {
            let parent = match args.get("parent") {
                Some(p) if !p.is_null() => Some(resolve_entity(world, p)?),
                _ => None,
            };
            let doc: EntityDoc =
                serde_json::from_value(args).map_err(|e| format!("bad entity: {e}"))?;
            let e = scene::spawn_entity_doc(world, &doc, parent)?;
            Ok(json!({"id": e.to_bits()}))
        },
    )
    .register_api(
        "entity.set",
        "Patch components on an entity (JSON merge; missing components are inserted).",
        r#"{"entity": {"name": "Crate"}, "components": {"Transform": {"translation": [5, 2, 0]}}}"#,
        |world, args| {
            let e = resolve_entity(world, &args["entity"])?;
            let comps = args["components"]
                .as_object()
                .ok_or("'components' must be an object")?
                .clone();
            for (name, value) in comps {
                let entry = world
                    .resource::<TypeRegistry>()
                    .component_or_err(&name)?
                    .clone();
                (entry.patch)(world, e, value)?;
            }
            Ok(describe_entity(world, e))
        },
    )
    .register_api(
        "entity.remove_component",
        "Remove one component from an entity.",
        r#"{"entity": {"name": "Crate"}, "component": "RigidBody"}"#,
        |world, args| {
            let e = resolve_entity(world, &args["entity"])?;
            let name = args["component"].as_str().ok_or("missing 'component'")?;
            let entry = world.resource::<TypeRegistry>().component_or_err(name)?.clone();
            Ok(json!({"removed": (entry.remove)(world, e)}))
        },
    )
    .register_api(
        "entity.despawn",
        "Despawn an entity and its children.",
        r#"{"name": "Crate"}"#,
        |world, args| {
            let e = resolve_entity(world, &args)?;
            Ok(json!({"despawned": world.despawn(e)}))
        },
    )
    .register_api(
        "resource.get",
        "Read a registered resource as JSON.",
        r#"{"name": "Weather"}"#,
        |world, args| {
            let name = args["name"].as_str().ok_or("missing 'name'")?;
            let entry = world.resource::<TypeRegistry>().resource_or_err(name)?.clone();
            (entry.get)(world).ok_or_else(|| format!("resource {name} is not present"))
        },
    )
    .register_api(
        "resource.set",
        "Patch a registered resource (JSON merge).",
        r#"{"name": "Weather", "value": {"rain": 0.8}}"#,
        |world, args| {
            let name = args["name"].as_str().ok_or("missing 'name'")?;
            let entry = world.resource::<TypeRegistry>().resource_or_err(name)?.clone();
            (entry.patch)(world, args["value"].clone())?;
            Ok((entry.get)(world).unwrap_or(Value::Null))
        },
    )
    .register_api(
        "scene.spawn",
        "Spawn a whole scene document: {\"entities\": [EntityDoc…]}.",
        r#"{"entities": [{"name": "A", "components": {"Transform": {}}, "children": []}]}"#,
        |world, args| {
            let doc: SceneDoc =
                serde_json::from_value(args).map_err(|e| format!("bad scene: {e}"))?;
            let ids = scene::spawn_scene(world, &doc, None)?;
            Ok(json!({"ids": ids.iter().map(|e| e.to_bits()).collect::<Vec<_>>()}))
        },
    )
    .register_api(
        "scene.load",
        "Spawn a scene from a JSON file path (relative to the working directory).",
        r#"{"path": "assets/scenes/camp.scene.json"}"#,
        |world, args| {
            let path = args["path"].as_str().ok_or("missing 'path'")?;
            let doc = SceneDoc::load(path)?;
            let ids = scene::spawn_scene(world, &doc, None)?;
            Ok(json!({"ids": ids.iter().map(|e| e.to_bits()).collect::<Vec<_>>()}))
        },
    )
    .register_api(
        "scene.save",
        "Serialize entities (default: all root entities with a Name) into a scene document.",
        r#"{"entities": [{"name": "Camp"}], "path": "assets/scenes/camp.scene.json"}"#,
        |world, args| {
            let roots: Vec<Entity> = match args["entities"].as_array() {
                Some(list) => list
                    .iter()
                    .map(|v| resolve_entity(world, v))
                    .collect::<Result<_, _>>()?,
                None => world
                    .query_filtered::<Entity, (With<Name>, Without<ChildOf>)>()
                    .iter(world)
                    .collect(),
            };
            let doc = SceneDoc {
                entities: roots
                    .into_iter()
                    .map(|e| scene::snapshot_entity(world, e, true))
                    .collect(),
            };
            if let Some(path) = args["path"].as_str() {
                doc.save(path)?;
            }
            serde_json::to_value(&doc).map_err(|e| e.to_string())
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[derive(Component, Serialize, Deserialize, Default, Debug, PartialEq)]
    #[serde(default)]
    struct Health {
        current: f32,
        max: f32,
    }

    #[test]
    fn short_names() {
        assert_eq!(short_type_name("a::b::Health"), "Health");
        assert_eq!(short_type_name("a::Foo<b::Bar, c::Baz>"), "Foo<Bar, Baz>");
    }

    #[test]
    fn patch_merges_fields() {
        let mut app = App::new();
        app.register_component::<Health>("hp");
        let e = app.world.spawn(Health { current: 1.0, max: 10.0 }).id();
        let entry = app.world.resource::<TypeRegistry>().component("Health").unwrap().clone();
        (entry.patch)(&mut app.world, e, json!({"current": 7.0})).unwrap();
        assert_eq!(
            *app.world.get::<Health>(e).unwrap(),
            Health { current: 7.0, max: 10.0 }
        );
    }

    #[test]
    fn api_spawn_and_get_by_name() {
        let mut app = App::new();
        app.register_component::<Health>("hp");
        call_api(
            &mut app.world,
            "entity.spawn",
            json!({"name": "Orc", "components": {"Health": {"max": 30}, "Transform": {"translation": [1, 2, 3]}}}),
        )
        .unwrap();
        let got = call_api(&mut app.world, "entity.get", json!({"name": "Orc"})).unwrap();
        assert_eq!(got["components"]["Health"]["max"], json!(30.0));
        assert_eq!(got["components"]["Transform"]["translation"], json!([1.0, 2.0, 3.0]));
    }

    #[test]
    fn helpful_unknown_errors() {
        let mut app = App::new();
        app.register_component::<Health>("hp");
        let err = call_api(
            &mut app.world,
            "entity.spawn",
            json!({"components": {"Helth": {}}}),
        )
        .unwrap_err();
        assert!(err.contains("Did you mean 'Health'"), "{err}");
    }
}
