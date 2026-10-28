//! Prefabs: an entity and its children saved to a file and placed many times, with Unity's
//! workflow (per-property overrides, nested prefabs, variants, a prefab editing mode) and
//! O3DE's idea of keeping each copy's differences as patches against its file.
//!
//! - A placed copy's root carries [`PrefabInstance`]: the file and this copy's [`Override`]s.
//!   Every entity that came from the file carries [`PrefabPart`], its key in the file, so an
//!   override names the part it changes. Entities added under a copy have no key: they stay
//!   this copy's own until applied.
//! - A prefab file is an engine [`SceneDoc`] with one root, saved fully expanded so a game can
//!   spawn it without the editor. An entity in it with its own [`PrefabInstance`] is a nested
//!   prefab (at the root: the base of a variant). [`PrefabFiles::expand`] rebuilds those parts
//!   from their own files, so a change to the lamp post reaches the lamp posts in every street.
//! - [`track`] keeps every copy's overrides current by diffing it against its file. When a file
//!   changes (applied from a copy, saved from prefab editing, or edited outside the editor),
//!   every copy is rebuilt in place as the new file plus its own overrides ([`sync_all`]).
//! - A copy's root Transform and name say where and what it is; they are never overrides.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use engine::core::scene::{EntityDoc, SceneDoc, snapshot_entity};
use engine::prelude::*;
use serde_json::{Value, json};

use crate::editor::{self, Editor, now};
use crate::scene_ops::{self, InScene, SavedEntity};
use crate::undo::Command;

pub const PREFABS_DIR: &str = "prefabs";
/// Nested prefabs and variant bases deeper than this are refused (a prefab inside itself).
const MAX_DEPTH: usize = 8;
/// How often overrides are refreshed and prefab files checked for outside changes, seconds.
const TRACK_SECS: f64 = 0.5;

/// This entity (and its children) was placed from the prefab file at `path`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrefabInstance {
    pub path: String,
    /// What this copy changes, by part.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Keys of the file's parts deleted from this copy.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// One changed value of one part: a whole component (`field` empty; a null value means the
/// component was removed) or one field inside it (`field` is a JSON pointer, `/color/0`).
/// The pseudo-component `Name` is the part's name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub part: String,
    pub component: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub field: String,
    pub value: Value,
}

/// Which part of its prefab this entity is: a key unique in the file, "" for the root, and
/// `nested/inner` for the parts of a nested prefab.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrefabPart {
    pub key: String,
}

// ----------------------------------------------------------------------------- files

/// Prefab files as read (and as expanded, nested prefabs and variant bases resolved).
#[derive(Resource, Default)]
pub struct PrefabFiles {
    stored: HashMap<String, (Option<SystemTime>, EntityDoc)>,
    expanded: HashMap<String, EntityDoc>,
    /// Read values are written out in full through this (a hand-written `{"translation":
    /// [0, 2, 0]}` gains its rotation and scale), so they compare with what the scene holds.
    registry: Option<engine::core::registry::TypeRegistry>,
}

impl PrefabFiles {
    /// The file's root as saved.
    pub fn stored(&mut self, path: &str) -> Result<EntityDoc, String> {
        if let Some((_, doc)) = self.stored.get(path) {
            return Ok(doc.clone());
        }
        let mut doc = SceneDoc::load(path)
            .map_err(|e| format!("{path}: {e}"))?
            .entities
            .into_iter()
            .next()
            .ok_or_else(|| format!("{path} is empty"))?;
        ensure_keys(&mut doc);
        if let Some(registry) = &self.registry {
            canonical(registry, &mut World::new(), &mut doc);
        }
        self.stored.insert(path.into(), (stamp(path), doc.clone()));
        Ok(doc)
    }

    /// The prefab at `path` with its nested prefabs and variant base rebuilt from their own
    /// files (with the overrides this file gives them). Keys are in `path`'s namespace.
    pub fn expand(&mut self, path: &str) -> Result<EntityDoc, String> {
        if let Some(doc) = self.expanded.get(path) {
            return Ok(doc.clone());
        }
        let doc = self.expand_at(path, 0)?;
        self.expanded.insert(path.into(), doc.clone());
        Ok(doc)
    }

    fn expand_at(&mut self, path: &str, depth: usize) -> Result<EntityDoc, String> {
        if depth > MAX_DEPTH {
            return Err(format!(
                "{path}: prefabs nest more than {MAX_DEPTH} deep (does one contain itself?)"
            ));
        }
        let stored = self.stored(path)?;
        Ok(self.resolve(&stored, depth))
    }

    fn resolve(&mut self, doc: &EntityDoc, depth: usize) -> EntityDoc {
        let Some(link) = link_of(doc) else {
            let mut out = doc.clone();
            out.children = doc
                .children
                .iter()
                .map(|c| self.resolve(c, depth))
                .collect();
            return out;
        };
        let key = key_of(doc).unwrap_or("").to_string();
        let mut base = match self.expand_at(&link.path, depth + 1) {
            Ok(base) => base,
            // A missing nested file keeps the copy saved here.
            Err(e) => {
                engine::core::log::warn!("prefab: {e}");
                return doc.clone();
            }
        };
        apply_overrides(&mut base, &link);
        rekey(&mut base, &key);
        // Where and what this copy is comes from this file.
        if let Some(t) = doc.components.get("Transform") {
            base.components.insert("Transform".into(), t.clone());
        }
        base.name = doc.name.clone();
        set_link(&mut base, &link);
        set_key(&mut base, &key);
        // Entities this file added under the nested prefab's parts.
        let mut keys = HashSet::new();
        all_keys(&base, &mut keys);
        self.graft(doc, &mut base, &keys, depth);
        base
    }

    fn graft(
        &mut self,
        doc: &EntityDoc,
        base: &mut EntityDoc,
        keys: &HashSet<String>,
        depth: usize,
    ) {
        let parent = key_of(doc).unwrap_or("").to_string();
        for child in &doc.children {
            match key_of(child) {
                Some(k) if keys.contains(k) => self.graft(child, base, keys, depth),
                _ => {
                    let added = self.resolve(child, depth);
                    match find_mut(base, &parent) {
                        Some(p) => p.children.push(added),
                        None => base.children.push(added),
                    }
                }
            }
        }
    }

    /// Write (or with `None` delete) a prefab file and forget what was cached from it.
    pub fn write(&mut self, path: &str, text: Option<&str>) -> Result<(), String> {
        match text {
            Some(text) => {
                if let Some(dir) = std::path::Path::new(path).parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{path}: {e}"))?;
                }
                std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))?;
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
        self.stored.remove(path);
        self.expanded.clear();
        Ok(())
    }

    /// Files changed on disk since they were read (they are read again next time).
    fn changed_on_disk(&mut self) -> Vec<String> {
        let changed: Vec<String> = self
            .stored
            .iter()
            .filter(|(path, (at, _))| stamp(path) != *at)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &changed {
            self.stored.remove(path);
        }
        if !changed.is_empty() {
            self.expanded.clear();
        }
        changed
    }
}

/// Each registered component's value as its type writes it (defaults filled in).
fn canonical(
    registry: &engine::core::registry::TypeRegistry,
    scratch: &mut World,
    doc: &mut EntityDoc,
) {
    for (name, value) in doc.components.iter_mut() {
        let Some(entry) = registry.component(name) else {
            continue;
        };
        let e = scratch.spawn_empty().id();
        if (entry.insert)(scratch, e, value.clone()).is_ok()
            && let Some(full) = (entry.get)(scratch, e)
        {
            *value = full;
        }
        scratch.entity_mut(e).despawn();
    }
    for c in &mut doc.children {
        canonical(registry, scratch, c);
    }
}

fn stamp(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Run `f` with the prefab file cache.
pub fn with_files<R>(world: &mut World, f: impl FnOnce(&mut PrefabFiles, &mut World) -> R) -> R {
    if !world.contains_resource::<PrefabFiles>() {
        let registry = world
            .get_resource::<engine::core::registry::TypeRegistry>()
            .cloned();
        world.insert_resource(PrefabFiles {
            registry,
            ..Default::default()
        });
    }
    world.resource_scope(|world, mut files: Mut<PrefabFiles>| f(&mut files, world))
}

/// Write a prefab file's text (the undoable [`Command::PrefabFile`] does this).
pub fn write_text(world: &mut World, path: &str, text: Option<&str>) -> Result<(), String> {
    with_files(world, |files, _| files.write(path, text))
}

/// Prefab files in [`PREFABS_DIR`], sorted.
pub fn prefab_files() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(PREFABS_DIR)
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".prefab.json"))
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// The file a variant is based on, if `path` is a variant.
pub fn base_of(world: &mut World, path: &str) -> Option<String> {
    with_files(world, |files, _| files.stored(path).ok())
        .and_then(|doc| link_of(&doc))
        .map(|l| l.path)
}

