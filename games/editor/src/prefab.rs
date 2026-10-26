//! Prefabs: an entity and its children saved to a file and placed many times. Placed copies
//! carry [`PrefabInstance`]. Applying one instance's changes rewrites the file and rebuilds every
//! other instance from it; each keeps its own place (position, rotation, scale and parent).

use engine::core::scene::{EntityDoc, SceneDoc};
use engine::prelude::*;
use serde_json::json;

use crate::editor::{self, Editor, now};
use crate::scene_ops::{self, InScene, Remap, SavedEntity, SceneOrder};
use crate::undo::Command;

pub const PREFABS_DIR: &str = "prefabs";

/// This entity (and its children) was placed from the prefab file at `path`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrefabInstance {
    pub path: String,
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

/// "Lamp post" → "prefabs/lamp_post.prefab.json".
pub fn path_for(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let slug = if slug.is_empty() {
        "prefab".into()
    } else {
        slug
    };
    format!("{PREFABS_DIR}/{slug}.prefab.json")
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

/// A saved entity as a document tree, children nested.
fn tree_doc(saved: &SavedEntity) -> EntityDoc {
    let mut doc = saved.doc.clone();
    doc.children = saved.children.iter().map(tree_doc).collect();
    doc
}

/// The prefab document for `entity`: its tree without the instance link. The root keeps its
/// rotation, scale and height but not where it stood, so placing it puts it on the ground the
/// way it was built (a post centered 1.5 m up stays 1.5 m up).
fn prefab_doc(world: &World, entity: Entity) -> EntityDoc {
    let mut doc = tree_doc(&scene_ops::save_entity(world, entity));
    doc.components.remove("PrefabInstance");
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
    doc
}

/// Save `entity` as a new prefab named after it, and make it an instance of that prefab.
pub fn create(world: &mut World, editor: &mut Editor, entity: Entity) {
    let t = now(world);
    let path = path_for(&scene_ops::label(world, entity));
    let doc = SceneDoc {
        entities: vec![prefab_doc(world, entity)],
    };
    if let Err(e) = doc.save(&path) {
        editor.error(t, format!("Could not save prefab: {e}"));
        return;
    }
    let before = world
        .get::<PrefabInstance>(entity)
        .map(|p| json!({ "path": p.path }));
    editor::apply(
        world,
        editor,
        Command::Component {
            entity,
            name: "PrefabInstance".into(),
            before,
            after: Some(json!({ "path": path })),
        },
    );
    editor.info(t, format!("Saved prefab {path}"));
}

/// Place the prefab at `path` where the view is looking, and select it.
pub fn place(world: &mut World, editor: &mut Editor, path: &str) {
    let t = now(world);
    let doc = match SceneDoc::load(path) {
        Ok(doc) => doc,
        Err(e) => {
            editor.error(t, format!("Could not open prefab: {e}"));
            return;
        }
    };
    let at = editor::placement_point(world, editor);
    let mut commands = Vec::new();
    let mut placed = Vec::new();
    for root in &doc.entities {
        let e = scene_ops::spawn_doc_tree(world, root, None);
        let mut transform = world.get::<Transform>(e).copied().unwrap_or_default();
        transform.translation += at;
        world
            .entity_mut(e)
            .insert((transform, PrefabInstance { path: path.into() }));
        commands.push(Command::Spawned {
            saved: scene_ops::save_entity(world, e),
            parent: None,
        });
        placed.push(e);
    }
    let label = format!("Place {}", display_name(path));
    editor
        .history
        .record(world, Command::Group(label.clone(), commands), t);
    editor.selection = placed;
    editor.info(t, label);
}

/// Rebuild `old` from `doc`, keeping its place, parent, outliner position and instance link.
/// Returns the new entity and the commands that undo it.
fn rebuild(world: &mut World, old: Entity, doc: &EntityDoc) -> (Entity, [Command; 2]) {
    let saved_old = scene_ops::save_entity(world, old);
    let parent = world.get::<ChildOf>(old).map(|p| p.parent());
    let transform = world.get::<Transform>(old).copied().unwrap_or_default();
    let order = world.get::<SceneOrder>(old).copied();
    let link = world.get::<PrefabInstance>(old).cloned();
    world.entity_mut(old).despawn();
    let new = scene_ops::spawn_doc_tree(world, doc, parent);
    world.entity_mut(new).insert(transform);
    if let Some(order) = order {
        world.entity_mut(new).insert(order);
    }
    if let Some(link) = link {
        world.entity_mut(new).insert(link);
    }
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
    (new, commands)
}

/// Write `instance`'s current state to its prefab file and rebuild every other instance.
pub fn apply_changes(world: &mut World, editor: &mut Editor, instance: Entity) {
    let t = now(world);
    let Some(path) = world
        .get::<PrefabInstance>(instance)
        .map(|p| p.path.clone())
    else {
        return;
    };
    let doc = prefab_doc(world, instance);
    if let Err(e) = (SceneDoc {
        entities: vec![doc.clone()],
    })
    .save(&path)
    {
        editor.error(t, format!("Could not save prefab: {e}"));
        return;
    }
    let mut q = world.query_filtered::<(Entity, &PrefabInstance), With<InScene>>();
    let others: Vec<Entity> = q
        .iter(world)
        .filter(|(e, p)| *e != instance && p.path == path)
        .map(|(e, _)| e)
        .collect();
    let mut commands = Vec::new();
    let mut remap = Remap::new();
    for old in &others {
        let (new, undo) = rebuild(world, *old, &doc);
        remap.insert(*old, new);
        commands.extend(undo);
    }
    editor.history.remap_all(&remap);
    editor.remap_selection(&remap);
    let label = format!(
        "Apply to {} ({} other instances updated)",
        display_name(&path),
        others.len()
    );
    if !commands.is_empty() {
        editor
            .history
            .record(world, Command::Group(label.clone(), commands), t);
    }
    editor.info(t, label);
}

/// Throw away `instance`'s changes: rebuild it from its prefab file.
pub fn revert(world: &mut World, editor: &mut Editor, instance: Entity) {
    let t = now(world);
    let Some(path) = world
        .get::<PrefabInstance>(instance)
        .map(|p| p.path.clone())
    else {
        return;
    };
    let doc = match SceneDoc::load(&path) {
        Ok(scene) => match scene.entities.into_iter().next() {
            Some(doc) => doc,
            None => return,
        },
        Err(e) => {
            editor.error(t, format!("Could not open prefab: {e}"));
            return;
        }
    };
    let (new, commands) = rebuild(world, instance, &doc);
    let mut remap = Remap::new();
    remap.insert(instance, new);
    editor.history.remap_all(&remap);
    editor.remap_selection(&remap);
    let label = format!("Revert {} to prefab", display_name(&path));
    editor
        .history
        .record(world, Command::Group(label.clone(), commands.into()), t);
    editor.info(t, label);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instances(world: &mut World, path: &str) -> Vec<(f64, String, usize)> {
        let mut q = world.query::<(&PrefabInstance, &Transform, &Name, Option<&Children>)>();
        let mut found: Vec<_> = q
            .iter(world)
            .filter(|(p, ..)| p.path == path)
            .map(|(_, t, n, c)| {
                (
                    t.translation.x,
                    n.as_str().to_string(),
                    c.map_or(0, |c| c.len()),
                )
            })
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found
    }

    #[test]
    fn apply_updates_other_instances_and_revert_restores_one() {
        let dir = std::env::temp_dir().join(format!("editor-prefab-{}", std::process::id()));
        let path = dir
            .join("crate.prefab.json")
            .to_string_lossy()
            .replace('\\', "/");
        let mut app = App::new();
        app.register_component::<PrefabInstance>("test");
        app.update();
        let world = &mut app.world;
        let link = PrefabInstance { path: path.clone() };
        let a = world
            .spawn((
                InScene,
                SceneOrder::next(),
                Name::new("Crate"),
                Transform::from_xyz(0.0, 0.0, 0.0),
                link.clone(),
            ))
            .id();
        world.spawn((
            InScene,
            SceneOrder::next(),
            Name::new("Lid"),
            Transform::from_xyz(0.0, 1.0, 0.0),
            ChildOf(a),
        ));
        world.spawn((
            InScene,
            SceneOrder::next(),
            Name::new("Crate"),
            Transform::from_xyz(5.0, 0.0, 0.0),
            link,
        ));
        let camera = world.spawn_empty().id();
        let mut editor = Editor::new(camera);

        apply_changes(world, &mut editor, a);
        assert_eq!(
            instances(world, &path),
            [(0.0, "Crate".into(), 1), (5.0, "Crate".into(), 1)],
            "the other instance got the lid and kept its place"
        );
        editor::undo(world, &mut editor);
        assert_eq!(instances(world, &path)[1], (5.0, "Crate".into(), 0));

        world.entity_mut(a).insert(Name::new("Renamed"));
        revert(world, &mut editor, a);
        assert_eq!(instances(world, &path)[0], (0.0, "Crate".into(), 1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_and_paths() {
        assert_eq!(path_for("Lamp post"), "prefabs/lamp_post.prefab.json");
        assert_eq!(display_name("prefabs/lamp_post.prefab.json"), "Lamp post");
        assert_eq!(path_for("  "), "prefabs/prefab.prefab.json");
    }
}
