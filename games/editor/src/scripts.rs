//! The Scripts tab: the editor's Luau code editor, after Roblox Studio's script editor and
//! Godot's script workspace. The scripts folder with each file's state (running, failed, a
//! module), the code with syntax colors and line numbers, the file's functions to jump to,
//! find and go to line, and completion from the engine itself: every engine function with its
//! summary and example arguments, `vec3`, entities, `task`, `self`, script hooks and
//! component names (`engine_script::editing::vocabulary`, read from the same type definitions
//! luau-lsp checks against).
//!
//! Mistakes show where they are: a compile check marks a syntax error on its line as you type,
//! before you save; a call to an engine function that doesn't exist is underlined with the
//! nearest name; and while playing, the runtime's errors are marked on their line with how
//! often they repeated. Save (Ctrl+S) hot reloads the running game, which keeps every script's
//! state. While playing, a click in the gutter shows a line's values live (a logpoint, without
//! stopping the game) and Profile shows each line's share of the script time beside it. Under
//! the code, the Output (prints, warnings, errors; click one to open its line) and a command bar
//! that runs Luau in the game, as Roblox's.
//!
//! Scripts run only while playing ([`ScriptsHeld`] in edit mode) and Stop drops their VMs, so
//! every Play starts them afresh, as it does the scene.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::SystemTime;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, RichText, Stroke, Vec2};
use egui_dock::{DockState, NodeIndex};
use engine::core::registry::{call_api, unknown_name_error};
use engine::prelude::*;
use engine_script::editing::{self, FileMarks, OutputCursor, Word, WordKind};
use engine_script::{Script, ScriptRuntime, ScriptSettings, ScriptsHeld};
use serde_json::{Value, json};

use crate::editor::{self, Editor, Mode, Tab};
use crate::shots::{ShotScript, Step};
use crate::theme;
use crate::ui::EditorDock;

/// Scripting in the editor: scripts held still outside Play.
pub struct ScriptsTabPlugin;

impl Plugin for ScriptsTabPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ScriptsHeld)
            .init_resource::<ScriptsTab>()
            .add_systems(Update, hold_scripts.before(UpdateSet::Logic));
    }
}

/// Scripts run while playing (and stay paused with the game); Stop drops their VMs, so the
/// next Play starts every script from `start()`, as the scene starts from its snapshot.
fn hold_scripts(world: &mut World) {
    let Some(mode) = world.get_resource::<Editor>().map(|e| e.mode) else {
        return;
    };
    match (mode == Mode::Edit, world.contains_resource::<ScriptsHeld>()) {
        (true, false) => {
            world.insert_resource(ScriptsHeld);
            world.remove_resource::<ScriptRuntime>();
        }
        (false, true) => {
            world.remove_resource::<ScriptsHeld>();
        }
        _ => {}
    }
}

// ----------------------------------------------------------------------------- state

/// Width of the gutter left of the code: heat bar, markers, line numbers.
const GUTTER: f32 = 64.0;
/// Seconds between looks at the runtime (marks, output) and at the files on disk.
const POLL_SECS: f64 = 0.2;
const FILES_SECS: f64 = 1.0;
/// Output lines kept.
const KEEP_OUTPUT: usize = 400;

const CODE_BG: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
const GUTTER_BG: Color32 = Color32::from_rgb(0x13, 0x14, 0x16);
const CURRENT_LINE: Color32 = Color32::from_rgb(0x20, 0x22, 0x27);
const LINE_NO: Color32 = Color32::from_rgb(0x52, 0x57, 0x60);
const KEYWORD: Color32 = Color32::from_rgb(0xc6, 0x8f, 0xf0);
const STRING: Color32 = Color32::from_rgb(0x9c, 0xd0, 0x6c);
const NUMBER: Color32 = Color32::from_rgb(0xf2, 0xa6, 0x5e);
const COMMENT: Color32 = Color32::from_rgb(0x6a, 0x72, 0x80);
const SELF: Color32 = Color32::from_rgb(0xf0, 0x7a, 0x82);
const CALL: Color32 = Color32::from_rgb(0x6c, 0xb6, 0xff);
const ENGINE: Color32 = Color32::from_rgb(0x5a, 0xd6, 0xd2);
const ERROR: Color32 = Color32::from_rgb(0xff, 0x62, 0x5c);
const ERROR_BG: Color32 = Color32::from_rgba_premultiplied(0x30, 0x0c, 0x0c, 0x30);
const LINT: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x3c);
const VALUE: Color32 = Color32::from_rgb(0xff, 0xc9, 0x5c);
const VALUE_BG: Color32 = Color32::from_rgba_premultiplied(0x2c, 0x20, 0x08, 0x2c);
const FIND_BG: Color32 = Color32::from_rgb(0x5a, 0x4a, 0x14);
const HEAT: [Color32; 3] = [
    Color32::from_rgb(0x6e, 0x1c, 0x10),
    Color32::from_rgb(0xff, 0x6a, 0x1a),
    Color32::from_rgb(0xff, 0xdc, 0x6c),
];

/// The tab's state (a resource of its own, beside the editor's).
#[derive(Resource, Default)]
pub struct ScriptsTab {
    /// Open files, in tab order, and the one shown.
    pub docs: Vec<Doc>,
    pub current: usize,
    /// The file list's filter, the function list's.
    pub filter: String,
    pub fn_filter: String,
    files: Vec<FileRow>,
    files_next: f64,
    poll_next: f64,
    output: Vec<OutLine>,
    cursor: OutputCursor,
    /// The command bar's text, its history and where Up/Down are in it.
    pub command: String,
    commands: Vec<String>,
    command_at: Option<usize>,
    /// Run the command bar's text (as Enter does).
    pub run_command: bool,
    vocab: Option<Vocab>,
    completion: Option<Completion>,
    /// Open the completion list at the caret next frame (Ctrl+Space).
    pub complete_now: bool,
    /// The find bar's text while it is open, the go-to-line bar's.
    pub find: Option<String>,
    pub goto: Option<String>,
    /// A message over the code for a few seconds (text, until, an error).
    toast: Option<(String, f64, bool)>,
    /// Recording a profile (restarted on each Play).
    pub profiling: bool,
    /// The VS Code strip under the tool bar is open, and what it last reported.
    pub vscode: bool,
    vscode_status: String,
    /// The scripts were running last frame.
    playing: bool,
    /// The new-script name being typed (the New button's field).
    new_name: Option<String>,
}

/// An open file.
pub struct Doc {
    /// Relative to the scripts folder: `examples/lamp.luau`.
    pub path: String,
    pub text: String,
    /// The text as last read from or written to disk.
    saved: String,
    /// The file's modification time when read.
    stamp: Option<SystemTime>,
    /// Changed on disk while this had unsaved edits.
    conflict: bool,
    marks: FileMarks,
    /// The compile check of `text`, and the text it checked.
    syntax: Option<(u32, String)>,
    checked: String,
    /// Logpoints by the line they show on (1-based).
    logpoints: BTreeMap<u32, Logpoint>,
    /// Put the caret here (a char index) and scroll to it, next frame.
    pub jump: Option<usize>,
    /// The caret last frame (char index).
    caret: usize,
}

impl Doc {
    fn dirty(&self) -> bool {
        self.text != self.saved
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// A logpoint: shown beside `shown`, registered on `at` (the line after an assignment, since
/// a logpoint runs before its line does) with a message of `{expressions}`.
#[derive(Clone)]
struct Logpoint {
    at: u32,
    message: String,
}

struct FileRow {
    path: String,
    marks: FileMarks,
}

#[derive(Clone, Copy, PartialEq)]
enum OutKind {
    Print,
    Warn,
    Error,
    Command,
    Result,
    Note,
}

struct OutLine {
    text: String,
    kind: OutKind,
    at: Option<(String, u32)>,
    /// The same line again and again (an error every frame) shows once, counted.
    count: u32,
}

/// What completion knows, built once from the running engine.
pub(crate) struct Vocab {
    words: Vec<Word>,
    /// Engine globals and API namespaces, colored apart.
    engine: HashSet<String>,
    /// API namespaces whose calls are checked against the registry.
    api_tops: HashSet<String>,
    /// Every (scope, name) there is.
    known: HashSet<(String, String)>,
    /// Every engine function's full name.
    api_names: Vec<String>,
}

impl Vocab {
    pub(crate) fn new(world: &World) -> Self {
        let words = editing::vocabulary(world);
        let mut engine: HashSet<String> = [
            "vec3", "quat", "world", "task", "ease", "input", "api", "require", "print", "warn",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let builtin: HashSet<&str> = words
            .iter()
            .filter(|w| w.scope.is_empty() && w.detail != "engine functions")
            .map(|w| w.name.as_str())
            .collect();
        let mut api_tops = HashSet::new();
        let mut api_names = Vec::new();
        for w in &words {
            if w.scope.is_empty() && w.kind == WordKind::Namespace && w.detail == "engine functions"
            {
                engine.insert(w.name.clone());
                if !builtin.contains(w.name.as_str()) {
                    api_tops.insert(w.name.clone());
                }
            }
        }
        for w in &words {
            let top = w.scope.split('.').next().unwrap_or("");
            if w.kind == WordKind::Function && engine.contains(top) && !w.scope.is_empty() {
                api_names.push(format!("{}.{}", w.scope, w.name));
            }
        }
        let known = words
            .iter()
            .map(|w| (w.scope.clone(), w.name.clone()))
            .collect();
        Self {
            words,
            engine,
            api_tops,
            known,
            api_names,
        }
    }
}

/// The open completion list.
struct Completion {
    /// Where the typed prefix starts (char index) and what it is.
    start: usize,
    prefix: String,
    items: Vec<Item>,
    selected: usize,
}

#[derive(Clone)]
struct Item {
    label: String,
    insert: String,
    kind: WordKind,
    /// "task.wait", "Entity:set".
    full: String,
    detail: String,
    doc: String,
}

// ----------------------------------------------------------------------------- the tab

/// Draws the Scripts tab.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let mut tab = world.remove_resource::<ScriptsTab>().unwrap_or_default();
    let now = editor::now(world);
    if tab.vocab.is_none() {
        tab.vocab = Some(Vocab::new(world));
    }
    poll(world, editor, &mut tab, now);

    egui::Panel::left("script files")
        .resizable(true)
        .default_size(220.0)
        .size_range(160.0..=380.0)
        .show(ui, |ui| sidebar(ui, world, editor, &mut tab, now));
    egui::Panel::bottom("script output")
        .resizable(true)
        .default_size(230.0)
        .size_range(80.0..=480.0)
        .frame(
            egui::Frame::new()
                .fill(theme::PANEL)
                .inner_margin(egui::Margin::symmetric(6, 4)),
        )
        .show(ui, |ui| {
            // Fill the panel, or it shrinks to its lines and keeps that height.
            ui.set_min_height(ui.available_height());
            output(ui, world, &mut tab)
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            tool_bar(ui, world, &mut tab, now);
            if tab.docs.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(theme::dim(
                        "Open a script from the list, or make one with New. Scripts run on an \
                         entity through its Script component, and start when you press Play.",
                    ));
                });
                return;
            }
            tab.current = tab.current.min(tab.docs.len() - 1);
            find_bars(ui, &mut tab);
            let problem = first_problem(&tab.docs[tab.current], tab.vocab.as_ref());
            let status_h = if problem.is_some() { 46.0 } else { 22.0 };
            let code_h = (ui.available_height() - status_h).max(60.0);
            ui.allocate_ui(Vec2::new(ui.available_width(), code_h), |ui| {
                code_view(ui, world, &mut tab, now);
            });
            if let Some((line, text)) = problem {
                error_bar(ui, &mut tab, line, &text);
            }
            status_line(ui, editor, &mut tab);
        });
    if let Some((text, until, error)) = &tab.toast {
        if now < *until {
            toast(ui, text, *error);
        } else {
            tab.toast = None;
        }
    }
    world.insert_resource(tab);
}

