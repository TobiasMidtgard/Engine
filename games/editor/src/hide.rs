//! Hiding and locking things in the editor, as Unity's scene visibility and pickability and
//! Unreal's outliner eye and lock do: an eye and a lock beside every outliner row, H to hide or
//! show the selection, Shift+H to isolate it (only it is drawn until Shift+H again or Exit).
//!
//! It is the editor's state, not the scene's: as Unity keeps it in `Library/`, it is kept in
//! `.editor/visibility.json` by scene file and entity path, so a hidden prop is still in the
//! game, in saved scenes and in prefab files exactly as it was. To draw without changing the
//! data the editor and its undo history read, hidden meshes and lights are switched off only
//! between the end of `PostUpdate` and `Last`, the part of the frame where the renderer
//! gathers them, and switched back before anything else runs.
//!
//! A hidden entity hides its children; a locked one can't be clicked or box-selected in the
//! scene view nor moved by the gizmo (it can still be picked in the outliner, as in Unity).

use std::collections::{HashMap, HashSet};

use engine::prelude::*;
use engine_anim::skin::SkinnedMeshRenderer;
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Mode};
use crate::scene_ops::{InScene, Remap};

/// What is hidden and locked.
#[derive(Debug, Clone, Default)]
pub struct Hide {
    pub hidden: HashSet<Entity>,
    pub locked: HashSet<Entity>,
    /// While isolating: only these (and their children) are drawn.
    pub isolated: Option<HashSet<Entity>>,
}

