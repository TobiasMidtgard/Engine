//! What the Content Browser shows and does to files: assets found under the asset root, the
//! prefab folder and the scene folder, what kind each is, where each is used, and renaming,
//! moving, duplicating and deleting them without breaking what uses them.
//!
//! As Unreal's Content Browser and Unity's Project window do, a rename or move rewrites every
//! reference to the asset (scene and prefab files on disk, import settings, and the open
//! scene's entities), a delete first says what still uses the asset, and deleted files go to
//! a trash folder (`.editor/trash/`) instead of being lost.

use std::path::{Path, PathBuf};

use engine::prelude::*;
use serde_json::Value;

use crate::scene_ops::InScene;

/// Where the editor keeps its own files (favorites, the trash), next to `scenes/`.
pub const EDITOR_DIR: &str = ".editor";

/// The folder an asset lives under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Root {
    /// The asset server's root (models, materials, textures, sounds).
    Assets,
    Prefabs,
    Scenes,
}

/// A file or folder the browser shows: a root and a path inside it ("" is the root itself).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Asset {
    pub root: Root,
    pub rel: String,
}

/// What an asset is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Folder,
    Model,
    Material,
    Texture,
    Prefab,
    Scene,
    Audio,
    Other,
}

impl Kind {
    /// Kinds the type filter offers, with their names.
    pub const FILTERS: [(Kind, &'static str); 6] = [
        (Kind::Model, "Models"),
        (Kind::Material, "Materials"),
        (Kind::Texture, "Textures"),
        (Kind::Prefab, "Prefabs"),
        (Kind::Scene, "Scenes"),
        (Kind::Audio, "Sounds"),
    ];

    /// The word for `t:` in a search.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Model => "model",
            Kind::Material => "material",
            Kind::Texture => "texture",
            Kind::Prefab => "prefab",
            Kind::Scene => "scene",
            Kind::Audio => "sound",
            Kind::Other => "file",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Folder => "📁",
            Kind::Model => "🎲",
            Kind::Material => "🎨",
            Kind::Texture => "🖼",
            Kind::Prefab => "📦",
            Kind::Scene => "🎬",
            Kind::Audio => "🔊",
            Kind::Other => "📄",
        }
    }

    /// The strip under a tile (Unreal colors its tiles by asset type).
    pub fn color(self) -> egui::Color32 {
        match self {
            Kind::Folder => egui::Color32::from_rgb(0xc8, 0xa2, 0x4a),
            Kind::Model => egui::Color32::from_rgb(0x3d, 0xc8, 0xd8),
            Kind::Material => egui::Color32::from_rgb(0x6c, 0xc0, 0x4a),
            Kind::Texture => egui::Color32::from_rgb(0xd0, 0x4a, 0x4a),
            Kind::Prefab => crate::prefab_ui::PREFAB,
            Kind::Scene => egui::Color32::from_rgb(0xe0, 0x8a, 0x3a),
            Kind::Audio => egui::Color32::from_rgb(0xb0, 0x6c, 0xe0),
            Kind::Other => egui::Color32::from_rgb(0x80, 0x80, 0x80),
        }
    }
}

/// The kind of a file by its name (folders are found by the caller).
pub fn kind_of(name: &str) -> Kind {
    let lower = name.to_ascii_lowercase();
    if crate::materials::is_material(&lower) {
        Kind::Material
    } else if lower.ends_with(".glb")
        || lower.ends_with(".gltf")
        || lower.ends_with(".fbx")
        || lower.ends_with(".obj")
    {
        Kind::Model
    } else if lower.ends_with(".prefab.json") {
        Kind::Prefab
    } else if lower.ends_with(".scene.json") {
        Kind::Scene
    } else if [".png", ".jpg", ".jpeg", ".ktx2", ".tga", ".exr", ".hdr"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        Kind::Texture
    } else if [".wav", ".ogg", ".flac", ".mp3"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        Kind::Audio
    } else {
        Kind::Other
    }
}

/// Files the browser leaves out: the editor's sidecars and a glTF's buffers.
fn hidden(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with('.') || lower.ends_with(".import.json") || lower.ends_with(".bin")
}