/// Opens `path` (or shows it if open) and puts the caret on `line` (1-based) if given.
pub fn open(world: &World, tab: &mut ScriptsTab, path: &str, line: Option<u32>) {
    let path = engine_script::normalize(path);
    let i = match tab.docs.iter().position(|d| d.path == path) {
        Some(i) => i,
        None => {
            let full = engine_script::scripts_root(world).join(&path);
            let text = std::fs::read_to_string(&full).unwrap_or_default();
            tab.docs.push(Doc {
                path: path.clone(),
                saved: text.clone(),
                text,
                stamp: modified(&full),
                conflict: false,
                marks: editing::file_marks(world, &path),
                syntax: None,
                checked: String::new(),
                logpoints: BTreeMap::new(),
                jump: None,
                caret: 0,
            });
            tab.docs.len() - 1
        }
    };
    tab.current = i;
    tab.completion = None;
    if let Some(line) = line {
        let doc = &mut tab.docs[i];
        doc.jump = Some(line_start_char(&doc.text, line.saturating_sub(1) as usize));
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Looks at the runtime and the disk a few times a second: marks, output, hot reloads.
fn poll(world: &mut World, editor: &Editor, tab: &mut ScriptsTab, now: f64) {
    let playing = editor.mode != Mode::Edit;
    if playing != tab.playing {
        tab.playing = playing;
        if playing {
            // A fresh runtime: put the logpoints and the recording back.
            for i in 0..tab.docs.len() {
                apply_logpoints(world, &tab.docs[i]);
            }
            if tab.profiling {
                let _ = call_api(world, "script.profile", json!({"seconds": 0}));
            }
            push(tab, OutKind::Note, "Play: scripts start.".into(), None);
        } else {
            tab.cursor = OutputCursor::default();
            push(
                tab,
                OutKind::Note,
                "Stop: scripts stopped; the next Play starts them afresh.".into(),
                None,
            );
        }
    }
    if now < tab.poll_next {
        return;
    }
    tab.poll_next = now + POLL_SECS;
    if let Some(rt) = world.get_resource::<ScriptRuntime>() {
        let mut cursor = std::mem::take(&mut tab.cursor);
        let lines = rt.output_since(&mut cursor);
        tab.cursor = cursor;
        for l in lines {
            let kind = if l.error {
                OutKind::Error
            } else if l.text.contains("warning: ") {
                OutKind::Warn
            } else {
                OutKind::Print
            };
            push(tab, kind, l.text, l.at);
        }
    }
    let root = engine_script::scripts_root(world);
    let mut toasts = Vec::new();
    for doc in &mut tab.docs {
        let marks = editing::file_marks(world, &doc.path);
        if marks.version > doc.marks.version && doc.marks.version > 0 && marks.error.is_none() {
            toasts.push((
                format!(
                    "Hot reloaded {} (version {}): {} running script{} kept {} state",
                    doc.name(),
                    marks.version,
                    marks.instances,
                    if marks.instances == 1 { "" } else { "s" },
                    if marks.instances == 1 { "its" } else { "their" },
                ),
                false,
            ));
        }
        if let Some(e) = &marks.error
            && doc.marks.error.as_ref() != Some(e)
        {
            toasts.push((
                format!(
                    "{} didn't load{}: {} (the version before keeps running)",
                    doc.name(),
                    e.line.map(|l| format!(" (line {l})")).unwrap_or_default(),
                    e.message
                ),
                true,
            ));
        }
        doc.marks = marks;
        // Changed on disk (another editor, the AI bridge): take it unless edited here.
        let stamp = modified(&root.join(&doc.path));
        if stamp != doc.stamp {
            doc.stamp = stamp;
            if let Ok(text) = std::fs::read_to_string(root.join(&doc.path))
                && text != doc.saved
            {
                if doc.dirty() {
                    doc.conflict = true;
                } else {
                    doc.text = text.clone();
                    doc.saved = text;
                }
            }
        }
    }
    for (text, error) in toasts {
        tab.toast = Some((text, now + 4.0, error));
    }
    if now >= tab.files_next {
        tab.files_next = now + FILES_SECS;
        let mut paths = Vec::new();
        list_files(&root, &root, &mut paths);
        paths.sort();
        tab.files = paths
            .into_iter()
            .map(|path| FileRow {
                marks: editing::file_marks(world, &path),
                path,
            })
            .collect();
    }
}

/// Every `.luau`/`.lua` file under `dir`, relative to `root` (the generated type definitions
/// left out).
fn list_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            list_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "luau" || e == "lua")
            && !path.to_string_lossy().ends_with(".d.luau")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn push(tab: &mut ScriptsTab, kind: OutKind, text: String, at: Option<(String, u32)>) {
    if let Some(last) = tab.output.last_mut()
        && last.text == text
        && last.kind == kind
        && kind != OutKind::Command
    {
        last.count += 1;
        return;
    }
    tab.output.push(OutLine {
        text,
        kind,
        at,
        count: 1,
    });
    if tab.output.len() > KEEP_OUTPUT {
        let extra = tab.output.len() - KEEP_OUTPUT;
        tab.output.drain(..extra);
    }
}

fn apply_logpoints(world: &mut World, doc: &Doc) {
    let _ = call_api(
        world,
        "script.logpoint",
        json!({"path": doc.path, "clear": true}),
    );
    for lp in doc.logpoints.values() {
        let _ = call_api(
            world,
            "script.logpoint",
            json!({"path": doc.path, "line": lp.at, "message": lp.message}),
        );
    }
}

fn save(world: &mut World, tab: &mut ScriptsTab, now: f64) {
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    match call_api(
        world,
        "script.write",
        json!({"path": doc.path, "source": doc.text}),
    ) {
        Ok(_) => {
            doc.saved = doc.text.clone();
            doc.conflict = false;
            doc.stamp = modified(&engine_script::scripts_root(world).join(&doc.path));
            let what = if tab.playing && doc.marks.loaded {
                "saved; the running game reloads it"
            } else {
                "saved"
            };
            tab.toast = Some((format!("{} {what}", doc.name()), now + 2.5, false));
        }
        Err(e) => tab.toast = Some((format!("Can't save: {e}"), now + 5.0, true)),
    }
}

// ----------------------------------------------------------------------------- sidebar

fn sidebar(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut ScriptsTab,
    now: f64,
) {
    // The selection's script, or attaching the open file to it.
    if let Some(&e) = editor.selection.last() {
        let name = world
            .get::<Name>(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|| format!("{e}"));
        match world.get::<Script>(e).map(|s| s.path.clone()) {
            Some(path) => {
                if ui
                    .button(format!("📜 {name}'s script"))
                    .on_hover_text(format!("Open {path}"))
                    .clicked()
                {
                    open(world, tab, &path, None);
                }
            }
            None => {
                if let Some(doc) = tab.docs.get(tab.current)
                    && ui
                        .button(format!("➕ Run {} on {name}", doc.name()))
                        .on_hover_text("Add a Script component with this file to the selection")
                        .clicked()
                {
                    let script = Script::new(doc.path.clone());
                    editor::apply(
                        world,
                        editor,
                        crate::undo::Command::Component {
                            entity: e,
                            name: "Script".into(),
                            before: None,
                            after: serde_json::to_value(script).ok(),
                        },
                    );
                    editor.history.interaction += 1;
                }
            }
        }
        ui.add_space(2.0);
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("SCRIPTS")
                .small()
                .strong()
                .color(theme::TEXT_DIM),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("New").clicked() {
                tab.new_name = Some("examples/new_script.luau".into());
            }
        });
    });
    if let Some(name) = &mut tab.new_name {
        let mut create = false;
        ui.horizontal(|ui| {
            let r =
                ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width() - 52.0));
            create = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            create |= ui.small_button("Create").clicked();
        });
        if create {
            let path = name.clone();
            let class = class_name(&path);
            let source = format!(
                "--!strict\nlocal {class} = {{}}\n{class}.props = {{}} -- defaults; the Script component's props override them\n\n\
                 function {class}:start()\n\tprint(`{{self.entity.name}} starts`)\nend\n\n\
                 function {class}:update(dt: number)\nend\n\nreturn {class}\n"
            );
            match call_api(
                world,
                "script.write",
                json!({"path": path, "source": source}),
            ) {
                Ok(_) => {
                    tab.new_name = None;
                    tab.files_next = 0.0;
                    open(world, tab, &path, Some(5));
                }
                Err(e) => tab.toast = Some((format!("Can't make it: {e}"), now + 4.0, true)),
            }
        }
    }
    ui.add(
        egui::TextEdit::singleline(&mut tab.filter)
            .hint_text("🔍 Filter")
            .desired_width(f32::INFINITY),
    );
    let half = (ui.available_height() * 0.55).max(120.0);
    let mut open_path = None;
    egui::ScrollArea::vertical()
        .id_salt("script file list")
        .max_height(half)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let filter = tab.filter.to_lowercase();
            let mut folders: BTreeMap<&str, Vec<&FileRow>> = BTreeMap::new();
            for f in &tab.files {
                if !filter.is_empty() && !f.path.to_lowercase().contains(&filter) {
                    continue;
                }
                let dir = f.path.rsplit_once('/').map_or("", |(d, _)| d);
                folders.entry(dir).or_default().push(f);
            }
            let current = tab.docs.get(tab.current).map(|d| d.path.as_str());
            for (dir, rows) in folders {
                let row_ui = |ui: &mut egui::Ui| {
                    for f in rows {
                        if file_row(ui, f, current == Some(f.path.as_str())) {
                            open_path = Some(f.path.clone());
                        }
                    }
                };
                if dir.is_empty() {
                    row_ui(ui);
                } else {
                    egui::CollapsingHeader::new(format!("📁 {dir}"))
                        .default_open(true)
                        .show(ui, row_ui);
                }
            }
        });
    if let Some(path) = open_path {
        open(world, tab, &path, None);
    }
    ui.separator();
    ui.label(
        RichText::new("FUNCTIONS")
            .small()
            .strong()
            .color(theme::TEXT_DIM),
    );
    ui.add(
        egui::TextEdit::singleline(&mut tab.fn_filter)
            .hint_text("🔍 Filter functions")
            .desired_width(f32::INFINITY),
    );
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    let filter = tab.fn_filter.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("script functions")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in functions(&doc.text) {
                if !filter.is_empty() && !f.label.to_lowercase().contains(&filter) {
                    continue;
                }
                let color = if f.hook { ENGINE } else { CALL };
                let text = RichText::new(&f.label).monospace().color(color);
                let r = ui
                    .add(egui::Button::selectable(false, text))
                    .on_hover_text(if f.hook {
                        "A hook: the engine calls it"
                    } else {
                        "Your function"
                    });
                if r.clicked() {
                    doc.jump = Some(line_start_char(&doc.text, f.line));
                }
            }
        });
}

