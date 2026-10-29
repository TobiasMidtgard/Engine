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
//! state. While playing, Shift+click in the gutter shows a line's values live (a logpoint,
//! without stopping the game) and Profile shows each line's share of the script time beside it.
//! Under the code, the Output (prints, warnings, errors; click one to open its line) and a
//! command bar that runs Luau in the game, as Roblox's.
//!
//! Debugging, as in Roblox Studio and Visual Studio: a click in the gutter sets a breakpoint
//! (right-click for one with a condition). A script that reaches it waits there, and so do the
//! other scripts and the world's time, while the editor keeps running ([`engine_script::debug`]):
//! the stopped line gets an arrow, each local's value shows beside the line that declares it,
//! hovering a name shows its value, and a panel holds the call stack, the variables and the
//! watches. F5 goes on, F10 steps over, F11 in, Shift+F11 out; F9 sets a breakpoint on the
//! caret's line; Stop on errors stops where an error is raised, with its locals.
//!
//! Scripts run only while playing ([`ScriptsHeld`] in edit mode) and Stop drops their VMs, so
//! every Play starts them afresh, as it does the scene.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::SystemTime;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, RichText, Stroke, Vec2};
use egui_dock::{DockState, NodeIndex};
use engine::core::registry::{call_api, unknown_name_error};
use engine::prelude::*;
use engine_script::debug::{self, FileStop, Go, StopView};
use engine_script::editing::{self, FileMarks, OutputCursor, Word, WordKind};
use engine_script::props::{Prop, PropKind};
use engine_script::{Script, ScriptRuntime, ScriptSettings, ScriptsHeld};
use serde_json::{Value, json};

use crate::editor::{self, Editor, Mode, Tab};
use crate::scene_ops::SceneOrder;
use crate::shots::{ShotScript, Step};
use crate::theme;
use crate::ui::EditorDock;
use crate::undo::Command;

/// Scripting in the editor: scripts held still outside Play.
pub struct ScriptsTabPlugin;