impl Asset {
    pub fn new(root: Root, rel: impl Into<String>) -> Self {
        Self {
            root,
            rel: rel.into(),
        }
    }

    /// The folder this root is on disk.
    pub fn root_dir(root: Root, assets: &Path) -> PathBuf {
        match root {
            Root::Assets => assets.to_path_buf(),
            Root::Prefabs => PathBuf::from(crate::prefab::PREFABS_DIR),
            Root::Scenes => PathBuf::from(crate::editor::SCENES_DIR),
        }
    }

    pub fn path(&self, assets: &Path) -> PathBuf {
        let dir = Self::root_dir(self.root, assets);
        if self.rel.is_empty() {
            dir
        } else {
            dir.join(&self.rel)
        }
    }

    /// How scenes and prefabs name this asset: an asset key, or a prefab or scene path.
    pub fn key(&self) -> String {
        match self.root {
            Root::Assets => self.rel.clone(),
            Root::Prefabs => format!("{}/{}", crate::prefab::PREFABS_DIR, self.rel),
            Root::Scenes => format!("{}/{}", crate::editor::SCENES_DIR, self.rel),
        }
    }

    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// The name without its extension(s), as tiles show it.
    pub fn title(&self) -> String {
        let name = self.name();
        for ext in [".prefab.json", ".scene.json", ".material.gltf"] {
            if let Some(stem) = name.strip_suffix(ext) {
                return stem.replace('_', " ");
            }
        }
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        if stem.is_empty() {
            return root_title(self.root).into();
        }
        stem.replace('_', " ")
    }

    pub fn parent(&self) -> Asset {
        Asset::new(self.root, self.rel.rsplit_once('/').map_or("", |(d, _)| d))
    }

    pub fn child(&self, name: &str) -> Asset {
        Asset::new(
            self.root,
            if self.rel.is_empty() {
                name.to_string()
            } else {
                format!("{}/{name}", self.rel)
            },
        )
    }

    pub fn kind(&self, assets: &Path) -> Kind {
        if self.rel.is_empty() || self.path(assets).is_dir() {
            Kind::Folder
        } else {
            kind_of(self.name())
        }
    }
}

pub fn root_title(root: Root) -> &'static str {
    match root {
        Root::Assets => "Assets",
        Root::Prefabs => "Prefabs",
        Root::Scenes => "Scenes",
    }
}

/// A folder's contents: folders first, then files, by name.
pub fn list(folder: &Asset, assets: &Path) -> Vec<Asset> {
    let Ok(entries) = std::fs::read_dir(folder.path(assets)) else {
        return Vec::new();
    };
    let mut out: Vec<(bool, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (!hidden(&name)).then(|| (!e.path().is_dir(), name))
        })
        .collect();
    out.sort_by_key(|a| (a.0, a.1.to_lowercase()));
    out.into_iter().map(|(_, n)| folder.child(&n)).collect()
}