/// `examples/new_script.luau` → `NewScript`.
fn class_name(path: &str) -> String {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".luau")
        .trim_end_matches(".lua");
    let mut out = String::new();
    for part in stem.split(|c: char| !c.is_alphanumeric()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "Script");
    }
    out
}

/// One file in the list: a dot for its state, its name, how many scripts run it. True when
/// clicked.
fn file_row(ui: &mut egui::Ui, f: &FileRow, current: bool) -> bool {
    let m = &f.marks;
    let failed = m.error.is_some() || m.fault.is_some();
    let (dot, state) = if failed {
        (ERROR, "has an error")
    } else if m.instances > 0 {
        (theme::PLAY, "running")
    } else if m.loaded {
        (ENGINE, "loaded (a module)")
    } else {
        (LINE_NO, "not running")
    };
    let name = f.path.rsplit('/').next().unwrap_or(&f.path);
    let mut text = egui::text::LayoutJob::default();
    let body = FontId::proportional(13.0);
    // Room for the state dot, painted after (a circle, not a font glyph).
    text.append("     ", 0.0, TextFormat::simple(body.clone(), dot));
    text.append(name, 0.0, TextFormat::simple(body.clone(), theme::TEXT));
    if m.instances > 0 {
        text.append(
            &format!("  ×{}", m.instances),
            0.0,
            TextFormat::simple(FontId::proportional(11.0), theme::TEXT_DIM),
        );
    }
    let r = ui
        .add(egui::Button::selectable(current, text))
        .on_hover_text(format!("{}: {state}", f.path));
    let at = Pos2::new(
        r.rect.left() + ui.spacing().button_padding.x + 5.0,
        r.rect.center().y,
    );
    ui.painter().circle_filled(at, 3.5, dot);
    r.clicked()
}

struct Function {
    /// 0-based.
    line: usize,
    label: String,
    hook: bool,
}

/// The functions a file defines: `Lamp:update`, `wave.bob`, `local helper`.
fn functions(text: &str) -> Vec<Function> {
    let hooks: HashSet<&str> = [
        "start",
        "update",
        "fixed_update",
        "parallel_update",
        "on_interact",
        "on_contact",
        "on_message",
        "on_props",
        "on_reload",
        "on_destroy",
        "on_world_shift",
    ]
    .into();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        let (rest, local) = if let Some(r) = t.strip_prefix("local function ") {
            (r, true)
        } else if let Some(r) = t.strip_prefix("function ") {
            (r, false)
        } else {
            continue;
        };
        let Some((name, args)) = rest.split_once('(') else {
            continue;
        };
        let name = name.trim();
        let method = name.rsplit([':', '.']).next().unwrap_or(name);
        let args = args.split(')').next().unwrap_or("");
        let args: Vec<&str> = args
            .split(',')
            .map(|a| a.split(':').next().unwrap_or("").trim())
            .filter(|a| !a.is_empty())
            .collect();
        out.push(Function {
            line: i,
            label: format!(
                "{}{name}({})",
                if local { "local " } else { "" },
                args.join(", ")
            ),
            hook: name.contains(':') && hooks.contains(method),
        });
    }
    out
}

// ----------------------------------------------------------------------------- tool bar

fn tool_bar(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab, now: f64) {
    egui::Frame::new()
        .fill(theme::BG)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Open files.
                let mut close = None;
                for (i, doc) in tab.docs.iter().enumerate() {
                    let current = i == tab.current;
                    let label = format!("{}{}", doc.name(), if doc.dirty() { " •" } else { "" });
                    let mut text = RichText::new(label);
                    if doc.marks.error.is_some()
                        || doc.marks.fault.is_some()
                        || doc.syntax.is_some()
                    {
                        text = text.color(ERROR);
                    } else if current {
                        text = text.color(Color32::WHITE);
                    }
                    let r = ui
                        .add(egui::Button::selectable(current, text))
                        .on_hover_text(format!("{}  (middle click closes)", doc.path));
                    if r.clicked() {
                        tab.current = i;
                        tab.completion = None;
                    }
                    if r.middle_clicked() {
                        close = Some(i);
                    }
                }
                if let Some(i) = close {
                    tab.docs.remove(i);
                    if tab.current >= i && tab.current > 0 {
                        tab.current -= 1;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(tab.vscode, "VS Code")
                        .on_hover_text(
                            "Debug these files in VS Code: breakpoints, stepping, watches",
                        )
                        .clicked()
                    {
                        tab.vscode = !tab.vscode;
                    }
                    let profile = ui
                        .selectable_label(tab.profiling, "🔥 Profile")
                        .on_hover_text(
                            "Record where scripts spend their time while playing; each line's \
                             share shows beside it",
                        );
                    if profile.clicked() {
                        tab.profiling = !tab.profiling;
                        let args = if tab.profiling {
                            json!({"seconds": 0})
                        } else {
                            json!({"stop": true})
                        };
                        let _ = call_api(world, "script.profile", args);
                    }
                    if ui
                        .button("Go to")
                        .on_hover_text("Go to line (Ctrl+G)")
                        .clicked()
                    {
                        tab.goto = Some(String::new());
                    }
                    if ui.button("🔍").on_hover_text("Find (Ctrl+F)").clicked() {
                        tab.find = Some(tab.find.take().unwrap_or_default());
                    }
                    let dirty = tab.docs.get(tab.current).is_some_and(Doc::dirty);
                    if ui
                        .add_enabled(dirty, egui::Button::new("Revert"))
                        .on_hover_text("Back to the file on disk")
                        .clicked()
                        && let Some(doc) = tab.docs.get_mut(tab.current)
                    {
                        doc.text = doc.saved.clone();
                        doc.conflict = false;
                    }
                    if ui
                        .add_enabled(dirty, egui::Button::new("💾 Save"))
                        .on_hover_text("Save (Ctrl+S); a running game hot reloads it")
                        .clicked()
                    {
                        save(world, tab, now);
                    }
                });
            });
        });
    if tab.vscode {
        egui::Frame::new()
            .fill(theme::WIDGET)
            .inner_margin(egui::Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        "In VS Code: the Luau Language Server extension for completion and \
                         checks, and the engine's debugger (crates/script/vscode) for \
                         breakpoints.",
                    );
                    if ui
                        .button("Write types")
                        .on_hover_text("script.types: engine.d.luau for luau-lsp")
                        .clicked()
                    {
                        tab.vscode_status = match call_api(world, "script.types", json!({})) {
                            Ok(v) => {
                                format!("Wrote {}", v["file"].as_str().unwrap_or("engine.d.luau"))
                            }
                            Err(e) => e,
                        };
                    }
                    if ui
                        .button("Start debugger")
                        .on_hover_text(
                            "script.debug: listen on localhost:4711 for VS Code to attach",
                        )
                        .clicked()
                    {
                        tab.vscode_status = match call_api(
                            world,
                            "script.debug",
                            json!({"port": 4711}),
                        ) {
                            Ok(v) => format!(
                                "Listening on localhost:{}; attach with \"Attach to the engine\"",
                                v["port"].as_u64().unwrap_or(4711)
                            ),
                            Err(e) => e,
                        };
                    }
                    if !tab.vscode_status.is_empty() {
                        ui.label(theme::dim(&tab.vscode_status));
                    }
                });
            });
    }
    if let Some(doc) = tab.docs.get_mut(tab.current)
        && doc.conflict
    {
        egui::Frame::new()
            .fill(Color32::from_rgb(0x3a, 0x2c, 0x10))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("The file changed on disk while you edited it here.");
                    if ui.button("Take the file's").clicked() {
                        let full = engine_script::scripts_root(world).join(&doc.path);
                        if let Ok(text) = std::fs::read_to_string(full) {
                            doc.text = text.clone();
                            doc.saved = text;
                        }
                        doc.conflict = false;
                    }
                    if ui.button("Keep mine").clicked() {
                        doc.conflict = false;
                    }
                });
            });
    }
}

fn find_bars(ui: &mut egui::Ui, tab: &mut ScriptsTab) {
    let ctrl = |k: Key| ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
    if ctrl(Key::F) {
        tab.find = Some(tab.find.take().unwrap_or_default());
    }
    if ctrl(Key::G) {
        tab.goto = Some(String::new());
    }
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    if let Some(find) = &mut tab.find {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label("Find");
            let r = ui.add(egui::TextEdit::singleline(find).desired_width(220.0));
            let matches = find_all(&doc.text, find);
            ui.label(theme::dim(if find.is_empty() {
                String::new()
            } else {
                format!("{} found", matches.len())
            }));
            let next = (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                || ui.small_button("Next").clicked();
            if next && !matches.is_empty() {
                let caret_byte = char_to_byte(&doc.text, doc.caret);
                let m = matches
                    .iter()
                    .find(|(a, _)| *a > caret_byte)
                    .unwrap_or(&matches[0]);
                doc.jump = Some(doc.text[..m.0].chars().count());
            }
            close = ui.small_button("✖").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
        });
        if close {
            tab.find = None;
        }
    }
    if let Some(goto) = &mut tab.goto {
        let mut done = false;
        ui.horizontal(|ui| {
            ui.label("Go to line");
            let r = ui.add(egui::TextEdit::singleline(goto).desired_width(70.0));
            if !r.has_focus() && goto.is_empty() {
                r.request_focus();
            }
            if r.lost_focus() {
                if let Ok(n) = goto.trim().parse::<usize>() {
                    doc.jump = Some(line_start_char(&doc.text, n.saturating_sub(1)));
                }
                done = true;
            }
        });
        if done {
            tab.goto = None;
        }
    }
}

