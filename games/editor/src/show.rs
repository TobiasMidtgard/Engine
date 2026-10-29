//! The scene view's View and Show menus (Unreal's View Modes and Show flags, Unity's Draw Mode
//! and Gizmos menus). View picks what the renderer draws: the lit scene or one of its buffers
//! (normals, shadow cascades, sky visibility, ambient occlusion, reflections, bounced light,
//! virtual geometry clusters and levels, virtual texture levels). Show turns on the editor's
//! helpers and the modules' debug drawing (physics colliders and contacts, wind, ground wetness
//! and snow, gravity, focus points), and Game view hides every editor helper at once to see the
//! scene as a player will. Module debug views are reached through the type registry by name, so
//! a module that is swapped out or missing just leaves its line greyed out.

use engine::prelude::*;
use engine::render::{DebugView, RenderSettings};
use serde_json::{Value, json};

use crate::editor::Editor;

/// The renderer's views, as the View menu lists them.
pub const VIEWS: &[(DebugView, &str, &str)] = &[
    (DebugView::None, "Lit", "The scene as it renders"),
    (
        DebugView::Normals,
        "Normals",
        "World-space surface directions",
    ),
    (
        DebugView::Cascades,
        "Shadow cascades",
        "Which shadow cascade covers each surface",
    ),
    (
        DebugView::SkyVisibility,
        "Sky visibility",
        "How much open sky each surface sees (black: caves and interiors)",
    ),
    (
        DebugView::AmbientOcclusion,
        "Ambient occlusion",
        "Contact shadows in creases (black: crevices)",
    ),
    (
        DebugView::Reflections,
        "Reflections",
        "What surfaces reflect from the screen",
    ),
    (
        DebugView::IndirectLight,
        "Bounced light",
        "Light bounced onto surfaces from nearby surfaces",
    ),
    (
        DebugView::Clusters,
        "Geometry clusters",
        "Virtual geometry: every cluster in its own color",
    ),
    (
        DebugView::ClusterLevels,
        "Geometry detail",
        "Virtual geometry: blue full detail through red to white (7+ halvings)",
    ),
    (
        DebugView::VirtualTextures,
        "Texture streaming",
        "Virtual textures: the level each surface shows",
    ),
];

/// A module's debug toggle: registered resource, the JSON path to its bool, the menu label.
const MODULE_FLAGS: &[(&str, &[&str], &str)] = &[
    ("PhysicsDebug", &["colliders"], "Physics colliders"),
    ("PhysicsDebug", &["contacts"], "Physics contacts"),
    ("PhysicsDebug", &["joints"], "Physics joints"),
    ("DebugViews", &["wind", "enabled"], "Wind"),
    (
        "DebugViews",
        &["surface", "enabled"],
        "Ground wetness and snow",
    ),
    ("DebugViews", &["gravity"], "Gravity"),
    ("DebugViews", &["focus"], "Focus points"),
    ("ScriptGizmos", &["show"], "Script gizmos"),
];

/// The view mode's name, for the toolbar button.
pub fn view_name(world: &World) -> &'static str {
    let current = world
        .get_resource::<RenderSettings>()
        .map_or(DebugView::None, |s| s.debug_view);
    VIEWS.iter().find(|v| v.0 == current).map_or("Lit", |v| v.1)
}

pub fn set_view(world: &mut World, view: DebugView) {
    if let Some(mut s) = world.get_resource_mut::<RenderSettings>()
        && s.debug_view != view
    {
        s.debug_view = view;
    }
}

/// A registered resource's bool at `path`, or `None` when the resource isn't there.
pub fn flag(world: &World, resource: &str, path: &[&str]) -> Option<bool> {
    let entry = world.get_resource::<TypeRegistry>()?.resource(resource)?;
    let value = (entry.get)(world)?;
    path.iter()
        .try_fold(&value, |v, key| v.get(*key))
        .and_then(Value::as_bool)
}

/// Set a registered resource's bool at `path` (a merge patch, so nothing else changes).
pub fn set_flag(world: &mut World, resource: &str, path: &[&str], on: bool) {
    let Some(patch) = world
        .get_resource::<TypeRegistry>()
        .and_then(|r| r.resource(resource))
        .map(|e| e.patch)
    else {
        return;
    };
    let value = path
        .iter()
        .rev()
        .fold(json!(on), |v, key| json!({ *key: v }));
    if let Err(e) = patch(world, value) {
        engine::core::log::warn!("{resource}: {e}");
    }
}

/// The View menu.
pub fn view_menu(ui: &mut egui::Ui, world: &mut World) {
    let current = world
        .get_resource::<RenderSettings>()
        .map_or(DebugView::None, |s| s.debug_view);
    for &(view, name, tip) in VIEWS {
        if ui
            .selectable_label(current == view, name)
            .on_hover_text(tip)
            .clicked()
        {
            set_view(world, view);
            ui.close();
        }
    }
}

/// The Show menu.
pub fn show_menu(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    ui.checkbox(&mut editor.game_view, "Game view")
        .on_hover_text("Hide every editor helper: grid, icons, outlines, gizmos, hints");
    ui.separator();
    ui.label(crate::theme::dim("Editor"));
    ui.add_enabled_ui(!editor.game_view, |ui| {
        ui.checkbox(&mut editor.grid, "Grid");
        ui.checkbox(&mut editor.icons, "Icons for lights, cameras and demos");
        ui.checkbox(&mut editor.show_outline, "Selection outline");
        ui.checkbox(&mut editor.show_colliders, "Selection colliders");
    });
    ui.separator();
    ui.label(crate::theme::dim("Modules"));
    for &(resource, path, label) in MODULE_FLAGS {
        match flag(world, resource, path) {
            Some(mut on) => {
                let r = ui.checkbox(&mut on, label);
                let r = if resource == "PhysicsDebug" {
                    r.on_hover_text(
                        "Drawn from the running simulation, so these show while playing",
                    )
                } else {
                    r
                };
                if r.changed() {
                    set_flag(world, resource, path, on);
                }
            }
            None => {
                ui.add_enabled(false, egui::Checkbox::new(&mut false, label))
                    .on_disabled_hover_text(format!("{resource} isn't in this world"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_render_view_is_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for (view, _, _) in VIEWS {
            assert!(seen.insert(format!("{view:?}")));
        }
        assert_eq!(VIEWS[0].0, DebugView::None);
    }
}