/// "Lamp post" → "prefabs/lamp_post.prefab.json".
pub fn path_for(name: &str) -> String {
    format!("{PREFABS_DIR}/{}.prefab.json", slug(name, "prefab"))
}

fn slug(name: &str, empty: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { empty.into() } else { s }
}

/// A path for a new file named after `name` that isn't taken yet.
fn free_path(name: &str) -> String {
    let mut path = path_for(name);
    let mut n = 2;
    while std::path::Path::new(&path).exists() {
        path = path_for(&format!("{name} {n}"));
        n += 1;
    }
    path
}

/// The name shown for a prefab file.
pub fn display_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.trim_end_matches(".prefab.json").replace('_', " ");
    let mut chars = stem.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => stem,
    }
}

// ----------------------------------------------------------------------------- documents

pub fn key_of(doc: &EntityDoc) -> Option<&str> {
    doc.components.get("PrefabPart")?.get("key")?.as_str()
}

fn set_key(doc: &mut EntityDoc, key: &str) {
    doc.components
        .insert("PrefabPart".into(), json!({ "key": key }));
}

pub fn link_of(doc: &EntityDoc) -> Option<PrefabInstance> {
    serde_json::from_value(doc.components.get("PrefabInstance")?.clone()).ok()
}

fn set_link(doc: &mut EntityDoc, link: &PrefabInstance) {
    if let Ok(v) = serde_json::to_value(link) {
        doc.components.insert("PrefabInstance".into(), v);
    }
}

/// `key` inside the nested prefab whose root has key `prefix`.
pub fn join(prefix: &str, key: &str) -> String {
    match (prefix.is_empty(), key.is_empty()) {
        (true, _) => key.to_string(),
        (_, true) => prefix.to_string(),
        _ => format!("{prefix}/{key}"),
    }
}

/// `key` as the nested prefab at `prefix` names it, if it is one of its parts.
pub fn strip(prefix: &str, key: &str) -> Option<String> {
    if prefix.is_empty() {
        Some(key.to_string())
    } else if key == prefix {
        Some(String::new())
    } else {
        key.strip_prefix(prefix)
            .and_then(|r| r.strip_prefix('/'))
            .map(String::from)
    }
}

/// Give every entity of an older file without keys one (root "", others by name).
fn ensure_keys(doc: &mut EntityDoc) {
    let mut used = HashSet::new();
    all_keys(doc, &mut used);
    if key_of(doc).is_none() {
        set_key(doc, "");
    }
    fn walk(doc: &mut EntityDoc, used: &mut HashSet<String>) {
        for c in &mut doc.children {
            if key_of(c).is_none() {
                let k = fresh_key(c.name.as_deref(), used);
                set_key(c, &k);
            }
            walk(c, used);
        }
    }
    walk(doc, &mut used);
}

/// A key from `name` not in `used` (and now in it).
fn fresh_key(name: Option<&str>, used: &mut HashSet<String>) -> String {
    let base = slug(name.unwrap_or(""), "part");
    let mut key = base.clone();
    let mut n = 2;
    while used.contains(&key) || key.is_empty() {
        key = format!("{base}_{n}");
        n += 1;
    }
    used.insert(key.clone());
    key
}

fn all_keys(doc: &EntityDoc, out: &mut HashSet<String>) {
    if let Some(k) = key_of(doc) {
        out.insert(k.to_string());
    }
    for c in &doc.children {
        all_keys(c, out);
    }
}

fn rekey(doc: &mut EntityDoc, prefix: &str) {
    if let Some(k) = key_of(doc).map(String::from) {
        set_key(doc, &join(prefix, &k));
    }
    for c in &mut doc.children {
        rekey(c, prefix);
    }
}

/// The part with `key` (keyed entities only; added ones are not parts).
pub fn find<'a>(doc: &'a EntityDoc, key: &str) -> Option<&'a EntityDoc> {
    if key_of(doc) == Some(key) {
        return Some(doc);
    }
    doc.children.iter().find_map(|c| find(c, key))
}

fn find_mut<'a>(doc: &'a mut EntityDoc, key: &str) -> Option<&'a mut EntityDoc> {
    if key_of(doc) == Some(key) {
        return Some(doc);
    }
    doc.children.iter_mut().find_map(|c| find_mut(c, key))
}

fn remove_part(doc: &mut EntityDoc, key: &str) -> bool {
    if let Some(i) = doc.children.iter().position(|c| key_of(c) == Some(key)) {
        doc.children.remove(i);
        return true;
    }
    doc.children.iter_mut().any(|c| remove_part(c, key))
}

/// Parts of a tree by key, not looking inside entities without a key (added ones).
fn parts<'a>(doc: &'a EntityDoc, out: &mut HashMap<String, &'a EntityDoc>) {
    let Some(k) = key_of(doc) else {
        return;
    };
    out.insert(k.to_string(), doc);
    for c in &doc.children {
        parts(c, out);
    }
}

/// A copy of `doc` seen from inside the nested prefab at `prefix`: its parts keyed as that
/// file keys them, everything else (added by an outer file or the scene) without a key.
fn namespace(doc: &EntityDoc, prefix: &str) -> EntityDoc {
    let mut out = doc.clone();
    fn walk(doc: &mut EntityDoc, prefix: &str) {
        if let Some(k) = key_of(doc).map(String::from) {
            match strip(prefix, &k) {
                Some(s) => set_key(doc, &s),
                None => {
                    doc.components.remove("PrefabPart");
                }
            }
        }
        for c in &mut doc.children {
            walk(c, prefix);
        }
    }
    walk(&mut out, prefix);
    out
}

/// Put a copy's overrides onto its file's expanded tree.
pub fn apply_overrides(doc: &mut EntityDoc, link: &PrefabInstance) {
    for key in &link.removed {
        remove_part(doc, key);
    }
    for o in &link.overrides {
        if let Some(part) = find_mut(doc, &o.part) {
            set_value(part, &o.component, &o.field, o.value.clone());
        }
    }
}

fn set_value(part: &mut EntityDoc, component: &str, field: &str, value: Value) {
    if component == "Name" {
        part.name = value.as_str().map(String::from);
    } else if field.is_empty() {
        if value.is_null() {
            part.components.remove(component);
        } else {
            part.components.insert(component.into(), value);
        }
    } else if let Some(c) = part.components.get_mut(component) {
        set_pointer(c, field, value);
    }
}

/// Set the value at a JSON pointer, making objects on the way when they are missing.
fn set_pointer(root: &mut Value, pointer: &str, value: Value) -> bool {
    let tokens: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some((last, path)) = tokens.split_last() else {
        *root = value;
        return true;
    };
    let mut cur = root;
    for t in path {
        cur = match cur {
            Value::Object(m) => m.entry(t.clone()).or_insert_with(|| json!({})),
            Value::Array(a) => match t.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                Some(v) => v,
                None => return false,
            },
            _ => return false,
        };
    }
    match cur {
        Value::Object(m) => {
            m.insert(last.clone(), value);
            true
        }
        Value::Array(a) => match last.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        },
        _ => false,
    }
}

/// The value at a JSON pointer of a part (`Name` is its name).
pub fn value_at(part: &EntityDoc, component: &str, field: &str) -> Option<Value> {
    if component == "Name" {
        return Some(json!(part.name));
    }
    let c = part.components.get(component)?;
    if field.is_empty() {
        Some(c.clone())
    } else {
        c.pointer(field).cloned()
    }
}

/// Whether two values match, numbers to float precision (values go through f32 fields).
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-5 * x.abs().max(y.abs()).max(1.0),
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// What `current` changes from `base` (both keyed the same way): overrides and removed parts.
/// The root's Transform and name are the copy's own, never overrides.
pub fn diff(base: &EntityDoc, current: &EntityDoc) -> (Vec<Override>, Vec<String>) {
    let mut now = HashMap::new();
    parts(current, &mut now);
    let mut out = Vec::new();
    let mut removed = Vec::new();
    diff_part(base, &now, true, &mut out, &mut removed);
    (out, removed)
}