// ----------------------------------------------------------------------------- the code

fn code_view(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab, now: f64) {
    let vocab = tab.vocab.take().expect("built before drawing");
    let i = tab.current;
    let id = egui::Id::new(("script code", tab.docs[i].path.as_str()));
    let font = FontId::monospace(13.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));

    // Keys the completion list and the editor take before the text field sees them.
    let focused = ui.memory(|m| m.has_focus(id));
    if focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
        save(world, tab, now);
    }
    if focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Space)) {
        tab.complete_now = true;
    }
    let mut accept = None;
    let mut close_list = false;
    if focused && let Some(c) = &mut tab.completion {
        let (up, down, enter, close) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Tab)
                    || i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            c.selected = c.selected.checked_sub(1).unwrap_or(c.items.len() - 1);
        }
        if down {
            c.selected = (c.selected + 1) % c.items.len();
        }
        if enter {
            accept = Some(c.selected);
        }
        close_list = close;
    }
    if close_list {
        tab.completion = None;
    }
    if let Some(n) = accept
        && let Some(c) = tab.completion.take()
    {
        let doc = &mut tab.docs[i];
        let item = &c.items[n];
        let a = char_to_byte(&doc.text, c.start);
        let b = char_to_byte(&doc.text, doc.caret);
        doc.text.replace_range(a..b, &item.insert);
        doc.jump = Some(c.start + item.insert.chars().count());
    }

    let doc = &mut tab.docs[i];
    if doc.checked != doc.text {
        doc.syntax = editing::check(&doc.text);
        doc.checked = doc.text.clone();
    }
    let mut jump_line = None;
    if let Some(at) = doc.jump.take() {
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(at))));
        state.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
        jump_line = Some(line_of(&doc.text, at));
    }

    let find = tab.find.clone().filter(|f| !f.is_empty());
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
        let job = highlight(buf.as_str(), &font, &vocab, find.as_deref());
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut output = None;
    egui::ScrollArea::both()
        .id_salt(("script scroll", doc.path.as_str()))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let backdrop = ui.painter().add(egui::Shape::Noop);
            let rows = ((ui.available_height() / row_h) as usize).max(4);
            let out = egui::TextEdit::multiline(&mut doc.text)
                .id(id)
                .code_editor()
                .lock_focus(true)
                .desired_width(f32::INFINITY)
                .desired_rows(rows)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    left: (GUTTER + 8.0) as i8,
                    right: 8,
                    top: 4,
                    bottom: 4,
                }))
                .layouter(&mut layouter)
                .show(ui);
            let caret = out
                .cursor_range
                .map(|r| r.primary.index.0)
                .unwrap_or(doc.caret);
            let lines = line_rects(&out.galley, out.galley_pos);
            let current_line = line_of(&doc.text, caret);
            paint_backdrop(ui, backdrop, &out.response.rect, &lines, current_line, doc);
            paint_gutter(
                ui,
                &out.response.rect,
                &lines,
                current_line,
                doc,
                &font,
                row_h,
            );
            let lints = lints(&doc.text, &vocab);
            paint_notes(ui, &lines, doc, &lints, &font);
            if let Some(line) = jump_line
                && let Some(r) = lines.get(line)
            {
                ui.scroll_to_rect(r.expand(row_h * 3.0), Some(egui::Align::Center));
            }
            // Gutter clicks: a logpoint on that line, or off again.
            {
                let gutter = Rect::from_min_max(
                    out.response.rect.min,
                    Pos2::new(
                        out.response.rect.left() + GUTTER,
                        out.response.rect.bottom(),
                    ),
                );
                let click = ui.interact(gutter, id.with("gutter"), egui::Sense::click());
                if click.clicked()
                    && let Some(pos) = click.interact_pointer_pos()
                    && let Some(line) = lines
                        .iter()
                        .position(|r| r.top() <= pos.y && pos.y < r.bottom())
                {
                    toggle_logpoint(world, doc, line);
                }
                let _ = click.on_hover_text("Click a line to show its values live while playing");
            }
            output = Some((
                out.response.changed(),
                caret,
                out.galley_pos,
                out.galley.clone(),
            ));
        });
    let Some((changed, caret, galley_pos, galley)) = output else {
        tab.vocab = Some(vocab);
        return;
    };
    let moved = caret != doc.caret;
    doc.caret = caret;

    // Completion: opened by typing a word, `.`, `:` or a component's quote; Ctrl+Space.
    let typed = changed && moved;
    if typed || tab.complete_now {
        let explicit = std::mem::take(&mut tab.complete_now);
        tab.completion = complete(&doc.text, caret, &vocab, explicit);
    } else if moved {
        tab.completion = None;
    }
    let clicked = tab.completion.as_ref().and_then(|c| {
        let at = galley.pos_from_cursor(CCursor::new(c.start));
        let pos = galley_pos + at.left_bottom().to_vec2() + Vec2::new(-4.0, 2.0);
        completion_popup(ui, c, pos)
    });
    if let Some(n) = clicked {
        accept_later(tab, n);
    }
    tab.vocab = Some(vocab);
}

/// A click on a completion row: applied like Enter on the next frame.
fn accept_later(tab: &mut ScriptsTab, n: usize) {
    if let Some(c) = tab.completion.take() {
        let doc = &mut tab.docs[tab.current];
        let item = &c.items[n];
        let a = char_to_byte(&doc.text, c.start);
        let b = char_to_byte(&doc.text, doc.caret);
        doc.text.replace_range(a..b, &item.insert);
        doc.jump = Some(c.start + item.insert.chars().count());
    }
}

/// Screen rects of each line (0-based), from the laid out text.
fn line_rects(galley: &egui::Galley, origin: Pos2) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut start_new = true;
    for row in &galley.rows {
        let r = row.rect().translate(origin.to_vec2());
        if start_new {
            out.push(r);
        } else if let Some(last) = out.last_mut() {
            *last = last.union(r);
        }
        start_new = row.ends_with_newline;
    }
    out
}

/// The 0-based line of a char index.
fn line_of(text: &str, char_index: usize) -> usize {
    text.chars().take(char_index).filter(|&c| c == '\n').count()
}

/// The char index where a 0-based line starts (the end of the text past the last line).
fn line_start_char(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut seen = 0;
    for (i, c) in text.chars().enumerate() {
        if c == '\n' {
            seen += 1;
            if seen == line {
                return i + 1;
            }
        }
    }
    text.chars().count()
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(b, _)| b)
}

/// The line under each one's marks: the caret's line, errors, logpoints.
fn paint_backdrop(
    ui: &egui::Ui,
    slot: egui::layers::ShapeIdx,
    area: &Rect,
    lines: &[Rect],
    current: usize,
    doc: &Doc,
) {
    let width = area.width().max(ui.clip_rect().width()) + 2000.0;
    let row = |r: &Rect| {
        Rect::from_min_size(
            Pos2::new(area.left() + GUTTER, r.top()),
            Vec2::new(width, r.height()),
        )
    };
    let mut shapes = vec![
        egui::Shape::rect_filled(
            Rect::from_min_size(
                area.min,
                Vec2::new(width, area.height().max(ui.clip_rect().height())),
            ),
            0.0,
            CODE_BG,
        ),
        egui::Shape::rect_filled(
            Rect::from_min_size(
                area.min,
                Vec2::new(GUTTER, area.height().max(ui.clip_rect().height())),
            ),
            0.0,
            GUTTER_BG,
        ),
    ];
    if let Some(r) = lines.get(current) {
        shapes.push(egui::Shape::rect_filled(row(r), 0.0, CURRENT_LINE));
    }
    for line in doc.logpoints.keys() {
        if let Some(r) = lines.get(*line as usize - 1) {
            shapes.push(egui::Shape::rect_filled(row(r), 0.0, VALUE_BG));
        }
    }
    for line in error_lines(doc) {
        if let Some(r) = lines.get(line as usize - 1) {
            shapes.push(egui::Shape::rect_filled(row(r), 0.0, ERROR_BG));
        }
    }
    ui.painter().set(slot, egui::Shape::Vec(shapes));
}

/// Lines (1-based) with an error on them: the compile check's, the load error's, the last
/// runtime error's.
fn error_lines(doc: &Doc) -> Vec<u32> {
    let mut out = Vec::new();
    if let Some((line, _)) = &doc.syntax {
        out.push((*line).max(1));
    }
    // Marks from the runtime belong to the text on disk; with unsaved edits they may be off.
    for m in [&doc.marks.error, &doc.marks.fault].into_iter().flatten() {
        if let Some(line) = m.line
            && doc.syntax.is_none()
        {
            out.push(line);
        }
    }
    out
}

fn heat_color(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0) * 2.0;
    let (a, b, k) = if t < 1.0 {
        (HEAT[0], HEAT[1], t)
    } else {
        (HEAT[1], HEAT[2], t - 1.0)
    };
    a.lerp_to_gamma(b, k)
}

/// Line numbers, the profiler's heat bars and the markers (error, logpoint).
fn paint_gutter(
    ui: &egui::Ui,
    area: &Rect,
    lines: &[Rect],
    current: usize,
    doc: &Doc,
    font: &FontId,
    row_h: f32,
) {
    let painter = ui.painter();
    let max_heat = doc
        .marks
        .heat
        .values()
        .cloned()
        .fold(0.0f32, f32::max)
        .max(0.01);
    let errors = error_lines(doc);
    let numbers = FontId::monospace(font.size * 0.92);
    for (n, r) in lines.iter().enumerate() {
        if r.bottom() < ui.clip_rect().top() || r.top() > ui.clip_rect().bottom() {
            continue;
        }
        let line = n as u32 + 1;
        let color = if n == current { theme::TEXT } else { LINE_NO };
        painter.text(
            Pos2::new(area.left() + GUTTER - 8.0, r.center().y),
            Align2::RIGHT_CENTER,
            line.to_string(),
            numbers.clone(),
            color,
        );
        if let Some(&pct) = doc.marks.heat.get(&line) {
            let t = (pct / max_heat).sqrt();
            let w = 3.0 + 13.0 * t;
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(area.left() + 2.0, r.top() + 2.0),
                    Vec2::new(w, (r.height() - 4.0).max(2.0)),
                ),
                1.5,
                heat_color(t),
            );
        }
        let marker = Pos2::new(area.left() + 24.0, r.center().y);
        if errors.contains(&line) {
            painter.circle_filled(marker, row_h * 0.26, ERROR);
        } else if doc.logpoints.contains_key(&line) {
            let s = row_h * 0.28;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    marker + Vec2::new(0.0, -s),
                    marker + Vec2::new(s, 0.0),
                    marker + Vec2::new(0.0, s),
                    marker + Vec2::new(-s, 0.0),
                ],
                VALUE,
                Stroke::NONE,
            ));
        }
    }
}

