//! Models: glTF files under the asset root, listed in the Content panel and placed into the
//! scene by a click or by dragging one into the scene view. The placed tree draws the file's
//! meshes and materials by asset key, so scene files save it like anything else and load the
//! file again when opened. A file with a skin is placed as an animated rig instead
//! ([`crate::rigs`]).

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use engine::assets::AssetServer;
use engine::prelude::*;
use engine::render::{GltfScene, Mesh};
use engine::world::physics::ColliderShape;

use crate::editor::{Editor, now};
use crate::scene_ops::{self, InScene, Remap, SceneOrder};
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
    let (low, _) = match bounds(world, key) {
        Ok(Some(b)) => b,
        Ok(None) => {
            editor.info(
                t,
                format!("{key} has nothing to draw (an animation clip or an empty file)"),
            );
            return None;
        }
        Err(e) => {
            editor.error(t, format!("Could not load {key}: {e}"));
            return None;
        }
    };
    let root = Transform::from_translation(at - DVec3::Y * low.y);
    let entity = match spawn(world, key, root, (None, None)) {
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

/// Read `key` from disk again and rebuild every placed copy of it, keeping each one's place,
/// parent, name and outliner position (edits inside the model are replaced). One undo step.
pub fn reimport(world: &mut World, editor: &mut Editor, key: &str) {
    let t = now(world);
    let file = engine::assets::normalize_path(key);
    if let Err(e) = engine::assets::load_now(world, &file) {
        editor.error(t, format!("Could not reimport {key}: {e}"));
        return;
    }
    world
        .get_resource_or_init::<crate::rigs::Rigs>()
        .forget(key);
    if let Some(mut thumbs) = world.get_resource_mut::<crate::thumbs::Thumbs>() {
        thumbs.forget(key);
    }
    let mut q = world.query_filtered::<(Entity, &Model), With<InScene>>();
    let copies: Vec<Entity> = q
        .iter(world)
        .filter(|(_, m)| engine::assets::normalize_path(&m.path) == file)
        .map(|(e, _)| e)
        .collect();
    let mut commands = Vec::new();
    let mut remap = Remap::new();
    for old in copies {
        match rebuild(world, old) {
            Ok((new, undo)) => {
                remap.insert(old, new);
                commands.extend(undo);
            }
            Err(e) => editor.error(t, format!("Could not reimport {key}: {e}")),
        }
    }
    editor.history.remap_all(&remap);
    editor.remap_selection(&remap);
    let label = format!("Reimport {} ({} placed)", display_name(key), remap.len());
    if !commands.is_empty() {
        editor
            .history
            .record(world, Command::Group(label.clone(), commands), t);
    }
    editor.info(t, label);
}

/// Spawn `old`'s model again from the loaded file in its place. Returns the new root and the
/// commands that undo it.
fn rebuild(world: &mut World, old: Entity) -> Result<(Entity, [Command; 2]), String> {
    let key = world
        .get::<Model>(old)
        .map(|m| m.path.clone())
        .ok_or("not a model")?;
    let saved_old = scene_ops::save_entity(world, old);
    let parent = world.get::<ChildOf>(old).map(|p| p.parent());
    let transform = world.get::<Transform>(old).copied().unwrap_or_default();
    let order = world.get::<SceneOrder>(old).copied();
    let name = world.get::<Name>(old).cloned();
    let settings = world.get::<engine_anim::AnimatorSettings>(old).cloned();
    let clips = world.get::<crate::rigs::ModelClips>(old).cloned();
    let carried = (settings, clips);
    world.entity_mut(old).despawn();
    let new = spawn(world, &key, transform, carried)?;
    let mut e = world.entity_mut(new);
    e.insert(Model { path: key });
    if let Some(parent) = parent {
        e.insert(ChildOf(parent));
    }
    if let Some(order) = order {
        e.insert(order);
    }
    if let Some(name) = name {
        e.insert(name);
    }
    tag_tree(world, new);
    let commands = [
        Command::Despawned {
            saved: saved_old,
            parent,
        },
        Command::Spawned {
            saved: scene_ops::save_entity(world, new),
            parent,
        },
    ];
    Ok((new, commands))
}

/// Seconds between looks at the placed models' files.
const WATCH_SECS: f64 = 1.0;

/// Modification times of the model files in the scene, to reimport the ones that change.
#[derive(Default)]
pub struct Watch {
    next: f64,
    stamps: HashMap<String, SystemTime>,
}

/// Reimport placed models whose file changed on disk (Update, exclusive).
pub fn watch_files(world: &mut World) {
    let t = now(world);
    let Some(root) = world.get_resource::<AssetServer>().map(|s| s.root()) else {
        return;
    };
    world.resource_scope(|world, mut editor: Mut<Editor>| {
        if t < editor.model_watch.next {
            return;
        }
        editor.model_watch.next = t + WATCH_SECS;
        let mut q = world.query_filtered::<&Model, With<InScene>>();
        let mut keys: Vec<String> = q.iter(world).map(|m| m.path.clone()).collect();
        keys.sort();
        keys.dedup();
        let mut changed = Vec::new();
        for key in &keys {
            // An imported file whose source changed is copied in again first.
            match crate::import::refresh_from_source(&root, key) {
                Ok(true) => editor.info(t, format!("{key}: its source changed, copied it again")),
                Ok(false) => {}
                Err(e) => editor.error(t, format!("{key}: {e}")),
            }
            let Ok(stamp) = std::fs::metadata(root.join(key)).and_then(|m| m.modified()) else {
                continue;
            };
            if let Some(old) = editor.model_watch.stamps.insert(key.clone(), stamp)
                && old != stamp
            {
                changed.push(key.clone());
            }
        }
        editor.model_watch.stamps.retain(|k, _| keys.contains(k));
        for key in changed {
            reimport(world, &mut editor, &key);
        }
    });
}

/// A box collider shape around the model (in its root's space), offset to its middle.
pub fn fitted_collider(world: &mut World, root: Entity) -> Option<ColliderShape> {
    let key = world.get::<Model>(root)?.path.clone();
    if crate::import::settings_for(&asset_root(world), &key).is_some_and(|s| s.collider == "none") {
        return None;
    }
    let (low, high) = bounds(world, &key).ok()??;
    let half = ((high - low) * 0.5).as_vec3().max(Vec3::splat(0.01));
    let center = ((high + low) * 0.5).as_vec3();
    Some(ColliderShape::Compound(vec![(
        center,
        Quat::IDENTITY,
        ColliderShape::Cuboid { half_extents: half },
    )]))
}

fn asset_root(world: &World) -> std::path::PathBuf {
    world
        .get_resource::<AssetServer>()
        .map(|s| s.root())
        .unwrap_or_default()
}

/// Spawn `key` at `transform`: as a rig when the file has a skin (keeping the clip choice and
/// clip files carried over from a copy being rebuilt), otherwise as its node tree.
fn spawn(
    world: &mut World,
    key: &str,
    transform: Transform,
    carried: (
        Option<engine_anim::AnimatorSettings>,
        Option<crate::rigs::ModelClips>,
    ),
) -> Result<Entity, String> {
    let scale = import_scale(world, key);
    let rigged = if let Some(rig) = crate::rigs::rig(world, key, scale)? {
        let mut e = world.spawn(transform);
        if let Some(settings) = carried.0 {
            e.insert(settings);
        }
        if let Some(clips) = carried.1 {
            e.insert(clips);
        }
        let entity = e.id();
        crate::rigs::dress(world, &rig, entity);
        entity
    } else {
        let scene = load(world, key)?;
        let entity = engine::render::spawn_gltf_scene(world, scene, transform)?;
        scale_nodes(world, entity, scale);
        entity
    };
    let animated = world.get::<engine_anim::AnimatorSettings>(rigged).is_some();
    world.entity_mut(rigged).insert(crate::rigs::Rigged {
        animated,
        preview: true,
    });
    Ok(rigged)
}

/// A model's bounds in its root's space, import scale applied (`None`: nothing to draw).
fn bounds(world: &mut World, key: &str) -> Result<Option<(DVec3, DVec3)>, String> {
    let scale = import_scale(world, key);
    if let Some(rig) = crate::rigs::rig(world, key, scale)? {
        return Ok(Some((rig.bounds.0.as_dvec3(), rig.bounds.1.as_dvec3())));
    }
    let scene = load(world, key)?;
    let Some(data) = world.resource::<Assets<GltfScene>>().get(scene).cloned() else {
        return Ok(None);
    };
    Ok(scene_bounds(world, &data).map(|(l, h)| (l * scale, h * scale)))
}

/// The scale an imported file's settings ask for (1 for files that weren't imported).
pub fn import_scale(world: &World, key: &str) -> f64 {
    crate::import::settings_for(&asset_root(world), key)
        .map(|s| s.scale)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0)
}