fn diff_part(
    base: &EntityDoc,
    now: &HashMap<String, &EntityDoc>,
    root: bool,
    out: &mut Vec<Override>,
    removed: &mut Vec<String>,
) {
    let key = key_of(base).unwrap_or("");
    let Some(cur) = now.get(key) else {
        removed.push(key.to_string());
        return;
    };
    let ov = |component: &str, field: String, value: Value| Override {
        part: key.to_string(),
        component: component.to_string(),
        field,
        value,
    };
    if !root && base.name != cur.name {
        out.push(ov("Name", String::new(), json!(cur.name)));
    }
    let skip = |name: &str| {
        name == "PrefabPart" || name == "PrefabInstance" || (root && name == "Transform")
    };
    for (name, b) in &base.components {
        if skip(name) {
            continue;
        }
        match cur.components.get(name) {
            None => out.push(ov(name, String::new(), Value::Null)),
            Some(c) => leaf_diff(b, c, String::new(), &mut |field, v| {
                out.push(ov(name, field, v))
            }),
        }
    }
    for (name, c) in &cur.components {
        if !skip(name) && !base.components.contains_key(name) {
            out.push(ov(name, String::new(), c.clone()));
        }
    }
    for child in &base.children {
        diff_part(child, now, false, out, removed);
    }
}

/// Changed leaves, as pointers. Objects with the same keys are compared field by field;
/// anything else (arrays, a different enum variant) is one value.
fn leaf_diff(a: &Value, b: &Value, pointer: String, push: &mut dyn FnMut(String, Value)) {
    match (a, b) {
        (Value::Object(x), Value::Object(y))
            if !x.is_empty() && x.len() == y.len() && x.keys().all(|k| y.contains_key(k)) =>
        {
            for (k, av) in x {
                let p = format!("{pointer}/{}", k.replace('~', "~0").replace('/', "~1"));
                leaf_diff(av, &y[k], p, push);
            }
        }
        _ if same(a, b) => {}
        _ => push(pointer, b.clone()),
    }
}

/// A tree ready to save as a file: every nested prefab's (and a variant root's) overrides
/// worked out again from what the tree holds.
pub fn encode(files: &mut PrefabFiles, tree: &EntityDoc) -> EntityDoc {
    let mut out = tree.clone();
    encode_links(files, &mut out);
    out
}

fn encode_links(files: &mut PrefabFiles, doc: &mut EntityDoc) {
    if let Some(mut link) = link_of(doc) {
        let key = key_of(doc).unwrap_or("").to_string();
        if let Ok(base) = files.expand(&link.path) {
            let (overrides, removed) = diff(&base, &namespace(doc, &key));
            link.overrides = overrides;
            link.removed = removed;
            set_link(doc, &link);
        }
    }
    for c in &mut doc.children {
        encode_links(files, c);
    }
}

/// Key a tree for the file whose root is at `prefix` (in the tree's current keys): parts of
/// that file keep their keys there, entities added under it get new ones, a prefab copy added
/// under it becomes a nested prefab, and parts of an outer file are left out. Returns the new
/// key of each entity in the order [`preorder`] visits them (`None`: left out).
fn adopt(tree: &mut EntityDoc, prefix: &str) -> Vec<Option<String>> {
    adopt_with(tree, &|k: &str| strip(prefix, k))
}

/// [`adopt`] with keys mapped by `map` (`None`: leave the entity out).
fn adopt_with(tree: &mut EntityDoc, map: &dyn Fn(&str) -> Option<String>) -> Vec<Option<String>> {
    let mut used = HashSet::new();
    let mut existing = HashSet::new();
    all_keys(tree, &mut existing);
    used.extend(existing.iter().filter_map(|k| map(k)));
    let mut order = vec![Some(String::new())];
    set_key(tree, "");
    let children = std::mem::take(&mut tree.children);
    tree.children = adopt_children(children, map, &mut used, &mut order);
    order
}

fn adopt_children(
    children: Vec<EntityDoc>,
    map: &dyn Fn(&str) -> Option<String>,
    used: &mut HashSet<String>,
    order: &mut Vec<Option<String>>,
) -> Vec<EntityDoc> {
    let mut out = Vec::new();
    for mut c in children {
        let key = key_of(&c).map(String::from).filter(|k| !k.is_empty());
        let added_copy = key.is_none() && link_of(&c).is_some();
        let new = match &key {
            Some(k) => match map(k) {
                Some(new) => new,
                None => {
                    skip(&c, order);
                    continue;
                }
            },
            None => fresh_key(c.name.as_deref(), used),
        };
        set_key(&mut c, &new);
        order.push(Some(new.clone()));
        let grand = std::mem::take(&mut c.children);
        c.children = if added_copy {
            // A prefab copy added here: its parts move under its new key.
            let inner = |k: &str| Some(join(&new, k));
            adopt_children(grand, &inner, used, order)
        } else {
            adopt_children(grand, map, used, order)
        };
        out.push(c);
    }
    out
}

fn skip(doc: &EntityDoc, order: &mut Vec<Option<String>>) {
    order.push(None);
    for c in &doc.children {
        skip(c, order);
    }
}

// ----------------------------------------------------------------------------- the world

/// Whether `entity` is the root of a placed copy (not a prefab nested inside one).
pub fn is_root(world: &World, entity: Entity) -> bool {
    world.get::<PrefabInstance>(entity).is_some()
        && world
            .get::<PrefabPart>(entity)
            .is_none_or(|p| p.key.is_empty())
}

/// The root of the placed copy `entity` came from, and its key there.
pub fn owner(world: &World, entity: Entity) -> Option<(Entity, String)> {
    let key = match world.get::<PrefabPart>(entity) {
        Some(p) if !p.key.is_empty() => p.key.clone(),
        _ => return is_root(world, entity).then(|| (entity, String::new())),
    };
    let mut e = entity;
    loop {
        e = world.get::<ChildOf>(e)?.parent();
        if is_root(world, e) {
            return Some((e, key));
        }
        world.get::<PrefabPart>(e)?;
    }
}

/// Every placed copy's root.
pub fn instance_roots(world: &mut World) -> Vec<Entity> {
    let mut q = world
        .query_filtered::<(Entity, Option<&PrefabPart>), (With<PrefabInstance>, With<InScene>)>();
    q.iter(world)
        .filter(|(_, p)| p.is_none_or(|p| p.key.is_empty()))
        .map(|(e, _)| e)
        .collect()
}

/// Scene children of `entity`, in outliner order.
fn scene_children(world: &World, entity: Entity) -> Vec<Entity> {
    world
        .get::<Children>(entity)
        .map(|c| {
            c.iter()
                .filter(|&k| world.get::<InScene>(k).is_some())
                .collect()
        })
        .unwrap_or_default()
}

/// `entity` and its scene descendants in the order [`scene_ops::save_entity`] saves them.
fn preorder(world: &World, entity: Entity, out: &mut Vec<Entity>) {
    out.push(entity);
    for c in scene_children(world, entity) {
        preorder(world, c, out);
    }
}

/// `entity`'s scene subtree as a document.
pub fn tree(world: &World, entity: Entity) -> EntityDoc {
    fn nest(saved: &SavedEntity) -> EntityDoc {
        let mut doc = saved.doc.clone();
        doc.children = saved.children.iter().map(nest).collect();
        doc
    }
    nest(&scene_ops::save_entity(world, entity))
}

/// The parts of a placed copy by key.
pub fn part_entities(world: &World, root: Entity) -> HashMap<String, Entity> {
    let mut out = HashMap::new();
    out.insert(String::new(), root);
    fn walk(world: &World, e: Entity, out: &mut HashMap<String, Entity>) {
        for c in scene_children(world, e) {
            if let Some(p) = world.get::<PrefabPart>(c).filter(|p| !p.key.is_empty()) {
                out.insert(p.key.clone(), c);
                walk(world, c, out);
            }
        }
    }
    walk(world, root, &mut out);
    out
}

/// Entities added under a placed copy (with their parent), not counting their own children.
pub fn added_entities(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    for (_, part) in part_entities(world, root) {
        for c in scene_children(world, part) {
            if world.get::<PrefabPart>(c).is_none_or(|p| p.key.is_empty()) {
                out.push(c);
            }
        }
    }
    out.sort_by_key(|e| world.get::<scene_ops::SceneOrder>(*e).copied());
    out
}

/// What a placed copy should be: its file plus its overrides, where it stands.
fn target_doc(files: &mut PrefabFiles, world: &World, root: Entity) -> Result<EntityDoc, String> {
    let link = world
        .get::<PrefabInstance>(root)
        .cloned()
        .ok_or("not a prefab copy")?;
    let mut doc = files.expand(&link.path)?;
    apply_overrides(&mut doc, &link);
    let here = snapshot_entity(world, root, false);
    match here.components.get("Transform") {
        Some(t) => doc.components.insert("Transform".into(), t.clone()),
        None => doc.components.remove("Transform"),
    };
    doc.name = here.name;
    set_link(&mut doc, &link);
    set_key(&mut doc, "");
    Ok(doc)
}