/// Notes after the end of lines, as Error Lens draws them: errors, unknown engine functions,
/// logpoint values, heat.
fn paint_notes(ui: &egui::Ui, lines: &[Rect], doc: &Doc, lints: &[Lint], font: &FontId) {
    let painter = ui.painter();
    let small = FontId::monospace(font.size * 0.92);
    let mut notes: BTreeMap<usize, Vec<(String, Color32)>> = BTreeMap::new();
    if let Some((line, message)) = &doc.syntax {
        notes
            .entry((*line).max(1) as usize - 1)
            .or_default()
            .push((format!("✖ {message}"), ERROR));
    }
    if doc.syntax.is_none() {
        if let Some(e) = &doc.marks.error
            && let Some(line) = e.line
        {
            notes
                .entry(line as usize - 1)
                .or_default()
                .push((format!("✖ didn't load: {}", e.message), ERROR));
        }
        if let Some(f) = &doc.marks.fault
            && let Some(line) = f.line
        {
            let count = if f.count > 1 {
                format!("  (×{})", f.count)
            } else {
                String::new()
            };
            notes
                .entry(line as usize - 1)
                .or_default()
                .push((format!("✖ {}{count}", f.message), ERROR));
        }
    }
    for l in lints {
        notes
            .entry(l.line)
            .or_default()
            .push((format!("⚠ {}", l.message), LINT));
    }
    for (shown, lp) in &doc.logpoints {
        let text = doc
            .marks
            .values
            .get(&lp.at)
            .cloned()
            .unwrap_or_else(|| format!("{}  (waiting for the line to run)", lp.message));
        notes
            .entry(*shown as usize - 1)
            .or_default()
            .push((format!("» {text}"), VALUE));
    }
    for (line, pct) in &doc.marks.heat {
        if *pct >= 0.5 {
            notes
                .entry(*line as usize - 1)
                .or_default()
                .push((format!("{pct:.1} %"), heat_color(0.8)));
        }
    }
    for (line, items) in notes {
        let Some(r) = lines.get(line) else { continue };
        if r.bottom() < ui.clip_rect().top() || r.top() > ui.clip_rect().bottom() {
            continue;
        }
        let mut x = r.right() + 28.0;
        for (text, color) in items {
            // Cut to what fits; the error bar under the code has the whole message.
            let room = ((ui.clip_rect().right() - x - 12.0) / (small.size * 0.6)) as usize;
            if room < 6 {
                break;
            }
            let text = if text.chars().count() > room {
                let cut: String = text.chars().take(room.saturating_sub(1)).collect();
                format!("{cut}…")
            } else {
                text
            };
            let galley = painter.layout_no_wrap(text, small.clone(), color);
            let rect = Rect::from_min_size(
                Pos2::new(x - 4.0, r.top()),
                Vec2::new(galley.size().x + 8.0, r.height()),
            );
            painter.rect_filled(rect, 3.0, color.gamma_multiply(0.12));
            painter.galley(
                Pos2::new(x, r.center().y - galley.size().y / 2.0),
                galley.clone(),
                color,
            );
            x += galley.size().x + 18.0;
        }
    }
    // Squiggles under unknown engine functions.
    for l in lints {
        let Some(r) = lines.get(l.line) else { continue };
        let x0 = r.left() + l.col.0 as f32 * r.width() / l.line_len.max(1) as f32;
        let x1 = r.left() + l.col.1 as f32 * r.width() / l.line_len.max(1) as f32;
        let y = r.bottom() - 1.0;
        let mut points = Vec::new();
        let mut x = x0;
        let mut up = true;
        while x < x1 {
            points.push(Pos2::new(x, if up { y - 2.0 } else { y }));
            x += 2.5;
            up = !up;
        }
        painter.add(egui::Shape::line(points, Stroke::new(1.0, LINT)));
    }
}

/// A click in the gutter: a logpoint showing that line's values, or none again.
fn toggle_logpoint(world: &mut World, doc: &mut Doc, line: usize) {
    let shown = line as u32 + 1;
    if doc.logpoints.remove(&shown).is_none() {
        doc.logpoints.insert(shown, logpoint_for(&doc.text, line));
    }
    apply_logpoints(world, doc);
}

/// What a logpoint on a line shows: an assignment's value (registered on the next line with
/// code, since a logpoint runs before its line), or the locals the line uses.
fn logpoint_for(text: &str, line: usize) -> Logpoint {
    let lines: Vec<&str> = text.lines().collect();
    let code = lines.get(line).map(|l| l.trim()).unwrap_or("");
    let target = code
        .strip_prefix("local ")
        .unwrap_or(code)
        .split_once('=')
        .filter(|(lhs, rhs)| !rhs.starts_with('=') && !lhs.ends_with(['~', '<', '>', '=']))
        .map(|(lhs, _)| lhs.trim().trim_end_matches(['+', '-', '*', '/']).trim())
        .filter(|lhs| {
            !lhs.is_empty()
                && lhs
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == ',' || c == ' ')
        });
    if let Some(target) = target {
        let next = (line + 1..lines.len())
            .find(|&n| {
                let t = lines[n].trim();
                !t.is_empty() && !t.starts_with("--")
            })
            .unwrap_or(line);
        let message = target
            .split(',')
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(|v| format!("{v} = {{{v}}}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Logpoint {
            at: next as u32 + 1,
            message,
        };
    }
    // The line's own names that are locals of the file.
    let locals = local_names(text);
    let mut names: Vec<String> = Vec::new();
    for word in code.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if locals.contains(word) && !names.iter().any(|n| n == word) && names.len() < 3 {
            names.push(word.to_string());
        }
    }
    let message = if names.is_empty() {
        format!("line {} ran", line + 1)
    } else {
        names
            .iter()
            .map(|v| format!("{v} = {{{v}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Logpoint {
        at: line as u32 + 1,
        message,
    }
}

/// Names declared with `local` (and function parameters) anywhere in the file.
fn local_names(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("local ") {
            let rest = rest.strip_prefix("function ").unwrap_or(rest);
            let names = rest.split(['=', '(']).next().unwrap_or("");
            for n in names.split(',') {
                let n = n.split(':').next().unwrap_or("").trim();
                if !n.is_empty() {
                    out.insert(n.to_string());
                }
            }
        }
        if let Some(i) = t.find("function")
            && let Some(args) = t[i..]
                .split_once('(')
                .map(|(_, a)| a.split(')').next().unwrap_or(""))
        {
            for a in args.split(',') {
                let a = a.split(':').next().unwrap_or("").trim();
                if !a.is_empty() && a != "..." {
                    out.insert(a.to_string());
                }
            }
        }
        if let Some(rest) = t.strip_prefix("for ") {
            let vars = rest.split([' ', '=']).next().unwrap_or("");
            let vars = rest.split_once(" in ").map_or(vars, |(v, _)| v);
            for v in vars.split(',') {
                let v = v.trim();
                if !v.is_empty() {
                    out.insert(v.to_string());
                }
            }
        }
    }
    out
}

fn status_line(ui: &mut egui::Ui, editor: &Editor, tab: &mut ScriptsTab) {
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    egui::Frame::new()
        .fill(theme::BG)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let line = line_of(&doc.text, doc.caret);
                let col = doc.caret - line_start_char(&doc.text, line) + 1;
                ui.label(theme::dim(format!("Ln {}, Col {col}", line + 1)).monospace());
                ui.label(theme::dim("·  Luau"));
                let errors = error_lines(doc);
                if !errors.is_empty() {
                    let r = ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "✖ {} error{}",
                                errors.len(),
                                if errors.len() == 1 { "" } else { "s" }
                            ))
                            .color(ERROR),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if r.on_hover_text("Go to it").clicked() {
                        doc.jump = Some(line_start_char(&doc.text, errors[0] as usize - 1));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let m = &doc.marks;
                    let state = match editor.mode {
                        Mode::Edit => "Edit mode: scripts start when you press Play".to_string(),
                        _ if m.instances > 0 => format!(
                            "{} · version {} · {} running · {:.3} ms a frame",
                            if editor.mode == Mode::Paused {
                                "Paused"
                            } else {
                                "Playing"
                            },
                            m.version,
                            m.instances,
                            m.ms
                        ),
                        _ if m.loaded => format!("Playing · version {} · a module", m.version),
                        _ => "Playing · no entity runs this file".to_string(),
                    };
                    let color = if editor.mode == Mode::Edit {
                        theme::TEXT_DIM
                    } else {
                        theme::PLAY
                    };
                    ui.label(RichText::new(state).color(color));
                });
            });
        });
}

/// The first thing wrong in the file, whole: (1-based line, message).
fn first_problem(doc: &Doc, vocab: Option<&Vocab>) -> Option<(u32, String)> {
    if let Some((line, message)) = &doc.syntax {
        return Some(((*line).max(1), format!("Syntax: {message}")));
    }
    if let Some(e) = &doc.marks.error {
        return Some((
            e.line.unwrap_or(1),
            format!(
                "Didn't load (the version before keeps running): {}",
                e.message
            ),
        ));
    }
    if let Some(f) = &doc.marks.fault {
        let count = if f.count > 1 {
            format!(" (×{})", f.count)
        } else {
            String::new()
        };
        return Some((f.line.unwrap_or(1), format!("{}{count}", f.message)));
    }
    let lint = lints(&doc.text, vocab?).into_iter().next()?;
    Some((lint.line as u32 + 1, lint.message))
}

/// Godot's error line under the code: the first problem, whole; a click goes to it.
fn error_bar(ui: &mut egui::Ui, tab: &mut ScriptsTab, line: u32, text: &str) {
    let doc = &mut tab.docs[tab.current];
    let r = egui::Frame::new()
        .fill(Color32::from_rgb(0x2a, 0x12, 0x12))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(format!("✖ line {line}: {text}"))
                        .monospace()
                        .color(ERROR),
                )
                .truncate(),
            )
        });
    if r.response.interact(egui::Sense::click()).clicked() || r.inner.clicked() {
        doc.jump = Some(line_start_char(&doc.text, line as usize - 1));
    }
}