impl Plugin for ScriptsTabPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ScriptsHeld)
            .init_resource::<ScriptsTab>()
            .add_systems(Update, hold_scripts.before(UpdateSet::Logic))
            .add_systems(Update, follow_debugger.after(UpdateSet::Logic))
            .add_systems(Update, keep_play_values.after(UpdateSet::Logic));
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
            // Stopped at a breakpoint: the world's time was held for it.
            debug::release_time(world);
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
/// Breakpoints (Visual Studio's red), the stopped line (its yellow arrow and line), a caller's
/// line picked in the call stack, and the values of the stop's locals.
const BREAKPOINT: Color32 = Color32::from_rgb(0xe5, 0x3e, 0x3e);
const STOP: Color32 = Color32::from_rgb(0xff, 0xd8, 0x4c);
const STOP_BG: Color32 = Color32::from_rgba_premultiplied(0x3c, 0x34, 0x0c, 0x3c);
const CALLER_BG: Color32 = Color32::from_rgba_premultiplied(0x10, 0x34, 0x22, 0x34);
const STOP_VALUE: Color32 = Color32::from_rgb(0x9c, 0xdc, 0xfe);
const STOP_STRIP: Color32 = Color32::from_rgb(0x3a, 0x31, 0x0e);
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
    pub debug: DebugState,
    /// The Details panel's script part.
    pub props: PropsPanel,
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
    /// Breakpoints by line (1-based), each with the condition it stops on, if any.
    pub breakpoints: BTreeMap<u32, Option<String>>,
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
    let stop = debug::stopped(world);
    if let Some(stop) = &stop {
        debug_keys(ui, world, stop);
    }

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
    if let Some(stop) = &stop {
        egui::Panel::right("script debugger")
            .resizable(true)
            .default_size(310.0)
            .size_range(220.0..=560.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| debug_panel(ui, world, &mut tab, stop));
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            tool_bar(ui, world, &mut tab, now);
            if let Some(stop) = &stop {
                stop_strip(ui, world, stop);
            }
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
                breakpoints: BTreeMap::new(),
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
                    let doc = tab.docs.remove(i);
                    if !doc.breakpoints.is_empty() {
                        debug::set_breakpoints(world, &doc.path, &[]);
                    }
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
                    if ui
                        .selectable_label(tab.debug.break_errors, "Stop on errors")
                        .on_hover_text(
                            "Stop where an error nothing catches is raised, with the call stack \
                             and its values, instead of only reporting it",
                        )
                        .clicked()
                    {
                        tab.debug.break_errors = !tab.debug.break_errors;
                        tab.debug.apply = true;
                    }
                    let can_break = tab.playing && debug::stopped(world).is_none();
                    if ui
                        .add_enabled(can_break, egui::Button::new("⏸ Break"))
                        .on_hover_text(
                            "Stop the scripts at the next line they run; the world's time holds \
                             with them",
                        )
                        .clicked()
                    {
                        debug::pause(world);
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
    if focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F9)) {
        let doc = &mut tab.docs[i];
        let line = line_of(&doc.text, doc.caret) as u32 + 1;
        toggle_breakpoint(doc, &mut tab.debug, line);
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

    let playing = tab.playing;
    let doc = &mut tab.docs[i];
    let dbg = &mut tab.debug;
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
    // The stop, where it is in this file (the frame picked in the call stack if it's here).
    let stop = debug::stop_at(world, &doc.path, &doc.text, Some(dbg.frame));
    // Breakpoints the running file has no code for yet (drawn hollow, as unverified).
    let unbound: HashSet<u32> = if playing {
        debug::breakpoints(world, &doc.path)
            .into_iter()
            .filter(|b| b.at.is_none())
            .map(|b| b.line)
            .collect()
    } else {
        HashSet::new()
    };
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
            paint_backdrop(
                ui,
                backdrop,
                &out.response.rect,
                &lines,
                current_line,
                doc,
                stop.as_ref(),
            );
            paint_gutter(
                ui,
                &out.response.rect,
                &lines,
                current_line,
                doc,
                &font,
                row_h,
                stop.as_ref(),
                &unbound,
            );
            let lints = lints(&doc.text, &vocab);
            paint_notes(ui, &lines, doc, &lints, &font, stop.as_ref());
            // Stopped: a name's value under the pointer, as Visual Studio's data tips.
            if let Some(st) = &stop
                && let Some(pos) = out.response.hover_pos()
                && pos.x > out.response.rect.left() + GUTTER
            {
                let at = out.galley.cursor_from_pos(pos - out.galley_pos).index.0;
                if let Some(expr) = expression_at(&doc.text, at)
                    && let Ok(v) = dbg.answer(world, st.id, st.frame, &expr).clone()
                {
                    let tip = debug::Variable { name: expr, ..v };
                    let _ = out.response.response.clone().on_hover_ui_at_pointer(|ui| {
                        ui.set_max_width(520.0);
                        ui.label(variable_text(ui, &tip, f32::INFINITY).0);
                    });
                }
            }
            if let Some(line) = jump_line
                && let Some(r) = lines.get(line)
            {
                // The line's height at the code's left edge: a long line must not scroll the
                // view sideways to its middle.
                let left = out.response.rect.left();
                let at = Rect::from_x_y_ranges(left..=left + GUTTER, r.y_range());
                ui.scroll_to_rect(
                    at.expand2(Vec2::new(0.0, row_h * 3.0)),
                    Some(egui::Align::Center),
                );
            }
            // Gutter clicks: a breakpoint on that line, or off again; Shift+click shows the
            // line's values live instead (a logpoint); right-click for a condition.
            {
                let gutter = Rect::from_min_max(
                    out.response.rect.min,
                    Pos2::new(
                        out.response.rect.left() + GUTTER,
                        out.response.rect.bottom(),
                    ),
                );
                let click = ui
                    .interact(gutter, id.with("gutter"), egui::Sense::click())
                    .on_hover_text(
                        "Click: a breakpoint (the game waits there). Shift+click: the line's \
                         values, live. Right-click: a breakpoint with a condition.",
                    );
                let line_at = |pos: Pos2| {
                    lines
                        .iter()
                        .position(|r| r.top() <= pos.y && pos.y < r.bottom())
                };
                if let Some(line) = click.interact_pointer_pos().and_then(line_at) {
                    if click.clicked() {
                        if ui.input(|i| i.modifiers.shift) {
                            toggle_logpoint(world, doc, line);
                        } else {
                            toggle_breakpoint(doc, dbg, line as u32 + 1);
                        }
                    }
                    if click.secondary_clicked() {
                        dbg.menu_line = line as u32 + 1;
                        dbg.condition = doc
                            .breakpoints
                            .get(&dbg.menu_line)
                            .cloned()
                            .flatten()
                            .unwrap_or_default();
                    }
                }
                click.context_menu(|ui| gutter_menu(ui, world, doc, dbg));
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

/// The line under each one's marks: the caret's line, errors, logpoints, the stop.
fn paint_backdrop(
    ui: &egui::Ui,
    slot: egui::layers::ShapeIdx,
    area: &Rect,
    lines: &[Rect],
    current: usize,
    doc: &Doc,
    stop: Option<&FileStop>,
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
    if let Some(stop) = stop
        && let Some(r) = lines.get((stop.line as usize).saturating_sub(1))
    {
        // Where it stopped in yellow; a caller's line (picked in the call stack) in green.
        let (fill, edge) = if stop.frame == 0 {
            (STOP_BG, STOP)
        } else {
            (CALLER_BG, theme::PLAY)
        };
        shapes.push(egui::Shape::rect_filled(row(r), 0.0, fill));
        shapes.push(egui::Shape::rect_stroke(
            row(r),
            0.0,
            Stroke::new(1.0, edge.gamma_multiply(0.45)),
            egui::StrokeKind::Inside,
        ));
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

/// Line numbers, the profiler's heat bars and the markers (breakpoint, error, logpoint), and
/// the stop's arrow.
#[allow(clippy::too_many_arguments)]
fn paint_gutter(
    ui: &egui::Ui,
    area: &Rect,
    lines: &[Rect],
    current: usize,
    doc: &Doc,
    font: &FontId,
    row_h: f32,
    stop: Option<&FileStop>,
    unbound: &HashSet<u32>,
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
        if let Some(condition) = doc.breakpoints.get(&line) {
            let radius = row_h * 0.3;
            if unbound.contains(&line) {
                painter.circle_stroke(marker, radius - 0.75, Stroke::new(1.5, BREAKPOINT));
            } else {
                painter.circle_filled(marker, radius, BREAKPOINT);
            }
            if condition.is_some() {
                // Visual Studio's conditional mark: a bar across the dot.
                let w = radius * 0.55;
                painter.line_segment(
                    [marker - Vec2::new(w, 0.0), marker + Vec2::new(w, 0.0)],
                    Stroke::new(1.6, Color32::WHITE),
                );
            }
        } else if errors.contains(&line) {
            let s = row_h * 0.2;
            let stroke = Stroke::new(2.0, ERROR);
            painter.line_segment(
                [marker + Vec2::new(-s, -s), marker + Vec2::new(s, s)],
                stroke,
            );
            painter.line_segment(
                [marker + Vec2::new(-s, s), marker + Vec2::new(s, -s)],
                stroke,
            );
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
    // The stop's arrow over the marker, as Visual Studio's (green on a caller's line).
    if let Some(stop) = stop
        && let Some(r) = lines.get((stop.line as usize).saturating_sub(1))
    {
        let c = Pos2::new(area.left() + 25.0, r.center().y);
        let h = row_h * 0.32;
        let color = if stop.frame == 0 { STOP } else { theme::PLAY };
        painter.add(egui::Shape::convex_polygon(
            vec![
                c + Vec2::new(-h * 1.3, -h * 0.55),
                c + Vec2::new(0.0, -h * 0.55),
                c + Vec2::new(0.0, -h),
                c + Vec2::new(h, 0.0),
                c + Vec2::new(0.0, h),
                c + Vec2::new(0.0, h * 0.55),
                c + Vec2::new(-h * 1.3, h * 0.55),
            ],
            color,
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        ));
    }
}

/// Notes after the end of lines, as Error Lens draws them: errors, unknown engine functions,
/// logpoint values, heat, and while stopped each local's value beside the line declaring it.
fn paint_notes(
    ui: &egui::Ui,
    lines: &[Rect],
    doc: &Doc,
    lints: &[Lint],
    font: &FontId,
    stop: Option<&FileStop>,
) {
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
    if let Some(stop) = stop {
        let why = match &stop.exception {
            Some(e) => (format!("⏸ {e}"), ERROR),
            None if stop.frame > 0 => (
                format!("⏸ calls on from here ({})", stop.function),
                theme::PLAY,
            ),
            None => (format!("⏸ paused: {}", stop.reason), STOP),
        };
        notes
            .entry((stop.line as usize).saturating_sub(1))
            .or_default()
            .insert(0, why);
        for (line, values) in &stop.values {
            notes
                .entry(*line as usize - 1)
                .or_default()
                .push((values.clone(), STOP_VALUE));
        }
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

// ----------------------------------------------------------------------------- debugging

/// The debugger in the tab: the stop it went to, the frame picked in the call stack, the
/// watches, and the answers for this stop (asked once per stop and frame, since a watch may do
/// more than read).
#[derive(Default)]
pub struct DebugState {
    /// Stop on errors nothing catches, where they are raised.
    pub break_errors: bool,
    /// Watch expressions (Luau), run in the picked frame at each stop.
    pub watches: Vec<String>,
    /// The call stack's frame shown (0: where it stopped).
    pub frame: usize,
    /// The stop the tab last went to.
    seen: u64,
    watch_new: String,
    answers: HashMap<String, Result<debug::Variable, String>>,
    answers_for: (u64, usize),
    /// The tab's breakpoints are in the running scripts; set them again.
    applied: bool,
    apply: bool,
    /// The gutter menu's line (1-based) and the condition typed in it.
    menu_line: u32,
    condition: String,
}

impl DebugState {
    /// The answer to `expr` at this stop, in frame `frame`.
    fn answer(
        &mut self,
        world: &mut World,
        stop: u64,
        frame: usize,
        expr: &str,
    ) -> &Result<debug::Variable, String> {
        if self.answers_for != (stop, frame) {
            self.answers.clear();
            self.answers_for = (stop, frame);
        }
        self.answers
            .entry(expr.to_string())
            .or_insert_with(|| debug::evaluate(world, frame, expr))
    }
}

/// Puts the tab's breakpoints in the running scripts. The editor's client attaches with the
/// first one (until then scripts run without the debugger's hooks) and stays for the Play.
fn apply_debugging(world: &mut World, tab: &ScriptsTab) {
    let any = tab.debug.break_errors || tab.docs.iter().any(|d| !d.breakpoints.is_empty());
    if !any && !debug::attached(world) {
        return;
    }
    debug::attach(world);
    debug::set_break_on_errors(world, tab.debug.break_errors);
    for doc in &tab.docs {
        let lines: Vec<(u32, Option<String>)> = doc
            .breakpoints
            .iter()
            .map(|(line, condition)| (*line, condition.clone()))
            .collect();
        debug::set_breakpoints_with(world, &doc.path, &lines);
    }
}

/// Keeps the running scripts' breakpoints the tab's, and goes to each stop: its file at its
/// line, in front (as Roblox Studio and Visual Studio bring the stopped script up), with a
/// note on the editor's status bar.
fn follow_debugger(world: &mut World) {
    let Some(mut tab) = world.remove_resource::<ScriptsTab>() else {
        return;
    };
    if world.contains_resource::<ScriptsHeld>() {
        tab.debug.applied = false;
        tab.debug.seen = 0;
    } else if !tab.debug.applied || std::mem::take(&mut tab.debug.apply) {
        apply_debugging(world, &tab);
        tab.debug.applied = true;
    }
    if let Some(stop) = debug::stopped(world)
        && stop.id != tab.debug.seen
    {
        tab.debug.seen = stop.id;
        tab.debug.frame = 0;
        if let Some(f) = stop.frames.iter().find(|f| openable(&f.path)) {
            let to = if f.path.ends_with(".graph") {
                if let Some(mut graphs) = world.remove_resource::<crate::graphs::GraphsTab>() {
                    crate::graphs::open(world, &mut graphs, &f.path);
                    world.insert_resource(graphs);
                }
                Tab::Graphs
            } else {
                open(world, &mut tab, &f.path, Some(f.line));
                Tab::Scripts
            };
            if let Some(mut dock) = world.get_resource_mut::<EditorDock>()
                && let Some(at) = dock.0.find_tab(&to)
            {
                let _ = dock.0.set_active_tab(at);
            }
        }
        let now = editor::now(world);
        let text = format!(
            "{}: the scripts wait and the world's time is held (F5 goes on)",
            stop_title(&stop)
        );
        if let Some(mut ed) = world.get_resource_mut::<Editor>() {
            ed.info(now, text);
        }
    }
    world.insert_resource(tab);
}

/// A frame's file the editor can show (not the engine's own Luau, nor the command bar's).
fn openable(path: &str) -> bool {
    (path.ends_with(".luau") || path.ends_with(".lua") || path.ends_with(".graph"))
        && !path.starts_with("engine/")
}

/// "Paused at a breakpoint in Lamp:update (lamp.luau:27)".
pub(crate) fn stop_title(stop: &StopView) -> String {
    let what = match (&stop.exception, stop.reason.as_str()) {
        (Some(_), _) => "Stopped on an error".to_string(),
        (None, "breakpoint") => "Paused at a breakpoint".into(),
        (None, "step") => "Paused after a step".into(),
        (None, "pause") => "Paused".into(),
        (None, other) => format!("Paused ({other})"),
    };
    match stop.frames.iter().find(|f| openable(&f.path)) {
        Some(f) => format!(
            "{what} in {} ({}:{})",
            f.name,
            f.path.rsplit('/').next().unwrap_or(&f.path),
            f.line
        ),
        None => what,
    }
}

/// F5, F10, F11 and Shift+F11 while stopped (taken before the editor's F5, Play/Stop).
fn debug_keys(ui: &egui::Ui, world: &mut World, stop: &StopView) {
    let key = |mods: Modifiers, k: Key| ui.input_mut(|i| i.consume_key(mods, k));
    let go = if key(Modifiers::NONE, Key::F5) {
        Some(Go::Continue)
    } else if key(Modifiers::NONE, Key::F10) {
        Some(Go::StepOver)
    } else if key(Modifiers::SHIFT, Key::F11) {
        Some(Go::StepOut)
    } else if key(Modifiers::NONE, Key::F11) {
        Some(Go::StepIn)
    } else {
        None
    };
    if let Some(go) = go
        && (stop.live || go == Go::Continue)
    {
        debug::go(world, go);
    }
}

/// The strip over the code while stopped: where and why, and the buttons to go on.
fn stop_strip(ui: &mut egui::Ui, world: &mut World, stop: &StopView) {
    egui::Frame::new()
        .fill(STOP_STRIP)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let buttons = [
                    ("▶ Continue", "F5", Go::Continue),
                    ("Step over", "F10", Go::StepOver),
                    ("Step into", "F11", Go::StepIn),
                    ("Step out", "Shift+F11", Go::StepOut),
                ];
                for (label, key, go) in buttons {
                    let enabled = stop.live || go == Go::Continue;
                    let text = if go == Go::Continue {
                        RichText::new(label).color(theme::PLAY).strong()
                    } else {
                        RichText::new(label)
                    };
                    if ui
                        .add_enabled(enabled, egui::Button::new(text))
                        .on_hover_text(key)
                        .on_disabled_hover_text(
                            "The error ended this script's call: look at its values, then go on",
                        )
                        .clicked()
                    {
                        debug::go(world, go);
                    }
                }
                ui.add_space(8.0);
                ui.label(RichText::new(stop_title(stop)).color(STOP).strong());
                if let Some(e) = &stop.exception {
                    ui.label(RichText::new(e).color(ERROR));
                } else {
                    ui.label(theme::dim("Other scripts wait, the world's time is held."));
                }
            });
        });
}

/// The stop's panel, as Visual Studio's and Roblox Studio's debugger windows: the call stack
/// (click a frame for its line and variables), the frame's variables (tables open up), the
/// watches and every breakpoint.
fn debug_panel(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab, stop: &StopView) {
    let heading = |ui: &mut egui::Ui, text: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(text).small().strong().color(theme::TEXT_DIM));
    };
    egui::ScrollArea::vertical()
        .id_salt("script debugger")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Every row is cut to the panel's width ("…"), never widening it under the code.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.set_max_width(ui.available_width());
            heading(ui, "CALL STACK");
            let mut pick = None;
            for (i, f) in stop.frames.iter().enumerate() {
                let file = f.path.rsplit('/').next().unwrap_or(&f.path);
                let mut job = LayoutJob::default();
                let mono = FontId::monospace(12.5);
                let arrow = if i == 0 { "▶ " } else { "   " };
                job.append(arrow, 0.0, fmt(&mono, STOP));
                let name_color = if openable(&f.path) {
                    theme::TEXT
                } else {
                    theme::TEXT_DIM
                };
                job.append(&f.name, 0.0, fmt(&mono, name_color));
                job.append(
                    &format!("  {file}:{}", f.line),
                    0.0,
                    fmt(&mono, theme::TEXT_DIM),
                );
                let r = ui.add(egui::Button::selectable(i == tab.debug.frame, job));
                if r.clicked() {
                    pick = Some(i);
                }
            }
            if let Some(i) = pick {
                tab.debug.frame = i;
                let f = &stop.frames[i];
                if openable(&f.path) && !f.path.ends_with(".graph") {
                    open(world, tab, &f.path, Some(f.line));
                }
            }
            let frame = tab.debug.frame.min(stop.frames.len().saturating_sub(1));

            heading(ui, "VARIABLES");
            for scope in debug::scopes(world, frame) {
                egui::CollapsingHeader::new(RichText::new(&scope.name).strong())
                    .id_salt(("script scope", frame, &scope.name))
                    .default_open(scope.name == "Locals")
                    .show(ui, |ui| {
                        variable_rows(ui, world, scope.reference, &scope.name, 0)
                    });
            }

            heading(ui, "WATCH");
            let mut remove = None;
            for i in 0..tab.debug.watches.len() {
                let expr = tab.debug.watches[i].clone();
                let answer = tab.debug.answer(world, stop.id, frame, &expr).clone();
                ui.horizontal(|ui| {
                    if ui
                        .small_button("✖")
                        .on_hover_text("Remove the watch")
                        .clicked()
                    {
                        remove = Some(i);
                    }
                    let width = ui.available_width() - 20.0;
                    match answer {
                        Ok(v) if v.reference != 0 => {
                            let (text, _) = variable_text(ui, &v, width);
                            egui::CollapsingHeader::new(text)
                                .id_salt(("script watch", &expr))
                                .show(ui, |ui| variable_rows(ui, world, v.reference, &expr, 1));
                        }
                        Ok(v) => {
                            let (text, cut) = variable_text(ui, &v, width);
                            let r = ui.label(text);
                            if cut {
                                r.on_hover_text(&v.value);
                            }
                        }
                        Err(e) => {
                            ui.label(RichText::new(format!("{expr}: {e}")).color(ERROR))
                                .on_hover_text(e);
                        }
                    }
                });
            }
            if let Some(i) = remove {
                tab.debug.watches.remove(i);
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut tab.debug.watch_new)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("Add a watch: self.t * 2")
                    .desired_width(f32::INFINITY),
            );
            if r.lost_focus()
                && ui.input(|i| i.key_pressed(Key::Enter))
                && !tab.debug.watch_new.trim().is_empty()
            {
                let w = std::mem::take(&mut tab.debug.watch_new).trim().to_string();
                tab.debug.watches.push(w);
            }

            heading(ui, "BREAKPOINTS");
            breakpoint_list(ui, world, tab);
        });
}

