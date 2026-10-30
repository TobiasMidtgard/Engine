//! Entity handles and the `world` table.
//!
//! ```lua
//! local lamp = world.find("Lantern 3")             -- by Name; nil when there is none
//! lamp.position = lamp.position + vec3.up * 0.1     -- f64 meters, the entity's Transform
//! lamp:set("PointLight", { intensity_lm = 1200 })  -- any registered component, JSON-merged
//! print(lamp:get("PointLight").color[1])
//! local spark = self:spawn { name = "Spark", components = { Transform = {} } }  -- a child
//! lamp:send("flicker", { seconds = 2 })            -- calls on_message on lamp's script
//! lamp:material { emissive_strength = 40 }         -- its own material, edited in place
//! ```
//!
//! Positions and rotations read and write the entity's own `Transform` (relative to its parent,
//! the world for roots); `world_position` is where it ends up after the hierarchy.
//!
//! `entity:material(patch)` gives the entity a material of its own the first time ([`OwnMaterial`])
//! and edits that one in place after: setting an inline material through `entity:set` every
//! frame would add a new shared material to the renderer's cache for every value it passes
//! through.

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use engine_assets::Assets;
use engine_core::prelude::GlobalTransform;
use engine_core::registry::{TypeRegistry, call_api};
use mlua::{
    FromLua, Function, Lua, MetaMethod, Table, UserData, UserDataFields, UserDataMethods, Value,
};
use serde_json::json;

use crate::access::{
    Outgoing, edit_transform_as, in_parallel, missing, read_transform_as, read_world_as, shared,
    with_world_as, write_world_as,
};
use engine_render::{MaterialRef, MeshRenderer, StandardMaterial};

use crate::convert::{merge, to_json, to_lua};
use crate::math::{Quat, Vec3, look_rotation};

/// A handle to an entity. Compares equal to other handles of the same entity; a handle of a
/// despawned entity reads as missing (`exists()` is false and reads fail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityRef(pub Entity);

impl FromLua for EntityRef {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        match &value {
            Value::UserData(ud) => {
                if let Ok(e) = ud.borrow::<EntityRef>() {
                    return Ok(*e);
                }
            }
            Value::Integer(i) => {
                if let Some(e) = Entity::try_from_bits(*i as u64) {
                    return Ok(EntityRef(e));
                }
            }
            Value::Number(n) => {
                if let Some(e) = Entity::try_from_bits(*n as u64) {
                    return Ok(EntityRef(e));
                }
            }
            _ => {}
        }
        Err(mlua::Error::runtime(format!(
            "expected an entity, got {}",
            value.type_name()
        )))
    }
}

/// The material a script made this entity's own (`entity:material`), edited in place and freed
/// when the entity or this component goes.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct OwnMaterial(pub engine_assets::Handle<StandardMaterial>);

/// Every [`OwnMaterial`] handed out, by entity, so it can be freed once its entity is gone.
#[derive(Resource, Default)]
pub(crate) struct OwnMaterials(HashMap<Entity, engine_assets::Handle<StandardMaterial>>);

/// Frees the materials of entities that lost their [`OwnMaterial`] (or were despawned).
pub(crate) fn free_own_materials(
    mut gone: RemovedComponents<OwnMaterial>,
    mut owned: ResMut<OwnMaterials>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    still: Query<&OwnMaterial>,
) {
    for e in gone.read() {
        if still.contains(e) {
            continue;
        }
        if let (Some(h), Some(materials)) = (owned.0.remove(&e), materials.as_deref_mut()) {
            materials.remove(h);
        }
    }
}

