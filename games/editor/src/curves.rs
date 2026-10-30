//! The Curves tab: the curves scripts animate with, the timelines that play them and the
//! tweens running now (Unreal's curve editor and Timeline editor, Unity's Curve window).
//!
//! The left column lists the project's `.curve` files (each with a small picture of it), the
//! selected script's timelines (their tracks and events, where each is now) and the tweens
//! running (what each moves, its ease, how far along it is). Pick a curve to edit it: drag
//! keys (Ctrl snaps), drag the picked key's tangent handles (Shift breaks them), double-click
//! to add a key on the curve, Delete removes one, right-click for its interpolation and
//! tangent; F fits, the wheel zooms (Ctrl: time only, Shift: value only), dragging the
//! background pans. Each change reaches the scripts at once ([`ScriptCurves::set`]) and the
//! file when the mouse is let go, as one undo step (Ctrl+Z, Ctrl+Y). Over a curve a timeline
//! plays, its playhead and events are drawn, following the game. The Eases view shows every
//! named ease moving; click one to copy its name.

use std::collections::HashMap;

use egui::{
    Align2, Color32, FontId, Key, Modifiers, PointerButton, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};
use engine::prelude::*;
use engine_script::curve::{self, Curve, Extend, Interp, Tangent};
use engine_script::ease::{self, Named};
use engine_script::tween::{CurveSrc, TimelineInfo, TweenRow};
use engine_script::{ScriptCurves, ScriptRuntime};

use crate::editor::{self, Editor, Tab};
use crate::scene_ops;
use crate::theme;
use crate::ui::EditorDock;

pub struct CurvesTabPlugin;

impl Plugin for CurvesTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurvesTab>();
    }
}

/// The left column's width; the value labels' gutter, the time labels' gutter, the events
/// strip and the key bar's heights; the motion preview's width.
const SIDE_W: f32 = 280.0;
const GUTTER_L: f32 = 48.0;
const GUTTER_B: f32 = 20.0;
const EVENTS_H: f32 = 28.0;
const KEYBAR_H: f32 = 34.0;
const MOTION_W: f32 = 34.0;
/// Tangent handles' length, and how near the pointer must be to pick a key or handle.
const HANDLE_PX: f32 = 58.0;
const HIT_PX: f32 = 9.0;

const CURVE: Color32 = Color32::from_rgb(0x5d, 0xb4, 0xff);
const KEY: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
const PICKED: Color32 = Color32::from_rgb(0xff, 0xc8, 0x4a);
const HANDLE: Color32 = Color32::from_rgb(0xf0, 0x8a, 0x3c);
const PLAYHEAD: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
const EVENT: Color32 = Color32::from_rgb(0x3f, 0xd8, 0xd0);
const GRID: Color32 = Color32::from_rgb(0x24, 0x26, 0x2a);
const GRID_MAIN: Color32 = Color32::from_rgb(0x3a, 0x3d, 0x44);
const CANVAS: Color32 = Color32::from_rgb(0x15, 0x16, 0x19);
const SPAN: Color32 = Color32::from_rgb(0x1a, 0x1c, 0x20);
/// A timeline's tracks, in order.
const TRACKS: [Color32; 6] = [
    Color32::from_rgb(0x5d, 0xb4, 0xff),
    Color32::from_rgb(0xf2, 0xc9, 0x4c),
    Color32::from_rgb(0x6c, 0xd0, 0x5c),
    Color32::from_rgb(0xe0, 0x6c, 0xd8),
    Color32::from_rgb(0xf0, 0x8a, 0x3c),
    Color32::from_rgb(0x3f, 0xd8, 0xd0),
];
/// The ease gallery's columns: in, out, in_out, out_in.
const MODES: [(&str, Color32); 4] = [
    ("in", Color32::from_rgb(0x5d, 0xb4, 0xff)),
    ("out", Color32::from_rgb(0x6c, 0xd0, 0x5c)),
    ("in_out", Color32::from_rgb(0xc0, 0x8c, 0xff)),
    ("out_in", Color32::from_rgb(0xf0, 0x8a, 0x3c)),
];

/// What the tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Curve,
    Eases,
}

/// The curve open: a file, or keys a timeline's track has in its script (shown, not edited).
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    /// The file (None: keys written in the script).
    pub path: Option<String>,
    /// The track it was opened from: the script's entity, its timeline, the track.
    pub track: Option<(Entity, String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    Key(usize),
    In(usize),
    Out(usize),
    Pan,
}

/// The tab's own state.
#[derive(Resource)]
pub struct CurvesTab {
    pub view: View,
    pub open: Option<Open>,
    /// The curve as edited, and as last written (the undo step it is on).
    curve: Option<Curve>,
    committed: Option<Curve>,
    /// [`ScriptCurves::version`] when the curve was last read or sent.
    seen: u64,
    error: Option<String>,
    /// Time (left, right) and value (bottom, top) in view; fit them next frame.
    pub t_view: (f64, f64),
    pub v_view: (f64, f64),
    pub fit: bool,
    /// The picked key.
    pub picked: Option<usize>,
    drag: Option<Drag>,
    /// The key the context menu is for.
    menu_key: Option<usize>,
    undo: Vec<Curve>,
    redo: Vec<Curve>,
    files: Vec<String>,
    files_next: f64,
    new_name: String,
    /// Where things were drawn (shot scripts point at them).
    pub drawn_at: HashMap<String, Rect>,
}

impl Default for CurvesTab {
    fn default() -> Self {
        Self {
            view: View::Curve,
            open: None,
            curve: None,
            committed: None,
            seen: u64::MAX,
            error: None,
            t_view: (-0.1, 1.1),
            v_view: (-0.15, 1.15),
            fit: true,
            picked: None,
            drag: None,
            menu_key: None,
            undo: Vec::new(),
            redo: Vec::new(),
            files: Vec::new(),
            files_next: 0.0,
            new_name: String::new(),
            drawn_at: HashMap::new(),
        }
    }
}

impl CurvesTab {
    /// Opens a curve (dropping the last one's undo steps).
    pub fn open(&mut self, open: Open) {
        if self.open.as_ref() == Some(&open) {
            return;
        }
        self.open = Some(open);
        self.curve = None;
        self.committed = None;
        self.seen = u64::MAX;
        self.error = None;
        self.picked = None;
        self.drag = None;
        self.undo.clear();
        self.redo.clear();
        self.fit = true;
        self.view = View::Curve;
    }

    /// Changes the curve as an edit would (shot scripts; the next frame sends and saves it).
    pub fn edit(&mut self, f: impl FnOnce(&mut Curve)) {
        if let Some(c) = self.curve.as_mut()
            && self.open.as_ref().is_some_and(|o| o.path.is_some())
        {
            f(c);
        }
    }
}

/// Opens a timeline's track (`e`'s script) and brings the tab to the front.
pub fn show_track(world: &mut World, e: Entity, timeline: &str, track: &str) {
    let path = timelines_of(world, e)
        .into_iter()
        .find(|t| t.name == timeline)
        .and_then(|t| t.tracks.into_iter().find(|(n, ..)| n == track))
        .and_then(|(_, src, _)| match src {
            CurveSrc::File(p) => Some(curve::normalize(&p)),
            CurveSrc::Inline(_) => None,
        });
    world.resource_mut::<CurvesTab>().open(Open {
        path,
        track: Some((e, timeline.to_string(), track.to_string())),
    });
    to_front(world);
}

fn to_front(world: &mut World) {
    let mut dock = world.resource_mut::<EditorDock>();
    if let Some(at) = dock.0.find_tab(&Tab::Curves) {
        let _ = dock.0.set_active_tab(at);
    }
}

/// `e`'s timelines as its script has them now.
fn timelines_of(world: &mut World, e: Entity) -> Vec<TimelineInfo> {
    if !world.contains_resource::<ScriptRuntime>() {
        return Vec::new();
    }
    world.resource_scope(|world, rt: Mut<ScriptRuntime>| rt.timelines_of(world, e))
}

/// The script whose timelines the tab lists: the selection's, else the open track's.
fn focus(world: &World, editor: &Editor, tab: &CurvesTab) -> Option<Entity> {
    let with = world
        .get_resource::<ScriptRuntime>()
        .map(|rt| rt.with_timelines())
        .unwrap_or_default();
    editor.primary().filter(|e| with.contains(e)).or(tab
        .open
        .as_ref()
        .and_then(|o| o.track.as_ref())
        .map(|t| t.0)
        .filter(|e| with.contains(e)))
}