fn fmt(font: &FontId, color: Color32) -> TextFormat {
    TextFormat::simple(font.clone(), color)
}

/// `name = value`, colored as the code is; a value too long for `width` points is cut with "…"
/// (true when cut: hover shows it whole).
fn variable_text(ui: &egui::Ui, v: &debug::Variable, width: f32) -> (LayoutJob, bool) {
    let mono = FontId::monospace(12.5);
    let em = ui.fonts_mut(|f| f.glyph_width(&mono, '0')).max(1.0);
    let room = ((width / em) as usize)
        .saturating_sub(v.name.chars().count() + 3)
        .max(6);
    let cut = v.value.chars().count() > room;
    let value: String = if cut {
        v.value.chars().take(room - 1).chain(['…']).collect()
    } else {
        v.value.clone()
    };
    let mut job = LayoutJob::default();
    let name_color = if v.name == "self" { SELF } else { theme::TEXT };
    job.append(&v.name, 0.0, fmt(&mono, name_color));
    job.append(" = ", 0.0, fmt(&mono, LINE_NO));
    let value_color = match v.kind.as_str() {
        "number" | "integer" => NUMBER,
        "string" => STRING,
        "boolean" | "nil" => KEYWORD,
        "vec3" | "quat" => ENGINE,
        "function" => CALL,
        _ => theme::TEXT_DIM,
    };
    job.append(&value, 0.0, fmt(&mono, value_color));
    (job, cut)
}