fn toast(ui: &egui::Ui, text: &str, error: bool) {
    let rect = ui.max_rect();
    egui::Area::new(egui::Id::new("script toast"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::new(rect.center().x - 220.0, rect.top() + 40.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .fill(if error {
                    Color32::from_rgb(0x4a, 0x16, 0x16)
                } else {
                    Color32::from_rgb(0x15, 0x3a, 0x24)
                })
                .show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.label(RichText::new(text).color(Color32::WHITE));
                });
        });
}

// ----------------------------------------------------------------------------- highlighting

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tok {
    Keyword,
    Str,
    Number,
    Comment,
    SelfWord,
    Call,
    Engine,
}

/// The colored spans of Luau source: byte ranges, in order.
fn tokens(text: &str, engine: &HashSet<String>) -> Vec<(usize, usize, Tok)> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    // A long bracket `[[` or `[==[` at `at`: its level.
    let long_open = |at: usize| -> Option<usize> {
        if b.get(at) != Some(&b'[') {
            return None;
        }
        let mut j = at + 1;
        while b.get(j) == Some(&b'=') {
            j += 1;
        }
        (b.get(j) == Some(&b'[')).then_some(j - at - 1)
    };
    let long_close = |from: usize, level: usize| -> usize {
        let close = format!("]{}]", "=".repeat(level));
        text[from..]
            .find(&close)
            .map_or(n, |k| from + k + close.len())
    };
    while i < n {
        let c = b[i];
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            let start = i;
            i = match long_open(i + 2) {
                Some(level) => long_close(i + 2, level),
                None => text[i..].find('\n').map_or(n, |k| i + k),
            };
            out.push((start, i, Tok::Comment));
            continue;
        }
        if let Some(level) = long_open(i) {
            let start = i;
            i = long_close(i, level);
            out.push((start, i, Tok::Str));
            continue;
        }
        if c == b'"' || c == b'\'' || c == b'`' {
            let start = i;
            i += 1;
            while i < n && b[i] != c && b[i] != b'\n' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            out.push((start, i, Tok::Str));
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            while i < n && (ident(b[i]) || b[i] == b'.') {
                i += 1;
            }
            out.push((start, i, Tok::Number));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < n && ident(b[i]) {
                i += 1;
            }
            let word = &text[start..i];
            let prev = text[..start].trim_end().chars().last();
            let next = text[i..].trim_start().chars().next();
            let member = matches!(prev, Some('.') | Some(':'));
            let kind = if KEYWORDS.contains(&word) {
                Some(Tok::Keyword)
            } else if word == "self" {
                Some(Tok::SelfWord)
            } else if !member && engine.contains(word) {
                Some(Tok::Engine)
            } else if matches!(next, Some('(') | Some('{') | Some('"') | Some('`')) {
                Some(Tok::Call)
            } else {
                None
            };
            if let Some(kind) = kind {
                out.push((start, i, kind));
            }
            continue;
        }
        i += 1;
    }
    out
}

const KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "export", "false", "for",
    "function", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true",
    "type", "until", "while",
];

/// Every case-insensitive match of `needle` (byte ranges).
fn find_all(text: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let hay = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(k) = hay[from..].find(&needle) {
        out.push((from + k, from + k + needle.len()));
        from += k + needle.len().max(1);
    }
    out
}

/// The text as colored sections, with find matches behind and unknown engine functions under.
pub(crate) fn highlight(text: &str, font: &FontId, vocab: &Vocab, find: Option<&str>) -> LayoutJob {
    let toks = tokens(text, &vocab.engine);
    let matches = find.map(|f| find_all(text, f)).unwrap_or_default();
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for (a, b, _) in &toks {
        cuts.extend([*a, *b]);
    }
    for (a, b) in &matches {
        cuts.extend([*a, *b]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let (mut t, mut m) = (0, 0);
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        while t < toks.len() && toks[t].1 <= a {
            t += 1;
        }
        while m < matches.len() && matches[m].1 <= a {
            m += 1;
        }
        let tok = toks.get(t).filter(|x| x.0 <= a).map(|x| x.2);
        let color = match tok {
            Some(Tok::Keyword) => KEYWORD,
            Some(Tok::Str) => STRING,
            Some(Tok::Number) => NUMBER,
            Some(Tok::Comment) => COMMENT,
            Some(Tok::SelfWord) => SELF,
            Some(Tok::Call) => CALL,
            Some(Tok::Engine) => ENGINE,
            None => theme::TEXT,
        };
        let mut format = TextFormat::simple(font.clone(), color);
        if matches.get(m).is_some_and(|x| x.0 <= a) {
            format.background = FIND_BG;
        }
        job.append(&text[a..b], 0.0, format);
    }
    job
}

/// A call to an engine function that doesn't exist.
struct Lint {
    /// 0-based line; the name's chars on it, and the line's length in chars.
    line: usize,
    col: (usize, usize),
    line_len: usize,
    message: String,
}

/// Calls like `fire.ignit {...}` whose name isn't registered, with the registry's "did you
/// mean" (Roblox's script analysis underlines these too).
fn lints(text: &str, vocab: &Vocab) -> Vec<Lint> {
    let toks = tokens(text, &vocab.engine);
    let mut out = Vec::new();
    let b = text.as_bytes();
    for (a, end, kind) in &toks {
        if *kind != Tok::Engine || !vocab.api_tops.contains(&text[*a..*end]) {
            continue;
        }
        // The dotted chain after it.
        let mut j = *end;
        while j < b.len() && b[j] == b'.' {
            let k = j + 1;
            let mut e = k;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e == k {
                break;
            }
            j = e;
        }
        let full = &text[*a..j];
        let Some((scope, name)) = full.rsplit_once('.') else {
            continue;
        };
        let called = matches!(text[j..].trim_start().chars().next(), Some('(' | '{' | '"'));
        if !called || vocab.known.contains(&(scope.to_string(), name.to_string())) {
            continue;
        }
        let top = full.split('.').next().unwrap_or("");
        let siblings: Vec<&str> = vocab
            .api_names
            .iter()
            .filter(|n| n.split('.').next() == Some(top))
            .map(String::as_str)
            .collect();
        let line = text[..*a].matches('\n').count();
        let line_start = text[..*a].rfind('\n').map_or(0, |k| k + 1);
        let line_end = text[*a..].find('\n').map_or(text.len(), |k| a + k);
        let c0 = text[line_start..*a].chars().count();
        out.push(Lint {
            line,
            col: (c0, c0 + full.chars().count()),
            line_len: text[line_start..line_end].chars().count(),
            message: unknown_name_error("engine function", full, siblings.into_iter()),
        });
    }
    out
}

// ----------------------------------------------------------------------------- completion

/// What is being typed at the caret, and what fits there. `explicit`: asked for (Ctrl+Space),
/// so an empty word lists everything.
fn complete(text: &str, caret: usize, vocab: &Vocab, explicit: bool) -> Option<Completion> {
    let before: Vec<char> = text.chars().take(caret).collect();
    let mut start = before.len();
    while start > 0 && (before[start - 1].is_alphanumeric() || before[start - 1] == '_') {
        start -= 1;
    }
    let prefix: String = before[start..].iter().collect();
    let line_start = before[..start]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |k| k + 1);
    let line: String = before[line_start..start].iter().collect();
    let sep = start.checked_sub(1).map(|k| before[k]);
    let quote = matches!(sep, Some('"') | Some('\''));
    // Not in a comment, nor in a string (but a component name's string, below).
    let code_before: String = if quote {
        line.chars().take(line.chars().count() - 1).collect()
    } else {
        line.clone()
    };
    if in_comment_or_string(&code_before) {
        return None;
    }
    let receiver = |end: usize| -> String {
        let mut s = end;
        while s > 0
            && (before[s - 1].is_alphanumeric() || before[s - 1] == '_' || before[s - 1] == '.')
        {
            s -= 1;
        }
        before[s..end].iter().collect()
    };
    let words = &vocab.words;
    let mut items: Vec<Item> = Vec::new();
    let item = |w: &Word, scope_label: &str, sep: &str| -> Item {
        Item {
            label: w.name.clone(),
            insert: match w.kind {
                WordKind::Hook => format!("{}{}", w.name, w.detail),
                _ => w.name.clone(),
            },
            kind: w.kind,
            full: if scope_label.is_empty() {
                w.name.clone()
            } else {
                format!("{scope_label}{sep}{}", w.name)
            },
            detail: w.detail.clone(),
            doc: w.doc.clone(),
        }
    };
    if quote {
        // e:get("Po…, e:set("Po…: component names.
        let call = line.trim_end_matches(['"', '\'']);
        if !["get(", "set(", "has(", "remove("]
            .iter()
            .any(|m| call.ends_with(&format!(":{m}")))
        {
            return None;
        }
        for w in words.iter().filter(|w| w.kind == WordKind::Component) {
            items.push(item(w, "", ""));
        }
    } else if sep == Some('.') || sep == Some(':') {
        let recv = receiver(start - 1);
        if recv.is_empty() || recv.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let colon = sep == Some(':');
        if colon && line.trim_start().starts_with("function ") {
            for w in words.iter().filter(|w| w.kind == WordKind::Hook) {
                items.push(item(w, &recv, ":"));
            }
        } else if !colon && words.iter().any(|w| w.scope == recv) {
            for w in words.iter().filter(|w| w.scope == recv) {
                items.push(item(w, &recv, "."));
            }
        } else if recv == "self" {
            let scope = "ScriptSelf";
            for w in words.iter().filter(|w| w.scope == scope) {
                let method = w.kind == WordKind::Function;
                if method == colon {
                    items.push(item(w, "self", if colon { ":" } else { "." }));
                }
            }
            // The class's own methods and fields.
            let (methods, fields) = own_members(text);
            for name in if colon { methods } else { fields } {
                let w = Word {
                    name,
                    scope: "self".into(),
                    kind: if colon {
                        WordKind::Method
                    } else {
                        WordKind::Field
                    },
                    detail: String::new(),
                    doc: "Defined in this script".into(),
                };
                items.push(item(&w, "self", if colon { ":" } else { "." }));
            }
        } else {
            // A value: an entity, a vec3 or a quat.
            for class in ["Entity", "Vec3", "Quat"] {
                for w in words.iter().filter(|w| w.scope == class) {
                    let method = w.kind == WordKind::Method;
                    if method == colon && !items.iter().any(|i| i.label == w.name) {
                        items.push(item(w, class, if colon { ":" } else { "." }));
                    }
                }
            }
        }
    } else {
        if prefix.is_empty() && !explicit {
            return None;
        }
        for w in words.iter().filter(|w| w.scope.is_empty()) {
            items.push(item(w, "", ""));
        }
        for name in local_names(text) {
            if !items.iter().any(|i| i.label == name) {
                let w = Word {
                    name,
                    scope: String::new(),
                    kind: WordKind::Field,
                    detail: "local".into(),
                    doc: String::new(),
                };
                items.push(item(&w, "", ""));
            }
        }
    }
    let p = prefix.to_lowercase();
    let mut ranked: Vec<(u8, Item)> = items
        .into_iter()
        .filter_map(|it| {
            let l = it.label.to_lowercase();
            let rank = if l.starts_with(&p) {
                0
            } else if p.len() >= 2 && l.contains(&p) {
                1
            } else {
                return None;
            };
            (it.label != prefix).then_some((rank, it))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.cmp(&b.1.label)));
    ranked.dedup_by(|a, b| a.1.label == b.1.label);
    if ranked.is_empty() {
        return None;
    }
    Some(Completion {
        start,
        prefix,
        items: ranked.into_iter().map(|(_, it)| it).take(60).collect(),
        selected: 0,
    })
}

