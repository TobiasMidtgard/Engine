//! The editor window: menu bar, tool bar with play controls, the dock of panels (scene view,
//! outliner, details, world settings, content, console, history) and the status bar. Runs once
//! per frame as an exclusive system with the whole world in reach.

use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use engine::prelude::*;

use crate::camera::EditorCamera;
use crate::demos::DEMOS;
use crate::editor::{self, Create, Editor, Mode, Tab, Tool};
use crate::egui_bridge::{Egui, EguiFrame, EguiInput};
use crate::inspector;
use crate::prefab;
use crate::scene_ops::{self, EditorOnly, InScene};
use crate::theme;

/// The dock layout (kept apart from [`Editor`] so both can be borrowed at once).
#[derive(Resource)]
pub struct EditorDock(pub DockState<Tab>);

impl Default for EditorDock {
    fn default() -> Self {
        Self(default_layout())
    }
}

/// Scene view in the middle, outliner left, details right, content below.
pub fn default_layout() -> DockState<Tab> {
    layout(0.71)
}

/// The default layout with the scene view taking `scene` of the middle column's height.
pub fn layout(scene: f32) -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [center, _right] =
        surface.split_right(NodeIndex::root(), 0.77, vec![Tab::Inspector, Tab::World]);
    let [center, _left] = surface.split_left(center, 0.21, vec![Tab::Outliner]);
    let [_center, _bottom] = surface.split_below(
        center,
        scene,
        vec![Tab::Content, Tab::Console, Tab::History],
    );
    dock
}

/// Run one egui pass over the editor (Update, exclusive).
pub fn editor_ui(world: &mut World) {
    let ctx = world.resource::<Egui>().ctx.clone();
    let raw = std::mem::take(&mut world.resource_mut::<EguiInput>().raw);
    world.resource_scope(|world, mut dock: Mut<EditorDock>| {
        world.resource_scope(|world, mut editor: Mut<Editor>| {
            let editor = &mut *editor;
            let output = ctx.run_ui(raw, |ui| draw(ui, world, editor, &mut dock.0));
            world.resource_mut::<EguiFrame>().submit(&ctx, output);
            // One mouse drag is one undo step.
            if !ctx.input(|i| i.pointer.any_down()) {
                editor.history.interaction += 1;
            }
        });
    });
}

fn draw(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, dock: &mut DockState<Tab>) {
    editor.viewport_hovered = false;
    egui::Panel::top("menu bar")
        .frame(
            egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(8, 3)),
        )
        .show(ui, |ui| menu_bar(ui, world, editor, dock));
    egui::Panel::top("tool bar")
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .stroke(egui::Stroke::new(1.0, theme::BORDER)),
        )
        .show(ui, |ui| tool_bar(ui, world, editor));
    egui::Panel::bottom("status bar")
        .frame(
            egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(egui::Margin::symmetric(8, 3)),
        )
        .show(ui, |ui| status_bar(ui, world, editor));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let style = theme::dock_style(ui.style());
            DockArea::new(dock)
                .style(style)
                .show_close_buttons(false)
                .show_add_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut Viewer { world, editor });
        });
    if !editor.file_drops.is_empty() {
        crate::import::import_dropped(world, editor, None, None);
    }
    shortcuts(ui.ctx(), world, editor);
    crate::prefab_ui::run_actions(world, editor);
}

struct Viewer<'a> {
    world: &'a mut World,
    editor: &'a mut Editor,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("tab", *tab as u8))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Viewport => "🎬 Scene",
            Tab::Outliner => "☰ Outliner",
            Tab::Inspector => "🔧 Details",
            Tab::World => "🌍 World Settings",
            Tab::Content => "📦 Content",
            Tab::Console => "📋 Console",
            Tab::History => "↺ History",
        }
        .into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let (world, editor) = (&mut *self.world, &mut *self.editor);
        match tab {
            Tab::Viewport => crate::viewport::scene_view(ui, world, editor),
            Tab::Outliner => outliner(ui, world, editor),
            Tab::Inspector => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| inspector::entity_details(ui, world, editor));
            }
            Tab::World => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| inspector::resources(ui, world, editor));
            }
            Tab::Content => crate::browser::content(ui, world, editor),
            Tab::Console => console(ui, editor),
            Tab::History => history(ui, world, editor),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }

    fn tab_style_override(
        &self,
        tab: &Tab,
        global: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        (*tab == Tab::Viewport).then(|| {
            let mut s = global.clone();
            s.tab_body.inner_margin = egui::Margin::ZERO;
            s.tab_body.bg_fill = egui::Color32::TRANSPARENT;
            s
        })
    }
}