/// `entity:material(patch)`: the entity's material as a table, after merging `patch` into its
/// own copy (made on the first patch).
pub(crate) fn material(
    world: &mut World,
    e: Entity,
    patch: Option<serde_json::Value>,
) -> mlua::Result<serde_json::Value> {
    let err = |m: String| mlua::Error::runtime(m);
    let renderer = world
        .get_entity(e)
        .map_err(|_| missing(e))?
        .get::<MeshRenderer>()
        .ok_or_else(|| err(format!("entity {e} has no MeshRenderer, so no material")))?
        .material
        .clone();
    let Some(materials) = world.get_resource::<Assets<StandardMaterial>>() else {
        return Err(err("there is no renderer (no materials)".into()));
    };
    let own = world
        .get::<OwnMaterial>(e)
        .map(|o| o.0)
        .filter(|h| renderer == MaterialRef::Handle(*h) && materials.contains(*h));
    let current = match (&renderer, own) {
        (_, Some(h)) => materials.get(h).copied(),
        (MaterialRef::Inline(m), None) => Some(*m),
        (MaterialRef::Handle(h), None) => materials.get(*h).copied(),
        (MaterialRef::Asset(key), None) => materials
            .by_key(key)
            .and_then(|h| materials.get(h))
            .copied(),
    }
    .ok_or_else(|| err(format!("entity {e}'s material hasn't loaded yet")))?;
    let mut value =
        serde_json::to_value(current).map_err(|x| err(format!("material to JSON: {x}")))?;
    let Some(patch) = patch else {
        return Ok(value);
    };
    merge(&mut value, patch);
    let next: StandardMaterial = serde_json::from_value(value.clone())
        .map_err(|x| err(format!("bad material patch: {x}")))?;
    match own {
        Some(h) => {
            if next != current {
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                if let Some(m) = materials.get_mut(h) {
                    *m = next;
                }
            }
        }
        None => {
            let h = world.resource_mut::<Assets<StandardMaterial>>().add(next);
            let old = world.get_resource_or_init::<OwnMaterials>().0.insert(e, h);
            if let Some(old) = old {
                world.resource_mut::<Assets<StandardMaterial>>().remove(old);
            }
            let mut ent = world.entity_mut(e);
            ent.insert(OwnMaterial(h));
            if let Some(mut r) = ent.get_mut::<MeshRenderer>() {
                r.material = MaterialRef::Handle(h);
            }
        }
    }
    Ok(value)
}

/// Registry name of the hidden table of script instances by entity id (`entity.script`).
pub(crate) const SELVES: &str = "engine_script.selves";

