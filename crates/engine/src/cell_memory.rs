//! What happened in a remembering cell survives its unload ([`CellMemory`]).
//!
//! When a cell is all in, each entity it spawned gets a [`PersistentId`] from its name and
//! where it stands (from the cell's center for roots, from its parent for children), unless
//! its spawn gave it one, and the cell notes the values of the remembered components
//! ([`CellMemory::components`]) as spawned. When it leaves, what is gone and what differs from
//! those values goes into [`CellMemory`]; when it comes back, right after its spawn, what is
//! gone is despawned again and what changed is set again, before anything is shown. It is the
//! model of Unreal's level streaming persistence (persistent properties of actors in streamed
//! levels, placed actors "ejected" for good) and of Bethesda's change forms (per reference:
//! moved, deleted, inventory), kept as plain JSON per component so a save game stores it whole.
//!
//! A root's `Transform` translation is kept from the cell's center, so memory survives world
//! shifts; content a cell hangs on an entity from before (a frame's) keeps its translation on
//! that entity, so it survives the frame's motion.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_core::registry::TypeRegistry;
use engine_world::{CellMemory, PersistentId};
use serde_json::{Map, Value};

/// The remembered components' values of a loaded cell's entities as spawned, by id.
#[derive(Debug, Default)]
pub(crate) struct Baseline {
    values: HashMap<u64, Map<String, Value>>,
}

fn fnv(h: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes
        .into_iter()
        .fold(h, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// Centimetres, so float noise in a spawn doesn't change an id.
fn cm(v: DVec3) -> [u8; 24] {
    let mut out = [0; 24];
    for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
        out[k * 8..k * 8 + 8].copy_from_slice(&((c * 100.0).round() as i64).to_le_bytes());
    }
    out
}

/// Gives every entity of the trees under `roots` an id (keeping one its spawn gave), parents
/// before children; `seen` counts twins (same name, same place) across a cell's slices. Runs
/// right after each spawn slice, before anything can move what it spawned. Returns them with
/// their ids.
pub(crate) fn assign_ids(
    world: &mut World,
    key: &str,
    center: DVec3,
    roots: &[Entity],
    seen: &mut HashMap<u64, u32>,
) -> Vec<(Entity, u64)> {
    let mut out = Vec::new();
    let cell = fnv(FNV_START, key.bytes());
    let mut stack: Vec<(Entity, u64, bool)> =
        roots.iter().rev().map(|&e| (e, cell, true)).collect();
    while let Some((e, parent, root)) = stack.pop() {
        let Ok(entity) = world.get_entity(e) else {
            continue;
        };
        let id = match entity.get::<PersistentId>() {
            Some(id) => id.0,
            None => {
                let name = entity.get::<Name>().map(|n| n.as_str().to_string());
                let at = entity
                    .get::<Transform>()
                    .map_or(DVec3::ZERO, |t| t.translation);
                // A top hung on something from before (a frame) keeps its place on it.
                let at = if root && entity.get::<ChildOf>().is_none() {
                    at - center
                } else {
                    at
                };
                let mut base = fnv(parent, name.unwrap_or_default().bytes());
                base = fnv(base, cm(at));
                let n = seen.entry(base).or_default();
                let id = if *n == 0 {
                    base
                } else {
                    fnv(base, n.to_le_bytes())
                };
                *n += 1;
                world.entity_mut(e).insert(PersistentId(id));
                id
            }
        };
        out.push((e, id));
        if let Some(children) = world.get::<Children>(e) {
            let kids: Vec<Entity> = children.iter().collect();
            stack.extend(kids.into_iter().rev().map(|c| (c, id, false)));
        }
    }
    out
}

/// The remembered components of `e`, a root's translation taken from `center`.
fn values(world: &World, e: Entity, components: &[String], center: DVec3) -> Map<String, Value> {
    let registry = world.resource::<TypeRegistry>();
    let root = world.get::<ChildOf>(e).is_none();
    let mut out = Map::new();
    for name in components {
        let Ok(entry) = registry.component_or_err(name) else {
            continue;
        };
        if !(entry.has)(world, e) {
            continue;
        }
        let Some(mut v) = (entry.get)(world, e) else {
            continue;
        };
        if name == "Transform" && root {
            offset(&mut v, -center);
        }
        out.insert(name.clone(), v);
    }
    out
}

/// Moves a serialized `Transform`'s translation by `by`.
fn offset(v: &mut Value, by: DVec3) {
    if let Some(t) = v.get_mut("translation").and_then(Value::as_array_mut)
        && t.len() == 3
    {
        for (k, d) in [by.x, by.y, by.z].into_iter().enumerate() {
            if let Some(x) = t[k].as_f64() {
                t[k] = Value::from(x + d);
            }
        }
    }
}

/// Equal but for float noise (a millimetre, a thousandth).
fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-3,
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| close(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, a)| y.get(k).is_some_and(|b| close(a, b)))
        }
        _ => a == b,
    }
}