/// Every file under every root (for searching), with its kind.
pub fn index(assets: &Path) -> Vec<(Asset, Kind)> {
    fn walk(dir: &Path, folder: &Asset, depth: usize, out: &mut Vec<(Asset, Kind)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if hidden(&name) {
                continue;
            }
            let asset = folder.child(&name);
            if e.path().is_dir() {
                if depth < 12 {
                    walk(&e.path(), &asset, depth + 1, out);
                }
            } else {
                out.push((asset, kind_of(&name)));
            }
        }
    }
    let mut out = Vec::new();
    for root in [Root::Assets, Root::Prefabs, Root::Scenes] {
        let top = Asset::new(root, "");
        walk(&top.path(assets), &top, 0, &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A search: words that must all be in the path, and `t:kind` words naming the kinds wanted.
pub struct Query {
    words: Vec<String>,
    pub kinds: Vec<Kind>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        let mut words = Vec::new();
        let mut kinds = Vec::new();
        for w in text.split_whitespace() {
            let w = w.to_lowercase();
            match w.strip_prefix("t:") {
                Some(t) => {
                    let all = [
                        Kind::Folder,
                        Kind::Model,
                        Kind::Material,
                        Kind::Texture,
                        Kind::Prefab,
                        Kind::Scene,
                        Kind::Audio,
                        Kind::Other,
                    ];
                    kinds.extend(
                        all.into_iter()
                            .filter(|k| k.word().starts_with(t) && !t.is_empty()),
                    );
                }
                None => words.push(w),
            }
        }
        Self { words, kinds }
    }

    pub fn matches(&self, asset: &Asset, kind: Kind) -> bool {
        let key = asset.key().to_lowercase();
        (self.kinds.is_empty() || self.kinds.contains(&kind))
            && self.words.iter().all(|w| key.contains(w.as_str()))
    }
}

// ----------------------------------------------------------------------------- references

/// Replace every JSON string equal to `old` with `new`, with keys under a folder that moved
/// (`old/…` becomes `new/…`) and labels inside a file (`old#Mesh0` becomes `new#Mesh0`).
/// Returns whether anything changed.
pub fn replace_refs(value: &mut Value, old: &str, new: &str) -> bool {
    match value {
        Value::String(s) => {
            let moved = if s == old {
                Some(new.to_string())
            } else {
                s.strip_prefix(old)
                    .filter(|rest| rest.starts_with('/') || rest.starts_with('#'))
                    .map(|rest| format!("{new}{rest}"))
            };
            match moved {
                Some(m) => {
                    *s = m;
                    true
                }
                None => false,
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |c, v| replace_refs(v, old, new) | c),
        Value::Object(map) => map
            .values_mut()
            .fold(false, |c, v| replace_refs(v, old, new) | c),
        _ => false,
    }
}

/// Whether a JSON value names `key`, something inside it (`key#Material0`) or inside the
/// folder `key`.
pub fn mentions(value: &Value, key: &str) -> bool {
    match value {
        Value::String(s) => {
            s == key
                || s.strip_prefix(key)
                    .is_some_and(|r| r.starts_with('/') || r.starts_with('#'))
        }
        Value::Array(items) => items.iter().any(|v| mentions(v, key)),
        Value::Object(map) => map.values().any(|v| mentions(v, key)),
        _ => false,
    }
}

/// Files that refer to assets: scenes, prefabs and import settings.
fn referring_files(assets: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                if depth < 12 {
                    walk(&p, depth + 1, out);
                }
            } else if name.ends_with(".scene.json")
                || name.ends_with(".prefab.json")
                || name.ends_with(".import.json")
            {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for root in [Root::Prefabs, Root::Scenes, Root::Assets] {
        walk(&Asset::root_dir(root, assets), 0, &mut out);
    }
    out
}

/// Where an asset is used: entities of the open scene and files on disk.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub entities: Vec<Entity>,
    pub files: Vec<String>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty() && self.files.is_empty()
    }
}

pub fn usage(world: &mut World, asset: &Asset, assets: &Path) -> Usage {
    let key = asset.key();
    let own = asset.path(assets);
    let files = referring_files(assets)
        .into_iter()
        .filter(|p| *p != own)
        .filter(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| mentions(&v, &key))
        })
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    let registry = world.resource::<TypeRegistry>().clone();
    let mut q = world.query_filtered::<Entity, With<InScene>>();
    let scene: Vec<Entity> = q.iter(world).collect();
    let mut entities: Vec<Entity> = scene
        .into_iter()
        .filter(|&e| {
            registry
                .components()
                .any(|c| (c.has)(world, e) && (c.get)(world, e).is_some_and(|v| mentions(&v, &key)))
        })
        .collect();
    // A placed copy's parts mention nothing; its root does. Keep roots only.
    entities.sort();
    Usage { entities, files }
}