/// A scope's variables or a table's entries; tables open up (their open state kept by the
/// path of names, so it stays from step to step).
fn variable_rows(ui: &mut egui::Ui, world: &World, reference: usize, path: &str, depth: usize) {
    let vars = debug::children(world, reference);
    if vars.is_empty() {
        ui.label(theme::dim("(none)"));
    }
    for v in vars.into_iter().take(300) {
        let at = format!("{path}.{}", v.name);
        // Both kinds of row start after the open/close arrow's room.
        let width = ui.available_width() - 20.0;
        let (text, cut) = variable_text(ui, &v, width);
        let r = if v.reference != 0 && depth < 8 {
            egui::CollapsingHeader::new(text)
                .id_salt(("script variable", &at))
                .show(ui, |ui| {
                    variable_rows(ui, world, v.reference, &at, depth + 1)
                })
                .header_response
        } else {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(text)
            })
            .inner
        };
        if cut {
            r.on_hover_ui(|ui| {
                ui.set_max_width(520.0);
                ui.label(RichText::new(&v.value).monospace());
            });
        }
    }
}

/// Every open file's breakpoints, with their conditions and how often they stopped.
fn breakpoint_list(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab) {
    let mut go_to = None;
    let mut remove = None;
    for (d, doc) in tab.docs.iter().enumerate() {
        let hits: HashMap<u32, u64> = debug::breakpoints(world, &doc.path)
            .into_iter()
            .map(|b| (b.line, b.hits))
            .collect();
        for (line, condition) in &doc.breakpoints {
            ui.horizontal(|ui| {
                if ui.small_button("✖").on_hover_text("Remove").clicked() {
                    remove = Some((d, *line));
                }
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.5, BREAKPOINT);
                let mut text = format!("{}:{line}", doc.name());
                if let Some(c) = condition {
                    text.push_str(&format!("  when {c}"));
                }
                if let Some(n) = hits.get(line).filter(|n| **n > 0) {
                    text.push_str(&format!("  · {n} hit{}", if *n == 1 { "" } else { "s" }));
                }
                if ui
                    .add(
                        egui::Label::new(RichText::new(text).monospace())
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Open its line")
                    .clicked()
                {
                    go_to = Some((doc.path.clone(), *line));
                }
            });
        }
    }
    if tab.docs.iter().all(|d| d.breakpoints.is_empty()) {
        ui.label(theme::dim("Click left of a line number to stop there."));
    }
    if let Some((d, line)) = remove {
        tab.docs[d].breakpoints.remove(&line);
        tab.debug.apply = true;
    }
    if let Some((path, line)) = go_to {
        open(world, tab, &path, Some(line));
    }
}

/// A click in the gutter: a breakpoint on that line (1-based), or none again.
fn toggle_breakpoint(doc: &mut Doc, debug: &mut DebugState, line: u32) {
    if doc.breakpoints.remove(&line).is_none() {
        doc.breakpoints.insert(line, None);
    }
    debug.apply = true;
}

/// The gutter's right-click menu: a breakpoint, one with a condition, or a logpoint.
fn gutter_menu(ui: &mut egui::Ui, world: &mut World, doc: &mut Doc, debug: &mut DebugState) {
    let line = debug.menu_line;
    ui.set_min_width(260.0);
    let has = doc.breakpoints.contains_key(&line);
    let label = if has {
        "Remove the breakpoint"
    } else {
        "Add a breakpoint (F9)"
    };
    if ui.button(label).clicked() {
        toggle_breakpoint(doc, debug, line);
        ui.close();
    }
    ui.label(theme::dim("Stop only when (Luau, in the line's scope):"));
    let r = ui.add(
        egui::TextEdit::singleline(&mut debug.condition)
            .font(egui::TextStyle::Monospace)
            .hint_text("self.t > 5")
            .desired_width(f32::INFINITY),
    );
    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if (ui.button("Set the conditional breakpoint").clicked() || enter)
        && !debug.condition.trim().is_empty()
    {
        doc.breakpoints
            .insert(line, Some(debug.condition.trim().to_string()));
        debug.apply = true;
        ui.close();
    }
    ui.separator();
    let label = if doc.logpoints.contains_key(&line) {
        "Stop showing its values"
    } else {
        "Show its values live (Shift+click)"
    };
    if ui.button(label).clicked() {
        toggle_logpoint(world, doc, line as usize - 1);
        ui.close();
    }
}

/// The name under the pointer in the code, with what leads to it (`self.props.period`), to
/// show its value while stopped.
fn expression_at(text: &str, char_index: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(char_index).is_some_and(|c| word(*c)) {
        return None;
    }
    let mut end = char_index;
    while end < chars.len() && word(chars[end]) {
        end += 1;
    }
    let mut start = char_index;
    while start > 0 && (word(chars[start - 1]) || chars[start - 1] == '.') {
        start -= 1;
    }
    let expr: String = chars[start..end].iter().collect();
    let expr = expr.trim_start_matches('.').to_string();
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
        "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        "continue", "export", "type",
    ];
    let first = expr.split('.').next().unwrap_or("");
    (!expr.is_empty()
        && !first.starts_with(|c: char| c.is_ascii_digit())
        && !KEYWORDS.contains(&first)
        && !expr.split('.').any(str::is_empty))
    .then_some(expr)
}

// ----------------------------------------------------------------------------- the Details panel

/// What the Details panel keeps for scripts between frames.
#[derive(Default)]
pub struct PropsPanel {
    /// Scene objects (by [`SceneOrder`]) whose script values Stop keeps, as Unreal's Keep
    /// Simulation Changes: their props as they were last frame.
    keep: BTreeMap<u64, Option<Value>>,
    /// A list prop's text while it is typed (by entity and prop).
    typing: HashMap<(Entity, String), String>,
    /// Each file's props as it last loaded, shown while it has a mistake.
    good: HashMap<String, Vec<Prop>>,
}

/// What a prop's row asks for.
enum RowAction {
    Set(Value),
    /// Back to the script's default.
    Reset,
    /// Show its line in the Scripts tab.
    Open(u32),
}