// ------------------------------------------------------------------------- bars

fn menu_bar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, dock: &mut DockState<Tab>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.label(
            egui::RichText::new("🔷 ENGINE")
                .strong()
                .color(theme::ACCENT),
        );
        ui.add_space(8.0);
        ui.menu_button("File", |ui| {
            if ui.button("New scene").clicked() {
                editor::new_scene(world, editor);
            }
            ui.menu_button("Open", |ui| {
                let files = editor::scene_files();
                if files.is_empty() {
                    ui.label(theme::dim(format!("No scenes in {}/", editor::SCENES_DIR)));
                }
                for f in files {
                    if ui.button(&f).clicked() {
                        editor::load_scene(world, editor, &f);
                    }
                }
            });
            if ui.button("Save        Ctrl+S").clicked() {
                let path = editor.scene_path.clone();
                editor::save_scene(world, editor, &path);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui
                .add_enabled(
                    editor.history.can_undo(),
                    egui::Button::new("Undo        Ctrl+Z"),
                )
                .clicked()
            {
                editor::undo(world, editor);
            }
            if ui
                .add_enabled(
                    editor.history.can_redo(),
                    egui::Button::new("Redo        Ctrl+Y"),
                )
                .clicked()
            {
                editor::redo(world, editor);
            }
            ui.separator();
            if ui.button("Copy        Ctrl+C").clicked() {
                editor::copy_selection(world, editor);
            }
            let can_paste = !editor.clipboard.is_empty();
            if ui
                .add_enabled(can_paste, egui::Button::new("Paste       Ctrl+V"))
                .clicked()
            {
                editor::paste(world, editor);
            }
            if ui.button("Drop to ground   End").clicked() {
                editor::drop_to_ground(world, editor);
            }
            if ui.button("Duplicate   Ctrl+D").clicked() {
                editor::duplicate_selection(world, editor);
            }
            if ui.button("Delete      Del").clicked() {
                editor::delete_selection(world, editor);
            }
            if ui.button("Frame selection   F").clicked() {
                editor::focus_selection(world, editor);
            }
            if ui.button("Select none   Esc").clicked() {
                editor.select(None);
            }
        });
        ui.menu_button("Create", |ui| create_menu(ui, world, editor));
        ui.menu_button("Window", |ui| {
            if ui.button("Reset layout").clicked() {
                *dock = default_layout();
            }
            ui.checkbox(&mut editor.grid, "Ground grid");
            ui.checkbox(&mut editor.icons, "Scene icons");
        });
        ui.menu_button("Help", |ui| {
            ui.label(egui::RichText::new("Scene view").strong());
            for line in [
                "Right mouse + WASD/QE: fly (wheel: speed)",
                "Middle drag: pan · Wheel: dolly",
                "Alt + left drag: orbit",
                "Click: select · Ctrl/Shift+click: add",
                "Q W E R: select, move, rotate, scale",
                "F: frame selection · Del: delete",
                "Ctrl+D duplicate · Ctrl+Z/Y undo/redo",
                "Ctrl+S save · Ctrl+P play/stop",
            ] {
                ui.label(line);
            }
        });
    });
}