/// Point every reference to `old` (a key, or a folder of keys) at `new`: files on disk and
/// the open scene's entities. Returns how many files and entities changed.
pub fn rewrite_refs(world: &mut World, assets: &Path, old: &str, new: &str) -> (usize, usize) {
    let mut files = 0;
    for p in referring_files(assets) {
        let Some(mut v) = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            continue;
        };
        if replace_refs(&mut v, old, new)
            && let Ok(text) = serde_json::to_string_pretty(&v)
            && std::fs::write(&p, text).is_ok()
        {
            files += 1;
        }
    }
    let registry = world.resource::<TypeRegistry>().clone();
    let mut q = world.query_filtered::<Entity, With<InScene>>();
    let scene: Vec<Entity> = q.iter(world).collect();
    let mut entities = 0;
    for e in scene {
        let mut changed = false;
        for c in registry.components() {
            if !(c.has)(world, e) {
                continue;
            }
            let Some(mut v) = (c.get)(world, e) else {
                continue;
            };
            if replace_refs(&mut v, old, new) && (c.insert)(world, e, v).is_ok() {
                changed = true;
            }
        }
        entities += changed as usize;
    }
    crate::prefab::forget_files(world);
    (files, entities)
}

// ----------------------------------------------------------------------------- file actions

/// Sidecar files that travel with an asset.
fn sidecars(path: &Path) -> Vec<(PathBuf, String)> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    vec![(
        path.with_file_name(format!("{name}.import.json")),
        ".import.json".into(),
    )]
}

/// A name in `folder` like `name` that isn't taken ("Crate copy", "Crate copy 2").
pub fn free_name(folder: &Path, name: &str, suffix: &str) -> String {
    let (stem, ext) = split_ext(name);
    let mut n = 1;
    loop {
        let candidate = match n {
            1 if suffix.is_empty() => format!("{stem}{ext}"),
            1 => format!("{stem}{suffix}{ext}"),
            _ => format!("{stem}{suffix} {n}{ext}"),
        };
        if !folder.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// "crate.prefab.json" → ("crate", ".prefab.json").
pub fn split_ext(name: &str) -> (&str, &str) {
    for ext in [".prefab.json", ".scene.json", ".material.gltf"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return (stem, &name[stem.len()..]);
        }
    }
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => (stem, &name[stem.len()..]),
        _ => (name, ""),
    }
}

/// Move (or rename) an asset to `to` (same root), carrying its sidecars and rewriting every
/// reference to it.
pub fn move_asset(
    world: &mut World,
    assets: &Path,
    from: &Asset,
    to: &Asset,
) -> Result<String, String> {
    if from == to {
        return Ok("Nothing to move".into());
    }
    if from.root != to.root {
        return Err("Assets stay under their own folder (Assets, Prefabs or Scenes)".into());
    }
    if to.rel.starts_with(&format!("{}/", from.rel)) {
        return Err("A folder can't go inside itself".into());
    }
    let (src, dst) = (from.path(assets), to.path(assets));
    if dst.exists() {
        return Err(format!("{} already exists", to.key()));
    }
    // A .gltf names its buffers and images relative to itself, so it can't leave their
    // folder, nor can they leave it; renamed in place, the .gltf files using it follow.
    let moving_out = from.parent() != to.parent();
    if moving_out && from.name().to_lowercase().ends_with(".gltf") {
        return Err(
            "A .gltf file reads its .bin and images from its own folder: rename it in \
                    place, or move the whole folder"
                .into(),
        );
    }
    let users = gltf_users(&src);
    if moving_out && let Some(user) = users.first() {
        return Err(format!(
            "{} reads it from this folder: rename it in place, or move the whole folder",
            user.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        ));
    }
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("{}: {e}", from.key()))?;
    for ((side, ext), (to_side, _)) in sidecars(&src).into_iter().zip(sidecars(&dst)) {
        if side.exists() {
            let _ = std::fs::rename(&side, &to_side);
            debug_assert!(to_side.to_string_lossy().ends_with(&ext));
        }
    }
    for user in users {
        rewrite_uris(&user, from.name(), to.name());
    }
    let (files, entities) = rewrite_refs(world, assets, &from.key(), &to.key());
    Ok(format!(
        "Moved {} to {} ({files} files and {entities} entities updated)",
        from.key(),
        to.key()
    ))
}

/// The .gltf files beside `path` whose buffers or images name it.
fn gltf_users(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let name = name.to_string_lossy();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("gltf"))
                && p != path
        })
        .filter(|p| {
            gltf_json(p).is_some_and(|v| {
                ["images", "buffers"].iter().any(|list| {
                    v[list]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|x| x["uri"].as_str() == Some(&name)))
                })
            })
        })
        .collect()
}