/// Whether the end of a line (the text before the caret) is inside a comment or a string; a
/// backtick string's `{expressions}` are code.
fn in_comment_or_string(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let mut quote: Option<char> = None;
    let mut braces = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' {
                    i += 1;
                } else if q == '`' && c == '{' {
                    quote = None;
                    braces = 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '-' && chars.get(i + 1) == Some(&'-') {
                    return true;
                }
                if braces > 0 && c == '}' {
                    braces -= 1;
                    if braces == 0 {
                        quote = Some('`');
                    }
                } else if braces > 0 && c == '{' {
                    braces += 1;
                } else if matches!(c, '"' | '\'' | '`') {
                    quote = Some(c);
                }
            }
        }
        i += 1;
    }
    quote.is_some()
}

/// The class's methods (`function X:name`) and `self.name =` fields.
fn own_members(text: &str) -> (Vec<String>, Vec<String>) {
    let mut methods = Vec::new();
    let mut fields = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("function ")
            && let Some((_, m)) = rest.split_once(':')
            && let Some(name) = m.split('(').next()
            && !methods.iter().any(|x: &String| x == name)
        {
            methods.push(name.trim().to_string());
        }
        let mut rest = t;
        while let Some(k) = rest.find("self.") {
            let after = &rest[k + 5..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !fields.contains(&name) && name != "entity" && name != "props" {
                fields.push(name.clone());
            }
            rest = &after[name.len()..];
        }
    }
    (methods, fields)
}

fn kind_color(kind: WordKind) -> (Color32, &'static str) {
    match kind {
        WordKind::Keyword => (KEYWORD, "kw"),
        WordKind::Global => (ENGINE, "fn"),
        WordKind::Namespace => (ENGINE, "ns"),
        WordKind::Function => (CALL, "fn"),
        WordKind::Method => (CALL, "fn"),
        WordKind::Field => (theme::TEXT, "  ."),
        WordKind::Hook => (VALUE, "on"),
        WordKind::Component => (STRING, "C"),
    }
}

/// The completion list at `pos`, with the chosen item's signature and docs beside it.
/// Returns a clicked row.
fn completion_popup(ui: &egui::Ui, c: &Completion, pos: Pos2) -> Option<usize> {
    let mut clicked = None;
    egui::Area::new(egui::Id::new("script completion"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            ui.horizontal_top(|ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::same(3))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        let first = c.selected.saturating_sub(8);
                        for (n, it) in c.items.iter().enumerate().skip(first).take(10) {
                            let (color, tag) = kind_color(it.kind);
                            let mut job = LayoutJob::default();
                            let mono = FontId::monospace(12.5);
                            job.append(
                                &format!("{tag:>2}  "),
                                0.0,
                                TextFormat::simple(
                                    FontId::monospace(11.0),
                                    color.gamma_multiply(0.8),
                                ),
                            );
                            let typed = c.prefix.chars().count().min(it.label.chars().count());
                            let (head, tail) = it.label.split_at(char_to_byte(&it.label, typed));
                            let mut strong = TextFormat::simple(mono.clone(), Color32::WHITE);
                            strong.underline = Stroke::new(1.0, theme::ACCENT);
                            if it
                                .label
                                .to_lowercase()
                                .starts_with(&c.prefix.to_lowercase())
                            {
                                job.append(head, 0.0, strong);
                                job.append(
                                    tail,
                                    0.0,
                                    TextFormat::simple(mono.clone(), theme::TEXT),
                                );
                            } else {
                                job.append(
                                    &it.label,
                                    0.0,
                                    TextFormat::simple(mono.clone(), theme::TEXT),
                                );
                            }
                            if !it.detail.is_empty() && it.kind != WordKind::Function {
                                let short: String = it.detail.chars().take(28).collect();
                                job.append(
                                    &format!("  {short}"),
                                    0.0,
                                    TextFormat::simple(FontId::monospace(11.0), theme::TEXT_DIM),
                                );
                            }
                            let r = ui.add(
                                egui::Button::selectable(n == c.selected, job)
                                    .min_size(Vec2::new(294.0, 0.0)),
                            );
                            if r.clicked() {
                                clicked = Some(n);
                            }
                        }
                        if c.items.len() > 10 {
                            ui.label(theme::dim(format!("{} more", c.items.len() - 10)).small());
                        }
                    });
                if let Some(it) = c.items.get(c.selected) {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.0);
                        let sig = match it.kind {
                            WordKind::Function | WordKind::Method | WordKind::Global
                                if it.detail.starts_with('(') =>
                            {
                                format!("{}{}", it.full, it.detail)
                            }
                            WordKind::Function => format!("{} {}", it.full, it.detail),
                            _ if !it.detail.is_empty() => format!("{}: {}", it.full, it.detail),
                            _ => it.full.clone(),
                        };
                        ui.label(RichText::new(sig).monospace().color(CALL));
                        if !it.doc.is_empty() {
                            ui.add_space(2.0);
                            ui.label(&it.doc);
                        }
                        ui.add_space(2.0);
                        ui.label(theme::dim("Tab or Enter puts it in, Esc closes").small());
                    });
                }
            });
        });
    clicked
}

// ----------------------------------------------------------------------------- output

fn output(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("OUTPUT")
                .small()
                .strong()
                .color(theme::TEXT_DIM),
        );
        let errors = tab
            .output
            .iter()
            .filter(|l| l.kind == OutKind::Error)
            .count();
        if errors > 0 {
            ui.label(RichText::new(format!("✖ {errors}")).small().color(ERROR));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Clear").clicked() {
                tab.output.clear();
            }
        });
    });
    let bar_h = 26.0;
    let mut open_at = None;
    egui::ScrollArea::vertical()
        .id_salt("script output lines")
        .max_height((ui.available_height() - bar_h).max(20.0))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if tab.output.is_empty() {
                ui.label(theme::dim(
                    "Prints, warnings and errors from running scripts show here (click one to \
                     open its line). Press Play to run the scripts; type Luau below to run it \
                     in the game.",
                ));
            }
            for l in &tab.output {
                let color = match l.kind {
                    OutKind::Print => theme::TEXT,
                    OutKind::Warn => LINT,
                    OutKind::Error => ERROR,
                    OutKind::Command => theme::TEXT_DIM,
                    OutKind::Result => ENGINE,
                    OutKind::Note => theme::TEXT_DIM,
                };
                let shown = if l.count > 1 {
                    format!("{}   ×{}", l.text, l.count)
                } else {
                    l.text.clone()
                };
                let text = RichText::new(shown).monospace().color(color);
                match &l.at {
                    Some(at) => {
                        let r = ui
                            .add(egui::Label::new(text).sense(egui::Sense::click()))
                            .on_hover_text(format!("Open {}:{}", at.0, at.1));
                        if r.clicked() {
                            open_at = Some(at.clone());
                        }
                    }
                    None => {
                        ui.label(text);
                    }
                }
            }
        });
    if let Some((path, line)) = open_at {
        open(world, tab, &path, Some(line));
    }
    // The command bar: Luau in the live game.
    let id = egui::Id::new("script command bar");
    let focused = ui.memory(|m| m.has_focus(id));
    if focused && !tab.commands.is_empty() {
        let (up, down) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        if up || down {
            let n = tab.commands.len();
            let at = match (tab.command_at, up) {
                (None, true) => Some(n - 1),
                (Some(i), true) => Some(i.saturating_sub(1)),
                (Some(i), false) if i + 1 < n => Some(i + 1),
                _ => None,
            };
            tab.command_at = at;
            tab.command = at.map(|i| tab.commands[i].clone()).unwrap_or_default();
        }
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(">").monospace().color(theme::ACCENT));
        let r = ui.add(
            egui::TextEdit::singleline(&mut tab.command)
                .id(id)
                .font(egui::TextStyle::Monospace)
                .hint_text("Luau in the game: return world.find(\"Lamp\").position")
                .desired_width(f32::INFINITY),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            tab.run_command = true;
            r.request_focus();
        }
    });
    if std::mem::take(&mut tab.run_command) && !tab.command.trim().is_empty() {
        let code = std::mem::take(&mut tab.command);
        tab.command_at = None;
        push(tab, OutKind::Command, format!("> {code}"), None);
        let result = call_api(world, "script.eval", json!({"code": code}));
        match result {
            Ok(v) => push(tab, OutKind::Result, show_value(&v), None),
            Err(e) => {
                let at = None;
                push(tab, OutKind::Error, e, at);
            }
        }
        if tab.commands.last() != Some(&code) {
            tab.commands.push(code);
        }
    }
}

/// A command's result as a line: strings bare, `[x, y, z]` as a vec3.
fn show_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "nil".into(),
        Value::Array(items) if items.len() == 3 && items.iter().all(Value::is_number) => {
            let n: Vec<String> = items
                .iter()
                .map(|x| format!("{:.3}", x.as_f64().unwrap_or(0.0)))
                .collect();
            format!("vec3({})", n.join(", "))
        }
        other => other.to_string(),
    }
}

// ----------------------------------------------------------------------------- screenshots

/// The example scripts the shots use (in the scripts folder): copied next to the shots, so
/// they edit copies.
const EXAMPLES: &str = "examples";