impl UserData for EntityRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("id", |_, e| Ok(e.0.to_bits() as f64));
        fields.add_field_method_get("name", |lua, e| {
            read_world_as(lua, "e.name", |w| {
                Ok(w.get::<Name>(e.0).map(|n| n.as_str().to_string()))
            })
        });
        fields.add_field_method_set("name", |lua, e, name: String| {
            let e = e.0;
            write_world_as(lua, "e.name =", move |w| {
                w.get_entity_mut(e)
                    .map_err(|_| missing(e))?
                    .insert(Name::new(name));
                Ok(())
            })
        });
        fields.add_field_method_get("position", |lua, e| {
            Ok(Vec3(read_transform_as(lua, "e.position", e.0)?.translation))
        });
        fields.add_field_method_set("position", |lua, e, v: Vec3| {
            edit_transform_as(lua, "e.position =", e.0, |t| t.translation = v.0)
        });
        fields.add_field_method_get("rotation", |lua, e| {
            Ok(Quat(read_transform_as(lua, "e.rotation", e.0)?.rotation))
        });
        fields.add_field_method_set("rotation", |lua, e, q: Quat| {
            edit_transform_as(lua, "e.rotation =", e.0, |t| t.rotation = q.0)
        });
        fields.add_field_method_get("scale", |lua, e| {
            Ok(Vec3(
                read_transform_as(lua, "e.scale", e.0)?.scale.as_dvec3(),
            ))
        });
        fields.add_field_method_set("scale", |lua, e, v: Value| {
            let s = match v {
                Value::Integer(i) => engine_core::math::Vec3::splat(i as f32),
                Value::Number(n) => engine_core::math::Vec3::splat(n as f32),
                other => Vec3::from_lua(other, lua)?.f32(),
            };
            edit_transform_as(lua, "e.scale =", e.0, |t| t.scale = s)
        });
        fields.add_field_method_get("world_position", |lua, e| {
            // Where the hierarchy put it last frame; for an entity this VM moved in the
            // parallel phase (or a root), its own position.
            if in_parallel(lua) {
                let own = read_transform_as(lua, "e.world_position", e.0)?;
                let root = read_world_as(lua, "e.world_position", |w| {
                    Ok(w.get::<ChildOf>(e.0).is_none())
                })?;
                if root {
                    return Ok(Vec3(own.translation));
                }
            }
            read_world_as(lua, "e.world_position", |w| {
                match w.get::<GlobalTransform>(e.0) {
                    Some(g) => Ok(Vec3(g.translation())),
                    None => Ok(Vec3(crate::access::transform_of(w, e.0)?.translation)),
                }
            })
        });
        fields.add_field_method_get("forward", |lua, e| {
            Ok(Vec3(
                read_transform_as(lua, "e.forward", e.0)?
                    .forward()
                    .as_dvec3(),
            ))
        });
        fields.add_field_method_get("right", |lua, e| {
            Ok(Vec3(
                read_transform_as(lua, "e.right", e.0)?.right().as_dvec3(),
            ))
        });
        fields.add_field_method_get("up", |lua, e| {
            Ok(Vec3(read_transform_as(lua, "e.up", e.0)?.up().as_dvec3()))
        });
        fields.add_field_method_get("parent", |lua, e| {
            read_world_as(lua, "e.parent", |w| {
                Ok(w.get::<ChildOf>(e.0).map(|c| EntityRef(c.0)))
            })
        });
        fields.add_field_method_get("script", |lua, e| {
            let selves: Table = lua.named_registry_value(SELVES)?;
            selves.raw_get::<Value>(e.0.to_bits() as f64)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("exists", |lua, e, ()| {
            read_world_as(lua, "e:exists", |w| Ok(w.get_entity(e.0).is_ok()))
        });
        methods.add_method("get", |lua, e, name: String| {
            read_world_as(lua, "e:get", |w| {
                let get = w
                    .resource::<TypeRegistry>()
                    .component_or_err(&name)
                    .map_err(mlua::Error::runtime)?
                    .get;
                if w.get_entity(e.0).is_err() {
                    return Err(missing(e.0));
                }
                match get(w, e.0) {
                    Some(v) => to_lua(lua, &v),
                    None => Ok(Value::Nil),
                }
            })
        });
        methods.add_method("set", |lua, e, (name, value): (String, Value)| {
            let value = to_json(&value)?;
            let e = e.0;
            write_world_as(lua, "e:set", move |w| {
                let patch = w
                    .resource::<TypeRegistry>()
                    .component_or_err(&name)
                    .map_err(mlua::Error::runtime)?
                    .patch;
                patch(w, e, value).map_err(mlua::Error::runtime)
            })
        });
        methods.add_method("has", |lua, e, name: String| {
            read_world_as(lua, "e:has", |w| {
                let has = w
                    .resource::<TypeRegistry>()
                    .component_or_err(&name)
                    .map_err(mlua::Error::runtime)?
                    .has;
                Ok(has(w, e.0))
            })
        });
        methods.add_method("remove", |lua, e, name: String| {
            if in_parallel(lua) {
                let e = e.0;
                write_world_as(lua, "e:remove", move |w| {
                    removal(w, &name)?(w, e);
                    Ok(())
                })?;
                return Ok(None);
            }
            with_world_as(lua, "e:remove", |w| Ok(Some(removal(w, &name)?(w, e.0))))
        });
        methods.add_method("material", |lua, e, patch: Option<Value>| {
            let patch = patch.map(|v| to_json(&v)).transpose()?;
            if in_parallel(lua) && patch.is_some() {
                // Changed after the parallel phase; nothing to return yet.
                let e = e.0;
                write_world_as(lua, "e:material", move |w| {
                    material(w, e, patch).map(|_| ())
                })?;
                return Ok(Value::Nil);
            }
            let out = with_world_as(lua, "e:material", |w| material(w, e.0, patch))?;
            to_lua(lua, &out)
        });
        methods.add_method("despawn", |lua, e, ()| {
            if in_parallel(lua) {
                let e = e.0;
                write_world_as(lua, "e:despawn", move |w| {
                    if w.get_entity(e).is_ok() {
                        w.despawn(e);
                    }
                    Ok(())
                })?;
                return Ok(None);
            }
            with_world_as(lua, "e:despawn", |w| {
                Ok(Some(w.get_entity(e.0).is_ok() && w.despawn(e.0)))
            })
        });
        methods.add_method("look_at", |lua, e, (target, up): (Vec3, Option<Vec3>)| {
            let at = read_transform_as(lua, "e:look_at", e.0)?.translation;
            let dir = (target.0 - at).as_vec3();
            if dir.length_squared() < 1e-12 {
                return Ok(());
            }
            let up = up.map(|u| u.f32()).unwrap_or(engine_core::math::Vec3::Y);
            let rot = look_rotation(dir.normalize(), up);
            edit_transform_as(lua, "e:look_at", e.0, |t| t.rotation = rot)
        });
        methods.add_method("translate", |lua, e, by: Vec3| {
            edit_transform_as(lua, "e:translate", e.0, |t| t.translation += by.0)
        });
        methods.add_method("rotate", |lua, e, q: Quat| {
            edit_transform_as(lua, "e:rotate", e.0, |t| t.rotation = q.0 * t.rotation)
        });
        methods.add_method("rotate_y", |lua, e, angle: f32| {
            edit_transform_as(lua, "e:rotate_y", e.0, |t| t.rotate_y(angle))
        });
        methods.add_method("children", |lua, e, ()| {
            read_world_as(lua, "e:children", |w| {
                let kids: Vec<EntityRef> = w
                    .get::<Children>(e.0)
                    .map(|c| c.iter().map(EntityRef).collect())
                    .unwrap_or_default();
                Ok(kids)
            })
        });
        methods.add_method("find", |lua, e, name: String| {
            read_world_as(lua, "e:find", |w| {
                Ok(find_descendant(w, e.0, &name).map(EntityRef))
            })
        });
        methods.add_method("spawn", |lua, e, spec: Value| spawn(lua, spec, Some(e.0)));
        methods.add_method("send", |lua, e, (event, data): (String, Option<Value>)| {
            let data = match data {
                Some(v) => to_json(&v)?,
                None => serde_json::Value::Null,
            };
            let mut s = shared(lua);
            let from = s.owner;
            if let (Some(trace), Some(me)) = (s.trace.as_mut(), from) {
                trace.push(crate::rewind::Traced {
                    owner: me,
                    kind: crate::rewind::EventKind::Sent,
                    text: event.clone(),
                    other: Some(e.0),
                });
            }
            s.outbox.push(Outgoing {
                to: e.0,
                event,
                data,
                from,
            });
            Ok(())
        });
        // Roblox's `Connect` and `Once` on its script's signals: the function runs (as a task
        // of the script that called this) each time, or the next time, the signal fires.
        for (name, once) in [("on", false), ("once", true)] {
            methods.add_method(name, move |lua, e, (signal, f): (String, Function)| {
                listen(lua, e.0, signal, f, once)
            });
        }
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (EntityRef, EntityRef)| {
            Ok(a == b)
        });
        methods.add_meta_method(MetaMethod::ToString, |lua, e, ()| {
            let name = read_world_as(lua, "tostring", |w| {
                Ok(w.get::<Name>(e.0).map(|n| n.as_str().to_string()))
            })
            .ok()
            .flatten();
            Ok(match name {
                Some(n) => format!("entity {} \"{n}\"", e.0),
                None => format!("entity {}", e.0),
            })
        });
    }
}