/// The timeline (of the listed script) that plays the open curve.
fn timeline_for<'a>(
    open: &Open,
    focus: Option<Entity>,
    tls: &'a [TimelineInfo],
) -> Option<&'a TimelineInfo> {
    if let Some((e, name, _)) = &open.track
        && Some(*e) == focus
        && let Some(t) = tls.iter().find(|t| &t.name == name)
    {
        return Some(t);
    }
    let path = open.path.as_ref()?;
    tls.iter().find(|t| {
        t.tracks
            .iter()
            .any(|(_, src, _)| matches!(src, CurveSrc::File(p) if curve::normalize(p) == *path))
    })
}

/// The tab.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let Some(mut tab) = world.remove_resource::<CurvesTab>() else {
        return;
    };
    tab.drawn_at.clear();
    let now = ui.input(|i| i.time);
    if now >= tab.files_next {
        tab.files = curve::find_files(&engine_script::scripts_root(world));
        tab.files.sort();
        tab.files_next = now + 2.0;
    }
    let focus = focus(world, editor, &tab);
    let timelines = focus.map(|e| timelines_of(world, e)).unwrap_or_default();
    follow_file(world, &mut tab, focus, &timelines);
    toolbar(ui, world, editor, &mut tab);
    ui.separator();
    let full = ui.available_rect_before_wrap();
    match tab.view {
        View::Eases => {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
            child.set_clip_rect(full.intersect(ui.clip_rect()));
            gallery(&mut child, world, editor, &mut tab);
        }
        View::Curve => {
            let side_w = SIDE_W.min(full.width() * 0.38);
            let side = Rect::from_min_max(full.min, Pos2::new(full.left() + side_w, full.bottom()));
            let main =
                Rect::from_min_max(Pos2::new(full.left() + side_w + 8.0, full.top()), full.max);
            let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side));
            side_ui.set_clip_rect(side.intersect(ui.clip_rect()));
            sidebar(&mut side_ui, world, editor, &mut tab, focus, &timelines);
            ui.painter().vline(
                side.right() + 4.0,
                full.y_range(),
                Stroke::new(1.0, theme::BORDER),
            );
            let mut main_ui = ui.new_child(egui::UiBuilder::new().max_rect(main));
            main_ui.set_clip_rect(main.intersect(ui.clip_rect()));
            curve_area(&mut main_ui, world, editor, &mut tab, focus, &timelines);
        }
    }
    ui.allocate_rect(full, Sense::hover());
    world.insert_resource(tab);
}

/// Keeps the open curve in step with its file (changed outside, or reloaded) or, for keys in
/// a script, with the script.
fn follow_file(
    world: &mut World,
    tab: &mut CurvesTab,
    focus: Option<Entity>,
    timelines: &[TimelineInfo],
) {
    let Some(open) = tab.open.clone() else {
        return;
    };
    if tab.drag.is_some() {
        return;
    }
    match &open.path {
        Some(path) => {
            let root = engine_script::scripts_root(world);
            let Some(mut curves) = world.get_resource_mut::<ScriptCurves>() else {
                return;
            };
            if tab.curve.is_some() && curves.version == tab.seen {
                return;
            }
            tab.seen = curves.version;
            match curves.get(&root, path) {
                Ok(c) => {
                    tab.error = None;
                    if tab.committed.as_ref() != Some(&*c) {
                        tab.curve = Some((*c).clone());
                        tab.committed = Some((*c).clone());
                        tab.picked = tab.picked.filter(|i| *i < c.keys.len());
                    }
                }
                Err(e) => {
                    tab.error = Some(e);
                    tab.curve = None;
                }
            }
        }
        None => {
            let curve = timeline_for(&open, focus, timelines).and_then(|tl| {
                let (_, track) = open.track.as_ref().map(|t| (&t.1, &t.2))?;
                tl.tracks
                    .iter()
                    .find(|(n, ..)| n == track)
                    .and_then(|(_, src, _)| src.get(world))
            });
            if let Some(c) = curve
                && tab.curve.as_ref() != Some(&*c)
            {
                tab.curve = Some((*c).clone());
                tab.committed = tab.curve.clone();
            }
        }
    }
}

/// Sends a changed curve to the scripts at once; writes it to its file, as one undo step,
/// once the mouse is let go.
fn commit(world: &mut World, editor: &mut Editor, tab: &mut CurvesTab, pointer_down: bool) {
    let (Some(open), Some(curve)) = (&tab.open, &tab.curve) else {
        return;
    };
    let Some(path) = open.path.clone() else {
        return;
    };
    let root = engine_script::scripts_root(world);
    let Some(mut curves) = world.get_resource_mut::<ScriptCurves>() else {
        return;
    };
    if curves.loaded(&path).as_deref() != Some(curve) {
        curves.set(&root, &path, curve.clone());
        tab.seen = curves.version;
    }
    if pointer_down || tab.committed.as_ref() == Some(curve) {
        return;
    }
    if let Some(before) = tab.committed.replace(curve.clone()) {
        tab.undo.push(before);
        if tab.undo.len() > 200 {
            tab.undo.remove(0);
        }
    }
    tab.redo.clear();
    save(world, editor, tab);
}

/// Writes the open curve to its file.
fn save(world: &mut World, editor: &mut Editor, tab: &mut CurvesTab) {
    let (Some(open), Some(curve)) = (&tab.open, &tab.curve) else {
        return;
    };
    let Some(path) = &open.path else {
        return;
    };
    let root = engine_script::scripts_root(world);
    let full = root.join(curve::normalize(path));
    if let Some(dir) = full.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&full, curve.to_text()) {
        let t = editor::now(world);
        editor.error(t, format!("Can't save {path}: {e}"));
        return;
    }
    if let Some(mut curves) = world.get_resource_mut::<ScriptCurves>() {
        curves.set(&root, path, curve.clone());
        tab.seen = curves.version;
    }
}

fn undo(world: &mut World, editor: &mut Editor, tab: &mut CurvesTab, back: bool) {
    let (from, to) = if back {
        (&mut tab.undo, &mut tab.redo)
    } else {
        (&mut tab.redo, &mut tab.undo)
    };
    let Some(c) = from.pop() else {
        return;
    };
    if let Some(now) = tab.curve.replace(c.clone()) {
        to.push(now);
    }
    tab.committed = Some(c);
    tab.picked = tab
        .picked
        .filter(|i| tab.curve.as_ref().is_some_and(|c| *i < c.keys.len()));
    save(world, editor, tab);
}

/// Makes a new curve file (from a preset) and opens it.
fn new_file(world: &mut World, editor: &mut Editor, tab: &mut CurvesTab, name: &str, c: Curve) {
    let root = engine_script::scripts_root(world);
    let path = curve::normalize(name);
    let full = root.join(&path);
    let t = editor::now(world);
    if full.exists() {
        editor.error(t, format!("{path} already exists"));
    } else {
        if let Some(dir) = full.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&full, c.to_text()) {
            editor.error(t, format!("Can't make {path}: {e}"));
            return;
        }
        editor.info(t, format!("Made {path}"));
    }
    tab.files_next = 0.0;
    tab.open(Open {
        path: Some(path),
        track: None,
    });
}

/// A free name `curves/<stem>`, `curves/<stem>_2`, ...
fn free_name(world: &World, stem: &str) -> String {
    let root = engine_script::scripts_root(world);
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("curves/{stem}")
        } else {
            format!("curves/{stem}_{n}")
        };
        if !root.join(curve::normalize(&name)).exists() {
            return name;
        }
        n += 1;
    }
}

// ------------------------------------------------------------------------- toolbar