/// Refresh a placed copy's overrides from what it holds now.
pub fn refresh(world: &mut World, root: Entity) {
    let Some(link) = world.get::<PrefabInstance>(root).cloned() else {
        return;
    };
    let Ok(base) = with_files(world, |files, _| files.expand(&link.path)) else {
        return;
    };
    let (overrides, removed) = diff(&base, &tree(world, root));
    if (overrides != link.overrides || removed != link.removed)
        && let Some(mut l) = world.get_mut::<PrefabInstance>(root)
    {
        l.overrides = overrides;
        l.removed = removed;
    }
}

pub fn refresh_all(world: &mut World) {
    for root in instance_roots(world) {
        refresh(world, root);
    }
}

/// Rebuild a placed copy in place as its file plus its overrides. Entities keep their ids;
/// returns what changed, already done.
pub fn sync(world: &mut World, root: Entity) -> Vec<Command> {
    let doc = match with_files(world, |files, world| target_doc(files, world, root)) {
        Ok(doc) => doc,
        Err(e) => {
            engine::core::log::warn!("prefab: {e}");
            return Vec::new();
        }
    };
    let mut done = Vec::new();
    sync_entity(world, root, &doc, &mut done);
    done
}

fn sync_entity(world: &mut World, entity: Entity, doc: &EntityDoc, done: &mut Vec<Command>) {
    let current = snapshot_entity(world, entity, false);
    let mut run = |world: &mut World, cmd: Command| match cmd.run_now(world) {
        Ok(_) => done.push(cmd),
        Err(e) => engine::core::log::warn!("prefab: {e}"),
    };
    for (name, value) in &doc.components {
        let before = current.components.get(name);
        if before.is_some_and(|b| same(b, value)) {
            continue;
        }
        run(
            world,
            Command::Component {
                entity,
                name: name.clone(),
                before: before.cloned(),
                after: Some(value.clone()),
            },
        );
    }
    for (name, value) in &current.components {
        if !doc.components.contains_key(name) {
            run(
                world,
                Command::Component {
                    entity,
                    name: name.clone(),
                    before: Some(value.clone()),
                    after: None,
                },
            );
        }
    }
    if current.name != doc.name {
        run(
            world,
            Command::Rename {
                entity,
                before: current.name.clone(),
                after: doc.name.clone(),
            },
        );
    }
    let mut by_key: HashMap<String, Entity> = scene_children(world, entity)
        .into_iter()
        .filter_map(|c| {
            let key = world.get::<PrefabPart>(c)?.key.clone();
            (!key.is_empty()).then_some((key, c))
        })
        .collect();
    for child in &doc.children {
        let key = key_of(child).unwrap_or("");
        match by_key.remove(key) {
            Some(e) => sync_entity(world, e, child, done),
            None => {
                let e = scene_ops::spawn_doc_tree(world, child, Some(entity));
                done.push(Command::Spawned {
                    saved: scene_ops::save_entity(world, e),
                    parent: Some(entity),
                });
            }
        }
    }
    let mut gone: Vec<Entity> = by_key.into_values().collect();
    gone.sort();
    for e in gone {
        let saved = scene_ops::save_entity(world, e);
        world.entity_mut(e).despawn();
        done.push(Command::Despawned {
            saved,
            parent: Some(entity),
        });
    }
}

/// Rebuild every placed copy from its file and overrides.
pub fn sync_all(world: &mut World) -> Vec<Command> {
    let mut done = Vec::new();
    for root in instance_roots(world) {
        if world.get_entity(root).is_ok() {
            done.extend(sync(world, root));
        }
    }
    done
}

/// Give a copy saved by an older editor (parts without keys) the keys of its file, matching
/// children in order.
fn migrate(world: &mut World, root: Entity) {
    let has_keys = part_entities(world, root).len() > 1 || world.get::<PrefabPart>(root).is_some();
    if has_keys {
        return;
    }
    let Some(path) = world.get::<PrefabInstance>(root).map(|l| l.path.clone()) else {
        return;
    };
    let Ok(doc) = with_files(world, |files, _| files.expand(&path)) else {
        return;
    };
    fn walk(world: &mut World, e: Entity, doc: &EntityDoc) {
        let key = key_of(doc).unwrap_or("").to_string();
        world.entity_mut(e).insert(PrefabPart { key });
        for (c, d) in scene_children(world, e).into_iter().zip(&doc.children) {
            walk(world, c, d);
        }
    }
    walk(world, root, &doc);
}

/// After a scene is opened: key older copies and bring every copy up to date with its file.
pub fn after_load(world: &mut World) {
    for root in instance_roots(world) {
        migrate(world, root);
    }
    with_files(world, |files, _| {
        files.stored.clear();
        files.expanded.clear();
    });
    sync_all(world);
}

/// Keep overrides current, and rebuild copies of files changed outside the editor.
pub fn track(world: &mut World) {
    let t = now(world);
    world.resource_scope(|world, mut editor: Mut<Editor>| {
        if editor.is_playing() || t < editor.prefab_track_next {
            return;
        }
        editor.prefab_track_next = t + TRACK_SECS;
        refresh_all(world);
        let changed = with_files(world, |files, _| files.changed_on_disk());
        if changed.is_empty() {
            return;
        }
        let done = sync_all(world);
        refresh_all(world);
        let names: Vec<String> = changed.iter().map(|p| display_name(p)).collect();
        let label = format!("{} changed on disk", names.join(", "));
        if !done.is_empty() {
            editor
                .history
                .record(world, Command::Group(label.clone(), done), t);
        }
        editor.info(t, format!("{label}: copies updated"));
    });
}

// ----------------------------------------------------------------------------- editing

/// Something in a copy that differs from its file.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A changed value (all overrides of a component when `field` is empty and the component
    /// itself isn't overridden, all of a part when `component` is empty too).
    Field {
        part: String,
        component: String,
        field: String,
    },
    /// A part deleted from the copy.
    Removed(String),
    /// An entity added under the copy.
    Added(Entity),
}

impl Item {
    fn covers(&self, o: &Override) -> bool {
        match self {
            Item::Field {
                part,
                component,
                field,
            } => {
                o.part == *part
                    && (component.is_empty()
                        || (o.component == *component
                            && (field.is_empty()
                                || o.field == *field
                                || o.field.starts_with(&format!("{field}/")))))
            }
            _ => false,
        }
    }
}

/// A file an override can be applied to: the copy's own, a variant's base, or a nested prefab
/// (whose root has key `prefix` in the copy).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub path: String,
    pub prefix: String,
}

/// Where the change to `part` (`component`, "" for a whole part) can be applied, the copy's
/// own file first.
pub fn targets(world: &mut World, root: Entity, part: &str, component: &str) -> Vec<Target> {
    let Some(path) = world.get::<PrefabInstance>(root).map(|l| l.path.clone()) else {
        return Vec::new();
    };
    let mut chains = vec![(path, String::new())];
    // Nested prefabs on the way from the root to the part.
    let parts = part_entities(world, root);
    let mut prefix = String::new();
    for step in part.split('/') {
        prefix = join(&prefix, step);
        let Some(&e) = parts.get(&prefix) else {
            break;
        };
        // A nested prefab's place and name (and the nested copy as a whole) are the outer
        // file's.
        let inside = part != prefix || !matches!(component, "Transform" | "Name" | "");
        if let Some(link) = world.get::<PrefabInstance>(e)
            && inside
        {
            chains.push((link.path.clone(), prefix.clone()));
        }
    }
    with_files(world, |files, _| {
        let mut out = Vec::new();
        for (path, prefix) in chains {
            let Some(key) = strip(&prefix, part) else {
                continue;
            };
            // The file, then the bases it is a variant of.
            let mut next = Some(path);
            let mut depth = 0;
            while let Some(path) = next.take()
                && depth <= MAX_DEPTH
            {
                depth += 1;
                if files
                    .expand(&path)
                    .is_ok_and(|d| find(&d, &key).is_some() || key.is_empty())
                {
                    out.push(Target {
                        path: path.clone(),
                        prefix: prefix.clone(),
                    });
                }
                next = files
                    .stored(&path)
                    .ok()
                    .and_then(|d| link_of(&d))
                    .map(|l| l.path);
            }
        }
        out
    })
}