/// A Script component in the Details panel (inspector.rs), as Unity's inspector shows a
/// MonoBehaviour's fields and Unreal its Instance Editable variables: the file (pick another,
/// or open it), Enabled, and each prop the script declares (`engine_script::props`) as a field
/// of its kind, in its groups, with its comment as the tooltip. A value set here overrides the
/// script's default for this object: bold, with the blue bar of an override (Unity's), and ↺ or
/// right-click puts the default back (Unreal's reset arrow). While playing, changes reach the
/// running script at once (`on_props`), the script's own values show live under its props, and
/// Keep after Stop keeps what was tuned.
pub fn details(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    value: &Value,
) {
    let path = value["path"].as_str().unwrap_or_default().to_string();
    let file = path.rsplit('/').next().unwrap_or(&path).to_string();
    let playing = editor.is_playing();
    let mut after = value.clone();
    let mut open_at: Option<Option<u32>> = None;

    ui.horizontal(|ui| {
        let shown = if path.is_empty() {
            "Choose a script…".to_string()
        } else {
            path.clone()
        };
        egui::ComboBox::from_id_salt(("script file", entity))
            .selected_text(RichText::new(format!("📜 {shown}")).monospace())
            .width((ui.available_width() - 62.0).max(80.0))
            .height(360.0)
            .show_ui(ui, |ui| {
                for f in engine_script::props::files(world) {
                    if ui.selectable_label(f == path, &f).clicked() && f != path {
                        after["path"] = json!(f);
                        // Another script: its own defaults.
                        after["props"] = json!({});
                    }
                }
            });
        if ui
            .add_enabled(!path.is_empty(), egui::Button::new("Open"))
            .on_hover_text("Show the file in the Scripts tab")
            .clicked()
        {
            open_at = Some(None);
        }
    });
    let mut enabled = value["enabled"].as_bool().unwrap_or(true);
    if ui
        .checkbox(&mut enabled, "Enabled")
        .on_hover_text("While off, the script's hooks and tasks pause (its state stays)")
        .changed()
    {
        after["enabled"] = json!(enabled);
    }
    if path.is_empty() {
        commit_script(world, editor, entity, value, after);
        return;
    }

    // What keeps it from running: a file that doesn't load, or an error while playing.
    let schema = engine_script::props::of(world, &path);
    let marks = editing::file_marks(world, &path);
    let problem = match &schema {
        Err(e) => Some((error_line(e), message_gist(e))),
        Ok(_) => marks
            .error
            .as_ref()
            .or(marks.fault.as_ref())
            .map(|m| (m.line, m.message.clone())),
    };
    if let Some((line, message)) = problem {
        let at = line.map(|l| format!("{file}:{l}: ")).unwrap_or_default();
        let r = ui.add(
            egui::Label::new(RichText::new(format!("✖ {at}{message}")).color(ERROR))
                .wrap()
                .sense(egui::Sense::click()),
        );
        if r.on_hover_text("Open its line").clicked() {
            open_at = Some(line);
        }
    }

    // A file with a mistake keeps the fields it had (as the running game keeps its old code).
    let loaded = schema.is_ok();
    let props = {
        let mut tab = world.get_resource_mut::<ScriptsTab>();
        match (schema, tab.as_deref_mut()) {
            (Ok(props), Some(tab)) => {
                tab.props.good.insert(path.clone(), props.clone());
                props
            }
            (Ok(props), None) => props,
            (Err(_), tab) => tab
                .and_then(|t| t.props.good.get(&path).cloned())
                .unwrap_or_default(),
        }
    };
    let shown: Vec<&Prop> = props
        .iter()
        .filter(|p| !p.hidden && !matches!(p.kind, PropKind::Other(_)))
        .collect();
    if shown.is_empty() && loaded && problem_free(&marks) {
        ui.label(theme::dim(
            "No props: settings the script reads as self.props.<name> show here once its \
             props table declares them.",
        ));
    } else if !loaded && !shown.is_empty() {
        ui.label(theme::dim("As they were before the mistake:"));
    }
    let names = if shown.iter().any(|p| has_entity(p)) {
        scene_names(world)
    } else {
        Vec::new()
    };
    let overrides = value["props"].as_object().cloned().unwrap_or_default();
    let mut actions: Vec<(String, RowAction)> = Vec::new();
    {
        let mut typing = world
            .get_resource_mut::<ScriptsTab>()
            .map(|mut t| std::mem::take(&mut t.props.typing))
            .unwrap_or_default();
        let mut row_ctx = RowCtx {
            entity,
            names: &names,
            typing: &mut typing,
        };
        // Props without a group first, then each group (Godot's and Unreal's categories).
        let mut groups: Vec<Option<&str>> = Vec::new();
        for p in &shown {
            if !groups.contains(&p.group.as_deref()) {
                groups.push(p.group.as_deref());
            }
        }
        groups.sort_by_key(|g| g.is_some());
        for group in groups {
            let mut rows = |ui: &mut egui::Ui| {
                for p in shown.iter().filter(|p| p.group.as_deref() == group) {
                    if let Some(a) = prop_row(ui, &mut row_ctx, p, overrides.get(&p.name)) {
                        actions.push((p.name.clone(), a));
                    }
                }
            };
            match group {
                None => rows(ui),
                Some(name) => {
                    egui::CollapsingHeader::new(RichText::new(name).strong().color(theme::TEXT))
                        .id_salt(("script group", entity, name))
                        .default_open(true)
                        .show(ui, rows);
                }
            }
        }
        if let Some(mut t) = world.get_resource_mut::<ScriptsTab>() {
            t.props.typing = typing;
        }
    }
    for (name, action) in actions {
        match action {
            RowAction::Set(v) => {
                let default = props.iter().find(|p| p.name == name).map(|p| &p.default);
                if let Some(map) = after["props"].as_object_mut() {
                    // The default again is no override (the scene stays clean, as Unity's).
                    if default.is_some_and(|d| same(d, &v)) {
                        map.remove(&name);
                    } else {
                        map.insert(name, v);
                    }
                } else {
                    after["props"] = json!({ name: v });
                }
            }
            RowAction::Reset => {
                if let Some(map) = after["props"].as_object_mut() {
                    map.remove(&name);
                }
            }
            RowAction::Open(line) => open_at = Some(Some(line)),
        }
    }

    if playing {
        live_values(ui, world, entity);
        keep_toggle(ui, world, entity);
    }
    commit_script(world, editor, entity, value, after);
    if let Some(line) = open_at {
        open_in_tab(world, &path, line);
    }
}

fn problem_free(marks: &FileMarks) -> bool {
    marks.error.is_none() && marks.fault.is_none()
}

fn has_entity(p: &Prop) -> bool {
    p.kind == PropKind::Entity || p.fields.iter().any(has_entity)
}

/// The scene's object names, for `@entity` props.
fn scene_names(world: &World) -> Vec<String> {
    let mut names: Vec<String> = world
        .iter_entities()
        .filter(|e| e.get::<crate::scene_ops::InScene>().is_some())
        .filter_map(|e| e.get::<Name>().map(|n| n.as_str().to_string()))
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Where a message's `file.luau:12:` (or `.graph`) ends, and the line.
fn located(message: &str) -> Option<(usize, u32)> {
    [".luau:", ".graph:"].iter().find_map(|ext| {
        let start = message.find(ext)? + ext.len();
        let digits: String = message[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let line = digits.parse().ok()?;
        let end = start + digits.len();
        Some((end + usize::from(message[end..].starts_with(':')), line))
    })
}

/// `lamp.luau:9: ...` → 9.
fn error_line(message: &str) -> Option<u32> {
    located(message).map(|(_, line)| line)
}

/// The message without its `file:line:` (shown apart) and the traceback.
fn message_gist(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message);
    match located(first) {
        Some((end, _)) => first[end..].trim().to_string(),
        None => first.trim().to_string(),
    }
}

/// Sets the Script component to `after` if anything changed (one undo step per drag).
fn commit_script(
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    before: &Value,
    after: Value,
) {
    if &after != before {
        editor::apply(
            world,
            editor,
            Command::Component {
                entity,
                name: "Script".into(),
                before: Some(before.clone()),
                after: Some(after),
            },
        );
    }
}

/// Opens `path` in the Scripts tab (a graph in the Graphs tab) at `line`, and shows that tab.
fn open_in_tab(world: &mut World, path: &str, line: Option<u32>) {
    let to = if path.ends_with(".graph") {
        if let Some(mut graphs) = world.remove_resource::<crate::graphs::GraphsTab>() {
            crate::graphs::open(world, &mut graphs, path);
            world.insert_resource(graphs);
        }
        Tab::Graphs
    } else {
        if let Some(mut tab) = world.remove_resource::<ScriptsTab>() {
            open(world, &mut tab, path, line);
            world.insert_resource(tab);
        }
        Tab::Scripts
    };
    if let Some(mut dock) = world.get_resource_mut::<EditorDock>()
        && let Some(at) = dock.0.find_tab(&to)
    {
        let _ = dock.0.set_active_tab(at);
    }
}

/// JSON values equal as props: numbers by value (`1800` and `1800.0`), arrays and objects
/// entry by entry.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            (x.as_f64().unwrap_or(f64::NAN) - y.as_f64().unwrap_or(f64::NAN)).abs() < 1e-9
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// A number as the scene stores it: whole numbers as integers.
fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

struct RowCtx<'a> {
    entity: Entity,
    names: &'a [String],
    typing: &'a mut HashMap<(Entity, String), String>,
}