fn toolbar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut CurvesTab) {
    ui.horizontal(|ui| {
        for (view, label, tip) in [
            (
                View::Curve,
                "📈 Curve editor",
                "Curve files, timelines and the tweens running",
            ),
            (View::Eases, "▣ Eases", "Every named ease, moving"),
        ] {
            let r = ui
                .selectable_label(tab.view == view, label)
                .on_hover_text(tip);
            tab.drawn_at.insert(format!("view {label}"), r.rect);
            if r.clicked() {
                tab.view = view;
            }
        }
        ui.separator();
        if tab.view == View::Eases {
            ui.label(theme::dim(
                "Click one to copy its name: ease = \"out_back\" in a tween's options, or \
                 ease.out_back(x) in code.",
            ));
            return;
        }
        let editable = tab.open.as_ref().is_some_and(|o| o.path.is_some()) && tab.curve.is_some();
        match &tab.open {
            Some(open) => {
                let name = open
                    .path
                    .clone()
                    .unwrap_or_else(|| "keys in the script".to_string());
                ui.label(RichText::new(name).strong().color(theme::TEXT));
                if let Some((e, tl, tr)) = &open.track {
                    ui.label(theme::dim(format!(
                        "{} · {tl} · {tr}",
                        scene_ops::label(world, *e)
                    )));
                }
            }
            None => {
                ui.label(theme::dim("No curve open"));
            }
        }
        ui.separator();
        ui.add_enabled_ui(editable, |ui| {
            ui.menu_button("Presets", |ui| {
                for name in Curve::PRESETS {
                    if ui.button(name).clicked() {
                        if let (Some(p), Some(c)) = (Curve::preset(name), tab.curve.as_mut()) {
                            c.keys = p.keys;
                            tab.picked = None;
                            tab.fit = true;
                        }
                        ui.close();
                    }
                }
            });
            if let Some(c) = tab.curve.as_mut() {
                extend_combo(ui, "Before", &mut c.before);
                extend_combo(ui, "After", &mut c.after);
            }
            if ui
                .add_enabled(!tab.undo.is_empty(), egui::Button::new("⟲"))
                .on_hover_text("Undo (Ctrl+Z)")
                .clicked()
            {
                undo(world, editor, tab, true);
            }
            if ui
                .add_enabled(!tab.redo.is_empty(), egui::Button::new("⟳"))
                .on_hover_text("Redo (Ctrl+Y)")
                .clicked()
            {
                undo(world, editor, tab, false);
            }
        });
        if ui
            .add_enabled(tab.curve.is_some(), egui::Button::new("⛶ Fit"))
            .on_hover_text("F: the whole curve in view")
            .clicked()
        {
            tab.fit = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(err) = &tab.error {
                ui.colored_label(theme::WARN, err);
            } else if editable {
                ui.label(theme::dim("Saved as you edit; scripts use it at once"));
            } else if tab.open.is_some() {
                ui.label(theme::dim("Keys written in the script"));
            }
        });
    });
}

fn extend_combo(ui: &mut egui::Ui, label: &str, mode: &mut Extend) {
    let name = |m: Extend| match m {
        Extend::Hold => "hold",
        Extend::Loop => "loop",
        Extend::Pingpong => "pingpong",
    };
    ui.label(theme::dim(label));
    egui::ComboBox::from_id_salt(("curve extend", label))
        .width(78.0)
        .selected_text(name(*mode))
        .show_ui(ui, |ui| {
            for m in [Extend::Hold, Extend::Loop, Extend::Pingpong] {
                ui.selectable_value(mode, m, name(m));
            }
        })
        .response
        .on_hover_text(format!(
            "What the curve does {} its keys: holds the end value, loops or goes back and forth",
            if label == "Before" { "before" } else { "after" }
        ));
}

// ------------------------------------------------------------------------- left column

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(text)
            .size(11.0)
            .strong()
            .color(theme::TEXT_DIM),
    );
    ui.add_space(2.0);
}

fn sidebar(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut CurvesTab,
    focus: Option<Entity>,
    timelines: &[TimelineInfo],
) {
    egui::ScrollArea::vertical()
        .id_salt("curves side")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            heading(ui, "CURVE FILES");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut tab.new_name)
                        .hint_text("curves/new_curve")
                        .desired_width((ui.available_width() - 66.0).max(60.0)),
                );
                if ui
                    .button("➕ New")
                    .on_hover_text("A new curve file in the scripts folder (ease in out)")
                    .clicked()
                {
                    let name = if tab.new_name.trim().is_empty() {
                        free_name(world, "curve")
                    } else {
                        tab.new_name.trim().to_string()
                    };
                    let c = Curve::preset("ease in out").unwrap_or_default();
                    new_file(world, editor, tab, &name, c);
                    tab.new_name.clear();
                }
            });
            if tab.files.is_empty() {
                ui.label(theme::dim("No .curve files in the scripts folder yet"));
            }
            let root = engine_script::scripts_root(world);
            let open_path = tab.open.as_ref().and_then(|o| o.path.clone());
            for path in tab.files.clone() {
                let picked = open_path.as_deref() == Some(path.as_str());
                let c = world
                    .get_resource_mut::<ScriptCurves>()
                    .and_then(|mut c| c.get(&root, &path).ok());
                let r = file_row(ui, &path, c.as_deref(), picked);
                tab.drawn_at.insert(format!("file {path}"), r.rect);
                if r.clicked() {
                    tab.open(Open {
                        path: Some(path.clone()),
                        track: None,
                    });
                }
            }
            ui.add_space(10.0);
            heading(ui, "TIMELINES");
            match focus {
                None => {
                    let with = world
                        .get_resource::<ScriptRuntime>()
                        .map(|rt| rt.with_timelines())
                        .unwrap_or_default();
                    if with.is_empty() {
                        ui.label(theme::dim(if editor.is_playing() {
                            "No script here declares timelines"
                        } else {
                            "Play to follow the timelines scripts declare (a class's \
                             timelines table)"
                        }));
                    } else {
                        ui.label(theme::dim("Pick a script:"));
                        for e in with {
                            let text = format!(
                                "{} {}",
                                theme::entity_icon(world, e),
                                scene_ops::label(world, e)
                            );
                            if ui.selectable_label(false, text).clicked() {
                                editor.select(Some(e));
                            }
                        }
                    }
                }
                Some(e) => {
                    ui.label(
                        RichText::new(format!(
                            "{} {}",
                            theme::entity_icon(world, e),
                            scene_ops::label(world, e)
                        ))
                        .color(theme::TEXT)
                        .strong(),
                    );
                    for tl in timelines {
                        timeline_block(ui, tab, e, tl);
                    }
                }
            }
            ui.add_space(10.0);
            heading(ui, "TWEENS NOW");
            tween_rows(ui, world, editor, tab);
        });
}

fn file_row(ui: &mut egui::Ui, path: &str, c: Option<&Curve>, picked: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    let p = ui.painter();
    if picked {
        p.rect_filled(r, 3.0, theme::ACCENT_DIM);
    } else if resp.hovered() {
        p.rect_filled(r, 3.0, theme::HOVER);
    }
    let thumb = Rect::from_min_size(r.min + Vec2::new(4.0, 4.0), Vec2::new(56.0, 24.0));
    p.rect_filled(thumb, 2.0, CANVAS);
    match c {
        Some(c) => sparkline(p, thumb.shrink2(Vec2::new(3.0, 3.0)), c, CURVE),
        None => {
            p.text(
                thumb.center(),
                Align2::CENTER_CENTER,
                "⚠",
                FontId::proportional(12.0),
                theme::WARN,
            );
        }
    }
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let x = thumb.right() + 8.0;
    p.text(
        Pos2::new(x, r.center().y - 6.0),
        Align2::LEFT_CENTER,
        name.trim_end_matches(".curve"),
        FontId::proportional(13.0),
        theme::TEXT,
    );
    p.text(
        Pos2::new(x, r.center().y + 8.0),
        Align2::LEFT_CENTER,
        dir,
        FontId::proportional(10.0),
        theme::TEXT_DIM,
    );
    resp.on_hover_text(path)
}

/// A curve over its keys, scaled to `r`.
fn sparkline(p: &egui::Painter, r: Rect, c: &Curve, color: Color32) {
    let (a, b) = c.span();
    let (a, b) = if b > a { (a, b) } else { (a - 0.5, a + 0.5) };
    let (lo, hi) = c.range();
    let (lo, hi) = if hi - lo > 1e-9 {
        (lo, hi)
    } else {
        (lo - 0.5, hi + 0.5)
    };
    let n = r.width().max(2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = a + (b - a) * i as f64 / n as f64;
            Pos2::new(
                r.left() + r.width() * i as f32 / n as f32,
                r.bottom() - ((c.eval(t) - lo) / (hi - lo)) as f32 * r.height(),
            )
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(1.4, color)));
}