/// Save `tree` (keyed for `path`) to the file at `path` and rebuild every copy. `extra` are
/// changes already made that belong to the same step.
fn save_and_sync(
    world: &mut World,
    editor: &mut Editor,
    label: String,
    writes: Vec<(String, EntityDoc)>,
    mut extra: Vec<Command>,
) {
    let t = now(world);
    let mut done = Vec::new();
    for (path, doc) in writes {
        let doc = with_files(world, |files, _| encode(files, &doc));
        let cmd = Command::PrefabFile {
            path: path.clone(),
            before: std::fs::read_to_string(&path).ok(),
            after: Some(
                SceneDoc {
                    entities: vec![doc],
                }
                .to_json_pretty(),
            ),
        };
        match cmd.run_now(world) {
            Ok(_) => done.push(cmd),
            Err(e) => {
                editor.error(t, format!("Could not save prefab: {e}"));
                return;
            }
        }
    }
    done.append(&mut extra);
    done.extend(sync_all(world));
    editor.selection.retain(|e| world.get_entity(*e).is_ok());
    editor
        .history
        .record(world, Command::Group(label.clone(), done), t);
    refresh_all(world);
    editor.info(t, label);
}

/// Set `PrefabPart` keys on scene entities (in [`preorder`] from `top`) as `adopt` gave them.
fn key_scene(
    world: &mut World,
    top: Entity,
    prefix: &str,
    keys: &[Option<String>],
) -> Vec<Command> {
    let mut order = Vec::new();
    preorder(world, top, &mut order);
    let mut done = Vec::new();
    for (e, key) in order.into_iter().zip(keys) {
        let Some(key) = key else {
            continue;
        };
        let key = join(prefix, key);
        let before = world.get::<PrefabPart>(e).cloned();
        if before.as_ref().is_some_and(|p| p.key == key) {
            continue;
        }
        let cmd = Command::Component {
            entity: e,
            name: "PrefabPart".into(),
            before: before.map(|p| json!({ "key": p.key })),
            after: Some(json!({ "key": key })),
        };
        if cmd.run_now(world).is_ok() {
            done.push(cmd);
        }
    }
    done
}

/// Save `entity` as a new prefab named after it, and make it a copy of that prefab. A part of
/// a prefab being edited (prefab editing mode) becomes a prefab nested in it.
pub fn create(world: &mut World, editor: &mut Editor, entity: Entity) {
    let t = now(world);
    let nested_in = match owner(world, entity) {
        Some((root, key)) if !key.is_empty() => {
            if !editor
                .prefab_mode
                .iter()
                .any(|f| owner(world, f.entity).is_some_and(|o| o.0 == root))
            {
                editor.error(
                    t,
                    "That is part of a prefab: open the prefab (Edit prefab) to make one of its \
                     parts a prefab of its own",
                );
                return;
            }
            Some((root, key))
        }
        _ => None,
    };
    let prefix = nested_in.as_ref().map_or(String::new(), |(_, k)| k.clone());
    let path = free_path(&scene_ops::label(world, entity));
    let mut doc = tree(world, entity);
    // The part's own keys carry over (a nested prefab's parts lose its prefix).
    let keys = adopt_with(&mut doc, &|k: &str| {
        Some(strip(&prefix, k).unwrap_or_else(|| k.into()))
    });
    doc.components.remove("PrefabInstance");
    if nested_in.is_none() {
        // The root keeps its rotation, scale and height but not where it stood, so a placed
        // copy stands on the ground the way it was built.
        zero_ground_position(&mut doc);
    }
    let mut extra = key_scene(world, entity, &prefix, &keys);
    let before = world
        .get::<PrefabInstance>(entity)
        .and_then(|l| serde_json::to_value(l).ok());
    let link = Command::Component {
        entity,
        name: "PrefabInstance".into(),
        before,
        after: Some(json!({ "path": path })),
    };
    if link.run_now(world).is_ok() {
        extra.push(link);
    }
    let mut writes = vec![(path.clone(), doc)];
    if let Some(focus) = editor.prefab_mode.last().map(|f| f.entity)
        && nested_in.is_some()
    {
        // The prefab being edited now nests the new one.
        let (focus_path, _, focus_doc, keyed) = prepare_apply(world, focus);
        extra.extend(keyed);
        writes.push((focus_path, focus_doc));
    }
    save_and_sync(
        world,
        editor,
        format!("Save prefab {}", display_name(&path)),
        writes,
        extra,
    );
}

fn zero_ground_position(doc: &mut EntityDoc) {
    if let Some(t) = doc
        .components
        .get_mut("Transform")
        .and_then(|t| t.get_mut("translation"))
        .and_then(|t| t.as_array_mut())
        && t.len() == 3
    {
        t[0] = 0.0.into();
        t[2] = 0.0.into();
    }
}

/// Place the prefab at `path` at `at` (default: where the view is looking), or under `parent`
/// (an added prefab inside another copy, nested once applied), and select it.
pub fn place(
    world: &mut World,
    editor: &mut Editor,
    path: &str,
    at: Option<DVec3>,
    parent: Option<Entity>,
) -> Option<Entity> {
    let t = now(world);
    let doc = match with_files(world, |files, _| files.expand(path)) {
        Ok(doc) => doc,
        Err(e) => {
            editor.error(t, format!("Could not open prefab: {e}"));
            return None;
        }
    };
    let e = scene_ops::spawn_doc_tree(world, &doc, parent);
    let mut transform = world.get::<Transform>(e).copied().unwrap_or_default();
    if parent.is_some() {
        // At the parent's origin, at the height the prefab was built.
        transform.translation.x = 0.0;
        transform.translation.z = 0.0;
    } else {
        transform.translation += at.unwrap_or_else(|| editor::placement_point(world, editor));
    }
    world.entity_mut(e).insert((
        transform,
        PrefabInstance {
            path: path.into(),
            ..Default::default()
        },
        PrefabPart::default(),
    ));
    let label = format!("Place {}", display_name(path));
    editor.history.record(
        world,
        Command::Spawned {
            saved: scene_ops::save_entity(world, e),
            parent,
        },
        t,
    );
    editor.selection = vec![e];
    editor.info(t, label);
    Some(e)
}

/// The tree of `top` (a copy's root or a nested prefab's) keyed for its file, the keys set on
/// the scene, and the changes that set them.
fn prepare_apply(world: &mut World, top: Entity) -> (String, String, EntityDoc, Vec<Command>) {
    let path = world
        .get::<PrefabInstance>(top)
        .map(|l| l.path.clone())
        .unwrap_or_default();
    let prefix = world
        .get::<PrefabPart>(top)
        .map(|p| p.key.clone())
        .unwrap_or_default();
    let mut doc = tree(world, top);
    let keys = adopt(&mut doc, &prefix);
    let done = key_scene(world, top, &prefix, &keys);
    // The root as the file has it (where a new copy stands, its name), and a variant keeps
    // its base.
    let stored = with_files(world, |files, _| files.stored(&path).ok());
    match stored.as_ref().and_then(link_of) {
        Some(base) => set_link(
            &mut doc,
            &PrefabInstance {
                path: base.path,
                ..Default::default()
            },
        ),
        None => {
            doc.components.remove("PrefabInstance");
        }
    }
    if let Some(stored) = stored {
        match stored.components.get("Transform") {
            Some(t) => doc.components.insert("Transform".into(), t.clone()),
            None => doc.components.remove("Transform"),
        };
        doc.name = stored.name;
    }
    (path, prefix, doc, done)
}

/// Write everything a copy (or a nested prefab in it) changes to its file, and update every
/// other copy (Unity's Apply All).
pub fn apply_all(world: &mut World, editor: &mut Editor, top: Entity) {
    refresh_all(world);
    let (path, _, doc, extra) = prepare_apply(world, top);
    if path.is_empty() {
        return;
    }
    save_and_sync(
        world,
        editor,
        format!("Apply to {}", display_name(&path)),
        vec![(path, doc)],
        extra,
    );
}