/// One prop: its name (the override bar, the tooltip, a right-click menu), its field, and ↺
/// while it differs from the default. A table prop folds out with a row per entry.
fn prop_row(
    ui: &mut egui::Ui,
    ctx: &mut RowCtx,
    p: &Prop,
    current: Option<&Value>,
) -> Option<RowAction> {
    let overridden = current.is_some_and(|c| !same(c, &p.default));
    let mut value = current.cloned().unwrap_or_else(|| p.default.clone());
    if p.kind == PropKind::Table {
        // A part set keeps the rest of the table's defaults (engine_script's props merge).
        let mut set = None;
        let r = egui::CollapsingHeader::new(prop_label(p, overridden))
            .id_salt(("script table", ctx.entity, &p.name))
            .default_open(true)
            .show(ui, |ui| {
                for f in &p.fields {
                    let cur = value.get(&f.name).cloned();
                    match prop_row(ui, ctx, f, cur.as_ref()) {
                        Some(RowAction::Set(v)) => {
                            value[&f.name] = v;
                            set = Some(RowAction::Set(value.clone()));
                        }
                        Some(RowAction::Reset) => {
                            value[&f.name] = f.default.clone();
                            set = Some(RowAction::Set(value.clone()));
                        }
                        other => set = set.take().or(other),
                    }
                }
            });
        if overridden {
            crate::prefab_ui::bar(ui, r.header_response.rect);
        }
        let mut action = set;
        r.header_response
            .on_hover_ui(|ui| prop_tip(ui, p, overridden))
            .context_menu(|ui| {
                if let Some(a) = row_menu(ui, p, overridden) {
                    action = Some(a);
                }
            });
        return action;
    }
    let mut action = None;
    ui.horizontal(|ui| {
        let label_w = (ui.available_width() * 0.36).clamp(80.0, 150.0);
        let r = ui
            .allocate_ui_with_layout(
                Vec2::new(label_w, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(label_w);
                    ui.add(
                        egui::Label::new(prop_label(p, overridden))
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                },
            )
            .inner;
        if overridden {
            crate::prefab_ui::bar(ui, r.rect);
        }
        r.on_hover_ui(|ui| prop_tip(ui, p, overridden))
            .context_menu(|ui| {
                if let Some(a) = row_menu(ui, p, overridden) {
                    action = Some(a);
                }
            });
        // Room for ↺ at the end, so the fields line up whether or not it shows.
        let width = (ui.available_width() - 30.0).max(60.0);
        if edit_prop(ui, ctx, p, &mut value, width) {
            action = Some(RowAction::Set(value.clone()));
        }
        if overridden
            && ui
                .small_button("↺")
                .on_hover_text(format!(
                    "Back to the script's default ({})",
                    prop_value_text(p, &p.default)
                ))
                .clicked()
        {
            action = Some(RowAction::Reset);
        }
    });
    action
}

/// "Lumens" (bold blue while overridden).
fn prop_label(p: &Prop, overridden: bool) -> RichText {
    let text = RichText::new(crate::inspector::pretty(&p.name));
    if overridden {
        text.strong().color(crate::prefab_ui::PREFAB)
    } else {
        text
    }
}

fn prop_tip(ui: &mut egui::Ui, p: &Prop, overridden: bool) {
    ui.set_max_width(360.0);
    ui.label(RichText::new(&p.name).monospace().strong());
    if !p.doc.is_empty() {
        ui.label(&p.doc);
    }
    let mut facts = vec![format!("Default: {}", prop_value_text(p, &p.default))];
    if let (Some(lo), Some(hi)) = (p.min, p.max) {
        facts.push(format!("from {} to {}", short(lo), short(hi)));
    } else if let Some(lo) = p.min {
        facts.push(format!("at least {}", short(lo)));
    } else if let Some(hi) = p.max {
        facts.push(format!("at most {}", short(hi)));
    }
    ui.label(theme::dim(facts.join(", ")));
    if overridden {
        ui.label(theme::dim(
            "Set on this object (right-click or ↺ for the script's default)",
        ));
    }
}

fn row_menu(ui: &mut egui::Ui, p: &Prop, overridden: bool) -> Option<RowAction> {
    let mut out = None;
    if ui
        .add_enabled(
            overridden,
            egui::Button::new("Reset to the script's default"),
        )
        .clicked()
    {
        out = Some(RowAction::Reset);
        ui.close();
    }
    if p.line > 0 && ui.button(format!("Open its line ({})", p.line)).clicked() {
        out = Some(RowAction::Open(p.line));
        ui.close();
    }
    out
}

fn short(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0');
    s.strip_suffix('.').unwrap_or(s).to_string()
}

/// A value as the tooltip shows it: `1800 lm`, `45°`, `(1, 0.72, 0.42)`.
fn prop_value_text(p: &Prop, v: &Value) -> String {
    let unit = p
        .unit
        .as_deref()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    match (&p.kind, v) {
        (PropKind::Angle, Value::Number(n)) => {
            format!("{}°", short(n.as_f64().unwrap_or(0.0).to_degrees()))
        }
        (_, Value::Number(n)) => format!("{}{unit}", short(n.as_f64().unwrap_or(0.0))),
        (_, Value::String(s)) if s.is_empty() => "none".into(),
        (_, Value::String(s)) => format!("\"{s}\""),
        (_, Value::Array(a)) if a.iter().all(|x| x.is_number()) => format!(
            "({})",
            a.iter()
                .map(|x| short(x.as_f64().unwrap_or(0.0)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (_, other) => other.to_string(),
    }
}

/// How fast a number drags: about 1/200 of its default's size a point.
fn drag_speed(p: &Prop) -> f64 {
    if let Some(step) = p.step {
        return step;
    }
    let size = p.default.as_f64().unwrap_or(1.0).abs().max(0.2);
    size * 0.005
}

/// The field for one prop's value; true when it changed.
fn edit_prop(ui: &mut egui::Ui, ctx: &mut RowCtx, p: &Prop, value: &mut Value, width: f32) -> bool {
    let unit = p
        .unit
        .as_deref()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    match &p.kind {
        PropKind::Number | PropKind::Integer => {
            let mut v = value.as_f64().unwrap_or(0.0);
            let changed = if let (true, Some(lo), Some(hi)) = (p.slider, p.min, p.max) {
                // The slider and its number box share the width: the box as wide as its
                // longest value with the unit.
                let digits = format!("{:.2}", hi.abs().max(lo.abs())).len() + usize::from(lo < 0.0);
                let chars = digits + unit.chars().count();
                let number_w = 18.0 + 7.5 * chars as f32;
                ui.spacing_mut().slider_width =
                    (width - number_w - ui.spacing().item_spacing.x).max(40.0);
                let mut s = egui::Slider::new(&mut v, lo..=hi)
                    .suffix(unit)
                    .clamping(egui::SliderClamping::Always);
                if let Some(step) = p.step {
                    s = s.step_by(step);
                }
                if p.kind == PropKind::Integer {
                    s = s.integer();
                }
                ui.add(s).changed()
            } else {
                let mut d = egui::DragValue::new(&mut v)
                    .speed(drag_speed(p))
                    .suffix(unit)
                    .range(p.min.unwrap_or(f64::NEG_INFINITY)..=p.max.unwrap_or(f64::INFINITY));
                d = if p.kind == PropKind::Integer {
                    d.fixed_decimals(0).speed(drag_speed(p).max(0.05))
                } else {
                    d.max_decimals(4)
                };
                ui.add_sized([width, 20.0], d).changed()
            };
            if changed {
                *value = num(p.clamp(v));
            }
            changed
        }
        PropKind::Angle => {
            let mut deg = value.as_f64().unwrap_or(0.0).to_degrees();
            let d = egui::DragValue::new(&mut deg)
                .speed(0.5)
                .suffix("°")
                .max_decimals(2)
                .range(
                    p.min.map_or(f64::NEG_INFINITY, f64::to_degrees)
                        ..=p.max.map_or(f64::INFINITY, f64::to_degrees),
                );
            let changed = ui.add_sized([width, 20.0], d).changed();
            if changed {
                *value = num(p.clamp(deg.to_radians()));
            }
            changed
        }
        PropKind::Bool => {
            let mut b = value.as_bool().unwrap_or(false);
            let changed = ui.checkbox(&mut b, "").changed();
            if changed {
                *value = json!(b);
            }
            changed
        }
        PropKind::Text => {
            let mut s = value.as_str().unwrap_or_default().to_string();
            let changed = ui
                .add(egui::TextEdit::singleline(&mut s).desired_width(width))
                .changed();
            if changed {
                *value = json!(s);
            }
            changed
        }
        PropKind::Choice(list) => {
            let mut s = value.as_str().unwrap_or_default().to_string();
            let before = s.clone();
            egui::ComboBox::from_id_salt(("script choice", ctx.entity, &p.name))
                .selected_text(&s)
                .width(width)
                .show_ui(ui, |ui| {
                    for c in list {
                        ui.selectable_value(&mut s, c.clone(), c);
                    }
                });
            if s != before {
                *value = json!(s);
                return true;
            }
            false
        }
        PropKind::Entity => {
            let mut s = value.as_str().unwrap_or_default().to_string();
            let before = s.clone();
            let missing = !s.is_empty() && !ctx.names.contains(&s);
            let shown = if s.is_empty() {
                RichText::new("None").color(theme::TEXT_DIM)
            } else if missing {
                RichText::new(format!("{s} (not in the scene)")).color(ERROR)
            } else {
                RichText::new(format!("◼ {s}"))
            };
            egui::ComboBox::from_id_salt(("script entity", ctx.entity, &p.name))
                .selected_text(shown)
                .width(width)
                .height(320.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s, String::new(), theme::dim("None"));
                    for n in ctx.names {
                        ui.selectable_value(&mut s, n.clone(), n);
                    }
                });
            if s != before {
                *value = json!(s);
                return true;
            }
            false
        }
        PropKind::Vec3 | PropKind::Color => {
            let mut v = [0.0f64; 3];
            for (i, x) in v.iter_mut().enumerate() {
                *x = value.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            }
            let mut changed = false;
            let mut fields = width;
            if p.kind == PropKind::Color {
                let mut rgb = v.map(|x| x.clamp(0.0, 1.0) as f32);
                if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
                    v = rgb.map(f64::from);
                    changed = true;
                }
                fields -= ui.spacing().interact_size.x + ui.spacing().item_spacing.x;
            }
            let each = (fields / 3.0 - ui.spacing().item_spacing.x).max(30.0);
            let speed = if p.kind == PropKind::Color {
                0.005
            } else {
                drag_speed(p).max(0.01)
            };
            for x in &mut v {
                changed |= ui
                    .add_sized(
                        [each, 20.0],
                        egui::DragValue::new(x).speed(speed).max_decimals(3),
                    )
                    .changed();
            }
            if changed {
                *value = json!(v.map(|x| match num(x) {
                    Value::Number(n) => n,
                    _ => serde_json::Number::from(0),
                }));
            }
            changed
        }
        PropKind::Rotation => {
            let get = |i: usize| value.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let q = engine::core::math::Quat::from_xyzw(get(0), get(1), get(2), get(3));
            let (y, x, z) = q.to_euler(engine::core::math::EulerRot::YXZ);
            let mut deg = [x, y, z].map(|a| f64::from(a).to_degrees());
            let each = (width / 3.0 - ui.spacing().item_spacing.x).max(30.0);
            let mut changed = false;
            for a in &mut deg {
                changed |= ui
                    .add_sized(
                        [each, 20.0],
                        egui::DragValue::new(a)
                            .speed(0.5)
                            .suffix("°")
                            .max_decimals(1),
                    )
                    .changed();
            }
            if changed {
                let [x, y, z] = deg.map(|a| a.to_radians() as f32);
                let q = engine::core::math::Quat::from_euler(
                    engine::core::math::EulerRot::YXZ,
                    y,
                    x,
                    z,
                );
                *value = json!([q.x, q.y, q.z, q.w]);
            }
            changed
        }
        PropKind::List => {
            // Typed as JSON; set when it parses and Enter or leaving the field ends it.
            let key = (ctx.entity, p.name.clone());
            let text = ctx
                .typing
                .entry(key.clone())
                .or_insert_with(|| value.to_string());
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(width),
            );
            if r.lost_focus() {
                let parsed = serde_json::from_str::<Value>(text);
                ctx.typing.remove(&key);
                if let Ok(v) = parsed
                    && v != *value
                {
                    *value = v;
                    return true;
                }
            }
            false
        }
        PropKind::Table | PropKind::Other(_) => false,
    }
}