fn timeline_block(ui: &mut egui::Ui, tab: &mut CurvesTab, e: Entity, tl: &TimelineInfo) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
    let p = ui.painter();
    let state = if !tl.playing {
        "⏸"
    } else if tl.direction < 0.0 {
        "◀"
    } else {
        "▶"
    };
    p.text(
        r.left_top() + Vec2::new(2.0, 2.0),
        Align2::LEFT_TOP,
        format!("{state} {}", tl.name),
        FontId::proportional(14.0),
        if tl.playing { theme::PLAY } else { theme::TEXT },
    );
    p.text(
        r.right_top() + Vec2::new(-2.0, 4.0),
        Align2::RIGHT_TOP,
        format!("{:.2} / {:.2} s", tl.time, tl.length),
        FontId::monospace(11.0),
        theme::TEXT_DIM,
    );
    let bar = Rect::from_min_max(
        Pos2::new(r.left() + 2.0, r.bottom() - 9.0),
        Pos2::new(r.right() - 2.0, r.bottom() - 4.0),
    );
    p.rect_filled(bar, 2.0, theme::WIDGET);
    let f = if tl.length > 0.0 {
        (tl.time / tl.length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.rect_filled(
        Rect::from_min_max(
            bar.min,
            Pos2::new(bar.left() + bar.width() * f as f32, bar.bottom()),
        ),
        2.0,
        PLAYHEAD.gamma_multiply(0.8),
    );
    for (_, at) in &tl.events {
        if tl.length > 0.0 {
            let x = bar.left() + bar.width() * (at / tl.length).clamp(0.0, 1.0) as f32;
            p.vline(
                x,
                bar.top() - 3.0..=bar.bottom() + 2.0,
                Stroke::new(1.5, EVENT),
            );
        }
    }
    tab.drawn_at.insert(format!("timeline {}", tl.name), r);
    for (i, (name, src, value)) in tl.tracks.iter().enumerate() {
        let path = match src {
            CurveSrc::File(p) => Some(curve::normalize(p)),
            CurveSrc::Inline(_) => None,
        };
        let opened = tab.open.as_ref().is_some_and(|o| {
            o.track
                .as_ref()
                .is_some_and(|(oe, otl, otr)| *oe == e && *otl == tl.name && otr == name)
        });
        let (r, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
        let p = ui.painter();
        if opened {
            p.rect_filled(r, 3.0, theme::ACCENT_DIM);
        } else if resp.hovered() {
            p.rect_filled(r, 3.0, theme::HOVER);
        }
        let color = TRACKS[i % TRACKS.len()];
        p.circle_filled(Pos2::new(r.left() + 14.0, r.center().y), 4.0, color);
        p.text(
            Pos2::new(r.left() + 26.0, r.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(13.0),
            theme::TEXT,
        );
        p.text(
            Pos2::new(r.right() - 4.0, r.center().y),
            Align2::RIGHT_CENTER,
            format!("{value:.2}"),
            FontId::monospace(11.5),
            color,
        );
        let source = match &path {
            Some(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
            None => "keys".to_string(),
        };
        p.text(
            Pos2::new(r.right() - 50.0, r.center().y),
            Align2::RIGHT_CENTER,
            source,
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        tab.drawn_at.insert(format!("track {} {name}", tl.name), r);
        if resp.clicked() {
            tab.open(Open {
                path,
                track: Some((e, tl.name.clone(), name.clone())),
            });
        }
    }
    if !tl.events.is_empty() {
        let text = tl
            .events
            .iter()
            .map(|(n, at)| format!("• {n} {at:.2} s"))
            .collect::<Vec<_>>()
            .join("   ");
        ui.horizontal_wrapped(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new(text).size(11.0).color(EVENT));
        });
    }
}

/// What a tween moves, in words: "Door position, rotation".
fn moves_text(world: &World, moves: &[(Option<Entity>, String)]) -> String {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (e, prop) in moves {
        let who = e
            .map(|e| scene_ops::label(world, e))
            .unwrap_or_else(|| "table".into());
        match out.iter_mut().find(|(w, _)| *w == who) {
            Some((_, props)) if !props.contains(prop) => props.push(prop.clone()),
            Some(_) => {}
            None => out.push((who, vec![prop.clone()])),
        }
    }
    out.into_iter()
        .map(|(w, p)| format!("{w} {}", p.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

fn tween_rows(ui: &mut egui::Ui, world: &World, editor: &mut Editor, tab: &mut CurvesTab) {
    let rows: Vec<TweenRow> = world
        .get_resource::<ScriptRuntime>()
        .map(|rt| rt.tweens())
        .unwrap_or_default();
    if rows.is_empty() {
        ui.label(theme::dim(if editor.is_playing() {
            "None running"
        } else {
            "Tweens and springs run while playing"
        }));
        return;
    }
    ui.label(theme::dim(format!("{} running", rows.len())));
    const SHOWN: usize = 40;
    for (n, row) in rows.iter().take(SHOWN).enumerate() {
        let (r, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
        let p = ui.painter();
        if row.owner.is_some() && editor.primary() == row.owner {
            p.rect_filled(r, 3.0, theme::ACCENT_DIM.gamma_multiply(0.6));
        } else if resp.hovered() {
            p.rect_filled(r, 3.0, theme::HOVER);
        }
        let what = row
            .label
            .clone()
            .unwrap_or_else(|| moves_text(world, &row.moves));
        let owner = row
            .owner
            .map(|e| scene_ops::label(world, e))
            .unwrap_or_default();
        let title = clip(&what, 34);
        p.text(
            r.left_top() + Vec2::new(4.0, 3.0),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(12.5),
            theme::TEXT,
        );
        let ease_text = if row.spring {
            "spring".to_string()
        } else {
            match row.eases.as_slice() {
                [] => String::new(),
                [one] => one.clone(),
                [first, rest @ ..] => format!("{first} +{}", rest.len()),
            }
        };
        let passes = match row.loops {
            l if l < 0 => format!(" · pass {} (looping)", row.pass + 1),
            l if l > 1 => format!(" · pass {}/{l}", row.pass + 1),
            _ => String::new(),
        };
        let state = if row.paused { " · paused" } else { "" };
        p.text(
            Pos2::new(r.left() + 4.0, r.top() + 19.0),
            Align2::LEFT_TOP,
            format!("{ease_text}{passes}{state}"),
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        let time = if row.spring {
            format!("{:.1} s", row.time)
        } else {
            format!("{:.1} / {:.1} s", row.time, row.length)
        };
        p.text(
            Pos2::new(r.right() - 4.0, r.top() + 4.0),
            Align2::RIGHT_TOP,
            time,
            FontId::monospace(10.5),
            theme::TEXT_DIM,
        );
        let bar = Rect::from_min_max(
            Pos2::new(r.left() + 4.0, r.bottom() - 5.0),
            Pos2::new(r.right() - 4.0, r.bottom() - 2.0),
        );
        p.rect_filled(bar, 1.5, theme::WIDGET);
        let color = if row.spring { theme::PLAY } else { CURVE };
        p.rect_filled(
            Rect::from_min_max(
                bar.min,
                Pos2::new(
                    bar.left() + bar.width() * row.progress.clamp(0.0, 1.0) as f32,
                    bar.bottom(),
                ),
            ),
            1.5,
            color,
        );
        tab.drawn_at.insert(format!("tween {n}"), r);
        let resp = resp.on_hover_text(format!(
            "{owner}'s tween {}\nMoves: {}\nEase: {}",
            row.id,
            moves_text(world, &row.moves),
            if row.spring {
                "spring".to_string()
            } else {
                row.eases.join(", ")
            }
        ));
        if resp.clicked()
            && let Some(e) = row.owner
        {
            editor.select(Some(e));
        }
    }
    if rows.len() > SHOWN {
        ui.label(theme::dim(format!("and {} more", rows.len() - SHOWN)));
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

// ------------------------------------------------------------------------- curve editor

/// A round step (1, 2 or 5 times a power of ten) at least `min_px` apart at `px_per_unit`.
fn nice_step(px_per_unit: f64, min_px: f64) -> f64 {
    let raw = min_px / px_per_unit.max(1e-9);
    let p = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0] {
        if m * p >= raw {
            return m * p;
        }
    }
    10.0 * p
}

/// `v` with as many decimals as `step` needs.
fn num(v: f64, step: f64) -> String {
    let d = (-step.log10().floor()).clamp(0.0, 4.0) as usize;
    let v = if v.abs() < step * 1e-6 { 0.0 } else { v };
    format!("{v:.d$}")
}

/// The view that shows the whole curve (and the timeline playing it).
fn fit(tab: &mut CurvesTab, c: &Curve, tl: Option<&TimelineInfo>) {
    let (mut a, mut b) = c.span();
    if let Some(tl) = tl {
        a = a.min(0.0);
        b = b.max(tl.length);
    }
    if b - a < 1e-6 {
        a -= 0.5;
        b += 0.5;
    }
    let pad = (b - a) * 0.06;
    tab.t_view = (a - pad, b + pad);
    let (lo, hi) = c.range();
    let (lo, hi) = if hi - lo < 1e-6 {
        (lo - 0.5, hi + 0.5)
    } else {
        (lo, hi)
    };
    let pad = (hi - lo) * 0.16;
    tab.v_view = (lo - pad, hi + pad);
}

/// Where the picked key's tangent handles are (in, out), when the curve uses them.
fn handles(c: &Curve, i: usize, at: Pos2, sx: f64, sy: f64) -> (Option<Pos2>, Option<Pos2>) {
    let (s_in, s_out) = c.slopes(i);
    let dir = |s: f64| {
        let v = Vec2::new(sx as f32, (-s * sy) as f32);
        v.normalized() * HANDLE_PX
    };
    let cubic_in = i > 0 && c.keys[i - 1].interp == Interp::Cubic;
    let cubic_out = i + 1 < c.keys.len() && c.keys[i].interp == Interp::Cubic;
    (
        cubic_in.then(|| at - dir(s_in)),
        cubic_out.then(|| at + dir(s_out)),
    )
}

fn curve_area(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut CurvesTab,
    focus: Option<Entity>,
    timelines: &[TimelineInfo],
) {
    let full = ui.available_rect_before_wrap();
    let Some(open) = tab.open.clone() else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Pick a curve on the left")
                    .size(16.0)
                    .color(theme::TEXT),
            );
            ui.add_space(4.0);
            ui.label(theme::dim(
                "Curves are .curve files in the scripts folder. A tween eases by one \
                 (ease = \"curves/drop\"), a timeline plays them as tracks, and a script \
                 reads one with curve(\"curves/drop\"):at(t).",
            ));
        });
        return;
    };
    let Some(mut c) = tab.curve.take() else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(tab.error.clone().unwrap_or_else(|| "Loading…".into()))
                    .color(theme::WARN),
            );
        });
        return;
    };
    let editable = open.path.is_some();
    let tl = timeline_for(&open, focus, timelines);
    if tab.fit {
        fit(tab, &c, tl);
        tab.fit = false;
    }
    let events_h = if tl.is_some() { EVENTS_H } else { 0.0 };
    let keybar = Rect::from_min_max(Pos2::new(full.left(), full.bottom() - KEYBAR_H), full.max);
    let canvas = Rect::from_min_max(
        Pos2::new(full.left() + GUTTER_L, full.top() + events_h + 4.0),
        Pos2::new(full.right() - MOTION_W - 10.0, keybar.top() - GUTTER_B),
    );
    if canvas.width() < 40.0 || canvas.height() < 40.0 {
        tab.curve = Some(c);
        return;
    }
    let motion = Rect::from_min_max(
        Pos2::new(canvas.right() + 8.0, canvas.top()),
        Pos2::new(full.right() - 2.0, canvas.bottom()),
    );
    tab.drawn_at.insert("canvas".into(), canvas);
    let (t0, t1) = tab.t_view;
    let (v0, v1) = tab.v_view;
    let sx = canvas.width() as f64 / (t1 - t0);
    let sy = canvas.height() as f64 / (v1 - v0);
    let x_of = |t: f64| canvas.left() + ((t - t0) * sx) as f32;
    let y_of = |v: f64| canvas.bottom() - ((v - v0) * sy) as f32;
    let t_of = |x: f32| t0 + (x - canvas.left()) as f64 / sx;
    let v_of = |y: f32| v0 + (canvas.bottom() - y) as f64 / sy;
    let at = |k: &curve::Key| Pos2::new(x_of(k.t), y_of(k.v));

    let resp = ui.interact(
        canvas,
        ui.id().with("curve canvas"),
        Sense::click_and_drag(),
    );
    let mods = ui.input(|i| i.modifiers);
    let hit_key = |c: &Curve, pos: Pos2| -> Option<usize> {
        c.keys
            .iter()
            .enumerate()
            .map(|(i, k)| (i, at(k).distance(pos)))
            .filter(|(_, d)| *d <= HIT_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };

    // Picking and dragging.
    if resp.drag_started_by(PointerButton::Primary)
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
    {
        let picked_handles = tab
            .picked
            .filter(|i| *i < c.keys.len())
            .map(|i| (i, handles(&c, i, at(&c.keys[i]), sx, sy)));
        tab.drag = match picked_handles {
            Some((i, (Some(h), _))) if editable && h.distance(pos) <= HIT_PX => Some(Drag::In(i)),
            Some((i, (_, Some(h)))) if editable && h.distance(pos) <= HIT_PX => Some(Drag::Out(i)),
            _ => match hit_key(&c, pos) {
                Some(i) => {
                    tab.picked = Some(i);
                    editable.then_some(Drag::Key(i))
                }
                None => Some(Drag::Pan),
            },
        };
    }
    if resp.dragged_by(PointerButton::Middle) {
        tab.drag = Some(Drag::Pan);
    }
    if let (Some(drag), Some(pos)) = (tab.drag, resp.interact_pointer_pos()) {
        match drag {
            Drag::Pan => {
                let d = resp.drag_delta();
                let dt = -d.x as f64 / sx;
                let dv = d.y as f64 / sy;
                tab.t_view = (tab.t_view.0 + dt, tab.t_view.1 + dt);
                tab.v_view = (tab.v_view.0 + dv, tab.v_view.1 + dv);
            }
            Drag::Key(i) if i < c.keys.len() => {
                let (mut t, mut v) = (t_of(pos.x), v_of(pos.y));
                if mods.command {
                    t = (t / 0.05).round() * 0.05;
                    v = (v / 0.05).round() * 0.05;
                }
                let lo = if i > 0 {
                    c.keys[i - 1].t + 1e-3
                } else {
                    f64::MIN
                };
                let hi = c.keys.get(i + 1).map(|k| k.t - 1e-3).unwrap_or(f64::MAX);
                let k = &mut c.keys[i];
                k.t = t.clamp(lo, hi.max(lo));
                k.v = v;
            }
            Drag::In(i) | Drag::Out(i) if i < c.keys.len() => {
                let d = pos - at(&c.keys[i]);
                let dx = if matches!(drag, Drag::Out(_)) {
                    d.x.max(2.0)
                } else {
                    d.x.min(-2.0)
                };
                let s = (-d.y as f64 / sy) / (dx as f64 / sx);
                let (s_in, s_out) = c.slopes(i);
                let k = &mut c.keys[i];
                if k.tangent == Tangent::Break || mods.shift {
                    if k.tangent != Tangent::Break {
                        k.slope_in = Some(s_in);
                        k.slope_out = Some(s_out);
                        k.tangent = Tangent::Break;
                    }
                    if matches!(drag, Drag::Out(_)) {
                        k.slope_out = Some(s);
                    } else {
                        k.slope_in = Some(s);
                    }
                } else {
                    k.tangent = Tangent::User;
                    k.slope_out = Some(s);
                    k.slope_in = None;
                }
            }
            _ => {}
        }
    }
    if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
        tab.drag = None;
    }
    let pointer = resp.hover_pos();
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        tab.picked = hit_key(&c, pos);
    }
    if resp.double_clicked()
        && editable
        && let Some(pos) = resp.interact_pointer_pos()
        && hit_key(&c, pos).is_none()
    {
        tab.picked = Some(c.add_key(t_of(pos.x)));
    }
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        tab.menu_key = hit_key(&c, pos);
        if tab.menu_key.is_some() {
            tab.picked = tab.menu_key;
        }
    }
    let menu_at = resp.interact_pointer_pos();
    resp.context_menu(|ui| match tab.menu_key.filter(|i| *i < c.keys.len()) {
        Some(i) if editable => key_menu(ui, &mut c, i, &mut tab.picked),
        _ => {
            if ui
                .add_enabled(editable, egui::Button::new("➕ Add a key here"))
                .clicked()
            {
                if let Some(pos) = menu_at {
                    tab.picked = Some(c.add_key(t_of(pos.x)));
                }
                ui.close();
            }
            if ui.button("⛶ Fit").clicked() {
                tab.fit = true;
                ui.close();
            }
        }
    });

    // Keys: only while the pointer is over the curve area (taken before the editor's own).
    if ui.rect_contains_pointer(full) && !ui.ctx().egui_wants_keyboard_input() {
        let key = |m: Modifiers, k: Key| ui.input_mut(|i| i.consume_key(m, k));
        if editable
            && (key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace))
            && let Some(i) = tab.picked.filter(|i| *i < c.keys.len())
            && c.keys.len() > 1
        {
            c.keys.remove(i);
            tab.picked = None;
        }
        if key(Modifiers::NONE, Key::F) {
            tab.fit = true;
        }
        let redo =
            key(Modifiers::COMMAND, Key::Y) || key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let back = key(Modifiers::COMMAND, Key::Z);
        if back || redo {
            tab.curve = Some(c);
            undo(world, editor, tab, back);
            return;
        }
    }
    // Zoom around the pointer.
    if let Some(pos) = pointer {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        let (mut zt, mut zv) = (1.0f64, 1.0f64);
        if zoom != 1.0 {
            zt = zoom as f64;
        } else if scroll.y != 0.0 {
            zt = (scroll.y as f64 * 0.003).exp();
            zv = zt;
        } else if scroll.x != 0.0 {
            zv = (scroll.x as f64 * 0.003).exp();
        }
        if zt != 1.0 || zv != 1.0 {
            let (pt, pv) = (t_of(pos.x), v_of(pos.y));
            tab.t_view = (pt - (pt - t0) / zt, pt + (t1 - pt) / zt);
            tab.v_view = (pv - (pv - v0) / zv, pv + (v1 - pv) / zv);
        }
    }

    // Drawing.
    let p = ui.painter_at(full);
    p.rect_filled(canvas, 3.0, CANVAS);
    let (ka, kb) = c.span();
    if kb > ka {
        p.rect_filled(
            Rect::from_x_y_ranges(
                x_of(ka).max(canvas.left())..=x_of(kb).min(canvas.right()),
                canvas.y_range(),
            ),
            0.0,
            SPAN,
        );
    }
    let step_t = nice_step(sx, 90.0);
    let step_v = nice_step(sy, 44.0);
    let mut t = (t0 / step_t).ceil() * step_t;
    while t <= t1 {
        let x = x_of(t);
        let main = t.abs() < step_t * 1e-6;
        p.vline(
            x,
            canvas.y_range(),
            Stroke::new(1.0, if main { GRID_MAIN } else { GRID }),
        );
        p.text(
            Pos2::new(x, canvas.bottom() + 3.0),
            Align2::CENTER_TOP,
            format!("{} s", num(t, step_t)),
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        t += step_t;
    }
    let mut v = (v0 / step_v).ceil() * step_v;
    while v <= v1 {
        let y = y_of(v);
        let main = v.abs() < step_v * 1e-6 || (v - 1.0).abs() < step_v * 1e-6;
        p.hline(
            canvas.x_range(),
            y,
            Stroke::new(1.0, if main { GRID_MAIN } else { GRID }),
        );
        p.text(
            Pos2::new(canvas.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            num(v, step_v),
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        v += step_v;
    }
    // The timeline's events and its length.
    if let Some(tl) = tl {
        let strip = Rect::from_min_max(
            Pos2::new(canvas.left(), full.top()),
            Pos2::new(canvas.right(), full.top() + EVENTS_H),
        );
        p.rect_filled(strip, 3.0, theme::WIDGET);
        let x_end = x_of(tl.length);
        if (canvas.left()..=canvas.right()).contains(&x_end) {
            p.extend(Shape::dashed_line(
                &[
                    Pos2::new(x_end, canvas.top()),
                    Pos2::new(x_end, canvas.bottom()),
                ],
                Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(0.6)),
                4.0,
                4.0,
            ));
        }
        for (name, at) in &tl.events {
            let x = x_of(*at);
            if !(canvas.left() - 1.0..=canvas.right() + 1.0).contains(&x) {
                continue;
            }
            p.extend(Shape::dashed_line(
                &[Pos2::new(x, strip.bottom()), Pos2::new(x, canvas.bottom())],
                Stroke::new(1.0, EVENT.gamma_multiply(0.45)),
                3.0,
                5.0,
            ));
            let d = 5.0;
            let c0 = Pos2::new(x, strip.center().y);
            p.add(Shape::convex_polygon(
                vec![
                    c0 + Vec2::new(0.0, -d),
                    c0 + Vec2::new(d, 0.0),
                    c0 + Vec2::new(0.0, d),
                    c0 + Vec2::new(-d, 0.0),
                ],
                EVENT,
                Stroke::NONE,
            ));
            p.text(
                c0 + Vec2::new(8.0, 0.0),
                Align2::LEFT_CENTER,
                name,
                FontId::proportional(11.5),
                theme::TEXT,
            );
            tab.drawn_at.insert(
                format!("event {name}"),
                Rect::from_center_size(c0, Vec2::splat(12.0)),
            );
        }
    }
    // The curve: solid over its keys, dashed where it holds, loops or ping-pongs.
    let n = (canvas.width() / 1.5).max(8.0) as usize;
    let sample = |from: f64, to: f64| -> Vec<Pos2> {
        let n = ((to - from) / (t1 - t0) * n as f64).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| {
                let t = from + (to - from) * i as f64 / n as f64;
                Pos2::new(
                    x_of(t),
                    y_of(c.eval(t)).clamp(canvas.top() - 2000.0, canvas.bottom() + 2000.0),
                )
            })
            .collect()
    };
    let span_stroke = Stroke::new(2.2, CURVE);
    let ext_stroke = Stroke::new(1.4, CURVE.gamma_multiply(0.45));
    let painter = p.with_clip_rect(canvas);
    if c.keys.len() >= 2 {
        if t0 < ka {
            painter.extend(Shape::dashed_line(&sample(t0, ka), ext_stroke, 6.0, 4.0));
        }
        if t1 > kb {
            painter.extend(Shape::dashed_line(&sample(kb, t1), ext_stroke, 6.0, 4.0));
        }
        painter.add(Shape::line(sample(ka.max(t0), kb.min(t1)), span_stroke));
    } else if let Some(k) = c.keys.first() {
        painter.hline(canvas.x_range(), y_of(k.v), span_stroke);
    }
    // The playhead, with the value there.
    if let Some(tl) = tl {
        let x = x_of(tl.time);
        if (canvas.left()..=canvas.right()).contains(&x) {
            p.vline(x, full.top()..=canvas.bottom(), Stroke::new(1.6, PLAYHEAD));
            let y = y_of(c.eval(tl.time));
            painter.circle_filled(Pos2::new(x, y), 5.0, PLAYHEAD);
            let text = format!("{} at {:.2} s: {:.3}", tl.name, tl.time, c.eval(tl.time));
            let galley = p.layout_no_wrap(text, FontId::proportional(11.5), Color32::WHITE);
            let left = (x + 8.0).min(canvas.right() - galley.size().x - 6.0);
            let r = Rect::from_min_size(
                Pos2::new(left, canvas.top() + 6.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            p.rect_filled(r, 3.0, PLAYHEAD.gamma_multiply(0.85));
            p.galley(r.min + Vec2::new(4.0, 2.0), galley, Color32::WHITE);
        }
    }
    // Tangent handles of the picked key.
    let picked = tab.picked.filter(|i| *i < c.keys.len());
    if let Some(i) = picked {
        let kp = at(&c.keys[i]);
        let (hin, hout) = handles(&c, i, kp, sx, sy);
        for h in [hin, hout].into_iter().flatten() {
            painter.line_segment([kp, h], Stroke::new(1.3, HANDLE));
            painter.circle_filled(h, 4.5, HANDLE);
            painter.circle_stroke(h, 4.5, Stroke::new(1.0, Color32::BLACK));
        }
        if let Some(h) = hout.or(hin) {
            tab.drawn_at.insert(
                "handle".into(),
                Rect::from_center_size(h, Vec2::splat(10.0)),
            );
        }
    }
    // Keys.
    let hovered = pointer.and_then(|pos| hit_key(&c, pos));
    for (i, k) in c.keys.iter().enumerate() {
        let kp = at(k);
        if !canvas.expand(6.0).contains(kp) {
            continue;
        }
        let d = if hovered == Some(i) { 6.5 } else { 5.5 };
        let (fill, stroke) = if picked == Some(i) {
            (PICKED, Stroke::new(1.5, Color32::BLACK))
        } else {
            (KEY, Stroke::new(1.0, Color32::BLACK))
        };
        let shape = match k.interp {
            Interp::Constant => vec![
                kp + Vec2::new(-d * 0.8, -d * 0.8),
                kp + Vec2::new(d * 0.8, -d * 0.8),
                kp + Vec2::new(d * 0.8, d * 0.8),
                kp + Vec2::new(-d * 0.8, d * 0.8),
            ],
            _ => vec![
                kp + Vec2::new(0.0, -d),
                kp + Vec2::new(d, 0.0),
                kp + Vec2::new(0.0, d),
                kp + Vec2::new(-d, 0.0),
            ],
        };
        p.add(Shape::convex_polygon(shape, fill, stroke));
        tab.drawn_at.insert(
            format!("key {i}"),
            Rect::from_center_size(kp, Vec2::splat(12.0)),
        );
    }
    p.rect_stroke(
        canvas,
        3.0,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Outside,
    );
    // Where the pointer is.
    if let Some(pos) = pointer {
        let text = match hovered {
            Some(i) => format!("key {}  t {:.3}  v {:.3}", i + 1, c.keys[i].t, c.keys[i].v),
            None => format!("t {:.3}  v {:.3}", t_of(pos.x), v_of(pos.y)),
        };
        p.text(
            Pos2::new(canvas.right() - 6.0, canvas.bottom() - 6.0),
            Align2::RIGHT_BOTTOM,
            text,
            FontId::monospace(11.0),
            theme::TEXT_DIM,
        );
    }
    motion_preview(&p, motion, &c, tl, ui.input(|i| i.time), &y_of);
    // The picked key's numbers.
    let mut bar_ui =
        ui.new_child(egui::UiBuilder::new().max_rect(keybar.shrink2(Vec2::new(GUTTER_L, 4.0))));
    key_bar(&mut bar_ui, world, editor, tab, &mut c, &open, editable);
    tab.curve = Some(c);
    let down = ui.input(|i| i.pointer.any_down());
    commit(world, editor, tab, down);
}

/// A ball moving up and down as the curve goes (over its keys and back to the start), and a
/// red one at the playhead's value.
fn motion_preview(
    p: &egui::Painter,
    r: Rect,
    c: &Curve,
    tl: Option<&TimelineInfo>,
    now: f64,
    y_of: &dyn Fn(f64) -> f32,
) {
    p.rect_filled(r, 3.0, CANVAS);
    let x = r.center().x;
    p.vline(x, r.y_range(), Stroke::new(1.0, GRID_MAIN));
    let (a, b) = c.span();
    let len = (b - a).max(0.2);
    let cycle = len + 0.6;
    let t = a + ((now % cycle) / len).min(1.0) * len;
    let y = y_of(c.eval(t)).clamp(r.top() + 6.0, r.bottom() - 6.0);
    p.circle_filled(Pos2::new(x, y), 7.0, CURVE);
    if let Some(tl) = tl {
        let y = y_of(c.eval(tl.time)).clamp(r.top() + 6.0, r.bottom() - 6.0);
        p.circle_stroke(Pos2::new(x, y), 9.0, Stroke::new(2.0, PLAYHEAD));
    }
}

fn interp_name(i: Interp) -> &'static str {
    match i {
        Interp::Cubic => "cubic",
        Interp::Linear => "linear",
        Interp::Constant => "constant",
    }
}

fn tangent_name(t: Tangent) -> &'static str {
    match t {
        Tangent::Auto => "auto",
        Tangent::Flat => "flat",
        Tangent::User => "user",
        Tangent::Break => "break",
    }
}