/// Write one change of the copy at `root` to `target`'s file.
pub fn apply_item(
    world: &mut World,
    editor: &mut Editor,
    root: Entity,
    item: &Item,
    target: &Target,
) {
    let t = now(world);
    refresh_all(world);
    let Some(link) = world.get::<PrefabInstance>(root).cloned() else {
        return;
    };
    let mut doc = match with_files(world, |files, _| files.expand(&target.path)) {
        Ok(doc) => doc,
        Err(e) => {
            editor.error(t, e);
            return;
        }
    };
    let mut extra = Vec::new();
    let what = match item {
        Item::Field { .. } => {
            let mut patch = PrefabInstance::default();
            for o in link.overrides.iter().filter(|o| item.covers(o)) {
                if let Some(part) = strip(&target.prefix, &o.part) {
                    patch.overrides.push(Override { part, ..o.clone() });
                }
            }
            apply_overrides(&mut doc, &patch);
            "change"
        }
        Item::Removed(key) => {
            if let Some(k) = strip(&target.prefix, key) {
                remove_part(&mut doc, &k);
            }
            "removal"
        }
        Item::Added(e) => {
            let Some(parent) = world.get::<ChildOf>(*e).map(|p| p.parent()) else {
                return;
            };
            let parent_key = world
                .get::<PrefabPart>(parent)
                .map(|p| p.key.clone())
                .unwrap_or_default();
            let Some(parent_key) = strip(&target.prefix, &parent_key) else {
                return;
            };
            let mut used = HashSet::new();
            all_keys(&doc, &mut used);
            let mut order = Vec::new();
            let own = |k: &str| strip(&target.prefix, k);
            let added = adopt_children(vec![tree(world, *e)], &own, &mut used, &mut order);
            extra = key_scene(world, *e, &target.prefix, &order);
            match find_mut(&mut doc, &parent_key) {
                Some(p) => p.children.extend(added),
                None => doc.children.extend(added),
            }
            "added entity"
        }
    };
    save_and_sync(
        world,
        editor,
        format!("Apply {what} to {}", display_name(&target.path)),
        vec![(target.path.clone(), doc)],
        extra,
    );
}

/// Throw away one change of the copy at `root` (or all of them with `None`).
pub fn revert(world: &mut World, editor: &mut Editor, root: Entity, item: Option<&Item>) {
    let t = now(world);
    refresh(world, root);
    let Some(link) = world.get::<PrefabInstance>(root).cloned() else {
        return;
    };
    let mut done = Vec::new();
    let mut new = link.clone();
    match item {
        Some(Item::Added(e)) => {
            let parent = world.get::<ChildOf>(*e).map(|p| p.parent());
            let saved = scene_ops::save_entity(world, *e);
            world.entity_mut(*e).despawn();
            done.push(Command::Despawned { saved, parent });
        }
        Some(Item::Removed(key)) => new.removed.retain(|k| k != key),
        Some(item) => new.overrides.retain(|o| !item.covers(o)),
        None => {
            new.overrides.clear();
            new.removed.clear();
            for e in added_entities(world, root) {
                let parent = world.get::<ChildOf>(e).map(|p| p.parent());
                let saved = scene_ops::save_entity(world, e);
                world.entity_mut(e).despawn();
                done.push(Command::Despawned { saved, parent });
            }
        }
    }
    if new != link {
        let cmd = Command::Component {
            entity: root,
            name: "PrefabInstance".into(),
            before: serde_json::to_value(&link).ok(),
            after: serde_json::to_value(&new).ok(),
        };
        if cmd.run_now(world).is_ok() {
            done.push(cmd);
        }
    }
    done.extend(sync(world, root));
    editor.selection.retain(|e| world.get_entity(*e).is_ok());
    let label = format!(
        "Revert {} to {}",
        if item.is_some() {
            "a change"
        } else {
            "everything"
        },
        display_name(&link.path)
    );
    editor
        .history
        .record(world, Command::Group(label.clone(), done), t);
    refresh(world, root);
    editor.info(t, label);
}

/// Save the copy at `root` as a variant of its prefab (its overrides become the variant's)
/// and make it a copy of the variant.
pub fn save_variant(world: &mut World, editor: &mut Editor, root: Entity) {
    refresh(world, root);
    let Some(link) = world.get::<PrefabInstance>(root).cloned() else {
        return;
    };
    let name = format!("{} variant", scene_ops::label(world, root));
    let path = free_path(&name);
    let mut doc = tree(world, root);
    let keys = adopt(&mut doc, "");
    let mut extra = key_scene(world, root, "", &keys);
    set_link(
        &mut doc,
        &PrefabInstance {
            path: link.path.clone(),
            ..Default::default()
        },
    );
    let stored = with_files(world, |files, _| files.stored(&link.path).ok());
    if let Some(t) = stored.as_ref().and_then(|s| s.components.get("Transform")) {
        doc.components.insert("Transform".into(), t.clone());
    }
    doc.name = Some(name);
    let cmd = Command::Component {
        entity: root,
        name: "PrefabInstance".into(),
        before: serde_json::to_value(&link).ok(),
        after: Some(json!({ "path": path })),
    };
    if cmd.run_now(world).is_ok() {
        extra.push(cmd);
    }
    save_and_sync(
        world,
        editor,
        format!("Save variant {}", display_name(&path)),
        vec![(path, doc)],
        extra,
    );
}

/// Make a copy plain entities again, nested prefabs included (Unity's Unpack Completely).
pub fn unpack(world: &mut World, editor: &mut Editor, root: Entity) {
    let t = now(world);
    let mut order = Vec::new();
    preorder(world, root, &mut order);
    let mut done = Vec::new();
    for e in order {
        for name in ["PrefabInstance", "PrefabPart"] {
            let value = match name {
                "PrefabInstance" => world
                    .get::<PrefabInstance>(e)
                    .and_then(|l| serde_json::to_value(l).ok()),
                _ => world
                    .get::<PrefabPart>(e)
                    .and_then(|p| serde_json::to_value(p).ok()),
            };
            if let Some(before) = value {
                let cmd = Command::Component {
                    entity: e,
                    name: name.into(),
                    before: Some(before),
                    after: None,
                };
                if cmd.run_now(world).is_ok() {
                    done.push(cmd);
                }
            }
        }
    }
    let label = format!("Unpack {}", scene_ops::label(world, root));
    editor
        .history
        .record(world, Command::Group(label.clone(), done), t);
    editor.info(t, label);
}

// ----------------------------------------------------------------------------- editing mode

/// A prefab open for editing in the scene (Unity's prefab mode, in context): the rest of the
/// scene stays visible but can't be selected, the copy shows its file without this copy's
/// overrides, and leaving saves it to the file.
#[derive(Debug, Clone)]
pub struct Focus {
    /// The copy's root, or a nested prefab's root inside it.
    pub entity: Entity,
    pub path: String,
    /// The copy's overrides (and entities added to it) put aside while editing.
    stash: Vec<Override>,
    stash_removed: Vec<String>,
    stash_added: Vec<(String, SavedEntity)>,
}

/// Open the prefab of `entity` (a copy's root or a nested prefab in it) for editing.
pub fn enter(world: &mut World, editor: &mut Editor, entity: Entity) {
    let t = now(world);
    let Some((root, prefix)) = owner(world, entity) else {
        return;
    };
    let Some(path) = world.get::<PrefabInstance>(entity).map(|l| l.path.clone()) else {
        return;
    };
    refresh(world, root);
    let Some(link) = world.get::<PrefabInstance>(root).cloned() else {
        return;
    };
    // This copy's changes inside the prefab being opened (a nested prefab's place and name
    // belong to the outer one).
    let mine = |key: &str| strip(&prefix, key).is_some();
    let mut new = link.clone();
    let stash: Vec<Override> = link
        .overrides
        .iter()
        .filter(|o| {
            mine(&o.part)
                && !(!prefix.is_empty()
                    && o.part == prefix
                    && matches!(o.component.as_str(), "Transform" | "Name"))
        })
        .cloned()
        .collect();
    new.overrides.retain(|o| !stash.contains(o));
    let stash_removed: Vec<String> = link
        .removed
        .iter()
        .filter(|k| mine(k) && **k != prefix)
        .cloned()
        .collect();
    new.removed.retain(|k| !stash_removed.contains(k));
    let mut done = Vec::new();
    // Entities added to this copy aren't part of the prefab: put them aside.
    let mut stash_added = Vec::new();
    for e in added_entities(world, root) {
        let Some(parent) = world.get::<ChildOf>(e).map(|p| p.parent()) else {
            continue;
        };
        let key = world
            .get::<PrefabPart>(parent)
            .map(|p| p.key.clone())
            .unwrap_or_default();
        if !mine(&key) {
            continue;
        }
        let saved = scene_ops::save_entity(world, e);
        world.entity_mut(e).despawn();
        stash_added.push((key, saved.clone()));
        done.push(Command::Despawned {
            saved,
            parent: Some(parent),
        });
    }
    if new != link {
        let cmd = Command::Component {
            entity: root,
            name: "PrefabInstance".into(),
            before: serde_json::to_value(&link).ok(),
            after: serde_json::to_value(&new).ok(),
        };
        if cmd.run_now(world).is_ok() {
            done.push(cmd);
        }
        done.extend(sync(world, root));
    }
    let label = format!("Edit prefab {}", display_name(&path));
    editor
        .history
        .record(world, Command::Group(label.clone(), done), t);
    editor.prefab_mode.push(Focus {
        entity,
        path,
        stash,
        stash_removed,
        stash_added,
    });
    editor.selection = vec![entity];
    editor.info(t, label);
}