impl Hide {
    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty() && self.isolated.is_none()
    }

    pub fn remap(&mut self, map: &Remap) {
        for set in [&mut self.hidden, &mut self.locked] {
            *set = set.iter().map(|e| *map.get(e).unwrap_or(e)).collect();
        }
        if let Some(set) = &mut self.isolated {
            *set = set.iter().map(|e| *map.get(e).unwrap_or(e)).collect();
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

fn ancestors(world: &World, e: Entity) -> impl Iterator<Item = Entity> + '_ {
    std::iter::successors(Some(e), |&e| world.get::<ChildOf>(e).map(|c| c.parent()))
}

/// Whether `e` is drawn hidden: it or a parent is hidden, or isolation leaves it out.
pub fn is_hidden(world: &World, hide: &Hide, e: Entity) -> bool {
    if hide.is_empty() {
        return false;
    }
    let mut isolated = hide.isolated.is_none();
    for a in ancestors(world, e) {
        if hide.hidden.contains(&a) {
            return true;
        }
        if hide.isolated.as_ref().is_some_and(|s| s.contains(&a)) {
            isolated = true;
        }
    }
    !isolated
}

/// Whether `e` can't be picked or moved in the scene view: it or a parent is locked.
pub fn is_locked(world: &World, hide: &Hide, e: Entity) -> bool {
    !hide.locked.is_empty() && ancestors(world, e).any(|a| hide.locked.contains(&a))
}

/// Whether the scene view may click, box-select or move `e`.
pub fn pickable(world: &World, hide: &Hide, e: Entity) -> bool {
    !is_hidden(world, hide, e) && !is_locked(world, hide, e)
}

/// Hide the selection, or show it when all of it is hidden (H).
pub fn toggle_selection(world: &mut World, editor: &mut Editor) {
    let selection: Vec<Entity> = editor.selection.clone();
    if selection.is_empty() {
        return;
    }
    let all_hidden = selection.iter().all(|e| editor.hide.hidden.contains(e));
    for e in selection {
        if all_hidden {
            editor.hide.hidden.remove(&e);
        } else {
            editor.hide.hidden.insert(e);
        }
    }
    if !all_hidden {
        // Hidden things can't stay selected (Unreal deselects them too).
        let hide = editor.hide.clone();
        editor.selection.retain(|&e| !is_hidden(world, &hide, e));
    }
    save(world, editor);
}

/// Isolate the selection, or stop isolating (Shift+H).
pub fn toggle_isolation(world: &World, editor: &mut Editor) {
    let now = crate::editor::now(world);
    if editor.hide.isolated.take().is_some() {
        editor.info(now, "Showing everything again");
    } else if !editor.selection.is_empty() {
        editor.hide.isolated = Some(editor.selection.iter().copied().collect());
        editor.info(
            now,
            format!(
                "Isolated {} (Shift+H or Exit shows the rest)",
                plural(editor.selection.len())
            ),
        );
    }
}

pub fn show_all(world: &mut World, editor: &mut Editor) {
    editor.hide.hidden.clear();
    editor.hide.isolated = None;
    save(world, editor);
}

pub fn unlock_all(world: &mut World, editor: &mut Editor) {
    editor.hide.locked.clear();
    save(world, editor);
}

fn plural(n: usize) -> String {
    if n == 1 {
        "1 object".into()
    } else {
        format!("{n} objects")
    }
}

// ----------------------------------------------------------------------------- drawing

/// What was switched off for this frame's drawing.
#[derive(Resource, Default)]
pub struct Switched {
    meshes: Vec<Entity>,
    skinned: Vec<Entity>,
    points: Vec<(Entity, f32)>,
    suns: Vec<(Entity, f32)>,
}

/// End of `PostUpdate`: switch off what is hidden, for the renderer only.
pub fn switch_off(world: &mut World) {
    let hide = match world.get_resource::<Editor>() {
        Some(editor) if editor.mode == Mode::Edit && !editor.hide.is_empty() => editor.hide.clone(),
        _ => return,
    };
    let mut off = Switched::default();
    let mut q = world.query::<(Entity, &MeshRenderer)>();
    off.meshes = q
        .iter(world)
        .filter(|(e, r)| r.visible && is_hidden(world, &hide, *e))
        .map(|(e, _)| e)
        .collect();
    let mut q = world.query::<(Entity, &SkinnedMeshRenderer)>();
    off.skinned = q
        .iter(world)
        .filter(|(e, r)| r.visible && is_hidden(world, &hide, *e))
        .map(|(e, _)| e)
        .collect();
    let mut q = world.query::<(Entity, &PointLight)>();
    off.points = q
        .iter(world)
        .filter(|(e, _)| is_hidden(world, &hide, *e))
        .map(|(e, l)| (e, l.intensity_lm))
        .collect();
    let mut q = world.query::<(Entity, &DirectionalLight)>();
    off.suns = q
        .iter(world)
        .filter(|(e, _)| world.get::<InScene>(*e).is_some() && is_hidden(world, &hide, *e))
        .map(|(e, l)| (e, l.illuminance))
        .collect();
    for &e in &off.meshes {
        if let Some(mut r) = world.get_mut::<MeshRenderer>(e) {
            r.visible = false;
        }
    }
    for &e in &off.skinned {
        if let Some(mut r) = world.get_mut::<SkinnedMeshRenderer>(e) {
            r.visible = false;
        }
    }
    for &(e, _) in &off.points {
        if let Some(mut l) = world.get_mut::<PointLight>(e) {
            l.intensity_lm = 0.0;
        }
    }
    for &(e, _) in &off.suns {
        if let Some(mut l) = world.get_mut::<DirectionalLight>(e) {
            l.illuminance = 0.0;
        }
    }
    world.insert_resource(off);
}

/// `Last`: switch everything back before the next frame's editing reads it.
pub fn switch_on(world: &mut World) {
    let Some(off) = world.remove_resource::<Switched>() else {
        return;
    };
    for e in off.meshes {
        if let Some(mut r) = world.get_mut::<MeshRenderer>(e) {
            r.visible = true;
        }
    }
    for e in off.skinned {
        if let Some(mut r) = world.get_mut::<SkinnedMeshRenderer>(e) {
            r.visible = true;
        }
    }
    for (e, v) in off.points {
        if let Some(mut l) = world.get_mut::<PointLight>(e) {
            l.intensity_lm = v;
        }
    }
    for (e, v) in off.suns {
        if let Some(mut l) = world.get_mut::<DirectionalLight>(e) {
            l.illuminance = v;
        }
    }
}

// ----------------------------------------------------------------------------- keeping it

const FILE: &str = ".editor/visibility.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct SceneState {
    #[serde(default)]
    hidden: Vec<String>,
    #[serde(default)]
    locked: Vec<String>,
}