/// Items for creating things (menu bar and outliner).
fn create_menu(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let at = editor::placement_point(world, editor);
    let mut make = |what: Create| {
        editor::create(world, editor, what, at);
    };
    if ui.button("○ Empty").clicked() {
        make(Create::Empty);
    }
    ui.menu_button("◼ Shape", |ui| {
        for (label, shape) in editor::shape_presets() {
            if ui.button(label).clicked() {
                make(Create::Shape(label, shape));
            }
        }
    });
    ui.menu_button("💡 Light", |ui| {
        if ui.button("Point light").clicked() {
            make(Create::PointLight);
        }
        if ui.button("Directional light").clicked() {
            make(Create::SunLight);
        }
    });
    if ui.button("🎥 Camera").clicked() {
        make(Create::Camera);
    }
    ui.menu_button("💠 Module demo", |ui| {
        for (key, label, about, _) in DEMOS {
            if ui.button(*label).on_hover_text(*about).clicked() {
                make(Create::Demo(key));
            }
        }
    });
    if ui.button("🌄 Landscape").clicked() {
        crate::landscape::create(world, editor);
    }
}

fn tool_bar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    ui.horizontal(|ui| {
        let scene = editor
            .scene_path
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".scene.json")
            .to_string();
        let dirty = if editor.history.dirty { " •" } else { "" };
        ui.label(egui::RichText::new(format!("🎬 {scene}{dirty}")).strong())
            .on_hover_text(&editor.scene_path);
        ui.add_space(12.0);
        if ui
            .add_enabled(editor.history.can_undo(), egui::Button::new("⟲"))
            .on_hover_text("Undo (Ctrl+Z)")
            .clicked()
        {
            editor::undo(world, editor);
        }
        if ui
            .add_enabled(editor.history.can_redo(), egui::Button::new("⟳"))
            .on_hover_text("Redo (Ctrl+Y)")
            .clicked()
        {
            editor::redo(world, editor);
        }
        if ui.button("💾 Save").on_hover_text("Ctrl+S").clicked() {
            let path = editor.scene_path.clone();
            editor::save_scene(world, editor, &path);
        }

        // Play controls in the middle.
        let width = 150.0;
        let left = (ui.available_width() - width) * 0.5;
        ui.add_space(left.max(8.0) - 120.0);
        let playing = editor.mode == Mode::Play;
        let play = egui::Button::new(
            egui::RichText::new(if playing { "⏸" } else { "▶" })
                .size(16.0)
                .color(if playing { theme::WARN } else { theme::PLAY }),
        )
        .min_size(egui::vec2(36.0, 26.0));
        if ui
            .add(play)
            .on_hover_text(if playing { "Pause" } else { "Play (Ctrl+P)" })
            .clicked()
        {
            if playing {
                editor::pause(world, editor);
            } else {
                editor::play(world, editor);
            }
        }
        let stop =
            egui::Button::new(egui::RichText::new("⏹").size(16.0)).min_size(egui::vec2(36.0, 26.0));
        if ui
            .add_enabled(editor.is_playing(), stop)
            .on_hover_text("Stop and restore the scene (Esc)")
            .clicked()
        {
            editor::stop(world, editor);
        }
        let step =
            egui::Button::new(egui::RichText::new("⏭").size(16.0)).min_size(egui::vec2(36.0, 26.0));
        if ui
            .add_enabled(editor.mode == Mode::Paused, step)
            .on_hover_text("Advance one frame")
            .clicked()
        {
            editor.step_once = true;
        }
    });
}

fn status_bar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    ui.horizontal(|ui| {
        let (mode, color) = match editor.mode {
            Mode::Edit => ("EDIT", theme::TEXT_DIM),
            Mode::Play => ("PLAY", theme::PLAY),
            Mode::Paused => ("PAUSED", theme::WARN),
        };
        ui.label(egui::RichText::new(mode).color(color).strong().small());
        ui.separator();
        match editor.selection.len() {
            0 => ui.label(theme::dim("Nothing selected")),
            1 => ui.label(format!(
                "Selected: {}",
                scene_ops::label(world, editor.selection[0])
            )),
            n => ui.label(format!("{n} selected")),
        };
        if let Some(last) = editor.log.last() {
            ui.separator();
            let color = if last.error {
                theme::WARN
            } else {
                theme::TEXT_DIM
            };
            ui.label(egui::RichText::new(&last.text).color(color));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tool = match editor.tool {
                Tool::Select => "Select",
                Tool::Move => "Move",
                Tool::Rotate => "Rotate",
                Tool::Scale => "Scale",
                Tool::Sculpt => "Sculpt",
                Tool::Scatter => "Scatter",
                Tool::Measure => "Measure",
            };
            ui.label(theme::dim(format!(
                "{tool} · {}",
                if editor.local_space { "Local" } else { "World" }
            )));
        });
    });
}