/// Save the prefab being edited to its file.
pub fn save_focus(world: &mut World, editor: &mut Editor) {
    let Some(focus) = editor.prefab_mode.last().cloned() else {
        return;
    };
    if world.get_entity(focus.entity).is_err() {
        return;
    }
    if focus_changed(world, &focus) {
        apply_all(world, editor, focus.entity);
    }
}

/// Whether the prefab being edited differs from its file.
pub fn focus_changed(world: &mut World, focus: &Focus) -> bool {
    let prefix = world
        .get::<PrefabPart>(focus.entity)
        .map(|p| p.key.clone())
        .unwrap_or_default();
    let Ok(base) = with_files(world, |files, _| files.expand(&focus.path)) else {
        return false;
    };
    let here = namespace(&tree(world, focus.entity), &prefix);
    let (o, r) = diff(&base, &here);
    let mut any_added = false;
    fn added(doc: &EntityDoc, any: &mut bool) {
        for c in &doc.children {
            if key_of(c).is_none() {
                *any = true;
            }
            added(c, any);
        }
    }
    added(&here, &mut any_added);
    !o.is_empty() || !r.is_empty() || any_added
}

/// Leave prefab editing: save, then give the copy its overrides back.
pub fn exit(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    save_focus(world, editor);
    let Some(focus) = editor.prefab_mode.pop() else {
        return;
    };
    let Some((root, _)) = world
        .get_entity(focus.entity)
        .ok()
        .and_then(|_| owner(world, focus.entity))
    else {
        return;
    };
    refresh(world, root);
    let mut done = Vec::new();
    if let Some(link) = world.get::<PrefabInstance>(root).cloned() {
        let mut new = link.clone();
        new.overrides.extend(focus.stash.iter().cloned());
        new.removed.extend(focus.stash_removed.iter().cloned());
        if new != link {
            let cmd = Command::Component {
                entity: root,
                name: "PrefabInstance".into(),
                before: serde_json::to_value(&link).ok(),
                after: serde_json::to_value(&new).ok(),
            };
            if cmd.run_now(world).is_ok() {
                done.push(cmd);
            }
            done.extend(sync(world, root));
        }
    }
    let parts = part_entities(world, root);
    for (key, saved) in &focus.stash_added {
        let parent = parts.get(key).copied().unwrap_or(root);
        let mut remap = scene_ops::Remap::new();
        let e = scene_ops::restore_entity(world, saved, Some(parent), &mut remap);
        editor.history.remap_all(&remap);
        done.push(Command::Spawned {
            saved: scene_ops::save_entity(world, e),
            parent: Some(parent),
        });
    }
    let label = format!("Close prefab {}", display_name(&focus.path));
    if !done.is_empty() {
        editor
            .history
            .record(world, Command::Group(label.clone(), done), t);
    }
    refresh(world, root);
    editor.selection = vec![focus.entity];
    editor.info(t, label);
}

/// Leave every open prefab (before saving the scene, playing or opening another scene).
pub fn exit_all(world: &mut World, editor: &mut Editor) {
    while !editor.prefab_mode.is_empty() {
        exit(world, editor);
    }
}

