//! Camera bookmarks, as Unreal's: Ctrl+0…9 remembers where the scene view stands and looks,
//! 0…9 goes back there. They belong to the scene (kept in `.editor/bookmarks.json` by scene
//! file) and are listed in the scene view's ★ menu.

use std::collections::BTreeMap;

use engine::prelude::*;
use serde::{Deserialize, Serialize};

use crate::camera::EditorCamera;
use crate::editor::{self, Editor};

const FILE: &str = ".editor/bookmarks.json";

/// Where a view stood and how it looked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub at: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f64,
    #[serde(default)]
    pub ortho: bool,
}

type Slots = BTreeMap<u8, Bookmark>;

fn read_all() -> BTreeMap<String, Slots> {
    std::fs::read_to_string(FILE)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// This scene's bookmarks.
pub fn list(editor: &Editor) -> Slots {
    read_all().remove(&editor.scene_path).unwrap_or_default()
}

fn write(editor: &Editor, slots: Slots) {
    let mut all = read_all();
    if slots.is_empty() {
        all.remove(&editor.scene_path);
    } else {
        all.insert(editor.scene_path.clone(), slots);
    }
    let _ = std::fs::create_dir_all(crate::content::EDITOR_DIR);
    if let Ok(text) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::write(FILE, text);
    }
}

/// The active view as a bookmark.
pub fn current(world: &World, editor: &Editor) -> Option<Bookmark> {
    let t = world.get::<Transform>(editor.camera)?;
    let c = world.get::<EditorCamera>(editor.camera)?;
    Some(Bookmark {
        at: t.translation.to_array(),
        yaw: c.yaw,
        pitch: c.pitch,
        distance: c.pivot_distance,
        ortho: c.ortho,
    })
}

/// Remember the active view in `slot`.
pub fn set(world: &mut World, editor: &mut Editor, slot: u8) {
    let Some(mark) = current(world, editor) else {
        return;
    };
    let mut slots = list(editor);
    slots.insert(slot, mark);
    write(editor, slots);
    let t = editor::now(world);
    editor.info(
        t,
        format!("Bookmark {slot} set (press {slot} to come back)"),
    );
}

/// Forget `slot`.
pub fn clear(editor: &mut Editor, slot: u8) {
    let mut slots = list(editor);
    if slots.remove(&slot).is_some() {
        write(editor, slots);
    }
}

/// Put the active view where `slot` remembers.
pub fn jump(world: &mut World, editor: &mut Editor, slot: u8) {
    let Some(mark) = list(editor).get(&slot).copied() else {
        let t = editor::now(world);
        editor.info(t, format!("No bookmark {slot} yet: Ctrl+{slot} sets it"));
        return;
    };
    let camera = editor.camera;
    let Some(mut t) = world.get::<Transform>(camera).copied() else {
        return;
    };
    if let Some(mut c) = world.get_mut::<EditorCamera>(camera) {
        c.jump(
            &mut t,
            DVec3::from_array(mark.at),
            mark.yaw,
            mark.pitch,
            mark.distance,
        );
        c.ortho = mark.ortho;
    }
    world.entity_mut(camera).insert(t);
    let now = editor::now(world);
    editor.info(now, format!("Bookmark {slot}"));
}

/// The ★ menu: every slot, to go to, set or forget.
pub fn menu(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    ui.menu_button("★", |ui| {
        let slots = list(editor);
        for slot in 1..=9u8 {
            ui.horizontal(|ui| {
                let mark = slots.get(&slot);
                let text = match mark {
                    Some(m) => format!("{slot}   ({:.0}, {:.0}, {:.0})", m.at[0], m.at[1], m.at[2]),
                    None => format!("{slot}   empty"),
                };
                if ui
                    .add_enabled(mark.is_some(), egui::Button::new(text).frame(false))
                    .on_hover_text(format!("Go there ({slot})"))
                    .clicked()
                {
                    jump(world, editor, slot);
                    ui.close();
                }
                if ui
                    .small_button("Set")
                    .on_hover_text(format!("Remember this view (Ctrl+{slot})"))
                    .clicked()
                {
                    set(world, editor, slot);
                }
                if mark.is_some() && ui.small_button("✖").on_hover_text("Forget").clicked() {
                    clear(editor, slot);
                }
            });
        }
        ui.label(crate::theme::dim("Ctrl+1…9 sets, 1…9 goes there"));
    })
    .response
    .on_hover_text("Camera bookmarks");
}

/// The digit keys: Ctrl sets, alone goes (Unreal's keys).
pub fn keys(ctx: &egui::Context, world: &mut World, editor: &mut Editor) {
    use egui::Key::*;
    for (slot, key) in [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9]
        .into_iter()
        .enumerate()
    {
        let (pressed, ctrl) = ctx.input(|i| (i.key_pressed(key), i.modifiers.command));
        if !pressed {
            continue;
        }
        if ctrl {
            set(world, editor, slot as u8);
        } else {
            jump(world, editor, slot as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bookmark_puts_the_view_back() {
        let mut world = World::new();
        let start = Transform::from_xyz(3.0, 2.0, 8.0).looking_at(DVec3::ZERO, Vec3::Y);
        let camera = world
            .spawn((EditorCamera::looking(start.forward()), start))
            .id();
        let editor = Editor::new(camera);
        let mark = current(&world, &editor).unwrap();
        // Fly away, then back.
        let mut t = Transform::from_xyz(-40.0, 9.0, 1.0);
        let mut c = world.get::<EditorCamera>(camera).unwrap().clone();
        c.jump(
            &mut t,
            DVec3::from_array(mark.at),
            mark.yaw,
            mark.pitch,
            mark.distance,
        );
        assert!((t.translation - start.translation).length() < 1e-9);
        assert!(t.forward().dot(start.forward()) > 0.9999);
        let json = serde_json::to_string(&mark).unwrap();
        assert_eq!(serde_json::from_str::<Bookmark>(&json).unwrap(), mark);
    }
}