/// While playing: the script's own values on this object (`self.t`, `self.lit`, ...), live.
fn live_values(ui: &mut egui::Ui, world: &World, entity: Entity) {
    let Some(state) = engine_script::props::state(world, entity) else {
        return;
    };
    egui::CollapsingHeader::new(RichText::new("Live values").strong().color(theme::TEXT))
        .id_salt(("script live", entity))
        .default_open(true)
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            if state.is_empty() {
                ui.label(theme::dim("(the script keeps no values of its own yet)"));
            }
            for v in &state {
                let (job, cut) = variable_text(ui, v, ui.available_width());
                let r = ui.label(job);
                if cut {
                    r.on_hover_text(&v.value);
                }
            }
        });
}

/// While playing: Keep after Stop (Unreal's Keep Simulation Changes), for scene objects.
fn keep_toggle(ui: &mut egui::Ui, world: &mut World, entity: Entity) {
    let Some(order) = world.get::<SceneOrder>(entity).map(|o| o.0) else {
        ui.label(theme::dim("Made while playing: Stop removes it."));
        return;
    };
    let Some(mut tab) = world.get_resource_mut::<ScriptsTab>() else {
        return;
    };
    let mut keep = tab.props.keep.contains_key(&order);
    let r = ui
        .toggle_value(&mut keep, "📌 Keep after Stop")
        .on_hover_text(
            "Stop puts the scene back as it was before Play. Pinned, this object keeps the script \
         values set while playing (one undo step).",
        );
    if r.changed() {
        if keep {
            tab.props.keep.insert(order, None);
        } else {
            tab.props.keep.remove(&order);
        }
    }
    if !keep {
        ui.label(theme::dim("Values set while playing go back at Stop."));
    }
}

/// Keep after Stop: the pinned objects' script props, read every frame while playing, and set
/// again once Stop has put the scene back.
fn keep_play_values(world: &mut World) {
    let Some(mode) = world.get_resource::<Editor>().map(|e| e.mode) else {
        return;
    };
    let mut keep = match world.get_resource_mut::<ScriptsTab>() {
        Some(mut tab) if !tab.props.keep.is_empty() => std::mem::take(&mut tab.props.keep),
        _ => return,
    };
    let mut q = world.query::<(Entity, &SceneOrder, &Script)>();
    if mode != Mode::Edit {
        for (_, order, script) in q.iter(world) {
            if let Some(slot) = keep.get_mut(&order.0) {
                *slot = Some(script.props.clone());
            }
        }
        world.resource_mut::<ScriptsTab>().props.keep = keep;
        return;
    }
    // Stopped: the scene is back as it was; the kept values go on again.
    let changes: Vec<(Entity, Value, Value)> = q
        .iter(world)
        .filter_map(|(e, order, script)| {
            let props = keep.get(&order.0)?.clone()?;
            if script.props == props {
                return None;
            }
            let before = serde_json::to_value(script).ok()?;
            let mut after = before.clone();
            after["props"] = props;
            Some((e, before, after))
        })
        .collect();
    keep.clear();
    if changes.is_empty() {
        return;
    }
    let n = changes.len();
    world.resource_scope(|world, mut editor: Mut<Editor>| {
        for (entity, before, after) in changes {
            editor::apply(
                world,
                &mut editor,
                Command::Component {
                    entity,
                    name: "Script".into(),
                    before: Some(before),
                    after: Some(after),
                },
            );
        }
        let now = editor::now(world);
        editor.info(
            now,
            format!(
                "Kept the script values set while playing on {n} object{}",
                if n == 1 { "" } else { "s" }
            ),
        );
    });
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
        // Debugging: a breakpoint on the lamp's glow line. Its script waits there and the
        // world's time with it, while the editor runs on: the arrow, each local's value beside
        // its line, the call stack, the variables and the watches.
        Step::Do(Box::new(open_file("examples/lamp.luau", Some(26)))),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<EditorDock>().0 = debug_layout();
        })),
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &mut tab.docs[tab.current];
            if let Some(line) = doc.text.lines().position(|l| l.contains("local glow")) {
                doc.breakpoints.insert(line as u32 + 1, None);
            }
            tab.debug.watches = vec![
                "p.lumens * breath".into(),
                "self.t % p.period".into(),
                "self.props".into(),
            ];
            tab.debug.apply = true;
        }))),
        Step::Wait(40),
        Step::Shot("scripts_07_breakpoint"),
        // Two steps over: the glow is worked out and the light set; the arrow moves on.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            debug::go(world, Go::StepOver);
        })),
        Step::Wait(4),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            debug::go(world, Go::StepOver);
        })),
        Step::Wait(30),
        Step::Shot("scripts_08_stepped"),
        // On again without the breakpoint, and Stop on errors: the light's misspelt name stops
        // where the error is raised, with the values that led to it.
        Step::Do(Box::new(with_tab(|world, tab| {
            for doc in &mut tab.docs {
                doc.breakpoints.clear();
            }
            tab.debug.break_errors = true;
            tab.debug.apply = true;
            debug::go(world, Go::Continue);
            let doc = &mut tab.docs[tab.current];
            doc.text = doc
                .saved
                .replace("\"PointLight\", { color", "\"PointLigth\", { color");
            let now = editor::now(world);
            save(world, tab, now);
        }))),
        Step::Wait(60),
        Step::Shot("scripts_09_error_stop"),
        Step::Do(Box::new(with_tab(|world, tab| {
            tab.debug.break_errors = false;
            tab.debug.apply = true;
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.saved.replace("\"PointLigth\"", "\"PointLight\"");
            let now = editor::now(world);
            save(world, tab, now);
            debug::go(world, Go::Continue);
        }))),
        Step::Wait(10),
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