/// Whether `entity` can be selected: everything, or while editing a prefab only its entities.
pub fn in_focus(world: &World, editor: &Editor, entity: Entity) -> bool {
    match editor.prefab_mode.last() {
        Some(f) => scene_ops::is_ancestor(world, f.entity, entity),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, key: &str, components: Value, children: Vec<EntityDoc>) -> EntityDoc {
        let mut doc = EntityDoc {
            name: Some(name.into()),
            components: components.as_object().cloned().unwrap_or_default(),
            children,
        };
        set_key(&mut doc, key);
        doc
    }

    fn write(dir: &std::path::Path, name: &str, doc: EntityDoc) -> String {
        let path = dir.join(name).to_string_lossy().replace('\\', "/");
        SceneDoc {
            entities: vec![doc],
        }
        .save(&path)
        .unwrap();
        path
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lamp() -> EntityDoc {
        part(
            "Lamp post",
            "",
            json!({"Transform": {"translation": [0.0, 1.5, 0.0]}}),
            vec![part(
                "Lamp",
                "lamp",
                json!({"PointLight": {"color": [1.0, 0.9, 0.7], "intensity": 800.0}}),
                vec![],
            )],
        )
    }

    #[test]
    fn diff_and_apply_round_trip() {
        let base = lamp();
        let mut here = base.clone();
        here.children[0].components["PointLight"]["color"] = json!([0.3, 0.6, 1.0]);
        here.children[0].name = Some("Blue lamp".into());
        here.components
            .insert("Transform".into(), json!({"translation": [5.0, 1.5, 0.0]}));
        here.components
            .insert("Tag".into(), json!({"kind": "street"}));
        let (overrides, removed) = diff(&base, &here);
        assert!(removed.is_empty());
        let fields: Vec<(&str, &str, &str)> = overrides
            .iter()
            .map(|o| (o.part.as_str(), o.component.as_str(), o.field.as_str()))
            .collect();
        assert_eq!(
            fields,
            [
                ("", "Tag", ""),
                ("lamp", "Name", ""),
                ("lamp", "PointLight", "/color")
            ],
            "the root's Transform is where the copy stands, not an override"
        );
        let mut rebuilt = base.clone();
        apply_overrides(
            &mut rebuilt,
            &PrefabInstance {
                overrides,
                ..Default::default()
            },
        );
        let v = |d: &EntityDoc| serde_json::to_value(d).unwrap();
        assert_eq!(v(&rebuilt.children[0]), v(&here.children[0]));
        assert_eq!(rebuilt.components.get("Tag"), here.components.get("Tag"));

        // A removed part, and float noise that is not a change.
        let mut gone = base.clone();
        gone.children.clear();
        assert_eq!(diff(&base, &gone).1, ["lamp"]);
        let mut noisy = base.clone();
        noisy.children[0].components["PointLight"]["intensity"] = json!(800.000001);
        assert!(diff(&base, &noisy).0.is_empty());
    }

    #[test]
    fn nested_prefabs_and_variants_follow_their_files() {
        let dir = temp_dir("nested");
        let lamp_path = write(&dir, "lamp.prefab.json", lamp());
        // A street row nests two lamp posts; the second one is overridden red in the row.
        let nested = |key: &str, x: f64, red: bool| {
            let mut doc = lamp();
            rekey(&mut doc, key);
            doc.components
                .insert("Transform".into(), json!({"translation": [x, 0.0, 0.0]}));
            let mut link = PrefabInstance {
                path: lamp_path.clone(),
                ..Default::default()
            };
            if red {
                link.overrides.push(Override {
                    part: "lamp".into(),
                    component: "PointLight".into(),
                    field: "/color".into(),
                    value: json!([1.0, 0.0, 0.0]),
                });
            }
            set_link(&mut doc, &link);
            doc
        };
        let row = part(
            "Row",
            "",
            json!({}),
            vec![
                nested("a", 0.0, false),
                nested("b", 4.0, true),
                part("Bench", "bench", json!({}), vec![]),
            ],
        );
        let row_path = write(&dir, "row.prefab.json", row);
        let mut files = PrefabFiles::default();
        let expanded = files.expand(&row_path).unwrap();
        let color = |doc: &EntityDoc, key: &str| {
            find(doc, key).unwrap().components["PointLight"]["color"].clone()
        };
        assert_eq!(color(&expanded, "b/lamp"), json!([1.0, 0.0, 0.0]));

        // The lamp post's file changes: both nested posts get the new intensity, the red one
        // stays red, and the row's bench is still there.
        let mut brighter = lamp();
        brighter.children[0].components["PointLight"]["intensity"] = json!(2000.0);
        brighter.children[0]
            .children
            .push(part("Bulb", "bulb", json!({}), vec![]));
        let text = SceneDoc {
            entities: vec![brighter],
        }
        .to_json_pretty();
        files.write(&lamp_path, Some(&text)).unwrap();
        let expanded = files.expand(&row_path).unwrap();
        for key in ["a/lamp", "b/lamp"] {
            assert_eq!(
                find(&expanded, key).unwrap().components["PointLight"]["intensity"],
                json!(2000.0)
            );
        }
        assert!(find(&expanded, "b/bulb").is_some());
        assert_eq!(color(&expanded, "b/lamp"), json!([1.0, 0.0, 0.0]));
        assert_eq!(color(&expanded, "a/lamp"), json!([1.0, 0.9, 0.7]));
        assert!(find(&expanded, "bench").is_some());
        assert_eq!(
            find(&expanded, "b").unwrap().components["Transform"]["translation"],
            json!([4.0, 0.0, 0.0]),
            "a nested post stays where the row put it"
        );

        // A variant of the lamp post: dim and green, plus a sign. It follows its base.
        let mut variant = files.expand(&lamp_path).unwrap();
        variant
            .children
            .push(part("Sign", "sign", json!({}), vec![]));
        set_link(
            &mut variant,
            &PrefabInstance {
                path: lamp_path.clone(),
                ..Default::default()
            },
        );
        variant.children[0].components["PointLight"]["color"] = json!([0.0, 1.0, 0.0]);
        let variant = encode(&mut files, &variant);
        let link = link_of(&variant).unwrap();
        assert_eq!(
            link.overrides.len(),
            1,
            "only the color differs from the base"
        );
        let variant_path = write(&dir, "green.prefab.json", variant);
        let green = files.expand(&variant_path).unwrap();
        assert_eq!(color(&green, "lamp"), json!([0.0, 1.0, 0.0]));
        assert!(find(&green, "sign").is_some() && find(&green, "bulb").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adopt_keys_added_entities_and_copies() {
        let mut copy = lamp();
        copy.children.push(part("Lid", "", json!({}), vec![]));
        copy.children[1].components.remove("PrefabPart");
        let mut inner = lamp();
        set_link(
            &mut inner,
            &PrefabInstance {
                path: "x".into(),
                ..Default::default()
            },
        );
        copy.children.push(inner);
        let keys = adopt(&mut copy, "");
        assert_eq!(
            keys,
            [
                Some(String::new()),
                Some("lamp".into()),
                Some("lid".into()),
                Some("lamp_post".into()),
                Some("lamp_post/lamp".into())
            ]
        );
        // Seen from a nested prefab, outer parts are left out.
        let mut row = part("Row", "", json!({}), vec![]);
        let mut a = lamp();
        rekey(&mut a, "a");
        a.children.push(part("Flag", "flag", json!({}), vec![]));
        row.children.push(a);
        let mut a = row.children[0].clone();
        let keys = adopt(&mut a, "a");
        assert_eq!(keys, [Some(String::new()), Some("lamp".into()), None]);
        assert_eq!(a.children.len(), 1);
    }

    /// The scene side: copies follow applies, keep their own changes, take added entities,
    /// revert, undo, and nested copies follow the file they nest.
    #[test]
    fn copies_in_the_scene_follow_their_files() {
        let dir = temp_dir("scene");
        let post = part(
            "Lamp post",
            "",
            json!({"Transform": {"translation": [0.0, 1.5, 0.0]}}),
            vec![part(
                "Lamp",
                "lamp",
                json!({"Transform": {"translation": [0.0, 2.0, 0.0]}}),
                vec![],
            )],
        );
        let post_path = write(&dir, "post.prefab.json", post);
        let mut app = App::new();
        app.register_component::<PrefabInstance>("test")
            .register_component::<PrefabPart>("test");
        app.update();
        let world = &mut app.world;
        let camera = world.spawn_empty().id();
        let mut editor = Editor::new(camera);
        let a = place(world, &mut editor, &post_path, Some(DVec3::ZERO), None).unwrap();
        let b = place(world, &mut editor, &post_path, Some(DVec3::X * 5.0), None).unwrap();
        let lamp = |world: &World, root: Entity| part_entities(world, root)["lamp"];
        let scale = |world: &World, root: Entity| {
            world.get::<Transform>(lamp(world, root)).unwrap().scale.x
        };

        // A bigger lamp on A is an override; applying it reaches B.
        world.get_mut::<Transform>(lamp(world, a)).unwrap().scale = Vec3::splat(2.0);
        refresh_all(world);
        let link = world.get::<PrefabInstance>(a).unwrap().clone();
        assert_eq!(link.overrides.len(), 1);
        assert_eq!(link.overrides[0].field, "/scale");
        let item = Item::Field {
            part: "lamp".into(),
            component: "Transform".into(),
            field: "/scale".into(),
        };
        let target = targets(world, a, "lamp", "Transform").remove(0);
        apply_item(world, &mut editor, a, &item, &target);
        assert_eq!(scale(world, b), 2.0);
        assert!(world.get::<PrefabInstance>(a).unwrap().overrides.is_empty());
        assert_eq!(
            world.get::<Transform>(b).unwrap().translation.x,
            5.0,
            "B stays where it stands"
        );

        // B moves its lamp (its own change), then A's rename is applied: B gets the name and
        // keeps its lamp where it put it.
        world
            .get_mut::<Transform>(lamp(world, b))
            .unwrap()
            .translation
            .y = 3.0;
        refresh_all(world);
        world.entity_mut(lamp(world, a)).insert(Name::new("Bulb"));
        apply_all(world, &mut editor, a);
        let b_lamp = lamp(world, b);
        assert_eq!(world.get::<Name>(b_lamp).unwrap().as_str(), "Bulb");
        assert_eq!(world.get::<Transform>(b_lamp).unwrap().translation.y, 3.0);

        // A sign added to A, applied: B gets one, as a part.
        let sign = world
            .spawn((InScene, Name::new("Sign"), Transform::default(), ChildOf(a)))
            .id();
        assert_eq!(added_entities(world, a), [sign]);
        apply_item(world, &mut editor, a, &Item::Added(sign), &target);
        assert_eq!(world.get::<PrefabPart>(sign).unwrap().key, "sign");
        assert!(part_entities(world, b).contains_key("sign"));

        // Undo the sign: B loses it and the file is as it was.
        let text = std::fs::read_to_string(&post_path).unwrap();
        editor::undo(world, &mut editor);
        assert!(!part_entities(world, b).contains_key("sign"));
        assert!(std::fs::read_to_string(&post_path).unwrap().len() < text.len());

        // Revert everything on B: its lamp goes back to the file's height.
        revert(world, &mut editor, b, None);
        assert_eq!(
            world
                .get::<Transform>(lamp(world, b))
                .unwrap()
                .translation
                .y,
            2.0
        );

        // A row nesting the post: changing the post through a scene copy reaches the row's.
        let mut nested_post = with_files(world, |f, _| f.expand(&post_path)).unwrap();
        rekey(&mut nested_post, "p");
        set_link(
            &mut nested_post,
            &PrefabInstance {
                path: post_path.clone(),
                ..Default::default()
            },
        );
        let row_path = write(
            &dir,
            "row.prefab.json",
            part("Row", "", json!({}), vec![nested_post]),
        );
        let row = place(world, &mut editor, &row_path, Some(DVec3::Z * 9.0), None).unwrap();
        let row_lamp = part_entities(world, row)["p/lamp"];
        assert_eq!(world.get::<Transform>(row_lamp).unwrap().scale.x, 2.0);
        world.get_mut::<Transform>(lamp(world, a)).unwrap().scale = Vec3::splat(3.0);
        apply_all(world, &mut editor, a);
        assert_eq!(world.get::<Transform>(row_lamp).unwrap().scale.x, 3.0);
        assert_eq!(scale(world, b), 3.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_paths_and_keys() {
        assert_eq!(path_for("Lamp post"), "prefabs/lamp_post.prefab.json");
        assert_eq!(display_name("prefabs/lamp_post.prefab.json"), "Lamp post");
        assert_eq!(path_for("  "), "prefabs/prefab.prefab.json");
        assert_eq!(join("a", "b"), "a/b");
        assert_eq!(join("", "b"), "b");
        assert_eq!(join("a", ""), "a");
        assert_eq!(strip("a", "a/b").as_deref(), Some("b"));
        assert_eq!(strip("a", "a").as_deref(), Some(""));
        assert_eq!(strip("a", "ab"), None);
        let mut v = json!({"a": [1, 2]});
        assert!(set_pointer(&mut v, "/a/1", json!(5)));
        assert!(set_pointer(&mut v, "/b/c", json!(true)));
        assert_eq!(v, json!({"a": [1, 5], "b": {"c": true}}));
    }
}