// ------------------------------------------------------------------------- outliner

fn outliner(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut editor.outliner_filter)
                .hint_text("🔍 Search…")
                .desired_width(ui.available_width() - 34.0),
        );
        ui.menu_button("➕", |ui| create_menu(ui, world, editor))
            .response
            .on_hover_text("Create");
    });
    ui.add_space(2.0);
    let filter = editor.outliner_filter.to_lowercase();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // While a prefab is open, only it is listed.
            let roots = match editor.prefab_mode.last() {
                Some(focus) => vec![focus.entity],
                None => scene_ops::scene_roots(world),
            };
            for root in roots {
                outliner_row(ui, world, editor, root, &filter, 0);
            }
            // Dropping on the empty space below moves an entity back to the root.
            let (rect, response) = ui.allocate_at_least(
                egui::vec2(ui.available_width(), 40.0f32.max(ui.available_height())),
                egui::Sense::click(),
            );
            if let Some(dragged) = response.dnd_release_payload::<Entity>() {
                editor::reparent(world, editor, *dragged, None);
            }
            if response.dnd_hover_payload::<Entity>().is_some() {
                ui.painter().rect_stroke(
                    rect.shrink(2.0),
                    3,
                    egui::Stroke::new(1.0, theme::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            if response.clicked() {
                editor.select(None);
            }
            ui.separator();
            ui.checkbox(&mut editor.show_runtime, "Show runtime entities")
                .on_hover_text("Entities modules spawn while running; they are not saved");
            if editor.show_runtime {
                runtime_list(ui, world, editor, &filter);
            }
        });
}

