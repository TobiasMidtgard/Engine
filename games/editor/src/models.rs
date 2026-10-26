//! Models: glTF files under the asset root, listed in the Content panel and placed into the
//! scene by a click or by dragging one into the scene view. The placed tree draws the file's
//! meshes and materials by asset key, so scene files save it like anything else and load the
//! file again when opened.

use std::path::Path;

use engine::assets::AssetServer;
use engine::prelude::*;
use engine::render::{GltfScene, Mesh};

use crate::editor::{Editor, now};
use crate::scene_ops::{self, InScene, SceneOrder};
use crate::undo::Command;

/// This entity's children were placed from the glTF file at `path` (an asset key).
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    pub path: String,
}

/// Whether `entity` is a node inside a placed model (bones and groups get no empty icon).
pub fn inside_model(world: &World, mut entity: Entity) -> bool {
    while let Some(parent) = world.get::<ChildOf>(entity).map(|p| p.parent()) {
        if world.get::<Model>(parent).is_some() {
            return true;
        }
        entity = parent;
    }
    false
}

/// How deep the asset folder scan goes.
const MAX_DEPTH: usize = 8;

/// What a drag from the Content panel carries into the scene view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dragged {
    /// A glTF file, by asset key.
    Model(String),
    /// A prefab file path.
    Prefab(String),
}

/// Every `.glb` / `.gltf` file under `root`, as asset keys ("fetched/anim/horse.glb"),
/// sorted.
pub fn model_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let key = format!("{prefix}{name}");
            let path = entry.path();
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    walk(&path, &format!("{key}/"), depth + 1, out);
                }
            } else if is_model(&name) {
                out.push(key);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", 0, &mut out);
    out.sort();
    out
}

fn is_model(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".glb") || lower.ends_with(".gltf")
}

/// The models under the world's asset root, scanned once and kept until `rescan`.
pub fn cached<'a>(world: &World, editor: &'a mut Editor) -> &'a [String] {
    editor.models.get_or_insert_with(|| {
        world
            .get_resource::<AssetServer>()
            .map(|s| model_files(&s.root()))
            .unwrap_or_default()
    })
}

/// "fetched/anim/animals/horse/Horse.gltf" → "Horse".
pub fn display_name(key: &str) -> String {
    let file = key.rsplit('/').next().unwrap_or(key);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    stem.replace('_', " ")
}

/// The folder a key sits in ("" at the root).
pub fn folder(key: &str) -> &str {
    key.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Load `key` if needed and place it with its lowest point on `at`, as one undo step.
pub fn place(world: &mut World, editor: &mut Editor, key: &str, at: DVec3) -> Option<Entity> {
    let t = now(world);
    let scene = match load(world, key) {
        Ok(scene) => scene,
        Err(e) => {
            editor.error(t, format!("Could not load {key}: {e}"));
            return None;
        }
    };
    let data = world.resource::<Assets<GltfScene>>().get(scene)?.clone();
    let Some((low, _)) = scene_bounds(world, &data) else {
        editor.info(
            t,
            format!("{key} has nothing to draw (an animation clip or an empty file)"),
        );
        return None;
    };
    let root = Transform::from_translation(at - DVec3::Y * low.y);
    let entity = match engine::render::spawn_gltf_scene(world, scene, root) {
        Ok(e) => e,
        Err(e) => {
            editor.error(t, format!("Could not place {key}: {e}"));
            return None;
        }
    };
    let name = display_name(key);
    world.entity_mut(entity).insert((
        SceneOrder::next(),
        Name::new(name.clone()),
        Model { path: key.into() },
    ));
    tag_tree(world, entity);
    let saved = scene_ops::save_entity(world, entity);
    editor.history.record(
        world,
        Command::Spawned {
            saved,
            parent: None,
        },
        t,
    );
    editor.selection = vec![entity];
    editor.info(t, format!("Placed {name}"));
    Some(entity)
}

fn load(world: &mut World, key: &str) -> Result<Handle<GltfScene>, String> {
    let file = engine::assets::normalize_path(key);
    let loaded = world
        .get_resource::<AssetServer>()
        .ok_or("no AssetServer")?
        .is_loaded(&file);
    if !loaded {
        engine::assets::load_now(world, &file)?;
    }
    world
        .get_resource::<Assets<GltfScene>>()
        .and_then(|a| a.loaded_by_key(&file))
        .ok_or_else(|| format!("{file} has no glTF scene"))
}

/// Mark the spawned tree as scene content so it shows in the outliner and saves.
fn tag_tree(world: &mut World, root: Entity) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        world.entity_mut(e).insert(InScene);
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
}

/// The scene's bounds in its own space (root at the origin), from its meshes' boxes.
fn scene_bounds(world: &World, scene: &GltfScene) -> Option<(DVec3, DVec3)> {
    let meshes = world.get_resource::<Assets<Mesh>>()?;
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    let mut stack: Vec<(usize, Transform)> = scene
        .roots
        .iter()
        .map(|&r| (r, Transform::IDENTITY))
        .collect();
    let mut visited = std::collections::HashSet::new();
    while let Some((index, parent)) = stack.pop() {
        let Some(node) = scene.nodes.get(index) else {
            continue;
        };
        if !visited.insert(index) {
            continue;
        }
        let global = parent.mul_transform(&node.transform);
        for primitive in &node.primitives {
            let Some(mesh) = meshes
                .loaded_by_key(&primitive.mesh)
                .and_then(|h| meshes.get(h))
            else {
                continue;
            };
            let aabb = mesh.aabb();
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
                    if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
                    if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
                );
                let p = global.transform_point(corner.as_dvec3());
                low = low.min(p);
                high = high.max(p);
            }
        }
        stack.extend(node.children.iter().map(|&c| (c, global)));
    }
    low.x.is_finite().then_some((low, high))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_folders() {
        assert_eq!(
            display_name("fetched/anim/animals/horse/Horse.gltf"),
            "Horse"
        );
        assert_eq!(display_name("car_concept.glb"), "car concept");
        assert_eq!(folder("fetched/anim/clips/m_run.glb"), "fetched/anim/clips");
        assert_eq!(folder("rock.glb"), "");
    }

    #[test]
    fn scan_finds_gltf_files_only() {
        let dir = std::env::temp_dir().join(format!("editor-models-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        for f in [
            "a/b/tree.GLB",
            "a/rock.gltf",
            "a/rock.bin",
            "readme.md",
            ".hidden/x.glb",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        assert_eq!(model_files(&dir), ["a/b/tree.GLB", "a/rock.gltf"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