/// Sets a key's tangent mode, starting a user or broken one from the slopes it has now.
fn set_tangent(c: &mut Curve, i: usize, mode: Tangent) {
    let (s_in, s_out) = c.slopes(i);
    let k = &mut c.keys[i];
    match mode {
        Tangent::User => {
            k.slope_out = Some((s_in + s_out) / 2.0);
            k.slope_in = None;
        }
        Tangent::Break => {
            k.slope_in = Some(s_in);
            k.slope_out = Some(s_out);
        }
        Tangent::Auto | Tangent::Flat => {
            k.slope_in = None;
            k.slope_out = None;
        }
    }
    k.tangent = mode;
}

/// The right-click menu of key `i`.
fn key_menu(ui: &mut egui::Ui, c: &mut Curve, i: usize, picked: &mut Option<usize>) {
    ui.label(theme::dim(format!("Key {} at {:.2} s", i + 1, c.keys[i].t)));
    ui.separator();
    ui.label(theme::dim("To the next key"));
    for m in [Interp::Cubic, Interp::Linear, Interp::Constant] {
        if ui
            .selectable_label(c.keys[i].interp == m, interp_name(m))
            .clicked()
        {
            c.keys[i].interp = m;
            ui.close();
        }
    }
    ui.separator();
    ui.label(theme::dim("Tangent"));
    for (m, tip) in [
        (Tangent::Auto, "Smooth, never past its neighbors"),
        (Tangent::Flat, "Level"),
        (Tangent::User, "Drag the handles; both sides together"),
        (Tangent::Break, "Each side on its own"),
    ] {
        if ui
            .selectable_label(c.keys[i].tangent == m, tangent_name(m))
            .on_hover_text(tip)
            .clicked()
        {
            set_tangent(c, i, m);
            ui.close();
        }
    }
    ui.separator();
    if ui
        .add_enabled(c.keys.len() > 1, egui::Button::new("✖ Delete key"))
        .clicked()
    {
        c.keys.remove(i);
        *picked = None;
        ui.close();
    }
}