/// The registry's remover for component `name`.
fn removal(w: &World, name: &str) -> mlua::Result<fn(&mut World, Entity) -> bool> {
    Ok(w.resource::<TypeRegistry>()
        .component_or_err(name)
        .map_err(mlua::Error::runtime)?
        .remove)
}

/// Entities whose `Name` is `name` (or contains `needle`, any case), sorted by name so the
/// order doesn't depend on how they are stored.
fn named(w: &World, test: impl Fn(&str) -> bool) -> Vec<(String, Entity)> {
    let Some(mut q) = w.try_query::<(Entity, &Name)>() else {
        return Vec::new();
    };
    let mut found: Vec<(String, Entity)> = q
        .iter(w)
        .filter(|(_, n)| test(n.as_str()))
        .map(|(e, n)| (n.as_str().to_string(), e))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    found
}

/// Depth-first search under `root` for an entity named `name`.
pub(crate) fn find_descendant(world: &World, root: Entity, name: &str) -> Option<Entity> {
    let mut stack: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    while let Some(e) = stack.pop() {
        if world.get::<Name>(e).is_some_and(|n| n.as_str() == name) {
            return Some(e);
        }
        if let Some(c) = world.get::<Children>(e) {
            stack.extend(c.iter());
        }
    }
    None
}

