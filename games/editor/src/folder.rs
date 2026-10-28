//! Outliner folders, as Unreal's World Outliner has: a named group to sort the scene into,
//! not a thing in the world. A folder is a scene entity with [`Folder`] and an identity
//! transform, so what is put inside stays where it stands; the gizmo leaves folders alone.

use engine::prelude::*;
use serde::{Deserialize, Serialize};

use crate::editor::{self, Editor};
use crate::undo::Command;

/// This entity only sorts the outliner: its children are what is in it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Folder;

pub fn is_folder(world: &World, e: Entity) -> bool {
    world.get::<Folder>(e).is_some()
}

/// A new folder holding the selection (Unreal's "Create Folder"): where the selected things
/// are when they share a parent, at the top otherwise. Returns the folder.
pub fn create(world: &mut World, editor: &mut Editor) -> Entity {
    let t = editor::now(world);
    // The topmost selected entities only: a selected child goes with its parent.
    let picked: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| world.get::<crate::scene_ops::InScene>(e).is_some())
        .filter(|&e| {
            !editor
                .selection
                .iter()
                .any(|&o| o != e && crate::scene_ops::is_ancestor(world, o, e))
        })
        .collect();
    let parents: Vec<Option<Entity>> = picked
        .iter()
        .map(|&e| world.get::<ChildOf>(e).map(|c| c.parent()))
        .collect();
    let parent = match parents.first() {
        Some(&p) if parents.iter().all(|&q| q == p) => p,
        _ => None,
    };
    let folder = editor::spawn_with(
        world,
        editor,
        "New folder".into(),
        vec![("Folder", serde_json::Value::Null)],
        Transform::IDENTITY,
    );
    let mut moves = Vec::new();
    if let Some(p) = parent {
        moves.push(Command::Reparent {
            entity: folder,
            before: None,
            after: Some(p),
            before_local: Transform::IDENTITY,
            after_local: Transform::IDENTITY,
        });
    }
    for &e in &picked {
        let Some(local) = world.get::<Transform>(e).copied() else {
            continue;
        };
        // Under an identity folder in the same place, the local transform is unchanged;
        // from elsewhere, it becomes the world transform (the folder sits at the top).
        let after_local = if world.get::<ChildOf>(e).map(|c| c.parent()) == parent {
            local
        } else {
            world
                .get::<GlobalTransform>(e)
                .map_or(local, |g| editor::transform_from_affine(g.affine()))
        };
        moves.push(Command::Reparent {
            entity: e,
            before: world.get::<ChildOf>(e).map(|c| c.parent()),
            after: Some(folder),
            before_local: local,
            after_local,
        });
    }
    let mut done = Vec::new();
    for cmd in moves {
        match cmd.run_now(world) {
            Ok(_) => done.push(cmd),
            Err(e) => editor.error(t, e),
        }
    }
    if !done.is_empty() {
        editor
            .history
            .record(world, Command::Group("Move into folder".into(), done), t);
    }
    editor.select(Some(folder));
    folder
}