/// `editor --headless --size 1920x1080 --shots <dir> --script scripts`: the Scripts tab in use,
/// with the starter courtyard's lamp and a stone running the example scripts. The scripts are
/// copied into `<dir>/scripts`, so the run edits copies and leaves the repo's files alone.
pub fn shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = {
        let root = root.clone();
        move |world: &mut World, editor: &mut Editor| {
            // Dusk, so the lamp's light shows; no icons over the bulb.
            editor.icons = false;
            let sky = world
                .resource::<TypeRegistry>()
                .resource("Sky")
                .map(|r| r.patch);
            if let Some(patch) = sky {
                let _ = patch(world, json!({"hour": 19.3, "hours_per_second": 0.0}));
            }
            // From the scripts folder (before it points at the copies).
            let from = engine_script::scripts_root(world).join(EXAMPLES);
            copy_dir(&from, &root.join(EXAMPLES));
            world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
            world.remove_resource::<ScriptRuntime>();
            for (entity, path) in [
                ("Lamp", "examples/lamp.luau"),
                ("Stone", "examples/float.luau"),
            ] {
                if let Some(e) = crate::shots::find(world, entity) {
                    world.entity_mut(e).insert(Script::new(path));
                }
            }
            world.resource_mut::<EditorDock>().0 = shot_layout();
            world.resource_mut::<ScriptsTab>().files_next = 0.0;
        }
    };
    let open_file = |path: &'static str, line: Option<u32>| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, path, line)
            });
        }
    };
    let with_tab = |f: fn(&mut World, &mut ScriptsTab)| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| f(world, &mut tab));
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Select("Lamp"),
        // No gizmo over the lamp: its glow is part of the picture.
        Step::Tool(crate::editor::Tool::Select),
        // Low, so the stone shows the gap under it.
        Step::Camera(DVec3::new(1.5, 1.3, 9.0), DVec3::new(-0.5, 1.0, 2.5)),
        Step::Do(Box::new(open_file("examples/float.luau", None))),
        Step::Do(Box::new(open_file("examples/lamp.luau", Some(24)))),
        Step::Tab(Tab::Scripts),
        Step::Wait(40),
        Step::Shot("scripts_01_edit_mode"),
        // Play: the scripts start; a logpoint on the glow line and the profiler.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(20),
        Step::Do(Box::new(with_tab(|world, tab| {
            let doc = &mut tab.docs[tab.current];
            if let Some(line) = doc.text.lines().position(|l| l.contains("local glow")) {
                toggle_logpoint(world, doc, line);
            }
            tab.profiling = true;
            let _ = call_api(world, "script.profile", json!({"seconds": 0}));
        }))),
        Step::Wait(150),
        Step::Shot("scripts_02_playing"),
        // An edit, saved: the running lamp hot reloads with its state.
        Step::Do(Box::new(with_tab(|world, tab| {
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.text.replace("period = 2.4", "period = 0.6");
            doc.text = doc
                .text
                .replace("color = vec3(1, 0.72, 0.42)", "color = vec3(0.45, 0.75, 1)");
            let now = editor::now(world);
            save(world, tab, now);
        }))),
        Step::Wait(70),
        Step::Shot("scripts_03_hot_reload"),
        // Typing: a syntax error marked before saving, an engine function that doesn't exist,
        // and completion on `task.`.
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.text.replace(
                "\tself:every(0.08, \"shake\")\n",
                "\tself:every(0.08, \"shake\")\n\tfire.ignit({ at = self.entity.world_position })\n\ttask.w",
            );
            let at = doc
                .text
                .find("\ttask.w")
                .map(|b| b + "\ttask.w".len())
                .unwrap_or(0);
            doc.jump = Some(doc.text[..at].chars().count());
            tab.complete_now = true;
        }))),
        Step::Wait(4),
        Step::Do(Box::new(with_tab(|_, tab| tab.complete_now = true))),
        Step::Wait(12),
        Step::Shot("scripts_04_typing"),
        // A runtime error, saved: marked on its line with its count; the Output links to it.
        Step::Do(Box::new(with_tab(|world, tab| {
            tab.completion = None;
            let doc = &mut tab.docs[tab.current];
            doc.text = doc
                .saved
                .replace("\"PointLight\", { color", "\"PointLigth\", { color");
            let now = editor::now(world);
            save(world, tab, now);
        }))),
        Step::Wait(80),
        Step::Shot("scripts_05_runtime_error"),
        // Fixed again, and the command bar.
        Step::Do(Box::new(with_tab(|world, tab| {
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.saved.replace("\"PointLigth\"", "\"PointLight\"");
            let now = editor::now(world);
            save(world, tab, now);
        }))),
        Step::Wait(40),
        Step::Do(Box::new(open_file("examples/float.luau", Some(14)))),
        Step::Wait(10),
        Step::Do(Box::new(with_tab(|_, tab| {
            tab.command = "return world.find(\"Lamp\"):get(\"PointLight\").intensity_lm".into();
            tab.run_command = true;
        }))),
        Step::Wait(3),
        Step::Do(Box::new(with_tab(|_, tab| {
            tab.command = "return world.find(\"Stone\").position".into();
            tab.run_command = true;
        }))),
        Step::Wait(3),
        Step::Do(Box::new(with_tab(|_, tab| {
            tab.command = "return #world.find_all(\"Crate\")".into();
            tab.run_command = true;
        }))),
        Step::Wait(6),
        Step::Shot("scripts_06_console"),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed)
        })),
        Step::Wait(5),
    ];
    ShotScript::new(dir, steps)
}

/// The shots' layout: the Scripts tab big on the left, the scene view and Details on the right.
fn shot_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Scripts]);
    let surface = dock.main_surface_mut();
    let [_scripts, right] = surface.split_right(NodeIndex::root(), 0.55, vec![Tab::Viewport]);
    let [_view, _details] = surface.split_below(right, 0.52, vec![Tab::Inspector]);
    dock
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::create_dir_all(to);
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            let _ = std::fs::copy(&path, &target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        let mut world = World::new();
        let mut api = engine::core::registry::ApiRegistry::default();
        api.register(
            "fire.ignite",
            "Sets something alight.",
            r#"{"at": [0, 0, 0], "radius": 1.0}"#,
            |_: &mut World, _| Ok(Value::Null),
        );
        world.insert_resource(api);
        Vocab::new(&world)
    }

    #[test]
    fn completion_knows_the_engine() {
        let v = vocab();
        let text = "function S:update(dt)\n\ttask.w";
        let c = complete(text, text.chars().count(), &v, false).expect("a list");
        assert_eq!(c.items[0].label, "wait");
        assert!(c.items[0].doc.contains("Pauses"));
        let text = "\tfire.";
        let c = complete(text, text.chars().count(), &v, false).expect("a list");
        assert_eq!(c.items[0].label, "ignite");
        assert!(
            c.items[0].detail.contains("radius = 1.0"),
            "{}",
            c.items[0].detail
        );
        let text = "function Lamp:on_i";
        let c = complete(text, text.chars().count(), &v, false).expect("hooks");
        assert_eq!(c.items[0].insert, "on_interact(by)");
        let text = "\tself.entity:se";
        let c = complete(text, text.chars().count(), &v, false).expect("entity methods");
        assert!(
            c.items
                .iter()
                .any(|i| i.label == "set" && i.full == "Entity:set")
        );
        for text in ["-- task.w", "print(\"task.w", "local x = 1.", "x = 'ab"] {
            assert!(
                complete(text, text.chars().count(), &v, false).is_none(),
                "{text}"
            );
        }
        let text = "print(`{self.entity:se";
        assert!(complete(text, text.chars().count(), &v, false).is_some());
        let text = "self.entity:set(\"Poi";
        assert!(
            complete(text, text.chars().count(), &v, false).is_none(),
            "no components here"
        );
        let text = "local glow = 1\nlocal x = gl";
        let c = complete(text, text.chars().count(), &v, false).expect("locals");
        assert_eq!(c.items[0].label, "glow");
    }

    #[test]
    fn unknown_engine_functions_are_underlined() {
        let v = vocab();
        let found = lints(
            "fire.ignite { at = p }\nfire.ignit { at = p }\n-- fire.nope()\n",
            &v,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 1);
        assert_eq!(found[0].col, (0, 10));
        assert!(
            found[0].message.contains("fire.ignite"),
            "{}",
            found[0].message
        );
        assert!(lints("local fire = 1\nprint(math.floor(2))", &v).is_empty());
    }

    #[test]
    fn logpoints_show_what_a_line_computes() {
        let text = "local S = {}\nfunction S:update(dt)\n\tlocal glow = 0.5 * dt\n\n\tself.entity:set(\"PointLight\", { intensity_lm = 900 * glow })\nend";
        let lp = logpoint_for(text, 2);
        assert_eq!(lp.message, "glow = {glow}");
        assert_eq!(lp.at, 5, "the next line with code");
        let lp = logpoint_for(text, 4);
        assert_eq!(lp.message, "glow = {glow}");
        assert_eq!(lp.at, 5);
        let lp = logpoint_for("self.t += dt\nprint(1)", 0);
        assert_eq!(lp.message, "self.t = {self.t}");
    }

    #[test]
    fn tokens_color_luau() {
        let engine: HashSet<String> = ["vec3".to_string()].into();
        let text = "local v = vec3(1, 2) -- up\nprint(\"hi\") self.x = [[long]]";
        let toks = tokens(text, &engine);
        let kinds: Vec<(&str, Tok)> = toks.iter().map(|(a, b, k)| (&text[*a..*b], *k)).collect();
        assert!(kinds.contains(&("local", Tok::Keyword)));
        assert!(kinds.contains(&("vec3", Tok::Engine)));
        assert!(kinds.contains(&("1", Tok::Number)));
        assert!(kinds.contains(&("-- up", Tok::Comment)));
        assert!(kinds.contains(&("print", Tok::Call)));
        assert!(kinds.contains(&("\"hi\"", Tok::Str)));
        assert!(kinds.contains(&("self", Tok::SelfWord)));
        assert!(kinds.contains(&("[[long]]", Tok::Str)));
    }

    #[test]
    fn functions_list_hooks_and_helpers() {
        let text = "local function helper(a: number, b)\nend\nfunction Lamp:update(dt: number)\nend\nfunction wave.bob(t)\nend";
        let f = functions(text);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].label, "local helper(a, b)");
        assert!(f[1].hook && f[1].line == 2);
        assert_eq!(f[2].label, "wave.bob(t)");
        assert_eq!(class_name("examples/new_script.luau"), "NewScript");
    }

    #[test]
    fn lines_and_chars() {
        let text = "ab\ncdé\nf";
        assert_eq!(line_start_char(text, 1), 3);
        assert_eq!(line_start_char(text, 2), 7);
        assert_eq!(line_of(text, 8), 2);
        assert_eq!(char_to_byte(text, 6), 7);
    }
}