fn key_bar(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut CurvesTab,
    c: &mut Curve,
    open: &Open,
    editable: bool,
) {
    ui.horizontal(|ui| {
        if !editable {
            ui.label(theme::dim(
                "These keys are written in the script. Save them as a file to edit them here, \
                 then name the file in the timeline's tracks.",
            ));
            if ui.button("💾 Save as file").clicked() {
                let stem = match &open.track {
                    Some((_, tl, tr)) => format!("{tl}_{tr}"),
                    None => "curve".to_string(),
                };
                let name = free_name(world, &stem);
                let path = curve::normalize(&name);
                new_file(world, editor, tab, &name, c.clone());
                let t = editor::now(world);
                editor.info(
                    t,
                    format!("Saved {path}: put \"{name}\" in place of the keys in the script"),
                );
            }
            return;
        }
        let Some(i) = tab.picked.filter(|i| *i < c.keys.len()) else {
            ui.label(theme::dim(format!(
                "{} keys. Click one to edit it; double-click the curve to add one; drag its \
                 handles to shape it (Shift: each side alone); right-click for more.",
                c.keys.len()
            )));
            return;
        };
        ui.label(RichText::new(format!("Key {} of {}", i + 1, c.keys.len())).color(PICKED));
        let lo = if i > 0 { c.keys[i - 1].t + 1e-3 } else { -1e6 };
        let hi = c.keys.get(i + 1).map(|k| k.t - 1e-3).unwrap_or(1e6);
        ui.label(theme::dim("time"));
        ui.add(
            egui::DragValue::new(&mut c.keys[i].t)
                .speed(0.005)
                .range(lo..=hi.max(lo))
                .fixed_decimals(3)
                .suffix(" s"),
        );
        ui.label(theme::dim("value"));
        ui.add(
            egui::DragValue::new(&mut c.keys[i].v)
                .speed(0.005)
                .fixed_decimals(3),
        );
        ui.label(theme::dim("to the next"));
        let mut interp = c.keys[i].interp;
        egui::ComboBox::from_id_salt("curve key interp")
            .width(80.0)
            .selected_text(interp_name(interp))
            .show_ui(ui, |ui| {
                for m in [Interp::Cubic, Interp::Linear, Interp::Constant] {
                    ui.selectable_value(&mut interp, m, interp_name(m));
                }
            });
        c.keys[i].interp = interp;
        ui.label(theme::dim("tangent"));
        let mut tangent = c.keys[i].tangent;
        egui::ComboBox::from_id_salt("curve key tangent")
            .width(70.0)
            .selected_text(tangent_name(tangent))
            .show_ui(ui, |ui| {
                for m in [Tangent::Auto, Tangent::Flat, Tangent::User, Tangent::Break] {
                    ui.selectable_value(&mut tangent, m, tangent_name(m));
                }
            });
        if tangent != c.keys[i].tangent {
            set_tangent(c, i, tangent);
        }
        let (s_in, s_out) = c.slopes(i);
        ui.label(theme::dim(if (s_in - s_out).abs() < 1e-9 {
            format!("slope {s_out:.2}/s")
        } else {
            format!("slopes {s_in:.2} | {s_out:.2}/s")
        }));
        if ui
            .add_enabled(c.keys.len() > 1, egui::Button::new("✖"))
            .on_hover_text("Delete the key (Delete)")
            .clicked()
        {
            c.keys.remove(i);
            tab.picked = None;
        }
    });
}