fn outliner_row(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    filter: &str,
    depth: usize,
) {
    let name = scene_ops::label(world, entity);
    let children: Vec<Entity> = world
        .get::<Children>(entity)
        .map(|c| {
            c.iter()
                .filter(|&k| world.get::<InScene>(k).is_some())
                .collect()
        })
        .unwrap_or_default();
    if !filter.is_empty() && !subtree_matches(world, entity, filter) {
        return;
    }
    let selected = editor.is_selected(entity);
    let id = ui.make_persistent_id(("outliner", entity));
    let icon = theme::entity_icon(world, entity);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        depth == 0 && children.len() <= 8,
    );
    if !filter.is_empty() {
        state.set_open(true);
    }
    let style = crate::prefab_ui::row_style(world, entity);
    let row = |ui: &mut egui::Ui, world: &mut World, editor: &mut Editor| {
        let icon = style.icon.unwrap_or(icon);
        let text = egui::RichText::new(format!("{icon}  {name}"));
        let text = if selected {
            text.color(egui::Color32::WHITE)
        } else if let Some(color) = style.color {
            text.color(color)
        } else {
            text
        };
        let row = ui.dnd_drag_source(egui::Id::new(("drag", entity)), entity, |ui| {
            ui.add(egui::Button::selectable(selected, text).frame_when_inactive(selected))
        });
        let r = row.inner;
        if style.changed {
            crate::prefab_ui::bar(ui, r.rect);
        }
        if style.added {
            ui.label(
                egui::RichText::new("+")
                    .color(crate::prefab_ui::ADDED)
                    .strong(),
            )
            .on_hover_text("Added to this prefab copy (not in its file)");
        }
        if r.clicked() {
            let additive = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
            if additive {
                editor.toggle(entity);
            } else {
                editor.select(Some(entity));
            }
        }
        if r.double_clicked() {
            editor.select(Some(entity));
            editor::focus_selection(world, editor);
        }
        if let Some(dragged) = row.response.dnd_release_payload::<Entity>()
            && *dragged != entity
        {
            editor::reparent(world, editor, *dragged, Some(entity));
        }
        // A prefab dragged from the Content panel lands under this entity.
        if let Some(dragged) = row.response.dnd_release_payload::<crate::models::Dragged>()
            && let crate::models::Dragged::Prefab(path) = &*dragged
        {
            prefab::place(world, editor, path, None, Some(entity));
        }
        if row.response.dnd_hover_payload::<Entity>().is_some()
            || row
                .response
                .dnd_hover_payload::<crate::models::Dragged>()
                .is_some_and(|d| matches!(*d, crate::models::Dragged::Prefab(_)))
        {
            ui.painter().rect_stroke(
                row.response.rect,
                3,
                egui::Stroke::new(1.5, theme::ACCENT),
                egui::StrokeKind::Outside,
            );
        }
        r.context_menu(|ui| {
            editor.select(Some(entity));
            if ui.button("Frame").clicked() {
                editor::focus_selection(world, editor);
            }
            if ui.button("Copy").clicked() {
                editor::copy_selection(world, editor);
            }
            if world.get::<prefab::PrefabInstance>(entity).is_some()
                && !editor.prefab_mode.iter().any(|f| f.entity == entity)
                && ui.button("✏ Edit prefab").clicked()
            {
                editor
                    .prefab_actions
                    .push(crate::prefab_ui::Action::Enter(entity));
            }
            if ui.button("Save as prefab").clicked() {
                prefab::create(world, editor, entity);
            }
            if ui.button("Duplicate").clicked() {
                editor::duplicate_selection(world, editor);
            }
            if ui.button("Delete").clicked() {
                editor::delete_selection(world, editor);
            }
            if world.get::<ChildOf>(entity).is_some() && ui.button("Move to root").clicked() {
                editor::reparent(world, editor, entity, None);
            }
        });
        if world.get::<crate::demos::ModuleDemo>(entity).is_some() {
            ui.label(theme::dim("demo"));
        }
    };
    if children.is_empty() {
        // A leaf: no arrow, but indented to line up with rows that have one.
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent);
            row(ui, world, editor);
        });
        return;
    }
    state
        .show_header(ui, |ui| row(ui, world, editor))
        .body(|ui| {
            for child in children {
                outliner_row(ui, world, editor, child, filter, depth + 1);
            }
        });
}

fn subtree_matches(world: &World, entity: Entity, filter: &str) -> bool {
    if scene_ops::label(world, entity)
        .to_lowercase()
        .contains(filter)
    {
        return true;
    }
    world.get::<Children>(entity).is_some_and(|c| {
        c.iter()
            .any(|k| world.get::<InScene>(k).is_some() && subtree_matches(world, k, filter))
    })
}

/// Named runtime entities (not in the scene, not the editor's own), flat.
fn runtime_list(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, filter: &str) {
    let mut q = world.query_filtered::<(Entity, &Name), (Without<InScene>, Without<EditorOnly>, With<GlobalTransform>)>();
    let mut rows: Vec<(Entity, String)> = q
        .iter(world)
        .filter(|(_, n)| filter.is_empty() || n.as_str().to_lowercase().contains(filter))
        .map(|(e, n)| (e, n.as_str().to_string()))
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    let total = rows.len();
    for (e, name) in rows.into_iter().take(400) {
        let selected = editor.is_selected(e);
        let text = egui::RichText::new(format!("{}  {name}", theme::entity_icon(world, e)))
            .color(theme::TEXT_DIM);
        if ui
            .add(egui::Button::selectable(selected, text).frame_when_inactive(selected))
            .clicked()
        {
            editor.select(Some(e));
        }
    }
    if total > 400 {
        ui.label(theme::dim(format!(
            "…and {} more (search to narrow)",
            total - 400
        )));
    }
}