/// `entity.spawn` from a script: `{ name, components, children, parent }` (the scene format;
/// `parent` an entity), under `parent` when given. Returns the new entity's handle.
fn spawn(lua: &Lua, spec: Value, parent: Option<Entity>) -> mlua::Result<EntityRef> {
    let mut args = to_json(&spec)?;
    let Some(map) = args.as_object_mut() else {
        return Err(mlua::Error::runtime(
            "spawn takes a table: { name = \"Lamp\", components = { Transform = {} } }",
        ));
    };
    if let Some(p) = parent {
        map.insert("parent".into(), json!(p.to_bits()));
    }
    map.entry("components").or_insert_with(|| json!({}));
    let by = shared(lua).constructing;
    let out = with_world_as(lua, "spawn", |w| {
        let out = call_api(w, "entity.spawn", args).map_err(mlua::Error::runtime)?;
        // Built by a script's `construct`: it goes when the script builds again.
        if let Some(by) = by
            && let Some(e) = out["id"].as_u64().and_then(Entity::try_from_bits)
            && let Ok(mut spawned) = w.get_entity_mut(e)
        {
            spawned.insert(crate::Constructed { by });
        }
        Ok(out)
    })?;
    let bits = out["id"]
        .as_u64()
        .ok_or_else(|| mlua::Error::runtime("entity.spawn returned no id"))?;
    Entity::try_from_bits(bits)
        .map(EntityRef)
        .ok_or_else(|| mlua::Error::runtime("entity.spawn returned an invalid id"))
}

/// Adds the `world` global.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let world = lua.create_table()?;
    world.set(
        "find",
        lua.create_function(|lua, name: String| {
            read_world_as(lua, "world.find", |w| {
                let Some(mut q) = w.try_query::<(Entity, &Name)>() else {
                    return Ok(None);
                };
                Ok(q.iter(w)
                    .find(|(_, n)| n.as_str() == name)
                    .map(|(e, _)| EntityRef(e)))
            })
        })?,
    )?;
    world.set(
        "find_all",
        lua.create_function(|lua, needle: String| {
            read_world_as(lua, "world.find_all", |w| {
                let needle = needle.to_lowercase();
                Ok(named(w, |n| n.to_lowercase().contains(&needle))
                    .into_iter()
                    .map(|(_, e)| EntityRef(e))
                    .collect::<Vec<_>>())
            })
        })?,
    )?;
    world.set(
        "entity",
        lua.create_function(|lua, id: Value| EntityRef::from_lua(id, lua))?,
    )?;
    world.set(
        "spawn",
        lua.create_function(|lua, spec: Value| spawn(lua, spec, None))?,
    )?;
    world.set(
        "time",
        lua.create_function(|lua, ()| Ok(shared(lua).world_secs))?,
    )?;
    lua.globals().set("world", world)?;
    Ok(())
}

/// Adds a listener to `from`'s `signal`; returns its connection (`connection:disconnect()`).
fn listen(lua: &Lua, from: Entity, signal: String, f: Function, once: bool) -> mlua::Result<Table> {
    let func = lua.create_registry_value(f)?;
    let id = {
        let mut s = shared(lua);
        s.next_listener += 1;
        let id = s.next_listener;
        let owner = s.owner;
        s.listeners.push(crate::access::Listener {
            id,
            from,
            signal,
            owner,
            func,
            once,
        });
        id
    };
    let connection = lua.create_table()?;
    connection.raw_set(
        "disconnect",
        lua.create_function(move |lua, _: mlua::Variadic<Value>| {
            shared(lua).listeners.retain(|l| l.id != id);
            Ok(())
        })?,
    )?;
    connection.raw_set(
        "connected",
        lua.create_function(move |lua, _: mlua::Variadic<Value>| {
            Ok(shared(lua).listeners.iter().any(|l| l.id == id))
        })?,
    )?;
    Ok(connection)
}