fn gltf_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Point a .gltf file's buffer and image URIs named `old` at `new`.
fn rewrite_uris(gltf: &Path, old: &str, new: &str) {
    let Some(mut v) = gltf_json(gltf) else {
        return;
    };
    let mut changed = false;
    for list in ["images", "buffers"] {
        if let Some(items) = v[list].as_array_mut() {
            for item in items {
                if item["uri"].as_str() == Some(old) {
                    item["uri"] = Value::String(new.to_string());
                    changed = true;
                }
            }
        }
    }
    if changed && let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(gltf, text);
    }
}

/// Copy an asset next to itself ("Crate copy").
pub fn duplicate(assets: &Path, asset: &Asset) -> Result<Asset, String> {
    let src = asset.path(assets);
    let folder = asset.parent();
    let name = free_name(&folder.path(assets), asset.name(), " copy");
    let copy = folder.child(&name);
    let dst = copy.path(assets);
    if src.is_dir() {
        copy_dir(&src, &dst).map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", asset.key()))?;
        for ((side, _), (to_side, _)) in sidecars(&src).into_iter().zip(sidecars(&dst)) {
            if side.exists() {
                let _ = std::fs::copy(&side, &to_side);
            }
        }
    }
    Ok(copy)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let to = dst.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

/// Move an asset (and its sidecars) to the trash, `.editor/trash/<root>/<path>`, so a delete
/// can be taken back by hand. Returns where it went.
pub fn trash(assets: &Path, asset: &Asset) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dst = Path::new(EDITOR_DIR)
        .join("trash")
        .join(stamp.to_string())
        .join(root_title(asset.root))
        .join(&asset.rel);
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let src = asset.path(assets);
    if std::fs::rename(&src, &dst).is_err() {
        // Another disk: copy, then remove.
        if src.is_dir() {
            copy_dir(&src, &dst).map_err(|e| e.to_string())?;
            std::fs::remove_dir_all(&src).map_err(|e| e.to_string())?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
            std::fs::remove_file(&src).map_err(|e| e.to_string())?;
        }
    }
    for ((side, _), (to_side, _)) in sidecars(&src).into_iter().zip(sidecars(&dst)) {
        if side.exists() {
            let _ = std::fs::rename(&side, &to_side);
        }
    }
    Ok(dst)
}

/// A new empty folder in `parent` ("New folder", "New folder 2").
pub fn new_folder(assets: &Path, parent: &Asset) -> Result<Asset, String> {
    let name = free_name(&parent.path(assets), "New folder", "");
    let folder = parent.child(&name);
    std::fs::create_dir_all(folder.path(assets)).map_err(|e| e.to_string())?;
    Ok(folder)
}

// ----------------------------------------------------------------------------- favorites

fn favorites_path() -> PathBuf {
    Path::new(EDITOR_DIR).join("favorites.json")
}

pub fn load_favorites() -> Vec<Asset> {
    let Ok(text) = std::fs::read_to_string(favorites_path()) else {
        return Vec::new();
    };
    let list: Vec<(String, String)> = serde_json::from_str(&text).unwrap_or_default();
    list.into_iter()
        .filter_map(|(root, rel)| {
            let root = match root.as_str() {
                "assets" => Root::Assets,
                "prefabs" => Root::Prefabs,
                "scenes" => Root::Scenes,
                _ => return None,
            };
            Some(Asset::new(root, rel))
        })
        .collect()
}