/// The cell `key` is all in: ids are given, the spawned values noted, and what the memory
/// holds for the cell is applied (gone ones despawn, changed values are set).
pub(crate) fn loaded(world: &mut World, key: &str, center: DVec3, roots: &[Entity]) -> Baseline {
    let components = world.resource::<CellMemory>().components.clone();
    // Ids from the slices are kept; anything a system added since gets one now.
    let ids = assign_ids(world, key, center, roots, &mut HashMap::new());
    let baseline = Baseline {
        values: ids
            .iter()
            .map(|&(e, id)| (id, values(world, e, &components, center)))
            .collect(),
    };
    let Some(delta) = world.resource::<CellMemory>().cells.get(key).cloned() else {
        return baseline;
    };
    for (e, id) in ids {
        if world.get_entity(e).is_err() {
            continue;
        }
        if delta.removed.contains(&id) {
            world.entity_mut(e).despawn();
            continue;
        }
        let Some(changed) = delta.changed.get(&id) else {
            continue;
        };
        let root = world.get::<ChildOf>(e).is_none();
        for (name, value) in changed {
            let Ok(entry) = world
                .resource::<TypeRegistry>()
                .component_or_err(name)
                .cloned()
            else {
                continue;
            };
            let mut value = value.clone();
            if name == "Transform" && root {
                offset(&mut value, center);
            }
            if let Err(e) = (entry.insert)(world, e, value) {
                engine_core::log::warn!("cell memory: {key}: {name}: {e}");
            }
        }
    }
    baseline
}

/// The cell `key` is leaving (its entities still there): what is gone and what changed since
/// its spawn go into the memory.
pub(crate) fn leaving(
    world: &mut World,
    key: &str,
    center: DVec3,
    roots: &[Entity],
    baseline: Baseline,
) {
    let components = world.resource::<CellMemory>().components.clone();
    let mut alive: HashMap<u64, Entity> = HashMap::new();
    let mut stack: Vec<Entity> = roots.to_vec();
    while let Some(e) = stack.pop() {
        if let Some(id) = world.get::<PersistentId>(e) {
            alive.insert(id.0, e);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
    let mut delta = engine_world::CellDelta::default();
    for (id, spawned) in &baseline.values {
        let Some(&e) = alive.get(id) else {
            delta.removed.insert(*id);
            continue;
        };
        let now = values(world, e, &components, center);
        let differs: Map<String, Value> = now
            .into_iter()
            .filter(|(name, v)| !spawned.get(name).is_some_and(|s| close(s, v)))
            .collect();
        if !differs.is_empty() {
            delta.changed.insert(*id, differs);
        }
    }
    let mut memory = world.resource_mut::<CellMemory>();
    if delta == engine_world::CellDelta::default() {
        memory.cells.remove(key);
    } else {
        memory.cells.insert(key.to_string(), delta);
    }
}