fn console(ui: &mut egui::Ui, editor: &mut Editor) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &editor.log {
                let color = if line.error { theme::WARN } else { theme::TEXT };
                ui.horizontal(|ui| {
                    ui.label(theme::dim(format!("{:>7.1}s", line.at)).monospace());
                    ui.label(egui::RichText::new(&line.text).color(color));
                });
            }
        });
}

fn history(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let (done, undone) = editor.history.labels();
    let (done, undone): (Vec<String>, Vec<String>) = (
        done.into_iter().map(String::from).collect(),
        undone.into_iter().map(String::from).collect(),
    );
    let mut undo_to = None;
    let mut redo_n = 0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if ui
                .add(egui::Button::selectable(done.is_empty(), "📂 Opened scene"))
                .clicked()
            {
                undo_to = Some(0);
            }
            for (i, label) in done.iter().enumerate() {
                let current = i + 1 == done.len();
                if ui
                    .add(egui::Button::selectable(current, format!("  {label}")))
                    .clicked()
                {
                    undo_to = Some(i + 1);
                }
            }
            for (i, label) in undone.iter().enumerate() {
                if ui
                    .add(egui::Button::new(theme::dim(format!("  {label}"))).frame(false))
                    .on_hover_text("Undone; click to redo up to here")
                    .clicked()
                {
                    redo_n = i + 1;
                }
            }
        });
    if let Some(keep) = undo_to {
        for _ in keep..done.len() {
            editor::undo(world, editor);
        }
    }
    for _ in 0..redo_n {
        editor::redo(world, editor);
    }
}

// ------------------------------------------------------------------------- keys

fn shortcuts(ctx: &egui::Context, world: &mut World, editor: &mut Editor) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let flying = world
        .get::<EditorCamera>(editor.camera)
        .is_some_and(|c| c.is_dragging());
    let pressed = |key: egui::Key, ctrl: bool| {
        ctx.input(|i| i.key_pressed(key) && i.modifiers.command == ctrl)
    };
    if pressed(egui::Key::Z, true) && ctx.input(|i| !i.modifiers.shift) {
        editor::undo(world, editor);
    }
    if pressed(egui::Key::Y, true)
        || (pressed(egui::Key::Z, true) && ctx.input(|i| i.modifiers.shift))
    {
        editor::redo(world, editor);
    }
    if pressed(egui::Key::S, true) {
        let path = editor.scene_path.clone();
        editor::save_scene(world, editor, &path);
    }
    if pressed(egui::Key::C, true) {
        editor::copy_selection(world, editor);
    }
    if pressed(egui::Key::V, true) {
        editor::paste(world, editor);
    }
    if pressed(egui::Key::D, true) {
        editor::duplicate_selection(world, editor);
    }
    if pressed(egui::Key::P, true) || pressed(egui::Key::F5, false) {
        if editor.is_playing() {
            editor::stop(world, editor);
        } else {
            editor::play(world, editor);
        }
    }
    if flying {
        return;
    }
    if pressed(egui::Key::Delete, false) {
        editor::delete_selection(world, editor);
    }
    if pressed(egui::Key::End, false) {
        editor::drop_to_ground(world, editor);
    }
    if pressed(egui::Key::F, false) {
        editor::focus_selection(world, editor);
    }
    if pressed(egui::Key::Escape, false) {
        if crate::camera_preview::stop_pilot(world, editor) {
            // Esc ejects a pilot first.
        } else if editor.is_playing() {
            editor::stop(world, editor);
        } else {
            editor.select(None);
        }
    }
    for (key, tool) in [
        (egui::Key::Q, Tool::Select),
        (egui::Key::W, Tool::Move),
        (egui::Key::E, Tool::Rotate),
        (egui::Key::R, Tool::Scale),
        (egui::Key::T, Tool::Sculpt),
        (egui::Key::G, Tool::Scatter),
        (egui::Key::M, Tool::Measure),
    ] {
        if pressed(key, false) {
            editor.tool = tool;
        }
    }
}