pub fn save_favorites(list: &[Asset]) {
    let list: Vec<(&str, &str)> = list
        .iter()
        .map(|a| {
            let root = match a.root {
                Root::Assets => "assets",
                Root::Prefabs => "prefabs",
                Root::Scenes => "scenes",
            };
            (root, a.rel.as_str())
        })
        .collect();
    let _ = std::fs::create_dir_all(EDITOR_DIR);
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(favorites_path(), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kinds_titles_and_names() {
        assert_eq!(kind_of("Horse.gltf"), Kind::Model);
        assert_eq!(kind_of("bark.material.gltf"), Kind::Material);
        assert_eq!(kind_of("lamp_post.prefab.json"), Kind::Prefab);
        assert_eq!(kind_of("yard.scene.json"), Kind::Scene);
        assert_eq!(kind_of("rock_albedo.PNG"), Kind::Texture);
        assert_eq!(kind_of("step.ogg"), Kind::Audio);
        let a = Asset::new(Root::Prefabs, "street/lamp_post.prefab.json");
        assert_eq!(a.key(), "prefabs/street/lamp_post.prefab.json");
        assert_eq!(a.title(), "lamp post");
        assert_eq!(a.parent(), Asset::new(Root::Prefabs, "street"));
        assert_eq!(split_ext("a.prefab.json"), ("a", ".prefab.json"));
        assert_eq!(split_ext("tree.glb"), ("tree", ".glb"));
        assert_eq!(split_ext("README"), ("README", ""));
    }

    #[test]
    fn search_words_and_types() {
        let q = Query::parse("lamp t:prefab");
        let lamp = Asset::new(Root::Prefabs, "lamp_post.prefab.json");
        assert!(q.matches(&lamp, Kind::Prefab));
        assert!(!q.matches(&Asset::new(Root::Assets, "lamp.glb"), Kind::Model));
        assert!(
            Query::parse("t:mat")
                .matches(&Asset::new(Root::Assets, "x.material.gltf"), Kind::Material)
        );
    }

    #[test]
    fn references_follow_renames_and_moved_folders() {
        let mut v = json!({
            "Model": {"path": "fetched/trees/oak.glb"},
            "list": ["fetched/trees/oak.glb", "fetched/trees_old/oak.glb", "fetched/trees"],
        });
        assert!(mentions(&v, "fetched/trees/oak.glb"));
        assert!(mentions(&v, "fetched/trees"));
        assert!(!mentions(&v, "fetched/tree"));
        assert!(replace_refs(&mut v, "fetched/trees", "nature/trees"));
        assert_eq!(
            v,
            json!({
                "Model": {"path": "nature/trees/oak.glb"},
                "list": ["nature/trees/oak.glb", "fetched/trees_old/oak.glb", "nature/trees"],
            })
        );
    }

    #[test]
    fn labels_and_gltf_neighbors_follow_a_rename() {
        let mut v = json!({"mesh": "rocks/a.glb#Mesh0/Primitive0", "other": "rocks/a.glbx"});
        assert!(mentions(&v, "rocks/a.glb"));
        assert!(replace_refs(&mut v, "rocks/a.glb", "rocks/b.glb"));
        assert_eq!(
            v,
            json!({"mesh": "rocks/b.glb#Mesh0/Primitive0", "other": "rocks/a.glbx"})
        );

        let dir = std::env::temp_dir().join(format!("editor-gltf-uris-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gltf = dir.join("bark.material.gltf");
        std::fs::write(&gltf, r#"{"images": [{"uri": "bark_basecolor.jpg"}]}"#).unwrap();
        std::fs::write(dir.join("bark_basecolor.jpg"), b"x").unwrap();
        assert_eq!(
            gltf_users(&dir.join("bark_basecolor.jpg")),
            vec![gltf.clone()]
        );
        assert!(gltf_users(&gltf).is_empty());
        rewrite_uris(&gltf, "bark_basecolor.jpg", "bark_color.jpg");
        assert_eq!(
            gltf_json(&gltf).unwrap()["images"][0]["uri"],
            "bark_color.jpg"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn free_names() {
        // Each test its own folder: tests run in parallel.
        let dir = std::env::temp_dir().join(format!("editor-free-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("crate.glb"), "").unwrap();
        assert_eq!(free_name(&dir, "crate.glb", " copy"), "crate copy.glb");
        std::fs::write(dir.join("crate copy.glb"), "").unwrap();
        assert_eq!(free_name(&dir, "crate.glb", " copy"), "crate copy 2.glb");
        assert_eq!(free_name(&dir, "New folder", ""), "New folder");
        let _ = std::fs::remove_dir_all(dir);
    }
}