/// Scale a placed model's top nodes (the root keeps the copy's own transform).
fn scale_nodes(world: &mut World, root: Entity, scale: f64) {
    if (scale - 1.0).abs() < 1e-9 {
        return;
    }
    let children: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for child in children {
        if let Some(mut t) = world.get_mut::<Transform>(child) {
            t.translation *= scale;
            t.scale *= scale as f32;
        }
    }
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

    /// A one-triangle glTF (y from 1 to 2) whose node is called `node`.
    fn write_model(dir: &Path, node: &str) {
        let floats: [f32; 9] = [0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 2.0, 0.0];
        let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_le_bytes()).collect();
        std::fs::write(dir.join("tri.bin"), &bytes).unwrap();
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": node, "mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{
                "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                "min": [0.0, 1.0, 0.0], "max": [1.0, 2.0, 0.0]
            }],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "buffers": [{"uri": "tri.bin", "byteLength": 36}]
        });
        std::fs::write(dir.join("tri.gltf"), doc.to_string()).unwrap();
    }

    fn child_names(world: &mut World) -> Vec<String> {
        let mut q = world.query_filtered::<(&Name, &ChildOf), With<InScene>>();
        q.iter(world)
            .filter(|(_, p)| world.get::<Model>(p.parent()).is_some())
            .map(|(n, _)| n.as_str().to_string())
            .collect()
    }

    #[test]
    fn place_and_reimport_keep_the_spot_and_undo() {
        let dir = std::env::temp_dir().join(format!("editor-reimport-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_model(&dir, "Old node");
        let mut app = App::new();
        app.add_plugins(engine::assets::AssetPlugin {
            hot_reload: false,
            root: Some(dir.clone()),
        });
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<engine::render::Image>()
            .init_asset::<GltfScene>()
            .register_asset_loader(engine::render::GltfLoader)
            .register_component::<Model>("test");
        app.update();
        let world = &mut app.world;
        let camera = world.spawn(crate::scene_ops::EditorOnly).id();
        let mut editor = Editor::new(camera);

        let root = place(world, &mut editor, "tri.gltf", DVec3::new(3.0, 0.0, 0.0)).unwrap();
        let t = world.get::<Transform>(root).unwrap().translation;
        assert_eq!(
            (t.x, t.y),
            (3.0, -1.0),
            "the triangle's bottom sits on the point"
        );
        assert_eq!(child_names(world), ["Old node"]);
        world.get_mut::<Transform>(root).unwrap().translation.x = 5.0;

        write_model(&dir, "New node");
        reimport(world, &mut editor, "tri.gltf");
        assert_eq!(child_names(world), ["New node"]);
        let mut q = world.query_filtered::<&Transform, With<Model>>();
        assert_eq!(
            q.single(world).unwrap().translation.x,
            5.0,
            "kept where it was moved"
        );

        crate::editor::undo(world, &mut editor);
        assert_eq!(child_names(world), ["Old node"]);
        std::fs::remove_dir_all(&dir).unwrap();
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