/// Each scene entity's path: names from the root down, with `#n` for the n-th of a name
/// among its siblings ("Crate stack/Crate#2").
fn paths(world: &mut World) -> HashMap<Entity, String> {
    fn walk(world: &World, list: &[Entity], prefix: &str, out: &mut HashMap<Entity, String>) {
        let mut seen: HashMap<String, usize> = HashMap::new();
        for &e in list {
            let name = crate::scene_ops::label(world, e);
            let n = seen.entry(name.clone()).or_insert(0);
            let own = if *n == 0 {
                name.clone()
            } else {
                format!("{name}#{n}")
            };
            *n += 1;
            let path = if prefix.is_empty() {
                own
            } else {
                format!("{prefix}/{own}")
            };
            let children: Vec<Entity> = world
                .get::<Children>(e)
                .map(|c| {
                    c.iter()
                        .filter(|&k| world.get::<InScene>(k).is_some())
                        .collect()
                })
                .unwrap_or_default();
            walk(world, &children, &path, out);
            out.insert(e, path);
        }
    }
    let roots = crate::scene_ops::scene_roots(world);
    let mut out = HashMap::new();
    walk(world, &roots, "", &mut out);
    out
}

fn read_all() -> HashMap<String, SceneState> {
    std::fs::read_to_string(FILE)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Remember what is hidden and locked in this scene (for the next time it is opened).
pub fn save(world: &mut World, editor: &Editor) {
    let paths = paths(world);
    let state = SceneState {
        hidden: editor
            .hide
            .hidden
            .iter()
            .filter_map(|e| paths.get(e).cloned())
            .collect(),
        locked: editor
            .hide
            .locked
            .iter()
            .filter_map(|e| paths.get(e).cloned())
            .collect(),
    };
    let mut all = read_all();
    if state.hidden.is_empty() && state.locked.is_empty() {
        all.remove(&editor.scene_path);
    } else {
        all.insert(editor.scene_path.clone(), state);
    }
    let _ = std::fs::create_dir_all(crate::content::EDITOR_DIR);
    if let Ok(text) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::write(FILE, text);
    }
}

/// After a scene is opened: hide and lock what was hidden and locked in it.
pub fn load(world: &mut World, editor: &mut Editor) {
    editor.hide.clear();
    let Some(state) = read_all().remove(&editor.scene_path) else {
        return;
    };
    let by_path: HashMap<String, Entity> = paths(world).into_iter().map(|(e, p)| (p, e)).collect();
    editor.hide.hidden = state
        .hidden
        .iter()
        .filter_map(|p| by_path.get(p).copied())
        .collect();
    editor.hide.locked = state
        .locked
        .iter()
        .filter_map(|p| by_path.get(p).copied())
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_and_locked_reach_children_and_isolation_hides_the_rest() {
        let mut world = World::new();
        let parent = world.spawn((InScene, Name::new("Stack"))).id();
        let child = world
            .spawn((InScene, Name::new("Crate"), ChildOf(parent)))
            .id();
        let other = world.spawn((InScene, Name::new("Barrel"))).id();
        let mut hide = Hide::default();
        assert!(!is_hidden(&world, &hide, child));
        hide.hidden.insert(parent);
        assert!(is_hidden(&world, &hide, child));
        assert!(!is_hidden(&world, &hide, other));
        hide.hidden.clear();
        hide.isolated = Some([child].into());
        assert!(!is_hidden(&world, &hide, child));
        assert!(
            is_hidden(&world, &hide, parent),
            "a parent of the isolated is hidden"
        );
        assert!(is_hidden(&world, &hide, other));
        hide.isolated = None;
        hide.locked.insert(parent);
        assert!(is_locked(&world, &hide, child));
        assert!(!pickable(&world, &hide, child));
        assert!(pickable(&world, &hide, other));
        let moved = world.spawn_empty().id();
        hide.remap(&[(parent, moved)].into());
        assert!(hide.locked.contains(&moved));
    }
}