// ------------------------------------------------------------------------- eases

/// Every named ease (and a few others) as small moving pictures: a family a row, in, out,
/// in_out and out_in across.
fn gallery(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut CurvesTab) {
    let full = ui.available_rect_before_wrap();
    let label_w = 76.0;
    let families: Vec<&str> = ease::families().collect();
    let rows = families.len() + 2;
    let head_h = 20.0;
    let gap = 6.0;
    let cell_w = ((full.width() - label_w - gap * 5.0) / 4.0).clamp(90.0, 300.0);
    let cell_h = ((full.height() - head_h - gap * rows as f32) / rows as f32).clamp(50.0, 120.0);
    let now = ui.input(|i| i.time);
    let mut copied: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt("ease gallery")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let total = Vec2::new(
                label_w + (cell_w + gap) * 4.0,
                head_h + (cell_h + gap) * rows as f32,
            );
            let (area, _) = ui.allocate_exact_size(total, Sense::hover());
            let p = ui.painter();
            for (col, (mode, color)) in MODES.iter().enumerate() {
                let x = area.left() + label_w + (cell_w + gap) * col as f32 + cell_w / 2.0;
                p.text(
                    Pos2::new(x, area.top() + 2.0),
                    Align2::CENTER_TOP,
                    *mode,
                    FontId::proportional(13.0),
                    *color,
                );
            }
            let mut cells: Vec<(usize, usize, String, String, Color32)> = Vec::new();
            for (row, family) in families.iter().enumerate() {
                for (col, (mode, color)) in MODES.iter().enumerate() {
                    let name = format!("{mode}_{family}");
                    cells.push((row, col, name.clone(), format!("\"{name}\""), *color));
                }
            }
            let plain = families.len();
            for (col, name) in ["linear", "smooth", "smoother", "spring"]
                .iter()
                .enumerate()
            {
                cells.push((
                    plain,
                    col,
                    name.to_string(),
                    format!("\"{name}\""),
                    theme::TEXT,
                ));
            }
            let more = families.len() + 1;
            for (col, (name, code)) in [
                ("css ease", "ease.bezier(0.25, 0.1, 0.25, 1)"),
                ("bezier overshoot", "ease.bezier(0.68, -0.6, 0.32, 1.6)"),
                ("steps 5", "ease.steps(5)"),
            ]
            .iter()
            .enumerate()
            {
                cells.push((more, col, name.to_string(), code.to_string(), theme::TEXT));
            }
            for (row, label) in families
                .iter()
                .map(|f| f.to_string())
                .chain(["plain".to_string(), "more".to_string()])
                .enumerate()
            {
                let y = area.top() + head_h + (cell_h + gap) * row as f32 + cell_h / 2.0;
                p.text(
                    Pos2::new(area.left() + 6.0, y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(13.0),
                    theme::TEXT,
                );
            }
            for (row, col, name, code, color) in cells {
                let r = Rect::from_min_size(
                    Pos2::new(
                        area.left() + label_w + (cell_w + gap) * col as f32,
                        area.top() + head_h + (cell_h + gap) * row as f32,
                    ),
                    Vec2::new(cell_w, cell_h),
                );
                let resp = ui.interact(r, ui.id().with(("ease cell", &name)), Sense::click());
                let f = ease_fn(&name);
                ease_cell(ui.painter(), r, &name, &f, color, now, resp.hovered());
                tab.drawn_at.insert(format!("ease {name}"), r);
                let resp = resp.on_hover_text(format!("{code}: click to copy"));
                if resp.clicked() {
                    ui.ctx().copy_text(code.clone());
                    copied = Some(code);
                }
            }
        });
    if let Some(code) = copied {
        let t = editor::now(world);
        editor.info(t, format!("Copied {code}"));
    }
}