/// `editor --headless --size 1920x1080 --shots <dir> --script props`: scripts' props in the
/// Details panel. The lamp's sliders with their units; a beacon's groups, table, choice, entity
/// picker and angle; values set on it (the override bar and ↺); playing, with the script's
/// live values and Keep after Stop; after Stop, the kept values; a mistake in its props table.
pub fn props_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        let sky = world
            .resource::<TypeRegistry>()
            .resource("Sky")
            .map(|r| r.patch);
        if let Some(patch) = sky {
            let _ = patch(world, json!({"hour": 19.3, "hours_per_second": 0.0}));
        }
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
        // A beacon in front of the stone, made as a user would: a light with a script.
        let bulb = MeshRenderer::new(
            Shape::Sphere {
                radius: 0.14,
                segments: 0,
            },
            StandardMaterial::hex(0xb8e0ff).with_emissive([0.35, 0.75, 1.0], 2000.0),
        );
        editor::spawn_with(
            world,
            editor,
            "Beacon".into(),
            vec![
                (
                    "MeshRenderer",
                    serde_json::to_value(bulb).unwrap_or_default(),
                ),
                (
                    "PointLight",
                    json!({"color": [0.35, 0.75, 1.0], "intensity_lm": 2400.0, "range": 10.0}),
                ),
                (
                    "Script",
                    serde_json::to_value(Script::new("examples/beacon.luau")).unwrap_or_default(),
                ),
            ],
            Transform::from_translation(DVec3::new(-0.9, 1.1, 5.2)),
        );
        world.resource_mut::<EditorDock>().0 = props_layout();
    };
    // Sets props on an object as the Details panel does (one undo step each).
    let set = |name: &'static str, props: Value| {
        move |world: &mut World, ed: &mut Editor| {
            let Some(e) = crate::shots::find(world, name) else {
                return;
            };
            let Some(before) = world
                .get::<Script>(e)
                .and_then(|s| serde_json::to_value(s).ok())
            else {
                return;
            };
            let mut after = before.clone();
            if let (Some(map), Some(add)) = (after["props"].as_object_mut(), props.as_object()) {
                for (k, v) in add {
                    map.insert(k.clone(), v.clone());
                }
            }
            editor::apply(
                world,
                ed,
                Command::Component {
                    entity: e,
                    name: "Script".into(),
                    before: Some(before),
                    after: Some(after),
                },
            );
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(1.2, 1.5, 9.2), DVec3::new(-0.6, 1.0, 3.0)),
        Step::Select("Lamp"),
        Step::Wait(40),
        Step::Shot("props_01_lamp_sliders"),
        Step::Select("Beacon"),
        Step::Wait(20),
        Step::Shot("props_02_beacon_groups"),
        // Values set on this beacon: warmer, brighter, blinking, aimed at the lamp.
        Step::Do(Box::new(set(
            "Beacon",
            json!({"lumens": 5200, "color": [1.0, 0.45, 0.2], "mode": "blink",
                   "target": "Lamp", "pulse": {"rate": 1.6, "depth": 0.6}}),
        ))),
        Step::Wait(20),
        Step::Shot("props_03_overrides"),
        // Playing: the script runs with them; tuned live, pinned to keep.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(40),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            if let Some(e) = crate::shots::find(world, "Beacon")
                && let Some(order) = world.get::<SceneOrder>(e).map(|o| o.0)
            {
                world
                    .resource_mut::<ScriptsTab>()
                    .props
                    .keep
                    .insert(order, None);
            }
        })),
        Step::Do(Box::new(set(
            "Beacon",
            json!({"spin": 1.2, "mode": "pulse", "color": [0.55, 1.0, 0.45], "tilt": 0.6}),
        ))),
        Step::Wait(90),
        Step::Shot("props_04_playing_live"),
        // Stop: the scene goes back, but the pinned beacon keeps what was tuned.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed)
        })),
        Step::Wait(20),
        Step::Shot("props_05_kept_after_stop"),
        // A mistake in the props table: the panel says where.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            let path = engine_script::scripts_root(world).join("examples/beacon.luau");
            if let Ok(text) = std::fs::read_to_string(&path) {
                let _ = std::fs::write(&path, text.replace("every = 4,", "every = 4 +,"));
            }
        })),
        Step::Wait(20),
        Step::Shot("props_06_mistake"),
    ];
    ShotScript::new(dir, steps)
}

/// The props shots' layout: the viewport, and the Details panel beside it at full height.
fn props_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport, Tab::Scripts]);
    let surface = dock.main_surface_mut();
    let [_view, _details] = surface.split_right(NodeIndex::root(), 0.64, vec![Tab::Inspector]);
    dock
}

/// The debugging shots' layout: the Scripts tab wider, for the stop's panel beside the code.
fn debug_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Scripts]);
    let surface = dock.main_surface_mut();
    let [_scripts, right] = surface.split_right(NodeIndex::root(), 0.7, vec![Tab::Viewport]);
    let [_view, _details] = surface.split_below(right, 0.6, vec![Tab::Inspector]);
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
    fn hovering_a_name_asks_for_it_and_what_leads_to_it() {
        let text = "local glow = self.props.lumens * k2\nif x then end";
        let at = |needle: &str, k: usize| text.find(needle).unwrap() + k;
        assert_eq!(
            expression_at(text, at("lumens", 2)).as_deref(),
            Some("self.props.lumens")
        );
        assert_eq!(
            expression_at(text, at("props", 0)).as_deref(),
            Some("self.props")
        );
        assert_eq!(expression_at(text, at("glow", 1)).as_deref(), Some("glow"));
        assert_eq!(expression_at(text, at("k2", 1)).as_deref(), Some("k2"));
        assert_eq!(expression_at(text, at("local", 1)), None);
        assert_eq!(expression_at(text, at(" = ", 1)), None);
        assert_eq!(expression_at(text, at("then", 0)), None);
    }

    #[test]
    fn props_compare_by_value_and_store_whole_numbers_as_integers() {
        assert!(same(&json!(1800), &json!(1800.0)));
        assert!(same(&json!([1, 0.5, 0]), &json!([1.0, 0.5, 0.0])));
        assert!(same(
            &json!({"rate": 2, "depth": 0.5}),
            &json!({"depth": 0.5, "rate": 2.0})
        ));
        assert!(!same(&json!({"rate": 2}), &json!({"rate": 2, "depth": 1})));
        assert!(!same(&json!("calm"), &json!("storm")));
        assert_eq!(num(4.0), json!(4));
        assert!(num(4.0).is_i64());
        assert_eq!(num(0.25), json!(0.25));
    }

    #[test]
    fn load_errors_give_their_line_and_gist() {
        let e = "beacon.luau:23: Expected identifier when parsing expression, got ','\nstack";
        assert_eq!(error_line(e), Some(23));
        assert_eq!(
            message_gist(e),
            "Expected identifier when parsing expression, got ','"
        );
        assert_eq!(error_line("can't read x"), None);
        assert_eq!(message_gist("can't read x"), "can't read x");
        assert_eq!(error_line("crystal.graph:4: node 3 has no input"), Some(4));
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