/// The ease called `name` in the gallery.
fn ease_fn(name: &str) -> Box<dyn Fn(f64) -> f64> {
    if let Some(n) = Named::parse(name) {
        return Box::new(move |x| n.eval(x));
    }
    match name {
        "css ease" => Box::new(|x| ease::bezier([0.25, 0.1, 0.25, 1.0], x)),
        "bezier overshoot" => Box::new(|x| ease::bezier([0.68, -0.6, 0.32, 1.6], x)),
        "steps 5" => Box::new(|x| ease::steps(5, x)),
        _ => Box::new(|x| x),
    }
}

fn ease_cell(
    p: &egui::Painter,
    r: Rect,
    name: &str,
    f: &dyn Fn(f64) -> f64,
    color: Color32,
    now: f64,
    hovered: bool,
) {
    p.rect_filled(r, 4.0, if hovered { theme::HOVER } else { CANVAS });
    // 0..1 in time across, -0.35..1.35 in value (room for back and elastic).
    let plot = Rect::from_min_max(
        Pos2::new(r.left() + 8.0, r.top() + 16.0),
        Pos2::new(r.right() - 26.0, r.bottom() - 6.0),
    );
    let (lo, hi) = (-0.35, 1.35);
    let y_of = |v: f64| plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height();
    for v in [0.0, 1.0] {
        p.hline(plot.x_range(), y_of(v), Stroke::new(1.0, GRID_MAIN));
    }
    let n = 80;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = i as f64 / n as f64;
            Pos2::new(
                plot.left() + plot.width() * x as f32,
                y_of(f(x)).clamp(r.top() + 2.0, r.bottom() - 2.0),
            )
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(1.8, color)));
    // A dot going along it, and a ball at its value beside it.
    let cycle = 2.0;
    let x = ((now % cycle) / 1.4).min(1.0);
    let v = f(x);
    let dot = Pos2::new(
        plot.left() + plot.width() * x as f32,
        y_of(v).clamp(r.top() + 2.0, r.bottom() - 2.0),
    );
    p.circle_filled(dot, 3.5, Color32::WHITE);
    let bx = r.right() - 13.0;
    p.vline(bx, y_of(1.0)..=y_of(0.0), Stroke::new(2.0, GRID_MAIN));
    p.circle_filled(
        Pos2::new(bx, y_of(v).clamp(r.top() + 5.0, r.bottom() - 5.0)),
        5.0,
        color,
    );
    p.text(
        r.left_top() + Vec2::new(6.0, 3.0),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(11.5),
        theme::TEXT,
    );
    if hovered {
        p.rect_stroke(r, 4.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_script::curve::Key as CurveKey;

    #[test]
    fn grid_steps_are_round_and_far_enough_apart() {
        assert_eq!(nice_step(100.0, 90.0), 1.0);
        assert_eq!(nice_step(400.0, 90.0), 0.5);
        assert_eq!(nice_step(1000.0, 90.0), 0.1);
        assert_eq!(nice_step(30.0, 90.0), 5.0);
        assert_eq!(num(0.25, 0.05), "0.25");
        assert_eq!(num(2.0, 1.0), "2");
        assert_eq!(num(-1e-12, 0.1), "0.0");
    }

    #[test]
    fn a_user_tangent_starts_from_the_slope_it_had() {
        let mut c = Curve::new(vec![
            CurveKey::new(0.0, 0.0),
            CurveKey::new(1.0, 0.5),
            CurveKey::new(2.0, 2.0),
        ]);
        let (s_in, s_out) = c.slopes(1);
        set_tangent(&mut c, 1, Tangent::User);
        assert_eq!(c.keys[1].tangent, Tangent::User);
        assert!((c.slopes(1).1 - (s_in + s_out) / 2.0).abs() < 1e-12);
        // Broken: each side keeps its slope, and one side changes alone.
        set_tangent(&mut c, 1, Tangent::Break);
        c.keys[1].slope_in = Some(-1.0);
        let (a, b) = c.slopes(1);
        assert_eq!(a, -1.0);
        assert!((b - (s_in + s_out) / 2.0).abs() < 1e-12);
        set_tangent(&mut c, 1, Tangent::Flat);
        assert_eq!(c.slopes(1), (0.0, 0.0));
        assert!(c.keys[1].slope_in.is_none() && c.keys[1].slope_out.is_none());
    }

    #[test]
    fn handles_show_only_where_the_curve_is_cubic() {
        let mut c = Curve::new(vec![
            CurveKey::new(0.0, 0.0),
            CurveKey::new(1.0, 1.0),
            CurveKey::new(2.0, 0.0),
        ]);
        let at = Pos2::new(100.0, 100.0);
        // The ends have one handle each; the middle two, level (a peak: auto is flat there).
        assert!(matches!(handles(&c, 0, at, 100.0, 100.0), (None, Some(_))));
        let (a, b) = handles(&c, 1, at, 100.0, 100.0);
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!((a.y - at.y).abs() < 1e-3 && (b.y - at.y).abs() < 1e-3);
        assert!(((b - at).length() - HANDLE_PX).abs() < 1e-3);
        c.keys[1].interp = Interp::Linear;
        assert!(matches!(handles(&c, 1, at, 100.0, 100.0), (Some(_), None)));
    }

    #[test]
    fn fit_shows_the_keys_and_the_timeline() {
        let mut tab = CurvesTab::default();
        let c = Curve::new(vec![CurveKey::new(0.5, 2.0), CurveKey::new(1.5, 4.0)]);
        fit(&mut tab, &c, None);
        assert!(tab.t_view.0 < 0.5 && tab.t_view.1 > 1.5);
        assert!(tab.v_view.0 < 2.0 && tab.v_view.1 > 4.0);
        let tl = TimelineInfo {
            name: "open".into(),
            time: 0.0,
            length: 3.0,
            playing: false,
            direction: 1.0,
            tracks: Vec::new(),
            events: Vec::new(),
        };
        fit(&mut tab, &c, Some(&tl));
        assert!(tab.t_view.0 < 0.0 && tab.t_view.1 > 3.0);
    }
}
