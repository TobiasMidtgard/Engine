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
//! Types, as in Roblox Studio and Godot: Luau's own type checker ([`engine_script::analysis`])
//! reads every script on a thread of its own and knows what a script's `self` is (its class,
//! its props as declared, its entity), so a misspelt prop or entity field is underlined as you
//! type. Hovering a name shows its type and docs; after `.` and `:` completion lists the
//! members that value really has; inside a call the parameters show over the line with the
//! one being typed lit; F12 or Ctrl+click goes to where a name is declared. The Problems list
//! under the code has every script's type errors and warnings.
//!
//! Tests, as Unity's Test Runner: the Tests panel beside the Output runs the script tests
//! (`*.test.luau`) in the game, pressing Play for the run and Stop after, and lists each test
//! with its result; in a test file the gutter mark beside each test runs it and then shows how
//! it did, and a failed test's message sits on the line it stopped at.
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
use engine_script::analysis::{
    Ask, Checker, Hover, Job, Problem, Reply, Report, Signature, SuggestKind, Suggestion,
};
use engine_script::debug::{self, FileStop, Go, StopView};
use engine_script::editing::{self, FileMarks, OutputCursor, Word, WordKind};
use engine_script::props::{Prop, PropKind};
use engine_script::states::{Hit, Live, Machine, Problem as MachineProblem, StateBreakpoints};
use engine_script::{Script, ScriptGizmos, ScriptRuntime, ScriptSettings, ScriptsHeld};
use serde_json::{Value, json};

use crate::editor::{self, Editor, Mode, Tab};
use crate::scene_ops::SceneOrder;
use crate::shots::{ShotScript, Step};
use crate::states::Pick;
use crate::theme;
use crate::ui::EditorDock;
use crate::undo::Command;

/// Scripting in the editor: scripts held still outside Play.
pub struct ScriptsTabPlugin;

impl Plugin for ScriptsTabPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ScriptsHeld)
            .init_resource::<ScriptsTab>()
            // On by default, as in Unity's and Unreal's scene views.
            .insert_resource(ScriptGizmos {
                show: true,
                ..Default::default()
            })
            .add_systems(Update, hold_scripts.before(UpdateSet::Logic))
            .add_systems(Update, follow_selection.before(UpdateSet::Logic))
            .add_systems(Update, follow_debugger.after(UpdateSet::Logic))
            .add_systems(Update, pause_on_state_breaks.after(UpdateSet::Logic))
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

/// Script gizmos follow the editor: the selection draws `draw_gizmos_selected`, and Game view
/// hides them all.
fn follow_selection(world: &mut World) {
    let Some((selected, mut hide)) = world
        .get_resource::<Editor>()
        .map(|e| (e.selection.clone(), e.game_view))
    else {
        return;
    };
    // A moment shown in the Rewind tab draws the gizmos as they were then instead.
    hide |= world
        .get_resource::<engine_script::ScriptRewind>()
        .is_some_and(|r| r.shown.is_some());
    if let Some(mut g) = world.get_resource_mut::<ScriptGizmos>()
        && (g.selected != selected || g.hide != hide)
    {
        g.selected = selected;
        g.hide = hide;
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
const SIGNAL: Color32 = Color32::from_rgb(0xc8, 0x8c, 0xff);
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
    /// Luau's type checker and what it found.
    pub types: Types,
    /// The script tests' runs.
    pub tests: TestsPanel,
    /// The state machine beside the code.
    pub machine: MachinePanel,
    /// Screenshot runs: put the pointer over this (0-based line, char column) of the code.
    pub point_at: Option<(u32, u32)>,
    /// Where the pointer was last put for `point_at` (moved once: egui waits for a still
    /// pointer before it shows a tooltip).
    point_sent: Option<Pos2>,
    /// Where some widgets were last drawn, for screenshot runs to point at or click
    /// ("signal method 0", "heard 12").
    pub drawn_at: HashMap<String, Rect>,
    /// The prop handle being dragged in the scene view ([`handles`]).
    pub handle: Option<HandleDrag>,
    /// The Tools menu's tool files, and one picked there to run this frame.
    pub tools: ToolList,
    pub pending_tool: Option<ToolEntry>,
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
    tests_poll(world, editor, &mut tab, now);
    types_poll(world, &mut tab, now);
    if let Some((path, line, col)) = tab.types.goto.take() {
        open(world, &mut tab, &path, Some(line + 1));
        let doc = &mut tab.docs[tab.current];
        doc.jump = Some(char_at(&doc.text, line, col));
    }
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
    let machine = tab
        .docs
        .get(tab.current)
        .filter(|d| d.path.ends_with(".luau"))
        .map(|d| d.path.clone())
        .and_then(|path| machine_of(world, &path));
    tab.machine.has = machine.is_some();
    if let Some(machine) = machine.filter(|_| stop.is_none() && !tab.machine.hidden) {
        let selection = editor.selection.clone();
        let paused = editor.mode == Mode::Paused;
        // Leaves the code at least 520 points (its tool bar's width) where it can.
        let wide = (ui.available_width() - 520.0).clamp(300.0, 560.0);
        egui::Panel::right("script states")
            .resizable(true)
            .default_size(wide)
            .size_range(300.0..=1000.0)
            .frame(
                egui::Frame::new()
                    .fill(CODE_BG)
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                states_panel(ui, world, &mut tab, machine, &selection, paused)
            });
    }
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
    let tool = tab.pending_tool.take();
    world.insert_resource(tab);
    if let Some(t) = tool {
        run_tool(world, editor, ToolCall::File { path: t.file }, &t.title);
    }
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
                        let errors = tab.types.reports.get(&f.path).map_or(0, |r| r.errors());
                        let tests = tab.tests.counts(&f.path);
                        if file_row(ui, f, current == Some(f.path.as_str()), errors, tests) {
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
fn file_row(
    ui: &mut egui::Ui,
    f: &FileRow,
    current: bool,
    type_errors: usize,
    tests: Option<(usize, usize)>,
) -> bool {
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
    if type_errors > 0 {
        text.append(
            &format!("  ✖{type_errors}"),
            0.0,
            TextFormat::simple(FontId::proportional(11.0), ERROR),
        );
    }
    // A test file's last run.
    if let Some((passed, failed)) = tests {
        let (label, color) = if failed > 0 {
            (format!("  ✖ {failed} failing"), ERROR)
        } else {
            (format!("  ✔ {passed}"), theme::PLAY)
        };
        text.append(
            &label,
            0.0,
            TextFormat::simple(FontId::proportional(11.0), color),
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
        "construct",
        "draw_gizmos",
        "draw_gizmos_selected",
    ]
    .into();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        // A test file's groups and tests, indented as they nest.
        let test = [
            "describe(",
            "describe.skip(",
            "describe.only(",
            "it(",
            "it.skip(",
            "it.only(",
        ]
        .into_iter()
        .find(|p| t.starts_with(p));
        if let Some(p) = test {
            let rest = &t[p.len()..];
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let name = rest[1..].split(quote).next().unwrap_or("");
            let depth = line.len() - t.len();
            out.push(Function {
                line: i,
                label: format!("{}{} {name}", "  ".repeat(depth), &p[..p.len() - 1]),
                hook: false,
            });
            continue;
        }
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
            // The buttons take about 480 points: in a narrow code view (the States panel open
            // beside it) they get a row of their own instead of covering the file names.
            let right = egui::Layout::right_to_left(egui::Align::Center);
            if ui.available_width() < 780.0 {
                ui.horizontal_wrapped(|ui| doc_tabs(ui, world, tab));
                // Their own row, laid out in a rect of its own: buttons that don't fit are
                // cut off at its left end instead of pushing the code view out sideways (a
                // right-to-left row that overflows widens its parent to the left).
                let (row, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), ui.spacing().interact_size.y + 4.0),
                    egui::Sense::hover(),
                );
                let mut buttons = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(right));
                buttons.set_clip_rect(row.intersect(ui.clip_rect()));
                tool_buttons(&mut buttons, world, tab, now);
            } else {
                ui.horizontal(|ui| {
                    doc_tabs(ui, world, tab);
                    ui.with_layout(right, |ui| tool_buttons(ui, world, tab, now));
                });
            }
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

/// The open files, one button each (middle click closes).
fn doc_tabs(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab) {
    let mut close = None;
    for (i, doc) in tab.docs.iter().enumerate() {
        let current = i == tab.current;
        let label = format!("{}{}", doc.name(), if doc.dirty() { " •" } else { "" });
        let mut text = RichText::new(label);
        if doc.marks.error.is_some() || doc.marks.fault.is_some() || doc.syntax.is_some() {
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
}

/// The tool bar's buttons, right to left: Tools, States, VS Code, Stop on errors, Break,
/// Profile, Go to, Find, Revert, Save.
fn tool_buttons(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab, now: f64) {
    tools_menu(ui, world, tab, now);
    if tab.machine.has
        && ui
            .selectable_label(!tab.machine.hidden, "States")
            .on_hover_text(
                "The script's state machine (its states table) beside the code, \
                 live while playing",
            )
            .clicked()
    {
        tab.machine.hidden = !tab.machine.hidden;
    }
    if ui
        .selectable_label(tab.vscode, "VS Code")
        .on_hover_text("Debug these files in VS Code: breakpoints, stepping, watches")
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
    let heard = heard_marks(world, &tab.docs[i].path);
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
    // F12 (or Ctrl+click, below): where the name at the caret is declared.
    let mut ask_definition = focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F12));
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
    let point_at = tab.point_at;
    let doc = &mut tab.docs[i];
    let dbg = &mut tab.debug;
    let types = &mut tab.types;
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
    // A test file's describe and it lines, with how they did in the last run.
    let test_marks: Vec<(TestLine, Option<String>)> =
        if engine_script::testing::is_test_file(&doc.path) {
            test_lines(&doc.text)
                .into_iter()
                .map(|t| {
                    let status = line_status(&tab.tests, &doc.path, &t);
                    (t, status)
                })
                .collect()
        } else {
            Vec::new()
        };
    let test_notes = test_notes(&tab.tests, &doc.path);
    let mut run_test = None;
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
        let job = highlight(buf.as_str(), &font, &vocab, find.as_deref());
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut output = None;
    let mut scroll = egui::ScrollArea::both()
        .id_salt(("script scroll", doc.path.as_str()))
        .auto_shrink([false, false]);
    if jump_line.is_some() {
        // A jump shows its line from the start.
        scroll = scroll.horizontal_scroll_offset(0.0);
    }
    scroll.show(ui, |ui| {
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
        if let Some((l, c)) = point_at {
            let at = out
                .galley
                .pos_from_cursor(CCursor::new(char_at(&doc.text, l, c)));
            let p = out.galley_pos + at.center().to_vec2() + Vec2::new(3.0, 0.0);
            if tab.point_sent != Some(p) {
                tab.point_sent = Some(p);
                world
                    .resource_mut::<crate::egui_bridge::EguiInput>()
                    .injected
                    .push(egui::Event::PointerMoved(p));
            }
        }
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
        paint_heard(
            ui,
            &out.response.rect,
            &lines,
            &heard,
            row_h,
            &mut tab.drawn_at,
        );
        paint_test_marks(ui, &out.response.rect, &lines, &test_marks, row_h);
        for (t, _) in &test_marks {
            if let Some(r) = lines.get(t.line as usize - 1) {
                let key = format!("test {}", t.line);
                tab.drawn_at.insert(key, r.with_max_x(r.left()));
            }
        }
        let lints = lints(&doc.text, &vocab);
        let shown = shown_problems(doc, types, &lints);
        paint_notes(
            ui,
            &lines,
            doc,
            &lints,
            &shown,
            &test_notes,
            &font,
            stop.as_ref(),
        );
        paint_type_marks(ui, &out.galley, out.galley_pos, &doc.text, &shown);
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
        } else if stop.is_none()
            && let Some(pos) = out.response.hover_pos()
            && pos.x > out.response.rect.left() + GUTTER
        {
            // A name's type and docs, and the problems there, from the checker.
            let at = out.galley.cursor_from_pos(pos - out.galley_pos).index.0;
            let (line, col) = line_col(&doc.text, at);
            let here: Vec<Problem> = shown
                .iter()
                .copied()
                .filter(|p| {
                    p.span.line == line
                        && p.span.col <= col
                        && (col < p.span.end_col.max(p.span.col + 1) || p.span.end_line > line)
                })
                .cloned()
                .collect();
            let word = word_cols(&doc.text, line, col);
            if let Some((a, _)) = word {
                let key = (doc.path.clone(), line, a);
                if types.hover_asked.as_ref() != Some(&key) {
                    types.hover_asked = Some(key);
                    types.hover = None;
                    types.hover_ticket =
                        types.ask(&doc.path, Some(&doc.text), Ask::Hover { line, col });
                }
            }
            let hover = types
                .hover
                .as_ref()
                .filter(|(p, h)| {
                    *p == doc.path
                        && h.span.line == line
                        && word.is_some_and(|(a, _)| h.span.col == a)
                })
                .map(|(_, h)| h);
            if hover.is_some() || !here.is_empty() {
                let _ = out.response.response.clone().on_hover_ui_at_pointer(|ui| {
                    type_tip(ui, hover, &here.iter().collect::<Vec<_>>())
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
            let click = ui.interact(gutter, id.with("gutter"), egui::Sense::click());
            let line_at = |pos: Pos2| {
                lines
                    .iter()
                    .position(|r| r.top() <= pos.y && pos.y < r.bottom())
            };
            let test_at = |pos: Option<Pos2>| {
                let line = pos.and_then(line_at)? as u32 + 1;
                test_marks.iter().find(|(t, _)| t.line == line)
            };
            let click = match heard_tip(&heard, &tab.drawn_at, click.hover_pos()) {
                _ if test_at(click.hover_pos()).is_some() => {
                    let (t, status) = test_at(click.hover_pos()).expect("just found");
                    let what = if t.describe {
                        "these tests"
                    } else {
                        "this test"
                    };
                    click.on_hover_text(match status.as_deref() {
                        Some("passed") => format!("Passed. Click: run {what} again"),
                        Some("failed") => format!("Failed. Click: run {what} again"),
                        _ => format!("Click: run {what} in the game"),
                    })
                }
                Some(from) => click.on_hover_ui(|ui| {
                    ui.label(RichText::new("Called by signals").strong());
                    for f in from {
                        ui.label(RichText::new(f).monospace());
                    }
                }),
                None => click.on_hover_text(
                    "Click: a breakpoint (the game waits there). Shift+click: the line's \
                         values, live. Right-click: a breakpoint with a condition.",
                ),
            };
            if let Some(line) = click.interact_pointer_pos().and_then(line_at) {
                if click.clicked() {
                    if let Some((t, _)) = test_at(click.interact_pointer_pos()) {
                        run_test = Some(t.line);
                    } else if ui.input(|i| i.modifiers.shift) {
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
        if out.response.clicked() && ui.input(|i| i.modifiers.command) {
            ask_definition = true;
        }
        output = Some((
            out.response.changed(),
            caret,
            out.galley_pos,
            out.galley.clone(),
        ));
    });
    if let Some(line) = run_test {
        tab.tests.run(json!({"file": doc.path, "line": line}));
    }
    let Some((changed, caret, galley_pos, galley)) = output else {
        tab.vocab = Some(vocab);
        return;
    };
    let moved = caret != doc.caret;
    doc.caret = caret;

    // Completion: opened by typing a word, `.`, `:` or a component's quote; Ctrl+Space. After
    // `.`/`:` on a value completion's word list doesn't know (`self.props.`, `target:`), the
    // checker's members, typed.
    let typed = changed && moved;
    if typed || tab.complete_now {
        let explicit = std::mem::take(&mut tab.complete_now);
        tab.completion = complete(&doc.text, caret, &vocab, explicit);
        if let Some(start) = typed_member_at(&doc.text, caret, &vocab) {
            match &tab.types.typed {
                Some((p, s, list)) if *p == doc.path && *s == start => {
                    let prefix: String = doc.text.chars().skip(start).take(caret - start).collect();
                    let items = typed_items(list, &receiver_text(&doc.text, start), &prefix);
                    tab.completion = (!items.is_empty()).then_some(Completion {
                        start,
                        prefix,
                        items,
                        selected: 0,
                    });
                }
                _ => {
                    let (line, col) = line_col(&doc.text, caret);
                    tab.types.complete_for = Some((doc.path.clone(), start));
                    tab.types.complete_ticket =
                        tab.types
                            .ask(&doc.path, Some(&doc.text), Ask::Complete { line, col });
                }
            }
        }
    } else if moved {
        tab.completion = None;
    }
    if std::mem::take(&mut tab.types.typed_fresh)
        && let Some((p, start, list)) = &tab.types.typed
        && *p == doc.path
        && typed_member_at(&doc.text, caret, &vocab) == Some(*start)
    {
        let prefix: String = doc.text.chars().skip(*start).take(caret - start).collect();
        let items = typed_items(list, &receiver_text(&doc.text, *start), &prefix);
        if !items.is_empty() {
            tab.completion = Some(Completion {
                start: *start,
                prefix,
                items,
                selected: 0,
            });
        }
    }
    // The call the caret is in: its parameters over the line.
    if changed || moved {
        if in_call(&doc.text, caret) {
            let (line, col) = line_col(&doc.text, caret);
            let key = (doc.path.clone(), line, col);
            if tab.types.signature_asked.as_ref() != Some(&key) {
                tab.types.signature_asked = Some(key);
                tab.types.signature_ticket =
                    tab.types
                        .ask(&doc.path, Some(&doc.text), Ask::Signature { line, col });
            }
        } else {
            tab.types.signature = None;
            tab.types.signature_asked = None;
        }
    }
    if ui.memory(|m| m.has_focus(id))
        && let Some((p, line, s)) = &tab.types.signature
        && *p == doc.path
        && *line as usize == line_of(&doc.text, caret)
        && in_call(&doc.text, caret)
    {
        let at = galley.pos_from_cursor(CCursor::new(caret));
        let pos = galley_pos + at.left_top().to_vec2() + Vec2::new(-6.0, -3.0);
        signature_popup(ui, s, pos);
    }
    if ask_definition {
        let (line, col) = line_col(&doc.text, caret);
        tab.types.definition_ticket =
            tab.types
                .ask(&doc.path, Some(&doc.text), Ask::Definition { line, col });
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

/// The methods of `path` that signals in the scene call: (1-based line, who calls it), for
/// Godot's connection mark in the margin.
fn heard_marks(world: &mut World, path: &str) -> Vec<(u32, Vec<String>)> {
    let path = engine_script::normalize(path);
    let objects = scripted_objects(world);
    let mut calls: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut q = world.query_filtered::<(Entity, &Name, &Script), With<crate::scene_ops::InScene>>();
    for (e, n, s) in q.iter(world) {
        for c in &s.connections {
            let runs = target_script(&objects, e, &c.target)
                .is_some_and(|f| engine_script::normalize(&f) == path);
            if runs && !c.method.is_empty() {
                let to = if c.target.is_empty() {
                    n.as_str()
                } else {
                    c.target.as_str()
                };
                calls.entry(c.method.clone()).or_default().push(format!(
                    "{} ⚡{} (on {to})",
                    n.as_str(),
                    c.signal
                ));
            }
        }
    }
    if calls.is_empty() {
        return Vec::new();
    }
    let methods = engine_script::props::declared(world, &path)
        .map(|d| d.methods)
        .unwrap_or_default();
    calls
        .into_iter()
        .filter_map(|(m, from)| {
            let line = methods.iter().find(|x| x.name == m)?.line;
            (line > 0).then_some((line, from))
        })
        .collect()
}

/// A small plug in the margin of each method a signal calls; hovering it says whose.
fn paint_heard(
    ui: &egui::Ui,
    area: &Rect,
    lines: &[Rect],
    heard: &[(u32, Vec<String>)],
    row_h: f32,
    drawn_at: &mut HashMap<String, Rect>,
) {
    for (line, _) in heard {
        let Some(r) = lines.get(*line as usize - 1) else {
            continue;
        };
        if r.bottom() < ui.clip_rect().top() || r.top() > ui.clip_rect().bottom() {
            continue;
        }
        let c = Pos2::new(area.left() + 10.0, r.center().y);
        let h = row_h * 0.3;
        let painter = ui.painter();
        // A lightning bolt (Godot shows a connection icon there).
        painter.add(egui::Shape::convex_polygon(
            vec![
                c + Vec2::new(h * 0.25, -h * 1.2),
                c + Vec2::new(-h * 0.6, h * 0.15),
                c + Vec2::new(-h * 0.05, h * 0.15),
            ],
            SIGNAL,
            Stroke::NONE,
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![
                c + Vec2::new(0.05, -h * 0.15),
                c + Vec2::new(h * 0.6, -h * 0.15),
                c + Vec2::new(-h * 0.25, h * 1.2),
            ],
            SIGNAL,
            Stroke::NONE,
        ));
        // Its tooltip comes with the gutter's (`heard_tip`), which is on top of it.
        drawn_at.insert(
            format!("heard {line}"),
            Rect::from_center_size(c, Vec2::splat(row_h)),
        );
    }
}

/// The heard plug under the pointer, if any: who calls that method.
fn heard_tip<'h>(
    heard: &'h [(u32, Vec<String>)],
    drawn_at: &HashMap<String, Rect>,
    pointer: Option<Pos2>,
) -> Option<&'h [String]> {
    let p = pointer?;
    heard
        .iter()
        .find(|(line, _)| {
            drawn_at
                .get(&format!("heard {line}"))
                .is_some_and(|r| r.contains(p))
        })
        .map(|(_, from)| from.as_slice())
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
#[allow(clippy::too_many_arguments)]
fn paint_notes(
    ui: &egui::Ui,
    lines: &[Rect],
    doc: &Doc,
    lints: &[Lint],
    types: &[&Problem],
    tests: &[(u32, String, Color32)],
    font: &FontId,
    stop: Option<&FileStop>,
) {
    let painter = ui.painter();
    let small = FontId::monospace(font.size * 0.92);
    let mut notes: BTreeMap<usize, Vec<(String, Color32)>> = BTreeMap::new();
    for (line, text, color) in tests {
        notes
            .entry((*line).max(1) as usize - 1)
            .or_default()
            .push((text.clone(), *color));
    }
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
    // The checker's: the first on each line, errors before warnings.
    for p in types {
        let line = notes.entry(p.span.line as usize).or_default();
        if !line.iter().any(|(_, c)| *c == ERROR || *c == LINT) {
            let (icon, color) = if p.error {
                ("✖", ERROR)
            } else {
                ("⚠", LINT)
            };
            line.push((format!("{icon} {}", p.message), color));
        }
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
                // The checker's verdict on this file.
                let types = &tab.types;
                if let Some(e) = &types.failed {
                    ui.label(theme::dim("·  types off"))
                        .on_hover_text(format!("The type checker didn't start: {e}"));
                } else if let Some(r) = types.reports.get(&doc.path) {
                    let (e, w) = (r.errors(), r.problems.len() - r.errors());
                    let (text, color) = if e + w == 0 {
                        ("·  ✔ types".to_string(), theme::PLAY)
                    } else {
                        (format!("·  types ✖ {e} ⚠ {w}"), if e > 0 { ERROR } else { LINT })
                    };
                    let tip = format!(
                        "Luau's type checker: {e} error{}, {w} warning{} ({:.1} ms). Click: the first.",
                        if e == 1 { "" } else { "s" },
                        if w == 1 { "" } else { "s" },
                        r.micros as f64 / 1000.0
                    );
                    let first = r.problems.first().map(|p| (p.span.line, p.span.col));
                    let label = ui
                        .add(
                            egui::Label::new(RichText::new(text).color(color))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text(tip);
                    if label.clicked()
                        && let Some((l, c)) = first
                    {
                        doc.jump = Some(char_at(&doc.text, l, c));
                    }
                }
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
            let scope = "ScriptBase";
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
                        ui.vertical(|ui| completion_rows(ui, c, &mut clicked));
                    });
                if let Some(it) = c.items.get(c.selected) {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.vertical(|ui| completion_doc(ui, it));
                    });
                }
            });
        });
    clicked
}

/// The completion list's rows (ten around the chosen one).
fn completion_rows(ui: &mut egui::Ui, c: &Completion, clicked: &mut Option<usize>) {
    let first = c.selected.saturating_sub(8);
    for (n, it) in c.items.iter().enumerate().skip(first).take(10) {
        let (color, tag) = kind_color(it.kind);
        let mut job = LayoutJob::default();
        let mono = FontId::monospace(12.5);
        job.append(
            &format!("{tag:>2}  "),
            0.0,
            TextFormat::simple(FontId::monospace(11.0), color.gamma_multiply(0.8)),
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
            job.append(tail, 0.0, TextFormat::simple(mono.clone(), theme::TEXT));
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
        let r =
            ui.add(egui::Button::selectable(n == c.selected, job).min_size(Vec2::new(294.0, 0.0)));
        if r.clicked() {
            *clicked = Some(n);
        }
    }
    if c.items.len() > 10 {
        ui.label(theme::dim(format!("{} more", c.items.len() - 10)).small());
    }
}

/// The chosen completion's signature and docs.
fn completion_doc(ui: &mut egui::Ui, it: &Item) {
    let sig = match it.kind {
        WordKind::Function | WordKind::Method | WordKind::Global if it.detail.starts_with('(') => {
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
}

// ----------------------------------------------------------------------------- types

/// Luau's type checker on the scripts (`engine_script::analysis`, on a thread of its own): a
/// report per file, and its answers to what the code view asks (Roblox Studio's script
/// analysis, Godot's typed GDScript: problems as you type, types under the pointer).
#[derive(Default)]
pub struct Types {
    checker: Option<Checker>,
    /// Why there is no checker (the engine's definitions didn't load).
    failed: Option<String>,
    ticket: u64,
    /// The last report for each file.
    pub reports: HashMap<String, Report>,
    /// The text last sent for each open file; the other files' times on disk.
    sent: HashMap<String, String>,
    disk: HashMap<String, Option<SystemTime>>,
    disk_next: f64,
    /// The word under the pointer: asked for at (path, line, word start), and the answer.
    hover_asked: Option<(String, u32, u32)>,
    hover_ticket: u64,
    hover: Option<(String, Hover)>,
    /// Members asked for at (path, char index where the word starts); the last list got.
    complete_ticket: u64,
    complete_for: Option<(String, usize)>,
    typed: Option<(String, usize, Vec<Suggestion>)>,
    /// A new member list came in: show it.
    typed_fresh: bool,
    /// The call the caret is in: asked for at (path, line, col), and the answer.
    signature_asked: Option<(String, u32, u32)>,
    signature_ticket: u64,
    signature: Option<(String, u32, Signature)>,
    /// Go to definition: asked, and where it is.
    definition_ticket: u64,
    goto: Option<(String, u32, u32)>,
    /// The bottom panel shows the problems instead of the output.
    pub show_problems: bool,
    /// The Graphs tab's open graphs: the Luau each was last checked as.
    graph_sent: HashMap<String, String>,
}

impl Types {
    fn next(&mut self) -> u64 {
        self.ticket += 1;
        self.ticket
    }

    fn ask(&mut self, path: &str, text: Option<&str>, ask: Ask) -> u64 {
        let ticket = self.next();
        if let Some(c) = &self.checker {
            c.send(Job {
                ticket,
                path: path.to_string(),
                text: text.map(str::to_string),
                ask,
            });
        }
        ticket
    }

    /// The current problems of `path` (none while it has a syntax error: those come first).
    fn problems(&self, path: &str) -> &[Problem] {
        self.reports
            .get(path)
            .map_or(&[], |r| r.problems.as_slice())
    }

    /// Errors and warnings over every file.
    fn counts(&self) -> (usize, usize) {
        let all = self.reports.values().flat_map(|r| &r.problems);
        let (e, w): (Vec<&Problem>, Vec<&Problem>) = all.partition(|p| p.error);
        (e.len(), w.len())
    }
}

/// Keeps the checker fed: open files as typed, the others when they change on disk; takes its
/// answers.
fn types_poll(world: &mut World, tab: &mut ScriptsTab, now: f64) {
    let t = &mut tab.types;
    if t.checker.is_none() && t.failed.is_none() {
        t.checker = Some(Checker::for_world(world));
    }
    if t.checker.is_none() {
        return;
    }
    for doc in &tab.docs {
        if t.sent.get(&doc.path) != Some(&doc.text) {
            t.sent.insert(doc.path.clone(), doc.text.clone());
            t.ask(&doc.path, Some(&doc.text), Ask::Check);
        }
    }
    let closed: Vec<String> = t
        .sent
        .keys()
        .filter(|p| !tab.docs.iter().any(|d| &d.path == *p))
        .cloned()
        .collect();
    for p in closed {
        t.sent.remove(&p);
        t.ask(&p, None, Ask::Closed);
    }
    if now >= t.disk_next && !tab.files.is_empty() {
        t.disk_next = now + FILES_SECS;
        let root = engine_script::scripts_root(world);
        for f in &tab.files {
            let stamp = modified(&root.join(&f.path));
            if t.disk.get(&f.path) == Some(&stamp) {
                continue;
            }
            let seen = t.disk.insert(f.path.clone(), stamp).is_some();
            if t.sent.contains_key(&f.path) {
                continue;
            }
            if seen {
                t.ask(&f.path, None, Ask::Touched);
            }
            t.ask(&f.path, None, Ask::Check);
        }
    }
    take_answers(t);
}

/// The Graphs tab's use of the checker: each open graph `(path, its file text, the Luau it
/// compiles to)` is checked when its Luau changes (not on every drag); the answers are taken
/// here too, as the Scripts tab may not be drawn. Returns `path`'s problems, each on the node
/// its line came from.
pub fn graph_types(
    world: &mut World,
    open: &[(String, String, String)],
    path: &str,
) -> Vec<Problem> {
    if !world.contains_resource::<ScriptsTab>() {
        return Vec::new();
    }
    world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
        let t = &mut tab.types;
        if t.checker.is_none() && t.failed.is_none() {
            t.checker = Some(Checker::for_world(world));
        }
        for (p, text, luau) in open {
            if t.graph_sent.get(p) != Some(luau) {
                t.graph_sent.insert(p.clone(), luau.clone());
                t.ask(p, Some(text.as_str()), Ask::Check);
            }
        }
        let closed: Vec<String> = t
            .graph_sent
            .keys()
            .filter(|p| !open.iter().any(|o| &o.0 == *p))
            .cloned()
            .collect();
        for p in closed {
            t.graph_sent.remove(&p);
            t.ask(&p, None, Ask::Closed);
        }
        take_answers(t);
        t.problems(path).to_vec()
    })
}

/// The checker's answers since last frame.
fn take_answers(t: &mut Types) {
    let answers = t.checker.as_ref().map(|c| c.answers()).unwrap_or_default();
    for a in answers {
        match a.reply {
            Reply::Report(r) => {
                t.reports.insert(a.path, r);
            }
            Reply::Hover(h) if a.ticket == t.hover_ticket => {
                t.hover = h.map(|h| (a.path, h));
            }
            Reply::Complete(list) if a.ticket == t.complete_ticket => {
                if let Some((path, start)) = t.complete_for.take() {
                    t.typed = Some((path, start, list));
                    t.typed_fresh = true;
                }
            }
            Reply::Signature(s) if a.ticket == t.signature_ticket => {
                let line = t.signature_asked.as_ref().map_or(0, |s| s.1);
                t.signature = s.map(|s| (a.path, line, s));
            }
            Reply::Definition(d) if a.ticket == t.definition_ticket => {
                t.goto = d;
            }
            Reply::Failed(e) => {
                t.failed = Some(e);
                t.checker = None;
            }
            _ => {}
        }
    }
}

/// The char index of a (0-based line, char column) in `text`.
fn char_at(text: &str, line: u32, col: u32) -> usize {
    let start = line_start_char(text, line as usize);
    let len = text
        .split('\n')
        .nth(line as usize)
        .map_or(0, |l| l.chars().count());
    start + (col as usize).min(len)
}

/// (0-based line, char column) of a char index.
fn line_col(text: &str, at: usize) -> (u32, u32) {
    let line = line_of(text, at);
    (line as u32, (at - line_start_char(text, line)) as u32)
}

/// The identifier around a char index: (start, end) char columns on its line.
fn word_cols(text: &str, line: u32, col: u32) -> Option<(u32, u32)> {
    let chars: Vec<char> = text.split('\n').nth(line as usize)?.chars().collect();
    let w = |c: char| c.is_alphanumeric() || c == '_';
    let mut a = (col as usize).min(chars.len());
    if a == chars.len() || !w(chars[a]) {
        return None;
    }
    while a > 0 && w(chars[a - 1]) {
        a -= 1;
    }
    let mut b = col as usize;
    while b < chars.len() && w(chars[b]) {
        b += 1;
    }
    (!chars[a].is_ascii_digit()).then_some((a as u32, b as u32))
}

/// Whether the caret is inside a call's parentheses on its line (a quick look before asking
/// the checker for the call's parameters).
fn in_call(text: &str, caret: usize) -> bool {
    let (line, col) = line_col(text, caret);
    let Some(l) = text.split('\n').nth(line as usize) else {
        return false;
    };
    let before: String = l.chars().take(col as usize).collect();
    if in_comment_or_string(&before) {
        return false;
    }
    let mut depth = 0i32;
    for c in before.chars().rev() {
        match c {
            ')' => depth += 1,
            '(' if depth == 0 => return true,
            '(' => depth -= 1,
            _ => {}
        }
    }
    false
}

/// Where completion should ask the checker: a member after `.`/`:` on something completion's
/// word list doesn't know (`self.`, `p.pulse.`, `target:`). Returns the word's start.
fn typed_member_at(text: &str, caret: usize, vocab: &Vocab) -> Option<usize> {
    let before: Vec<char> = text.chars().take(caret).collect();
    let mut start = before.len();
    while start > 0 && (before[start - 1].is_alphanumeric() || before[start - 1] == '_') {
        start -= 1;
    }
    let sep = *before.get(start.checked_sub(1)?)?;
    if sep != '.' && sep != ':' {
        return None;
    }
    let line_start = before[..start]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |k| k + 1);
    let line: String = before[line_start..start - 1].iter().collect();
    if in_comment_or_string(&line) || (sep == ':' && line.trim_start().starts_with("function ")) {
        return None;
    }
    let mut r = start - 1;
    while r > 0 && (before[r - 1].is_alphanumeric() || before[r - 1] == '_' || before[r - 1] == '.')
    {
        r -= 1;
    }
    let recv: String = before[r..start - 1].iter().collect();
    if recv.is_empty() || recv.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let known = vocab.words.iter().any(|w| w.scope == recv);
    (!known).then_some(start)
}

/// The checker's members as completion rows, filtered by what is typed.
fn typed_items(list: &[Suggestion], recv_sep: &str, prefix: &str) -> Vec<Item> {
    let p = prefix.to_lowercase();
    let mut out: Vec<(u8, Item)> = list
        .iter()
        .filter_map(|s| {
            let l = s.name.to_lowercase();
            let rank = if l.starts_with(&p) {
                0
            } else if p.len() >= 2 && l.contains(&p) {
                1
            } else {
                return None;
            };
            let kind = match s.kind {
                SuggestKind::Field | SuggestKind::Local => WordKind::Field,
                SuggestKind::Method => WordKind::Method,
                SuggestKind::Function => WordKind::Function,
                SuggestKind::Keyword => WordKind::Keyword,
                SuggestKind::Type => WordKind::Global,
                SuggestKind::Module => WordKind::Namespace,
            };
            Some((
                rank,
                Item {
                    label: s.name.clone(),
                    insert: s.name.clone(),
                    kind,
                    full: format!("{recv_sep}{}", s.name),
                    detail: s.ty.clone(),
                    doc: s.doc.clone(),
                },
            ))
        })
        .filter(|(_, it)| it.label != prefix)
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.cmp(&b.1.label)));
    out.into_iter().map(|(_, it)| it).take(60).collect()
}

/// The receiver and separator before a member being typed: `self.props.` for `self.props.lu`.
fn receiver_text(text: &str, start: usize) -> String {
    let before: Vec<char> = text.chars().take(start).collect();
    let mut r = before.len().saturating_sub(1);
    while r > 0 && (before[r - 1].is_alphanumeric() || before[r - 1] == '_' || before[r - 1] == '.')
    {
        r -= 1;
    }
    before[r..].iter().collect()
}

/// The type problems to draw for a file: none while it has a syntax error (that one is shown
/// alone), and none where completion's own check already says the same.
fn shown_problems<'a>(doc: &Doc, types: &'a Types, lints: &[Lint]) -> Vec<&'a Problem> {
    if doc.syntax.is_some() {
        return Vec::new();
    }
    types
        .problems(&doc.path)
        .iter()
        .filter(|p| {
            !lints
                .iter()
                .any(|l| l.line as u32 == p.span.line && p.message.contains("not found"))
        })
        .collect()
}

/// Squiggles under the checker's problems (red errors, amber warnings), where the text is.
fn paint_type_marks(
    ui: &egui::Ui,
    galley: &egui::Galley,
    origin: Pos2,
    text: &str,
    problems: &[&Problem],
) {
    let painter = ui.painter();
    let clip = ui.clip_rect();
    for p in problems {
        let a = char_at(text, p.span.line, p.span.col);
        let b = if p.span.end_line == p.span.line && p.span.end_col > p.span.col {
            char_at(text, p.span.line, p.span.end_col)
        } else {
            char_at(text, p.span.line, u32::MAX)
        };
        let ra = galley.pos_from_cursor(CCursor::new(a));
        let rb = galley.pos_from_cursor(CCursor::new(b.max(a + 1)));
        let y = origin.y + ra.max.y - 1.0;
        if y < clip.top() || y > clip.bottom() + 4.0 {
            continue;
        }
        let x0 = origin.x + ra.min.x;
        let x1 = (origin.x + rb.min.x).max(x0 + 6.0);
        let color = if p.error { ERROR } else { LINT };
        let mut points = Vec::new();
        let mut x = x0;
        let mut up = true;
        while x < x1 {
            points.push(Pos2::new(x, if up { y - 2.0 } else { y }));
            x += 2.5;
            up = !up;
        }
        points.push(Pos2::new(x1, if up { y - 2.0 } else { y }));
        painter.add(egui::Shape::line(points, Stroke::new(1.2, color)));
    }
}

/// The tooltip over code: the type of the name and its docs (from the checker), and any
/// problem there.
fn type_tip(ui: &mut egui::Ui, hover: Option<&Hover>, problems: &[&Problem]) {
    ui.set_max_width(560.0);
    if let Some(h) = hover {
        let (name, ty) = h.text.split_once(": ").unwrap_or((&h.text, ""));
        let mono = FontId::monospace(12.5);
        let mut job = LayoutJob::default();
        job.append(name, 0.0, TextFormat::simple(mono.clone(), Color32::WHITE));
        if !ty.is_empty() {
            job.append(": ", 0.0, TextFormat::simple(mono.clone(), theme::TEXT_DIM));
            job.append(ty, 0.0, TextFormat::simple(mono, CALL));
        }
        job.wrap.max_width = 540.0;
        ui.label(job);
        if !h.doc.is_empty() {
            ui.add_space(2.0);
            ui.label(&h.doc);
        }
    }
    for p in problems {
        if hover.is_some() {
            ui.separator();
        }
        let (icon, color) = if p.error {
            ("✖", ERROR)
        } else {
            ("⚠", LINT)
        };
        ui.label(RichText::new(format!("{icon} {}", p.message)).color(color));
        ui.label(theme::dim(p.code.as_str()).small());
    }
}

/// The parameters of the call the caret is in, over the caret's line.
fn signature_popup(ui: &egui::Ui, s: &Signature, pos: Pos2) {
    egui::Area::new(egui::Id::new("script signature"))
        .order(egui::Order::Foreground)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_max_width(620.0);
                    let mono = FontId::monospace(12.5);
                    let plain = TextFormat::simple(mono.clone(), theme::TEXT);
                    let mut job = LayoutJob::default();
                    job.append(&s.name, 0.0, TextFormat::simple(mono.clone(), CALL));
                    job.append("(", 0.0, plain.clone());
                    for (k, p) in s.params.iter().enumerate() {
                        if k > 0 {
                            job.append(", ", 0.0, plain.clone());
                        }
                        if k == s.active {
                            let mut f = TextFormat::simple(mono.clone(), Color32::WHITE);
                            f.underline = Stroke::new(1.5, theme::ACCENT);
                            f.background = theme::ACCENT.gamma_multiply(0.18);
                            job.append(p, 0.0, f);
                        } else {
                            job.append(p, 0.0, TextFormat::simple(mono.clone(), theme::TEXT_DIM));
                        }
                    }
                    job.append(")", 0.0, plain.clone());
                    if !s.ret.is_empty() && s.ret != "()" {
                        job.append(": ", 0.0, plain);
                        job.append(&s.ret, 0.0, TextFormat::simple(mono, ENGINE));
                    }
                    ui.label(job);
                    if !s.doc.is_empty() {
                        ui.label(theme::dim(s.doc.as_str()).small());
                    }
                });
        });
}

/// The problems list (Roblox Studio's Script Analysis, Unreal's Compiler Results): every
/// file's type errors and warnings; a click opens the line.
fn problems_list(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab) {
    let mut open_at = None;
    egui::ScrollArea::vertical()
        .id_salt("script problems")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if let Some(e) = &tab.types.failed {
                ui.label(RichText::new(format!("The type checker didn't start: {e}")).color(ERROR));
                return;
            }
            let mut files: Vec<&Report> = tab
                .types
                .reports
                .values()
                .filter(|r| !r.problems.is_empty())
                .collect();
            files.sort_by(|a, b| b.errors().cmp(&a.errors()).then(a.path.cmp(&b.path)));
            if files.is_empty() {
                ui.label(theme::dim(if tab.types.reports.is_empty() {
                    "Checking the scripts…".to_string()
                } else {
                    format!(
                        "✔ {} scripts, no problems: Luau's type checker found nothing wrong.",
                        tab.types.reports.len()
                    )
                }));
            }
            for r in files {
                ui.label(
                    RichText::new(format!(
                        "{}   {} error{}, {} warning{}",
                        r.path,
                        r.errors(),
                        if r.errors() == 1 { "" } else { "s" },
                        r.problems.len() - r.errors(),
                        if r.problems.len() - r.errors() == 1 {
                            ""
                        } else {
                            "s"
                        },
                    ))
                    .small()
                    .strong()
                    .color(theme::TEXT_DIM),
                );
                for p in &r.problems {
                    let (icon, color) = if p.error {
                        ("✖", ERROR)
                    } else {
                        ("⚠", LINT)
                    };
                    let place = match p.node {
                        Some(n) => format!("node {n}"),
                        None => format!("{}:{}", p.span.line + 1, p.span.col + 1),
                    };
                    let mut job = LayoutJob::default();
                    let mono = FontId::monospace(12.0);
                    job.append(
                        &format!("  {icon} "),
                        0.0,
                        TextFormat::simple(mono.clone(), color),
                    );
                    job.append(
                        &format!("{place:<8} "),
                        0.0,
                        TextFormat::simple(mono.clone(), theme::TEXT_DIM),
                    );
                    job.append(&p.message, 0.0, TextFormat::simple(mono, theme::TEXT));
                    let row = ui
                        .add(egui::Label::new(job).truncate().sense(egui::Sense::click()))
                        .on_hover_text(format!("{} ({}): {}", place, p.code, p.message));
                    if row.clicked() {
                        open_at = Some((r.path.clone(), p.span.line, p.span.col));
                    }
                }
            }
        });
    if let Some((path, line, col)) = open_at {
        if path.ends_with(".graph") {
            open_in_tab(world, &path, Some(line + 1));
        } else {
            open(world, tab, &path, Some(line + 1));
            let doc = &mut tab.docs[tab.current];
            doc.jump = Some(char_at(&doc.text, line, col));
        }
    }
}

// ----------------------------------------------------------------------------- tests

/// The Tests panel, as Unity's Test Runner and VS Code's test explorer: runs of the script
/// tests (`*.test.luau`, [`engine_script::testing`]) and what they found. A run needs the game
/// running, so Run in edit mode presses Play first and Stop after, as Unity's play mode tests
/// do.
#[derive(Default)]
pub struct TestsPanel {
    /// The bottom panel shows the tests (not the output or the problems).
    pub shown: bool,
    /// A run to start (`script.test`'s arguments) once the scripts run.
    pub pending: Option<Value>,
    /// Play was pressed for this run: Stop when it is over.
    started_play: bool,
    /// Frames the pending run has waited for the game to start.
    waited: u32,
    /// A run is going: ask how it does every frame.
    pub running: bool,
    /// The last `script.tests` answer (kept after Stop drops the VM that ran it).
    pub last: Value,
}

impl TestsPanel {
    /// The last run's results for `file`.
    fn results<'a>(&'a self, file: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.last["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(move |r| r["file"] == file)
    }

    /// (passed, failed) in `file`'s last run, when it ran.
    fn counts(&self, file: &str) -> Option<(usize, usize)> {
        let mut any = false;
        let (mut passed, mut failed) = (0, 0);
        for r in self.results(file) {
            any = true;
            match r["status"].as_str() {
                Some("passed") => passed += 1,
                Some("failed") => failed += 1,
                _ => {}
            }
        }
        let broken = self.last["broken"]
            .as_array()
            .is_some_and(|b| b.iter().any(|b| b["file"] == file));
        (any || broken).then_some((passed, failed + usize::from(broken)))
    }

    /// Asks for a run (it starts once the game runs).
    pub fn run(&mut self, args: Value) {
        self.pending = Some(args);
        self.waited = 0;
        self.shown = true;
    }
}

/// A `describe(` or `it(` line of a test file.
#[derive(Debug, Clone, PartialEq)]
struct TestLine {
    /// 1-based.
    line: u32,
    describe: bool,
    /// For a describe, its `end)` line: the tests between are in it.
    end: u32,
}

/// The test file's `describe` and `it` lines (`.skip` and `.only` too).
fn test_lines(text: &str) -> Vec<TestLine> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let code = l.trim_start();
        let describe = ["describe(", "describe.skip(", "describe.only("]
            .iter()
            .any(|p| code.starts_with(p));
        let it = ["it(", "it.skip(", "it.only("]
            .iter()
            .any(|p| code.starts_with(p));
        if !describe && !it {
            continue;
        }
        let end = if describe {
            lines[i + 1..]
                .iter()
                .position(|e| indent(e) == indent(l) && e.trim_start().starts_with("end"))
                .map_or(lines.len(), |k| i + 1 + k)
        } else {
            i
        };
        out.push(TestLine {
            line: i as u32 + 1,
            describe,
            end: end as u32 + 1,
        });
    }
    out
}

/// What a test line's mark shows: its test's status, or for a describe the worst of its tests'.
fn line_status(tests: &TestsPanel, file: &str, t: &TestLine) -> Option<String> {
    let statuses: Vec<&str> = tests
        .results(file)
        .filter(|r| {
            let line = r["line"].as_u64().unwrap_or(0) as u32;
            if t.describe {
                line > t.line && line < t.end
            } else {
                line == t.line
            }
        })
        .filter_map(|r| r["status"].as_str())
        .collect();
    [
        "running", "failed", "queued", "stopped", "passed", "skipped",
    ]
    .into_iter()
    .find(|s| statuses.contains(s))
    .map(str::to_string)
}

/// Starts a waiting run once the game runs (pressing Play for it in edit mode), follows the run
/// while it goes, and stops the game after it if it pressed Play.
fn tests_poll(world: &mut World, editor: &mut Editor, tab: &mut ScriptsTab, now: f64) {
    if let Some(args) = tab.tests.pending.clone() {
        if editor.mode != Mode::Play {
            // Saved first: the run reads the files.
            for i in 0..tab.docs.len() {
                if tab.docs[i].dirty() {
                    let current = std::mem::replace(&mut tab.current, i);
                    save(world, tab, now);
                    tab.current = current;
                }
            }
            tab.tests.started_play |= editor.mode == Mode::Edit;
            editor::play(world, editor);
            tab.tests.waited = 0;
            return;
        }
        tab.tests.waited += 1;
        // A couple of frames for the scene's scripts to start.
        if tab.tests.waited < 3 {
            return;
        }
        tab.tests.pending = None;
        match call_api(world, "script.test", args) {
            Ok(_) => {
                tab.tests.running = true;
                tab.tests.shown = true;
                tab.types.show_problems = false;
            }
            Err(e) => {
                tab.toast = Some((e, now + 5.0, true));
                if std::mem::take(&mut tab.tests.started_play) {
                    editor::stop(world, editor);
                }
            }
        }
    }
    if !tab.tests.running {
        return;
    }
    let Ok(v) = call_api(world, "script.tests", json!({})) else {
        return;
    };
    if v["run"] == 0 {
        // The game stopped under it (a new VM never ran it): keep what it got to.
        tab.tests.running = false;
        if let Some(results) = tab.tests.last["results"].as_array_mut() {
            for r in results {
                if r["status"] == "running" || r["status"] == "queued" {
                    r["status"] = json!("stopped");
                }
            }
        }
        tab.tests.last["state"] = json!("done");
        return;
    }
    let done = v["state"] != "running";
    tab.tests.last = v;
    if done {
        tab.tests.running = false;
        let l = &tab.tests.last;
        let (passed, failed) = (
            l["passed"].as_u64().unwrap_or(0),
            l["failed"].as_u64().unwrap_or(0),
        );
        let summary = format!(
            "Tests: {passed} passed, {failed} failed ({:.1} s of game time)",
            l["secs"].as_f64().unwrap_or(0.0)
        );
        let kind = if failed > 0 {
            OutKind::Error
        } else {
            OutKind::Note
        };
        push(tab, kind, summary, None);
        if std::mem::take(&mut tab.tests.started_play) {
            editor::stop(world, editor);
        }
    }
}

/// The Tests panel: Run all, the file's tests, Stop; each test with its status and time, and
/// under a failed one why, with the line (click to open it).
fn tests_list(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab) {
    let busy = tab.tests.running || tab.tests.pending.is_some();
    let current = tab
        .docs
        .get(tab.current)
        .map(|d| d.path.clone())
        .filter(|p| engine_script::testing::is_test_file(p));
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("▶ Run all"))
            .on_hover_text(
                "Run every test file (*.test.luau) in the game; in edit mode this presses Play \
                 for the run and Stop after it",
            )
            .clicked()
        {
            tab.tests.run(json!({}));
        }
        if let Some(path) = &current
            && ui
                .add_enabled(!busy, egui::Button::new("▶ This file"))
                .on_hover_text(format!("Run {path}"))
                .clicked()
        {
            tab.tests.run(json!({"files": [path]}));
        }
        if ui
            .add_enabled(tab.tests.running, egui::Button::new("⏹ Stop"))
            .clicked()
        {
            let _ = call_api(world, "script.test", json!({"stop": true}));
        }
        let l = &tab.tests.last;
        if !l.is_null() {
            let n = |k: &str| l[k].as_u64().unwrap_or(0);
            let state = if tab.tests.running {
                format!("running… {:.1} s", l["secs"].as_f64().unwrap_or(0.0))
            } else {
                format!("{:.1} s of game time", l["secs"].as_f64().unwrap_or(0.0))
            };
            ui.add_space(10.0);
            ui.label(RichText::new(format!("✔ {} passed", n("passed"))).color(theme::PLAY));
            let failed = n("failed");
            ui.label(
                RichText::new(format!("✖ {failed} failed")).color(if failed > 0 {
                    ERROR
                } else {
                    theme::TEXT_DIM
                }),
            );
            if n("skipped") > 0 {
                ui.label(theme::dim(format!("{} skipped", n("skipped"))));
            }
            ui.label(theme::dim(&state));
        }
    });
    let mut open_at = None;
    // A failed test's run, if the Rewind tab recorded it.
    let recorded = world
        .get_resource::<engine_script::ScriptRewind>()
        .and_then(|r| r.span());
    let mut rewind_to = None;
    egui::ScrollArea::vertical()
        .id_salt("script tests")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let l = &tab.tests.last;
            if l.is_null() {
                ui.label(theme::dim(
                    "Tests are Luau files named name.test.luau: describe groups, it tests, \
                     expect checks (examples/lamp.test.luau). Each test runs in the game, so it \
                     can spawn things, wait, press E and check what the scripts did. Run all \
                     runs them; the ▶ beside a test in its file runs that one.",
                ));
                return;
            }
            let mono = FontId::monospace(12.0);
            for b in l["broken"].as_array().into_iter().flatten() {
                let file = b["file"].as_str().unwrap_or("");
                let line = b["line"].as_u64().map(|n| n as u32);
                let r = ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "✖ {file} didn't load: {}",
                            b["message"].as_str().unwrap_or("")
                        ))
                        .color(ERROR),
                    )
                    .truncate()
                    .sense(egui::Sense::click()),
                );
                if r.clicked() {
                    open_at = Some((file.to_string(), line.unwrap_or(1)));
                }
            }
            let mut file = "";
            for r in l["results"].as_array().into_iter().flatten() {
                let this = r["file"].as_str().unwrap_or("");
                if this != file {
                    file = this;
                    ui.add_space(3.0);
                    ui.label(RichText::new(file).small().strong().color(theme::TEXT_DIM));
                }
                let status = r["status"].as_str().unwrap_or("");
                let (icon, color) = match status {
                    "passed" => ("✔", theme::PLAY),
                    "failed" => ("✖", ERROR),
                    "running" => ("⏳", VALUE),
                    "skipped" => ("○", LINE_NO),
                    "stopped" => ("⏹", LINT),
                    _ => ("○", theme::TEXT_DIM),
                };
                let mut job = LayoutJob::default();
                job.append(
                    &format!("  {icon}  "),
                    0.0,
                    TextFormat::simple(FontId::proportional(13.0), color),
                );
                job.append(
                    r["title"].as_str().unwrap_or(""),
                    0.0,
                    TextFormat::simple(
                        FontId::proportional(13.0),
                        if status == "skipped" {
                            theme::TEXT_DIM
                        } else {
                            theme::TEXT
                        },
                    ),
                );
                if matches!(status, "passed" | "failed") {
                    job.append(
                        &format!("   {:.2} s", r["secs"].as_f64().unwrap_or(0.0)),
                        0.0,
                        TextFormat::simple(mono.clone(), LINE_NO),
                    );
                }
                let line = r["line"].as_u64().unwrap_or(1) as u32;
                let row = ui
                    .add(egui::Label::new(job).truncate().sense(egui::Sense::click()))
                    .on_hover_text(format!("{file}:{line}"));
                if row.clicked() {
                    open_at = Some((file.to_string(), line));
                }
                if let Some(message) = r["message"].as_str() {
                    let at = r["failed_at"].as_u64().map(|n| n as u32).unwrap_or(line);
                    let text = format!("        {message}   ({}:{at})", file);
                    let why = ui
                        .add(
                            egui::Label::new(RichText::new(text).font(mono.clone()).color(ERROR))
                                .truncate()
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text(message);
                    if why.clicked() {
                        open_at = Some((file.to_string(), at));
                    }
                    let start = r["at"].as_f64().unwrap_or(0.0);
                    let end = start + r["secs"].as_f64().unwrap_or(0.0);
                    if status == "failed"
                        && recorded.is_some_and(|(a, b)| start >= a - 0.01 && end <= b + 0.01)
                    {
                        let rewind = ui
                            .horizontal(|ui| {
                                ui.add_space(30.0);
                                ui.small_button("⏪ Rewind to it").on_hover_text(
                                    "See what the scripts did in this test, at the moment it \
                                     failed (the Rewind tab)",
                                )
                            })
                            .inner;
                        if rewind.clicked() {
                            rewind_to = Some((start, end));
                        }
                    }
                }
            }
        });
    if let Some((path, line)) = open_at {
        open(world, tab, &path, Some(line));
    }
    if let Some((from, to)) = rewind_to {
        crate::rewind::open_at(world, from, to, to);
    }
}

/// The marks beside a test file's `describe` and `it` lines: ▶ to run it, or how it did.
fn paint_test_marks(
    ui: &egui::Ui,
    area: &Rect,
    lines: &[Rect],
    marks: &[(TestLine, Option<String>)],
    row_h: f32,
) {
    let painter = ui.painter();
    for (t, status) in marks {
        let Some(r) = lines.get(t.line as usize - 1) else {
            continue;
        };
        if r.bottom() < ui.clip_rect().top() || r.top() > ui.clip_rect().bottom() {
            continue;
        }
        let c = Pos2::new(area.left() + 24.0, r.center().y);
        let s = row_h * 0.3;
        match status.as_deref() {
            Some("passed") => {
                painter.circle_filled(c, s + 1.5, theme::PLAY.gamma_multiply(0.25));
                painter.line_segment(
                    [
                        c + Vec2::new(-s * 0.55, 0.0),
                        c + Vec2::new(-s * 0.1, s * 0.5),
                    ],
                    Stroke::new(2.0, theme::PLAY),
                );
                painter.line_segment(
                    [
                        c + Vec2::new(-s * 0.1, s * 0.5),
                        c + Vec2::new(s * 0.65, -s * 0.5),
                    ],
                    Stroke::new(2.0, theme::PLAY),
                );
            }
            Some("failed") => {
                painter.circle_filled(c, s + 1.5, ERROR);
                let w = s * 0.45;
                let stroke = Stroke::new(1.8, Color32::WHITE);
                painter.line_segment([c + Vec2::new(-w, -w), c + Vec2::new(w, w)], stroke);
                painter.line_segment([c + Vec2::new(-w, w), c + Vec2::new(w, -w)], stroke);
            }
            Some("running") | Some("queued") => {
                painter.circle_stroke(c, s, Stroke::new(2.0, VALUE));
            }
            _ => {
                // Not run: a play triangle.
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        c + Vec2::new(-s * 0.6, -s * 0.8),
                        c + Vec2::new(s * 0.8, 0.0),
                        c + Vec2::new(-s * 0.6, s * 0.8),
                    ],
                    if status.is_some() {
                        LINE_NO
                    } else {
                        theme::PLAY.gamma_multiply(0.75)
                    },
                    Stroke::NONE,
                ));
            }
        }
    }
}

/// Notes for a test file's lines after a run: each test's result on its `it` line, and why a
/// failed one failed on the line it stopped at.
fn test_notes(tests: &TestsPanel, file: &str) -> Vec<(u32, String, Color32)> {
    let mut out = Vec::new();
    for r in tests.results(file) {
        let line = r["line"].as_u64().unwrap_or(0) as u32;
        let secs = r["secs"].as_f64().unwrap_or(0.0);
        match r["status"].as_str() {
            Some("passed") => out.push((line, format!("✔ passed  {secs:.2} s"), theme::PLAY)),
            Some("failed") => {
                let at = r["failed_at"].as_u64().map(|n| n as u32).unwrap_or(line);
                let message = r["message"].as_str().unwrap_or("failed");
                if at != line {
                    out.push((line, format!("✖ failed at line {at}"), ERROR));
                }
                out.push((at, format!("✖ {message}"), ERROR));
            }
            Some("running") => out.push((line, "⏳ running".into(), VALUE)),
            _ => {}
        }
    }
    out
}

// ----------------------------------------------------------------------------- output

fn output(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab) {
    ui.horizontal(|ui| {
        let head = |text: &str, color: Color32| RichText::new(text).small().strong().color(color);
        let output_shown = !tab.types.show_problems && !tab.tests.shown;
        if ui
            .selectable_label(output_shown, head("OUTPUT", theme::TEXT_DIM))
            .clicked()
        {
            tab.types.show_problems = false;
            tab.tests.shown = false;
        }
        let errors = tab
            .output
            .iter()
            .filter(|l| l.kind == OutKind::Error)
            .count();
        if errors > 0 {
            ui.label(RichText::new(format!("✖ {errors}")).small().color(ERROR));
        }
        ui.add_space(8.0);
        let (e, w) = tab.types.counts();
        let color = if e > 0 {
            ERROR
        } else if w > 0 {
            LINT
        } else {
            theme::TEXT_DIM
        };
        if ui
            .selectable_label(
                tab.types.show_problems && !tab.tests.shown,
                head(&format!("PROBLEMS   ✖ {e}   ⚠ {w}"), color),
            )
            .on_hover_text("Luau's type checker on every script: type errors and warnings")
            .clicked()
        {
            tab.types.show_problems = true;
            tab.tests.shown = false;
        }
        ui.add_space(8.0);
        let last = &tab.tests.last;
        let (passed, failed) = (
            last["passed"].as_u64().unwrap_or(0),
            last["failed"].as_u64().unwrap_or(0),
        );
        let (text, color) = if tab.tests.running || tab.tests.pending.is_some() {
            ("TESTS   ⏳".to_string(), VALUE)
        } else if last.is_null() {
            ("TESTS".to_string(), theme::TEXT_DIM)
        } else if failed > 0 {
            (format!("TESTS   ✔ {passed}   ✖ {failed}"), ERROR)
        } else {
            (format!("TESTS   ✔ {passed}"), theme::PLAY)
        };
        if ui
            .selectable_label(tab.tests.shown, head(&text, color))
            .on_hover_text("The script tests (*.test.luau), run in the game")
            .clicked()
        {
            tab.tests.shown = true;
        }
        if output_shown {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    tab.output.clear();
                }
            });
        }
    });
    if tab.tests.shown {
        tests_list(ui, world, tab);
        return;
    }
    if tab.types.show_problems {
        problems_list(ui, world, tab);
        return;
    }
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
    /// A file and line to show once the panel is drawn (a method just written for a signal).
    open_after: Option<(String, u32)>,
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

    // Its tool buttons (`@button` methods), before Play too.
    if loaded {
        buttons_section(ui, world, editor, entity, &path);
    }

    // Its signals and who hears them (Godot's signal connections, Unity's events).
    let signals = engine_script::props::declared(world, &path)
        .map(|d| d.signals)
        .unwrap_or_default();
    if let Some(at) = signals_section(ui, world, entity, &signals, &mut after) {
        open_at = Some(Some(at));
    }
    if let Some((file, line)) = heard_section(ui, world, entity) {
        open_in_tab(world, &file, Some(line));
    }
    if let Some(Ok(m)) = engine_script::props::declared(world, &path)
        .ok()
        .and_then(|d| d.machine)
    {
        machine_section(ui, world, entity, &m, playing);
    }

    if playing {
        live_values(ui, world, entity);
        keep_toggle(ui, world, entity);
    }
    commit_script(world, editor, entity, value, after);
    if let Some(line) = open_at {
        open_in_tab(world, &path, line);
    }
    if let Some((file, line)) = world
        .get_resource_mut::<ScriptsTab>()
        .and_then(|mut t| t.props.open_after.take())
    {
        open_in_tab(world, &file, Some(line));
    }
}

// ----------------------------------------------------------------------------- state machines

/// The state machine panel beside the code (`crate::states` draws the diagram).
#[derive(Default)]
pub struct MachinePanel {
    /// Closed with its ✖ or the States button (it opens for every script with states).
    pub hidden: bool,
    /// The open script has a states table.
    has: bool,
    hovered: Option<Pick>,
    pub selected: Option<Pick>,
    /// Showing the machine as it was after this change of the log (its time and state),
    /// PlayMaker's Debug Flow; None: live.
    pub scrub: Option<(f64, String)>,
    /// The state a right-click opened the breakpoint menu on.
    menu: Option<String>,
    /// The breakpoint the game paused on.
    pub hit: Option<Hit>,
}

/// Pauses the game when a state machine reaches a breakpoint set in the States panel, selects
/// the object and shows its script's machine as it was then.
fn pause_on_state_breaks(world: &mut World) {
    let Some(hit) = world
        .get_resource_mut::<StateBreakpoints>()
        .and_then(|mut b| std::mem::take(&mut b.hits).into_iter().next())
    else {
        return;
    };
    let t = editor::now(world);
    let name = world
        .get::<Name>(hit.entity)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| format!("{}", hit.entity));
    let paused = world.resource_scope(|world, mut ed: Mut<Editor>| {
        if ed.mode != Mode::Play {
            return false;
        }
        editor::pause(world, &mut ed);
        ed.select(Some(hit.entity));
        let verb = if hit.kind == "exit" {
            "left"
        } else {
            "entered"
        };
        ed.info(
            t,
            format!(
                "Paused: {name} {verb} {} ({}). Play goes on.",
                hit.state, hit.why
            ),
        );
        true
    });
    if !paused {
        return;
    }
    if hit.path.ends_with(".luau") {
        open_in_tab(world, &hit.path, None);
    }
    let mut tab = world.resource_mut::<ScriptsTab>();
    tab.machine.hidden = false;
    tab.machine.scrub = None;
    tab.machine.hit = Some(hit);
}

/// The machine `path`'s class declares (None: no states table), or why it doesn't run.
fn machine_of(world: &mut World, path: &str) -> Option<Result<Machine, MachineProblem>> {
    engine_script::props::declared(world, path)
        .ok()
        .and_then(|d| d.machine)
}

/// The entity whose run of `path` to show: a selected one, else the first.
fn running_entity(world: &mut World, path: &str, selection: &[Entity]) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Script)>();
    let runs: Vec<Entity> = q
        .iter(world)
        .filter(|(_, s)| s.path == path && s.enabled)
        .map(|(e, _)| e)
        .collect();
    selection
        .iter()
        .copied()
        .find(|e| runs.contains(e))
        .or_else(|| runs.iter().copied().min())
}

/// Beside the code: the script's states and transitions, live while playing, and the log of
/// its changes. A click on a state or a transition goes to its line.
fn states_panel(
    ui: &mut egui::Ui,
    world: &mut World,
    tab: &mut ScriptsTab,
    machine: Result<Machine, MachineProblem>,
    selection: &[Entity],
    paused: bool,
) {
    let path = tab.docs[tab.current].path.clone();
    let entity = running_entity(world, &path, selection);
    let live = entity.and_then(|e| engine_script::states::live(world, e));
    if !paused {
        tab.machine.hit = None;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("STATES").strong().color(theme::TEXT));
        match (&live, entity) {
            (Some(l), Some(e)) => {
                let name = world
                    .get::<Name>(e)
                    .map(|n| n.as_str().to_string())
                    .unwrap_or_else(|| format!("{e}"));
                ui.label(RichText::new(name).color(theme::TEXT));
                if !l.current.is_empty() {
                    ui.label(theme::dim("in"));
                    ui.label(
                        RichText::new(l.current.join(" + "))
                            .color(theme::PLAY)
                            .strong(),
                    );
                }
            }
            _ if tab.playing => {
                ui.label(theme::dim("no entity runs this file"));
            }
            _ => {
                ui.label(theme::dim("Press Play to watch it run"));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("✖")
                .on_hover_text("Hide (the States button shows it again)")
                .clicked()
            {
                tab.machine.hidden = true;
            }
        });
    });
    let m = match machine {
        Err(p) => {
            ui.add_space(8.0);
            let r = ui.add(
                egui::Label::new(
                    RichText::new(format!("✖ line {}: {}", p.line, p.message)).color(ERROR),
                )
                .wrap()
                .sense(egui::Sense::click()),
            );
            if r.on_hover_text("Open its line").clicked() {
                open(world, tab, &path, Some(p.line));
            }
            return;
        }
        Ok(m) => m,
    };
    // Paused on a breakpoint: what reached it.
    if let Some(hit) = &tab.machine.hit {
        let verb = if hit.kind == "exit" {
            "left"
        } else {
            "entered"
        };
        egui::Frame::new()
            .fill(crate::states::BREAK.gamma_multiply(0.18))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("⏸ Paused:")
                            .strong()
                            .color(crate::states::BREAK),
                    );
                    ui.label(
                        RichText::new(format!("{verb} {} ({})", hit.state, hit.why))
                            .color(theme::TEXT),
                    );
                    ui.label(theme::dim("Play goes on, Step one frame."));
                });
            });
    }
    // Ended in a final state at the outer level: nothing changes it until a `go`.
    if let Some(l) = live
        .as_ref()
        .filter(|l| l.over && tab.machine.scrub.is_none())
    {
        egui::Frame::new()
            .fill(theme::PLAY.gamma_multiply(0.14))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("✔ Ended:").strong().color(theme::PLAY));
                    ui.label(
                        RichText::new(format!(
                            "it reached {}, a final state",
                            l.current().unwrap_or_default()
                        ))
                        .color(theme::TEXT),
                    );
                    ui.label(theme::dim("self:go(state) starts it again."));
                });
            });
    }
    // Showing an earlier moment of the log: which, and the way back to live.
    let shown = tab.machine.scrub.as_ref().and_then(|(t, to)| {
        live.as_ref()?
            .log
            .iter()
            .position(|c| c.t == *t && c.to == *to)
    });
    if tab.machine.scrub.is_some() && shown.is_none() {
        tab.machine.scrub = None;
    }
    if let (Some(k), Some(l)) = (shown, &live) {
        let n = l.log.len();
        let mut to = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("⏪").color(theme::ACCENT));
            ui.label(
                RichText::new(format!(
                    "As it was after change {} of {n}, at {:.2} s",
                    k + 1,
                    l.log[k].t
                ))
                .color(theme::TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Live")
                    .on_hover_text("Back to now")
                    .clicked()
                {
                    tab.machine.scrub = None;
                }
                if ui
                    .add_enabled(k + 1 < n, egui::Button::new("Later").small())
                    .clicked()
                {
                    to = Some(k + 1);
                }
                if ui
                    .add_enabled(k > 0, egui::Button::new("Earlier").small())
                    .clicked()
                {
                    to = Some(k - 1);
                }
            });
        });
        if let Some(k) = to {
            tab.machine.scrub = Some((l.log[k].t, l.log[k].to.clone()));
        }
    }
    let then: Option<Live> = match (shown, &live) {
        (Some(k), Some(l)) => Some(l.as_of(k, &m)),
        _ => live.clone(),
    };
    let breaks = world
        .get_resource::<StateBreakpoints>()
        .and_then(|b| b.of(&path).cloned());
    let log_h = if live.is_some() { 150.0 } else { 0.0 };
    let h = (ui.available_height() - log_h - 8.0).max(140.0);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), h), egui::Sense::click());
    let placed = crate::states::layout_for(
        &m,
        &BTreeMap::new(),
        Some(rect.width() / rect.height().max(1.0)),
    );
    let (hits, _) = crate::states::fitted(
        ui,
        rect,
        &crate::states::Diagram {
            machine: &m,
            placed: &placed,
            live: then.as_ref(),
            selected: tab.machine.selected.as_ref(),
            hovered: tab.machine.hovered.as_ref(),
            breaks: breaks.as_ref(),
        },
    );
    for (pick, r) in &hits {
        if let Pick::State(name) = pick {
            tab.drawn_at.insert(format!("state {name}"), *r);
        }
    }
    tab.machine.hovered = resp.hover_pos().and_then(|p| crate::states::hit(&hits, p));
    let line_of = |pick: &Pick| match pick {
        Pick::State(name) => m.state(name).map_or(0, |s| s.line),
        Pick::Transition(k) => m.transitions.get(*k).map_or(0, |t| t.line),
    };
    if let Some(pick) = tab.machine.hovered.clone() {
        let tip = match &pick {
            Pick::State(name) => {
                let st = m.state(name);
                let mut lines = vec![format!("state {name}")];
                if let Some(st) = st {
                    if !st.doc.is_empty() {
                        lines.push(st.doc.clone());
                    }
                    if m.is_group(name) && st.parallel {
                        lines.push("a parallel group: all its states run at once".into());
                    } else if m.is_group(name) {
                        lines.push(format!(
                            "a group: starts in {}{}",
                            st.initial.clone().unwrap_or_default(),
                            if st.deep {
                                "; with deep history it goes back to where it was, all the way in"
                            } else if st.history {
                                "; with history it goes back to where it was"
                            } else {
                                ""
                            }
                        ));
                    }
                    if m.is_group(name)
                        && m.transitions.iter().any(|t| {
                            t.from.as_deref() == Some(name.as_str())
                                && matches!(t.trigger, engine_script::states::Trigger::Done)
                        })
                    {
                        lines.push(if st.parallel {
                            "done when every region reaches a final state".into()
                        } else {
                            "done when it reaches a final state".into()
                        });
                    }
                    if st.is_final {
                        lines.push("final: reaching it ends its group".into());
                    }
                    for h in &st.hooks {
                        lines.push(format!("{h}_{name}"));
                    }
                }
                if let Some(secs) = live.as_ref().and_then(|l| l.secs_in(name)) {
                    lines.push(format!("active for {secs:.1} s"));
                }
                lines.join("\n")
            }
            Pick::Transition(k) => m
                .transitions
                .get(*k)
                .map(|t| {
                    format!(
                        "{} ➡ {}: {}",
                        t.from.as_deref().unwrap_or("any state"),
                        t.to,
                        t.label()
                    )
                })
                .unwrap_or_default(),
        };
        let line = line_of(&pick);
        let more = if matches!(pick, Pick::State(_)) {
            "\nright-click: pause the game when it starts or ends"
        } else {
            ""
        };
        resp.clone().on_hover_text(if line > 0 {
            format!("{tip}\nline {line}: click to go there{more}")
        } else {
            format!("{tip}{more}")
        });
    }
    // Breakpoints: right-click a state.
    if resp.secondary_clicked() {
        tab.machine.menu = match &tab.machine.hovered {
            Some(Pick::State(name)) => Some(name.clone()),
            _ => None,
        };
    }
    if let Some(name) = tab.machine.menu.clone() {
        resp.context_menu(|ui| {
            ui.label(RichText::new(format!("state {name}")).strong());
            for (exit, what) in [(false, "starts"), (true, "ends")] {
                let on = breaks.as_ref().is_some_and(|b| {
                    if exit {
                        b.exit.contains(&name)
                    } else {
                        b.enter.contains(&name)
                    }
                });
                let label = format!(
                    "{}Pause the game when it {what}",
                    if on { "✔ " } else { "" }
                );
                if ui.button(label).clicked() {
                    world
                        .resource_mut::<StateBreakpoints>()
                        .set(&path, &name, exit, !on);
                    ui.close();
                }
            }
        });
    }
    if resp.clicked()
        && let Some(pick) = tab.machine.hovered.clone()
    {
        let line = line_of(&pick);
        tab.machine.selected = Some(pick);
        if line > 0 {
            open(world, tab, &path, Some(line));
        }
    }
    if let Some(l) = &live {
        ui.separator();
        // The changes, and beside them the script's values as they were after the one shown.
        let picked = ui
            .horizontal_top(|ui| {
                let values_w = (ui.available_width() * 0.4).min(320.0);
                let log_w = ui.available_width() - values_w - 14.0;
                let picked = ui
                    .allocate_ui(Vec2::new(log_w, log_h - 10.0), |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("CHANGES").small().color(theme::TEXT_DIM))
                                .on_hover_text(
                                    "Click a change to see the machine as it was after it",
                                );
                            egui::ScrollArea::vertical()
                                .id_salt("state log")
                                .max_height(log_h - 30.0)
                                .show(ui, |ui| crate::states::log_rows(ui, l, 64, shown))
                                .inner
                        })
                        .inner
                    })
                    .inner;
                ui.separator();
                ui.vertical(|ui| {
                    let what = if shown.is_some() {
                        "VALUES THEN"
                    } else {
                        "VALUES NOW"
                    };
                    ui.label(RichText::new(what).small().color(theme::TEXT_DIM))
                        .on_hover_text(
                            "The script's props and fields, as they were right after the change \
                             shown above (each change keeps them)",
                        );
                    egui::ScrollArea::vertical()
                        .id_salt("state values")
                        .max_height(log_h - 30.0)
                        .show(ui, |ui| crate::states::value_rows(ui, l, shown));
                });
                picked
            })
            .inner;
        if let Some(k) = picked {
            tab.machine.scrub = Some((l.log[k].t, l.log[k].to.clone()));
        }
    }
}

/// The Details panel's state machine: the diagram, live while playing, and the last changes.
fn machine_section(ui: &mut egui::Ui, world: &World, entity: Entity, m: &Machine, playing: bool) {
    egui::CollapsingHeader::new(RichText::new("State machine").strong().color(theme::TEXT))
        .id_salt(("script states", entity))
        .default_open(true)
        .show(ui, |ui| {
            let live = playing
                .then(|| engine_script::states::live(world, entity))
                .flatten();
            match live.as_ref().and_then(|l| l.current().map(|c| (c, l))) {
                Some((c, l)) => {
                    ui.horizontal(|ui| {
                        ui.label(theme::dim(if l.over { "Ended in" } else { "In" }));
                        ui.label(
                            RichText::new(l.current.join(" + "))
                                .color(theme::PLAY)
                                .strong(),
                        );
                        if let Some(secs) = l.secs_in(c) {
                            ui.label(theme::dim(format!("for {secs:.1} s")));
                        }
                    });
                }
                None => {
                    ui.label(theme::dim(if playing {
                        "Not running (an error stopped it?)"
                    } else {
                        "Press Play to watch it change state."
                    }));
                }
            }
            // As wide as the panel, at most 300 high.
            let w = ui.available_width();
            let placed = crate::states::layout_for(m, &BTreeMap::new(), Some(w / 300.0));
            let h =
                (w * placed.bounds.height() / placed.bounds.width().max(1.0)).clamp(140.0, 300.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
            ui.painter().rect_filled(rect, 6.0, CODE_BG);
            let path = world.get::<Script>(entity).map(|s| s.path.clone());
            let breaks = world
                .get_resource::<StateBreakpoints>()
                .zip(path)
                .and_then(|(b, p)| b.of(&p).cloned());
            crate::states::fitted(
                ui,
                rect,
                &crate::states::Diagram {
                    machine: m,
                    placed: &placed,
                    live: live.as_ref(),
                    selected: None,
                    hovered: None,
                    breaks: breaks.as_ref(),
                },
            );
            if let Some(l) = &live {
                crate::states::log_rows(ui, l, 4, None);
            }
        });
}

/// The scene's objects that run a script: (name, entity, script path), by name.
fn scripted_objects(world: &mut World) -> Vec<(String, Entity, String)> {
    let mut q = world.query_filtered::<(Entity, &Name, &Script), With<crate::scene_ops::InScene>>();
    let mut out: Vec<(String, Entity, String)> = q
        .iter(world)
        .filter(|(_, n, s)| !n.as_str().is_empty() && !s.path.is_empty())
        .map(|(e, n, s)| (n.as_str().to_string(), e, s.path.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

/// The script file the scene object named `target` runs (empty: `me`'s).
fn target_script(objects: &[(String, Entity, String)], me: Entity, target: &str) -> Option<String> {
    objects
        .iter()
        .find(|(n, e, _)| {
            if target.is_empty() {
                *e == me
            } else {
                n == target
            }
        })
        .map(|o| o.2.clone())
}

/// What a connection row asks for.
enum Wire {
    Target(String),
    Method(String),
    Args(Vec<Value>),
    Once(bool),
    Remove,
    /// Write `function Class:name(params) end` into the target's script and wire it.
    NewMethod(String),
}

/// The "Signals" section: each signal the class declares, with its connections (target object,
/// method, fixed values, once) and "+ Connect". Edits go into `after["connections"]`. Returns
/// a line of this script to show (a signal's declaration).
fn signals_section(
    ui: &mut egui::Ui,
    world: &mut World,
    entity: Entity,
    signals: &[engine_script::props::Signal],
    after: &mut Value,
) -> Option<u32> {
    let wired: Vec<engine_script::Connection> =
        serde_json::from_value(after["connections"].clone()).unwrap_or_default();
    if signals.is_empty() && wired.is_empty() {
        return None;
    }
    let objects = scripted_objects(world);
    let mut open = None;
    let mut edits: Vec<(usize, Wire)> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    egui::CollapsingHeader::new(RichText::new("Signals").strong().color(theme::TEXT))
        .id_salt(("script signals", entity))
        .default_open(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let mut names: Vec<&str> = signals.iter().map(|s| s.name.as_str()).collect();
            // Connections to a signal the class no longer declares still show (in red).
            for c in &wired {
                if !names.contains(&c.signal.as_str()) {
                    names.push(c.signal.as_str());
                }
            }
            for name in names {
                let declared = signals.iter().find(|s| s.name == name);
                let head = ui.horizontal(|ui| {
                    let mut job = LayoutJob::default();
                    let mono = FontId::monospace(12.5);
                    let color = if declared.is_some() { SIGNAL } else { ERROR };
                    job.append("⚡ ", 0.0, TextFormat::simple(mono.clone(), color));
                    job.append(name, 0.0, TextFormat::simple(mono.clone(), Color32::WHITE));
                    let params = declared.map(|s| s.params.as_str()).unwrap_or("");
                    job.append(
                        &format!("({params})"),
                        0.0,
                        TextFormat::simple(mono, theme::TEXT_DIM),
                    );
                    ui.add(egui::Label::new(job).sense(egui::Sense::click()))
                });
                let r = head.inner;
                let r = match declared {
                    Some(s) if !s.doc.is_empty() => r.on_hover_text(format!(
                        "{}\n\nThe script fires it with self:emit(\"{}\", ...). Click to see where it \
                         is declared.",
                        s.doc, s.name
                    )),
                    Some(s) => r.on_hover_text(format!(
                        "The script fires it with self:emit(\"{}\", ...).",
                        s.name
                    )),
                    None => r.on_hover_text("The script no longer declares this signal"),
                };
                if r.clicked()
                    && let Some(s) = declared
                {
                    open = Some(s.line);
                }
                let params = declared.map(|s| s.params.clone()).unwrap_or_default();
                for (k, c) in wired.iter().enumerate().filter(|(_, c)| c.signal == name) {
                    for w in connection_row(ui, world, entity, &objects, k, c, &params) {
                        edits.push((k, w));
                    }
                }
                let none = !wired.iter().any(|c| c.signal == name);
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    if ui
                        .small_button("+ Connect")
                        .on_hover_text("Call a method of a scene object's script when this fires")
                        .clicked()
                    {
                        added.push(name.to_string());
                    }
                    if none {
                        ui.label(theme::dim("no one listens yet").small());
                    }
                });
            }
        });
    if edits.is_empty() && added.is_empty() {
        return open;
    }
    let mut wired = wired;
    for name in added {
        // A new wire to the first other scripted object (Godot's dialog starts on one too).
        let target = objects
            .iter()
            .find(|o| o.1 != entity)
            .map(|o| o.0.clone())
            .unwrap_or_default();
        wired.push(engine_script::Connection {
            signal: name,
            target,
            ..Default::default()
        });
    }
    let mut removed = Vec::new();
    for (k, w) in edits {
        let Some(c) = wired.get_mut(k) else { continue };
        match w {
            Wire::Target(t) => {
                c.target = t;
                c.method.clear();
            }
            Wire::Method(m) => c.method = m,
            Wire::Args(a) => c.args = a,
            Wire::Once(o) => c.once = o,
            Wire::Remove => removed.push(k),
            Wire::NewMethod(m) => {
                let params = signals
                    .iter()
                    .find(|s| s.name == c.signal)
                    .map(|s| s.params.clone())
                    .unwrap_or_default();
                if let Some(file) = target_script(&objects, entity, &c.target)
                    && let Some(line) = add_method(world, &file, &m, &params, &c.signal)
                {
                    c.method = m;
                    if let Some(mut t) = world.get_resource_mut::<ScriptsTab>() {
                        t.props.open_after = Some((file, line));
                    }
                }
            }
        }
    }
    let mut k = 0;
    wired.retain(|_| {
        k += 1;
        !removed.contains(&(k - 1))
    });
    after["connections"] = serde_json::to_value(&wired).unwrap_or(Value::Null);
    if wired.is_empty()
        && let Some(map) = after.as_object_mut()
    {
        map.remove("connections");
    }
    open
}

/// One connection: `→ [object] · [method] [values] [once] ✖`, red where the object or the
/// method is missing.
fn connection_row(
    ui: &mut egui::Ui,
    world: &mut World,
    entity: Entity,
    objects: &[(String, Entity, String)],
    k: usize,
    c: &engine_script::Connection,
    params: &str,
) -> Vec<Wire> {
    let mut out = Vec::new();
    let file = target_script(objects, entity, &c.target);
    let methods = file
        .as_deref()
        .and_then(|f| engine_script::props::declared(world, f).ok())
        .map(|d| d.methods)
        .unwrap_or_default();
    let method = methods.iter().find(|m| m.name == c.method);
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.label(RichText::new("➡").color(SIGNAL));
        let target_text = if c.target.is_empty() {
            "this object".to_string()
        } else {
            c.target.clone()
        };
        let target_color = if file.is_some() { theme::TEXT } else { ERROR };
        egui::ComboBox::from_id_salt(("signal target", entity, k))
            .selected_text(RichText::new(target_text).color(target_color))
            .width(130.0)
            .height(320.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(c.target.is_empty(), "this object")
                    .clicked()
                    && !c.target.is_empty()
                {
                    out.push(Wire::Target(String::new()));
                }
                for (name, e, path) in objects {
                    if *e == entity {
                        continue;
                    }
                    let r = ui.selectable_label(*name == c.target, name);
                    if r.on_hover_text(path).clicked() && *name != c.target {
                        out.push(Wire::Target(name.clone()));
                    }
                }
            })
            .response
            .on_hover_text(match &file {
                Some(f) => format!("Its script: {f}"),
                None => format!("No scene object named \"{}\" runs a script", c.target),
            });
        ui.label(theme::dim("·"));
        let method_text = match (c.method.is_empty(), method) {
            (true, _) => RichText::new("pick a method").color(ERROR),
            (false, Some(m)) => RichText::new(format!("{}({})", m.name, m.params)).monospace(),
            (false, None) => RichText::new(format!("{}  (missing)", c.method))
                .monospace()
                .color(ERROR),
        };
        let fresh = format!("on_{}", c.signal);
        let combo = egui::ComboBox::from_id_salt(("signal method", entity, k))
            .selected_text(method_text)
            .width(170.0)
            .height(360.0)
            .show_ui(ui, |ui| {
                for m in &methods {
                    let label = RichText::new(format!("{}({})", m.name, m.params)).monospace();
                    let label = if m.hook {
                        label.color(theme::TEXT_DIM)
                    } else {
                        label
                    };
                    if ui.selectable_label(m.name == c.method, label).clicked()
                        && m.name != c.method
                    {
                        out.push(Wire::Method(m.name.clone()));
                    }
                }
                if file.is_some() && !methods.iter().any(|m| m.name == fresh) {
                    ui.separator();
                    if ui
                        .selectable_label(false, format!("+ New method {fresh}({params})"))
                        .on_hover_text("Write it into the object's script and open it there")
                        .clicked()
                    {
                        out.push(Wire::NewMethod(fresh.clone()));
                    }
                }
            });
        if let Some(mut tab) = world.get_resource_mut::<ScriptsTab>() {
            tab.drawn_at
                .insert(format!("signal method {k}"), combo.response.rect);
        }
        // Fixed values (Unity's static parameters): JSON, empty passes the signal's own.
        let mut text = if c.args.is_empty() {
            String::new()
        } else {
            c.args
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(70.0)
                    .hint_text(if params.is_empty() { "values" } else { params }),
            )
            .on_hover_text(
                "Values to call it with, as JSON (2, \"open\", true). Empty: the signal's own \
                 values.",
            );
        if r.changed() {
            let parsed: Option<Vec<Value>> = if text.trim().is_empty() {
                Some(Vec::new())
            } else {
                serde_json::from_str::<Vec<Value>>(&format!("[{text}]")).ok()
            };
            if let Some(a) = parsed {
                out.push(Wire::Args(a));
            }
        }
        let mut once = c.once;
        if ui
            .checkbox(&mut once, "once")
            .on_hover_text("Only the first time it fires each play (Godot's one shot)")
            .changed()
        {
            out.push(Wire::Once(once));
        }
        if ui.small_button("✖").on_hover_text("Disconnect").clicked() {
            out.push(Wire::Remove);
        }
    });
    out
}

/// "Heard from": the scene's connections that call this object's script (the other end of
/// the wires, as Godot marks connected methods). Returns a file and line to open.
fn heard_section(ui: &mut egui::Ui, world: &mut World, entity: Entity) -> Option<(String, u32)> {
    let me = world.get::<Name>(entity)?.as_str().to_string();
    let mine = world.get::<Script>(entity)?.path.clone();
    let mut q = world.query_filtered::<(Entity, &Name, &Script), With<crate::scene_ops::InScene>>();
    let heard: Vec<(String, String, String)> = q
        .iter(world)
        .flat_map(|(e, n, s)| {
            s.connections
                .iter()
                .filter(|c| {
                    (c.target.is_empty() && e == entity) || (!me.is_empty() && c.target == me)
                })
                .map(|c| (n.as_str().to_string(), c.signal.clone(), c.method.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    if heard.is_empty() {
        return None;
    }
    let methods = engine_script::props::declared(world, &mine)
        .map(|d| d.methods)
        .unwrap_or_default();
    let mut open = None;
    egui::CollapsingHeader::new(RichText::new("Heard from").strong().color(theme::TEXT))
        .id_salt(("script heard", entity))
        .default_open(true)
        .show(ui, |ui| {
            for (from, signal, method) in &heard {
                let line = methods.iter().find(|m| &m.name == method).map(|m| m.line);
                let mut job = LayoutJob::default();
                let mono = FontId::monospace(12.0);
                job.append(from, 0.0, TextFormat::simple(mono.clone(), theme::TEXT));
                job.append(" ⚡", 0.0, TextFormat::simple(mono.clone(), SIGNAL));
                job.append(
                    signal,
                    0.0,
                    TextFormat::simple(mono.clone(), Color32::WHITE),
                );
                job.append(
                    "  ➡  ",
                    0.0,
                    TextFormat::simple(mono.clone(), theme::TEXT_DIM),
                );
                let color = if line.is_some() { CALL } else { ERROR };
                job.append(method, 0.0, TextFormat::simple(mono, color));
                let r = ui
                    .add(egui::Label::new(job).sense(egui::Sense::click()))
                    .on_hover_text(match line {
                        Some(_) => "Open the method",
                        None => "This script has no such method",
                    });
                if r.clicked()
                    && let Some(l) = line
                {
                    open = Some((mine.clone(), l));
                }
            }
        });
    open
}

/// Writes `function Class:name(params)` (an empty body with a comment) into `file`, before
/// its `return Class`, as Godot's connect dialog makes the receiving method. Returns the new
/// method's first line (1-based).
fn add_method(
    world: &mut World,
    file: &str,
    name: &str,
    params: &str,
    signal: &str,
) -> Option<u32> {
    let full = engine_script::scripts_root(world).join(engine_script::normalize(file));
    let text = std::fs::read_to_string(&full).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let ret = lines
        .iter()
        .rposition(|l| l.trim_start().starts_with("return "))?;
    let class = lines[ret].trim_start().trim_start_matches("return ").trim();
    if class.is_empty() || !class.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let names: Vec<&str> = params
        .split(',')
        .map(|p| p.split(':').next().unwrap_or("").trim())
        .filter(|p| !p.is_empty())
        .collect();
    let method = format!(
        "function {class}:{name}({})\n\t-- {signal} fired\nend\n",
        names.join(", ")
    );
    let mut out = lines[..ret].join("\n");
    if !out.ends_with("\n\n") {
        out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
    }
    let line = out.lines().count() as u32 + 1;
    out.push_str(&method);
    out.push('\n');
    out.push_str(&lines[ret..].join("\n"));
    out.push('\n');
    std::fs::write(&full, out).ok()?;
    Some(line)
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

// ----------------------------------------------------------------------------- tools

/// The scene as a tool found it: each scene entity's registered components, name and parent,
/// and every entity there was (to tell what the tool made).
struct SceneBefore {
    scene: HashMap<
        Entity,
        (
            engine::core::scene::EntityDoc,
            Option<Entity>,
            Option<SceneOrder>,
        ),
    >,
    all: HashSet<Entity>,
}

fn scene_before(world: &mut World) -> SceneBefore {
    let scene: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<crate::scene_ops::InScene>>();
        q.iter(world).collect()
    };
    let scene = scene
        .into_iter()
        .map(|e| {
            let doc = engine::core::scene::snapshot_entity(world, e, false);
            let parent = world.get::<ChildOf>(e).map(|p| p.parent());
            (e, (doc, parent, world.get::<SceneOrder>(e).copied()))
        })
        .collect();
    let all = world.iter_entities().map(|e| e.id()).collect();
    SceneBefore { scene, all }
}

/// What a tool made becomes part of the scene (saved, listed, undoable): new entities under
/// scene entities, and new named roots. Parts a script's `construct` built stay its own.
fn adopt_made(world: &mut World, before: &SceneBefore) {
    let made: Vec<(Entity, Option<Entity>, bool)> = {
        let mut q = world.query_filtered::<(Entity, Option<&ChildOf>, Has<Name>), (
            Without<crate::scene_ops::InScene>,
            Without<engine_script::Constructed>,
            Without<crate::scene_ops::EditorOnly>,
        )>();
        q.iter(world)
            .filter(|(e, _, _)| !before.all.contains(e))
            .map(|(e, p, named)| (e, p.map(|p| p.parent()), named))
            .collect()
    };
    // Parents first, so a child knows whether it hangs under the scene.
    let mut pending = made;
    loop {
        let mut adopted = false;
        pending.retain(|&(e, parent, named)| {
            let under_scene =
                parent.is_some_and(|p| world.get::<crate::scene_ops::InScene>(p).is_some());
            if under_scene || (parent.is_none() && named) {
                world
                    .entity_mut(e)
                    .insert((crate::scene_ops::InScene, SceneOrder::next()));
                adopted = true;
                false
            } else {
                true
            }
        });
        if !adopted {
            break;
        }
    }
}

/// The scene's changes since `before`, as undo commands: scene entities made (their
/// subtrees), removed, renamed or with components set, added or taken off.
fn scene_changes(world: &mut World, before: &SceneBefore) -> Vec<Command> {
    use crate::scene_ops::{InScene, SavedEntity};
    let now: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<InScene>>();
        q.iter(world).collect()
    };
    let mut commands = Vec::new();
    // Removed: each removed subtree once, from its top, as it was.
    let gone: HashSet<Entity> = before
        .scene
        .keys()
        .copied()
        .filter(|e| world.get::<InScene>(*e).is_none())
        .collect();
    fn saved_from(before: &SceneBefore, e: Entity) -> SavedEntity {
        let (doc, _, order) = before.scene[&e].clone();
        let mut kids: Vec<Entity> = before
            .scene
            .iter()
            .filter(|(_, (_, p, _))| *p == Some(e))
            .map(|(k, _)| *k)
            .collect();
        kids.sort_by_key(|k| before.scene[k].2.map_or(u64::MAX, |o| o.0));
        SavedEntity {
            old: e,
            doc,
            order,
            children: kids.into_iter().map(|k| saved_from(before, k)).collect(),
        }
    }
    for &e in &gone {
        let parent = before.scene[&e].1;
        if parent.is_some_and(|p| gone.contains(&p)) {
            continue;
        }
        commands.push(Command::Despawned {
            saved: saved_from(before, e),
            parent,
        });
    }
    for e in now {
        let parent = world.get::<ChildOf>(e).map(|p| p.parent());
        let Some((doc, _, _)) = before.scene.get(&e) else {
            // Made: each made subtree once, from its top.
            if parent.is_some_and(|p| {
                !before.scene.contains_key(&p) && world.get::<InScene>(p).is_some()
            }) {
                continue;
            }
            commands.push(Command::Spawned {
                saved: crate::scene_ops::save_entity(world, e),
                parent,
            });
            continue;
        };
        let after = engine::core::scene::snapshot_entity(world, e, false);
        if after.name != doc.name {
            commands.push(Command::Rename {
                entity: e,
                before: doc.name.clone(),
                after: after.name.clone(),
            });
        }
        let mut names: Vec<&String> = doc
            .components
            .keys()
            .chain(after.components.keys())
            .collect();
        names.sort();
        names.dedup();
        for name in names {
            let (b, a) = (doc.components.get(name), after.components.get(name));
            if b != a {
                commands.push(Command::Component {
                    entity: e,
                    name: name.clone(),
                    before: b.cloned(),
                    after: a.cloned(),
                });
            }
        }
    }
    commands
}

/// What a tool runs: a button's method on an entity, or a tool file on the selection.
#[derive(Debug, Clone)]
pub enum ToolCall {
    Button { entity: Entity, method: String },
    File { path: String },
}

/// Runs a tool and records what it changed in the scene as one undo step (Unity's
/// `Undo.RecordObject` around a `[ContextMenu]` command; Godot's EditorUndoRedoManager).
pub fn run_tool(world: &mut World, editor: &mut Editor, call: ToolCall, label: &str) {
    let t = editor::now(world);
    let before = scene_before(world);
    let result = match &call {
        ToolCall::Button { entity, method } => call_api(
            world,
            "script.tool",
            json!({"entity": entity.to_bits(), "method": method}),
        ),
        ToolCall::File { path } => {
            let selection: Vec<u64> = editor.selection.iter().map(|e| e.to_bits()).collect();
            call_api(
                world,
                "script.tool",
                json!({"file": path, "selection": selection}),
            )
        }
    };
    adopt_made(world, &before);
    let commands = scene_changes(world, &before);
    let changed = commands.len();
    if !commands.is_empty() {
        editor
            .history
            .record(world, Command::Group(label.to_string(), commands), t);
    }
    match result {
        Ok(_) if changed == 0 => editor.info(t, format!("{label}: nothing in the scene changed")),
        Ok(_) => editor.info(t, format!("{label} (Ctrl+Z undoes it)")),
        Err(e) => editor.error(t, format!("{label}: {}", message_gist(&e))),
    }
}

/// The Details panel's tool buttons for a script's `@button` methods.
fn buttons_section(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    entity: Entity,
    path: &str,
) {
    let methods: Vec<engine_script::props::Method> = engine_script::props::declared(world, path)
        .map(|d| d.methods)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.button.is_some())
        .collect();
    if methods.is_empty() {
        return;
    }
    let mut pressed = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for m in &methods {
            let label = m.button.clone().unwrap_or_default();
            let tip = if m.doc.is_empty() {
                format!("Runs {}() now, before Play too", m.name)
            } else {
                format!("{}\n\nRuns {}() now, before Play too", m.doc, m.name)
            };
            let r = ui
                .add(egui::Button::new(
                    RichText::new(format!("▶ {label}")).color(theme::TEXT),
                ))
                .on_hover_text(tip);
            if let Some(mut t) = world.get_resource_mut::<ScriptsTab>() {
                t.drawn_at.insert(format!("button {}", m.name), r.rect);
            }
            if r.clicked() {
                pressed = Some((m.name.clone(), label));
            }
        }
    });
    if let Some((method, label)) = pressed {
        let file = path.rsplit('/').next().unwrap_or(path);
        run_tool(
            world,
            editor,
            ToolCall::Button { entity, method },
            &format!("{label} ({file})"),
        );
    }
}

/// The tool files the Tools menu offers, read again when the folder changes.
#[derive(Default)]
pub struct ToolList {
    /// When it was last read (seconds of editor time), and what it said.
    read_at: f64,
    pub tools: Vec<ToolEntry>,
}

#[derive(Clone, Debug)]
pub struct ToolEntry {
    pub file: String,
    pub title: String,
    pub doc: String,
    pub error: Option<String>,
}

/// The Tools menu: each tool file runs on the selection, as one undo step.
fn tools_menu(ui: &mut egui::Ui, world: &mut World, tab: &mut ScriptsTab, now: f64) {
    if now - tab.tools.read_at > 2.0 || tab.tools.read_at == 0.0 {
        tab.tools.read_at = now.max(1e-6);
        if let Ok(Value::Array(list)) = call_api(world, "script.tools", json!({})) {
            tab.tools.tools = list
                .iter()
                .map(|t| ToolEntry {
                    file: t["file"].as_str().unwrap_or_default().to_string(),
                    title: t["title"].as_str().unwrap_or_default().to_string(),
                    doc: t["doc"].as_str().unwrap_or_default().to_string(),
                    error: t["error"].as_str().map(str::to_string),
                })
                .collect();
        }
    }
    let mut run = None;
    let menu = ui.menu_button("🛠 Tools", |ui| {
        if tab.tools.tools.is_empty() {
            ui.label(theme::dim(
                "No tools yet: a file in scripts/tools/ with a run(selection) function",
            ));
        }
        for t in &tab.tools.tools {
            let r = ui.add_enabled(t.error.is_none(), egui::Button::new(&t.title));
            let tip = match &t.error {
                Some(e) => format!("{}: {}", t.file, message_gist(e)),
                None => format!(
                    "{}\n\n{} — on the selection; Ctrl+Z undoes it",
                    t.doc, t.file
                ),
            };
            if r.on_hover_text(tip).clicked() {
                run = Some(t.clone());
                ui.close();
            }
        }
    });
    tab.drawn_at.insert("tools menu".into(), menu.response.rect);
    menu.response
        .on_hover_text("Run a tool script on the selected objects (scripts/tools/)");
    if let Some(t) = run {
        tab.pending_tool = Some(t);
    }
}

// ----------------------------------------------------------------------------- prop handles

/// Handle color, and while hovered or dragged.
const HANDLE: Color32 = Color32::from_rgb(0x5a, 0xd6, 0xd2);
const HANDLE_HOT: Color32 = Color32::from_rgb(0xff, 0xd2, 0x5a);
/// How near the pointer must be to take a handle, points.
const HANDLE_REACH: f32 = 10.0;

/// A prop handle being dragged: the entity, the prop, and the Script as it was (Godot's
/// restore value: letting go makes one undo step from it, Escape puts it back).
pub struct HandleDrag {
    entity: Entity,
    prop: String,
    before: Value,
}

/// One handle on screen: its prop, its point in the world and on screen.
struct Spot {
    prop: Prop,
    kind: engine_script::props::Handle,
    world: DVec3,
    screen: Pos2,
}

/// A prop's value now: the Script's own, or the class's default.
fn prop_value(script: &Script, prop: &Prop) -> Value {
    script
        .props
        .get(&prop.name)
        .cloned()
        .unwrap_or_else(|| prop.default.clone())
}

fn vec3_of(v: &Value) -> Option<DVec3> {
    let a = v.as_array()?;
    Some(DVec3::new(
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ))
}

/// The axis a handle uses, in the entity's space.
fn axis_of(kind: engine_script::props::Handle) -> DVec3 {
    match kind {
        engine_script::props::Handle::Axis(1) => DVec3::Y,
        engine_script::props::Handle::Axis(2) => DVec3::Z,
        _ => DVec3::X,
    }
}

/// Where a ray meets the plane through `at` across `normal`.
fn ray_plane(o: DVec3, d: DVec3, at: DVec3, normal: DVec3) -> Option<DVec3> {
    let den = d.dot(normal);
    if den.abs() < 1e-6 {
        return None;
    }
    let t = (at - o).dot(normal) / den;
    (t > 0.0).then(|| o + d * t)
}

/// The point of the line through `a` along unit `u` nearest the ray `o + t d`.
fn line_nearest_ray(a: DVec3, u: DVec3, o: DVec3, d: DVec3) -> f64 {
    let w = a - o;
    let (b, dd, e) = (u.dot(d), u.dot(w), d.dot(w));
    let den = 1.0 - b * b;
    if den.abs() < 1e-9 {
        return 0.0;
    }
    (b * e - dd) / den
}

/// Handles for the selected script's `@handle` props in the scene view (Unreal's
/// `MakeEditWidget`, Unity's `Handles`, Godot's gizmo handles): a diamond for a point (a
/// `vec3` in the entity's space), dots around a ring for a radius, a dot on an axis for a
/// distance. Dragging one sets the prop as it goes (in edit mode the script's `construct` builds
/// again, as Unreal's construction script runs on drag); a point moves across the entity's
/// ground plane, or up and down with Shift; Ctrl snaps to the prop's step (a quarter meter when
/// it has none). Letting go is one undo step; Escape puts it back. True while a handle is under
/// the pointer or dragged, so the view neither selects nor box-selects.
pub fn handles(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    view: &crate::view::ViewInfo,
    response: &egui::Response,
) -> bool {
    crate::rewind::overlay(ui, world, view);
    let Some(e) = editor.primary() else {
        return false;
    };
    let Some(script) = world.get::<Script>(e).cloned() else {
        return false;
    };
    let Some(global) = world.get::<GlobalTransform>(e).map(|g| g.0) else {
        return false;
    };
    let Ok(props) = engine_script::props::of(world, &script.path) else {
        return false;
    };
    let mut spots = Vec::new();
    for prop in props.into_iter().filter(|p| p.handle.is_some()) {
        let kind = prop.handle.expect("filtered");
        let value = prop_value(&script, &prop);
        let locals: Vec<DVec3> = match kind {
            engine_script::props::Handle::Point => vec3_of(&value).into_iter().collect(),
            engine_script::props::Handle::Radius => {
                let r = value.as_f64().unwrap_or(0.0);
                vec![DVec3::X * r, DVec3::Z * r, -DVec3::X * r, -DVec3::Z * r]
            }
            engine_script::props::Handle::Axis(_) => {
                vec![axis_of(kind) * value.as_f64().unwrap_or(0.0)]
            }
        };
        let mut debug = world.resource_mut::<DebugDraw>();
        let dim = [0.35, 0.84, 0.82, 0.55];
        match kind {
            engine_script::props::Handle::Radius => {
                let r = value.as_f64().unwrap_or(0.0);
                let up = global.transform_vector3(DVec3::Y).normalize_or(DVec3::Y);
                let scale = global.transform_vector3(DVec3::X).length();
                debug
                    .circle(global.translation, up.as_vec3(), (r * scale) as f32, dim)
                    .on_top();
            }
            engine_script::props::Handle::Axis(_) => {
                let end = global.transform_point3(locals[0]);
                debug.line(global.translation, end, dim).on_top();
            }
            engine_script::props::Handle::Point => {}
        }
        for local in locals {
            let at = global.transform_point3(local);
            if let Some(screen) = view.project(at) {
                spots.push(Spot {
                    prop: prop.clone(),
                    kind,
                    world: at,
                    screen,
                });
            }
        }
    }
    if spots.is_empty() {
        return false;
    }
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let dragging = world
        .resource::<ScriptsTab>()
        .handle
        .as_ref()
        .filter(|d| d.entity == e)
        .map(|d| d.prop.clone());
    let nearest = |at: Option<Pos2>| {
        at.and_then(|p| {
            spots
                .iter()
                .filter(|s| s.screen.distance(p) < HANDLE_REACH)
                .min_by(|a, b| a.screen.distance(p).total_cmp(&b.screen.distance(p)))
                .map(|s| s.prop.name.clone())
        })
    };
    let hovered = match &dragging {
        Some(_) => None,
        None => nearest(pointer),
    };
    // Draw them: a diamond for a point, a dot for the rest, each with its name and value.
    let painter = ui.painter_at(view.screen);
    let mut labelled = HashSet::new();
    for s in &spots {
        let hot = dragging.as_deref() == Some(s.prop.name.as_str())
            || hovered.as_deref() == Some(s.prop.name.as_str());
        let color = if hot { HANDLE_HOT } else { HANDLE };
        let r = if hot { 7.5 } else { 6.0 };
        let outline = Stroke::new(1.5, Color32::from_black_alpha(200));
        match s.kind {
            engine_script::props::Handle::Point => {
                let c = s.screen;
                let pts = vec![
                    c + Vec2::new(0.0, -r - 1.5),
                    c + Vec2::new(r + 1.5, 0.0),
                    c + Vec2::new(0.0, r + 1.5),
                    c + Vec2::new(-r - 1.5, 0.0),
                ];
                painter.add(egui::Shape::convex_polygon(pts, color, outline));
            }
            _ => {
                painter.circle(s.screen, r * 0.8, color, outline);
            }
        }
        let value = prop_value(&script, &s.prop);
        let shown = match s.kind {
            engine_script::props::Handle::Point => vec3_of(&value)
                .map(|v| format!("({:.1}, {:.1}, {:.1})", v.x, v.y, v.z))
                .unwrap_or_default(),
            _ => {
                let unit = s.prop.unit.as_deref().unwrap_or("");
                format!("{:.2} {unit}", value.as_f64().unwrap_or(0.0))
            }
        };
        // A ring of four dots shows its label once.
        if labelled.insert(s.prop.name.clone()) {
            let text = format!("{}  {}", s.prop.name, shown.trim());
            let at = s.screen + Vec2::new(11.0, -11.0);
            let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), color);
            let back = Rect::from_min_size(
                at - Vec2::new(4.0, 2.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            painter.rect_filled(back, 3.0, Color32::from_black_alpha(170));
            painter.galley(at, galley, color);
        }
    }
    let shift = ui.input(|i| i.modifiers.shift);
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    // Take one: the handle the button went down on (egui starts a drag once the pointer has
    // moved off it).
    let pressed_on = nearest(ui.input(|i| i.pointer.press_origin()));
    if dragging.is_none()
        && response.drag_started_by(egui::PointerButton::Primary)
        && let Some(name) = pressed_on.or(hovered.clone())
        && let Ok(before) = serde_json::to_value(&script)
    {
        world.resource_mut::<ScriptsTab>().handle = Some(HandleDrag {
            entity: e,
            prop: name,
            before,
        });
        return true;
    }
    let Some(name) = dragging else {
        return hovered.is_some();
    };
    let Some(spot) = spots.iter().find(|s| s.prop.name == name) else {
        return true;
    };
    // Escape: back to how it was, no undo step.
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        if let Some(d) = world.resource_mut::<ScriptsTab>().handle.take()
            && let Ok(back) = serde_json::from_value::<Script>(d.before)
        {
            world.entity_mut(e).insert(back);
        }
        return true;
    }
    if let Some(p) = pointer {
        let (o, d) = view.ray(p);
        let d = d.as_dvec3();
        let inv = global.inverse();
        let up = global.transform_vector3(DVec3::Y).normalize_or(DVec3::Y);
        let snap = |v: f64| {
            if ctrl {
                let step = spot.prop.step.unwrap_or(0.25);
                (v / step).round() * step
            } else {
                (v * 1000.0).round() / 1000.0
            }
        };
        let value = match spot.kind {
            engine_script::props::Handle::Point => {
                let normal = if shift {
                    // Up and down: a plane facing the camera, standing upright.
                    let f = (view.rotation * Vec3::NEG_Z).as_dvec3();
                    (f - up * f.dot(up)).normalize_or(DVec3::Z)
                } else {
                    up
                };
                ray_plane(o, d, spot.world, normal).map(|hit| {
                    let old = inv.transform_point3(spot.world);
                    let mut local = inv.transform_point3(hit);
                    if shift {
                        local = DVec3::new(old.x, local.y, old.z);
                    }
                    serde_json::json!([snap(local.x), snap(local.y), snap(local.z)])
                })
            }
            engine_script::props::Handle::Radius => {
                ray_plane(o, d, global.translation, up).map(|hit| {
                    let local = inv.transform_point3(hit);
                    let r = spot
                        .prop
                        .clamp(snap(DVec3::new(local.x, 0.0, local.z).length()));
                    serde_json::json!(r)
                })
            }
            engine_script::props::Handle::Axis(_) => {
                let axis = global.transform_vector3(axis_of(spot.kind));
                let scale = axis.length().max(1e-9);
                let t = line_nearest_ray(global.translation, axis / scale, o, d);
                Some(serde_json::json!(spot.prop.clamp(snap(t / scale))))
            }
        };
        if let Some(value) = value
            && world
                .get::<Script>(e)
                .is_some_and(|s| s.props.get(&name) != Some(&value))
            && let Some(mut s) = world.get_mut::<Script>(e)
        {
            if !s.props.is_object() {
                s.props = Value::Object(Default::default());
            }
            s.props[name.as_str()] = value;
        }
    }
    // Let go: one undo step from how it was.
    if (response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()))
        && let Some(d) = world.resource_mut::<ScriptsTab>().handle.take()
        && let Some(after) = world
            .get::<Script>(e)
            .and_then(|s| serde_json::to_value(s).ok())
    {
        commit_script(world, editor, e, &d.before, after);
    }
    true
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
                   "pulse": {"rate": 1.6, "depth": 0.6}}),
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

/// `editor --headless --size 1920x1080 --shots <dir> --script types`: Luau's type checker in the
/// Scripts tab: misspelt props and entity fields and a wrong argument underlined as typed (with
/// the Problems list), a field's inferred type under the pointer, the members of `self.props`
/// with their types, and a call's parameters while typing it.
pub fn types_shots(dir: String) -> ShotScript {
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
        if let Some(e) = crate::shots::find(world, "Lamp") {
            world
                .entity_mut(e)
                .insert(Script::new("examples/beacon.luau"));
        }
        world.resource_mut::<EditorDock>().0 = shot_layout();
        world.resource_mut::<ScriptsTab>().files_next = 0.0;
        world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
            open(world, &mut tab, "examples/beacon.luau", Some(34));
        });
    };
    let with_tab = |f: fn(&mut World, &mut ScriptsTab)| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| f(world, &mut tab));
        }
    };
    /// The caret at the end of the first line holding `needle`.
    fn caret_after(text: &str, needle: &str) -> Option<usize> {
        let b = text.find(needle)? + needle.len();
        Some(text[..b].chars().count())
    }
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Select("Lamp"),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(1.5, 1.3, 9.0), DVec3::new(-0.5, 1.0, 2.5)),
        Step::Tab(Tab::Scripts),
        Step::Wait(30),
        // Three mistakes typed, not saved: a misspelt prop, a misspelt entity field and text
        // where the signal sends a number. Each is underlined where it is; Problems lists them.
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &mut tab.docs[tab.current];
            doc.text = doc
                .saved
                .replace("p.lumens * level", "p.lumen * level")
                .replace(
                    "self.entity.position = self.home",
                    "self.entity.positon = self.home",
                )
                .replace(
                    "self:emit(\"flashed\", self.flashes)",
                    "self:emit(\"flashed\", p.mode)",
                );
            doc.jump = caret_after(&doc.text, "p.lumen");
            tab.types.show_problems = true;
        }))),
        Step::Wait(40),
        Step::Shot("types_01_mistakes_underlined"),
        // Fixed; the pointer on a field the script set itself: its type, found by the checker.
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.saved.clone();
            tab.types.show_problems = false;
            if let Some((l, line)) = doc
                .text
                .split('\n')
                .enumerate()
                .find(|(_, l)| l.contains("= self.home +"))
                && let Some(b) = line.find("self.home")
            {
                tab.point_at = Some((l as u32, line[..b].chars().count() as u32 + 6));
            }
        }))),
        Step::Wait(60),
        Step::Shot("types_02_hover_type"),
        // `self.props.` typed: the props as declared, with their types.
        Step::Do(Box::new(with_tab(|_, tab| {
            tab.point_at = None;
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.saved.replace(
                "\tlocal p = self.props\n",
                "\tlocal p = self.props\n\tlocal q = self.props.\n",
            );
            doc.jump = caret_after(&doc.text, "local q = self.props.");
            tab.complete_now = true;
        }))),
        Step::Wait(4),
        Step::Do(Box::new(with_tab(|_, tab| tab.complete_now = true))),
        Step::Wait(30),
        Step::Shot("types_03_props_members"),
        // A call being typed: its parameters over the line, the one being typed lit.
        Step::Do(Box::new(with_tab(|_, tab| {
            tab.completion = None;
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.saved.replace(
                "\tlocal p = self.props\n",
                "\tlocal p = self.props\n\tlocal lean = quat.axis_angle(vec3.right, \n",
            );
            doc.jump = caret_after(&doc.text, "quat.axis_angle(vec3.right, ");
        }))),
        Step::Wait(30),
        Step::Shot("types_04_call_parameters"),
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &mut tab.docs[tab.current];
            doc.text = doc.saved.clone();
        }))),
        Step::Wait(5),
    ];
    ShotScript::new(dir, steps)
}

/// A screenshot step: wires `from`'s signal to `to`'s method as the Details panel does (one
/// undo step), with fixed values when `args` holds some.
pub(crate) fn wire_step(
    from: &'static str,
    signal: &'static str,
    to: &'static str,
    method: &'static str,
    args: Value,
) -> impl Fn(&mut World, &mut Editor) + Send + Sync + 'static {
    move |world: &mut World, ed: &mut Editor| {
        let Some(e) = crate::shots::find(world, from) else {
            return;
        };
        let Some(before) = world
            .get::<Script>(e)
            .and_then(|s| serde_json::to_value(s).ok())
        else {
            return;
        };
        let mut script: Script = serde_json::from_value(before.clone()).unwrap_or_default();
        script.connections.push(engine_script::Connection {
            signal: signal.into(),
            target: to.into(),
            method: method.into(),
            args: args.as_array().cloned().unwrap_or_default(),
            once: false,
        });
        editor::apply(
            world,
            ed,
            Command::Component {
                entity: e,
                name: "Script".into(),
                before: Some(before),
                after: serde_json::to_value(script).ok(),
            },
        );
    }
}

/// `editor --headless --size 1920x1080 --shots <dir> --script signals`: signals wired in the
/// Details panel. The beacon's `flashed` signal wired to the lamp's `flare` (with a fixed
/// value), the list of the lamp's methods with "+ New method", the lamp's side (Heard from,
/// and the bolt by `Lamp:flare` in the code), playing (the lamp flares at each signal), and a
/// misspelt signal caught by the type checker.
pub fn signals_shots(dir: String) -> ShotScript {
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
            let _ = patch(world, json!({"hour": 19.6, "hours_per_second": 0.0}));
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
        let bulb = MeshRenderer::new(
            Shape::Sphere {
                radius: 0.14,
                segments: 0,
            },
            StandardMaterial::hex(0xb8e0ff).with_emissive([0.35, 0.75, 1.0], 2000.0),
        );
        // The beacon flashes often here, so the lamp's flares show in a still.
        let beacon = Script::new("examples/beacon.luau")
            .with_props(json!({"every": 1, "pulse": {"rate": 1.4, "depth": 0.6}}));
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
                ("Script", serde_json::to_value(beacon).unwrap_or_default()),
            ],
            Transform::from_translation(DVec3::new(-0.9, 1.1, 5.2)),
        );
        world.resource_mut::<EditorDock>().0 = props_layout();
    };
    let wire = wire_step;
    // A click on a widget the panel drew last frame (by its `drawn_at` name).
    let press = |what: &'static str, pressed: bool| {
        move |world: &mut World, _: &mut Editor| {
            let Some(r) = world
                .get_resource::<ScriptsTab>()
                .and_then(|t| t.drawn_at.get(what).copied())
            else {
                return;
            };
            let input = &mut world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected;
            input.push(egui::Event::PointerMoved(r.center()));
            input.push(egui::Event::PointerButton {
                pos: r.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    };
    let hover = |what: &'static str| {
        move |world: &mut World, _: &mut Editor| {
            let at = world.get_resource::<ScriptsTab>().and_then(|t| {
                t.drawn_at
                    .iter()
                    .find(|(k, _)| k.starts_with(what))
                    .map(|(_, r)| r.center())
            });
            if let Some(p) = at {
                world
                    .resource_mut::<crate::egui_bridge::EguiInput>()
                    .injected
                    .push(egui::Event::PointerMoved(p));
            }
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(1.2, 1.5, 9.2), DVec3::new(-0.6, 1.0, 3.0)),
        Step::Select("Beacon"),
        Step::Wait(30),
        // Beacon ⚡ flashed → Lamp · flare(strength) with 2.5 as its value.
        Step::Do(Box::new(wire(
            "Beacon",
            "flashed",
            "Lamp",
            "flare",
            json!([2.5]),
        ))),
        Step::Wait(30),
        Step::Shot("signals_01_wired"),
        // The lamp's methods to choose from, and a new one to write.
        Step::Do(Box::new(press("signal method 0", true))),
        Step::Wait(1),
        Step::Do(Box::new(press("signal method 0", false))),
        Step::Wait(20),
        Step::Shot("signals_02_methods"),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
        })),
        // The other end: the lamp hears it (Details), and its code says so by `flare`.
        Step::Select("Lamp"),
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, "examples/lamp.luau", Some(24));
            });
        })),
        Step::Wait(40),
        Step::Do(Box::new(hover("heard "))),
        Step::Wait(20),
        Step::Shot("signals_03_heard"),
        // A misspelt signal: the type checker knows the beacon's signals.
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, "examples/beacon.luau", Some(50));
                let i = tab.current;
                let doc = &mut tab.docs[i];
                doc.text = doc
                    .saved
                    .replace("self:emit(\"flashed\"", "self:emit(\"flash\"");
                tab.types.show_problems = true;
            });
        })),
        Step::Wait(40),
        Step::Shot("signals_05_misspelt"),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            let mut tab = world.resource_mut::<ScriptsTab>();
            let i = tab.current;
            let doc = &mut tab.docs[i];
            doc.text = doc.saved.clone();
        })),
        Step::Wait(5),
        // Playing: each flash of the beacon flares the lamp.
        Step::Tab(Tab::Viewport),
        Step::Select("Beacon"),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(10),
        // Play starts the sky from the scene's time; back to the evening.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            let sky = world
                .resource::<TypeRegistry>()
                .resource("Sky")
                .map(|r| r.patch);
            if let Some(patch) = sky {
                let _ = patch(world, json!({"hour": 19.6, "hours_per_second": 0.0}));
            }
        })),
        Step::Wait(140),
        Step::Shot("signals_04_playing"),
    ];
    ShotScript::new(dir, steps)
}

/// Screenshots of the script tests: a test file with a ▶ beside each test, a run in the game
/// (Run all presses Play, each test spawns its own lamp or beacon, Stop after), every test
/// passed, then a bug put into the lamp script caught by its test, on the line that checks it.
pub fn tests_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        dusk_sky(world);
        let from = engine_script::scripts_root(world).join(EXAMPLES);
        copy_dir(&from, &root.join(EXAMPLES));
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        world.resource_mut::<EditorDock>().0 = tests_layout();
        let mut tab = world.resource_mut::<ScriptsTab>();
        tab.tests.shown = true;
        tab.files_next = 0.0;
    };
    let open_file = |path: &'static str, line: u32| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, path, Some(line));
            });
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(0.4, 2.4, 10.5), DVec3::new(0.0, 1.3, 4.0)),
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(open_file("examples/lamp.test.luau", 1))),
        Step::Wait(40),
        Step::Shot("tests_01_test_file"),
        // Run all: Play, then each test in turn with a lamp or beacon of its own.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<ScriptsTab>().tests.run(json!({}));
        })),
        Step::Wait(8),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Wait(22),
        Step::Shot("tests_02_running_in_the_game"),
        Step::Wait(400),
        Step::Shot("tests_03_all_passed"),
        // A bug in the lamp: pressing E no longer puts it out. Its test catches it.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, "examples/lamp.luau", Some(44));
                let i = tab.current;
                let doc = &mut tab.docs[i];
                doc.text = doc
                    .saved
                    .replace("self.lit = not self.lit", "self.lit = self.lit or true");
                open(world, &mut tab, "examples/lamp.test.luau", Some(1));
                tab.tests.run(json!({"files": ["examples/lamp.test.luau"]}));
            });
        })),
        Step::Wait(8),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Wait(300),
        Step::Shot("tests_04_caught_a_bug"),
    ];
    ShotScript::new(dir, steps)
}

/// Where the state machine shots' gate stands, and the camera on it.
const GATE_AT: DVec3 = DVec3::new(-0.3, 0.0, -0.5);
/// Where the states shots put the rune seal, once the gate is gone.
const SEAL_AT: DVec3 = DVec3::new(-0.3, 0.0, -2.0);

/// `editor --headless --size 1920x1080 --shots <dir> --script states`: a gate run by its
/// `states` table (examples/gate.luau): the diagram beside its code, live while it opens, waits
/// and goes back up for someone under it, in the Details panel, its mistakes, its tests, and a
/// graph's States view.
pub fn states_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        dusk_sky(world);
        let from = engine_script::scripts_root(world).join(EXAMPLES);
        copy_dir(&from, &root.join(EXAMPLES));
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        if let Some(e) = crate::shots::find(world, "Lamp") {
            world
                .entity_mut(e)
                .insert(Script::new("examples/pulse.graph"));
        }
        // The gate, made as a user would: an empty object that can be used, with the script.
        editor::spawn_with(
            world,
            editor,
            "Gate".into(),
            vec![
                (
                    "Interactable",
                    json!({"verb": "Raise", "reach_m": 3.0, "offset": [0.0, 1.2, 0.0]}),
                ),
                (
                    "Script",
                    serde_json::to_value(Script::new("examples/gate.luau")).unwrap_or_default(),
                ),
            ],
            Transform::from_translation(GATE_AT),
        );
        // Someone to press E and to stand under it (the gate's `blocker` is "Player").
        let body = MeshRenderer::new(
            Shape::Capsule {
                radius: 0.28,
                height: 1.2,
                segments: 0,
            },
            StandardMaterial::hex(0x3d6fb6),
        );
        editor::spawn_with(
            world,
            editor,
            "Player".into(),
            vec![(
                "MeshRenderer",
                serde_json::to_value(body).unwrap_or_default(),
            )],
            Transform::from_translation(GATE_AT + DVec3::new(0.9, 0.88, 3.0)),
        );
        world.resource_mut::<EditorDock>().0 = tests_layout();
        let mut tab = world.resource_mut::<ScriptsTab>();
        tab.files_next = 0.0;
        tab.tests.shown = false;
    };
    let open_file = |path: &'static str, line: u32| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, path, Some(line));
            });
        }
    };
    // E on `target`, by the player.
    let press = |target: &'static str| {
        move |world: &mut World, _: &mut Editor| {
            // The runes are the seal's own parts, not scene entities: any entity by that name.
            let named = |world: &mut World, name: &str| {
                let mut q = world.query::<(Entity, &Name)>();
                q.iter(world)
                    .find(|(_, n)| n.as_str() == name)
                    .map(|(e, _)| e)
            };
            let (Some(target), Some(actor)) =
                (named(world, target), crate::shots::find(world, "Player"))
            else {
                return;
            };
            let from = world
                .get::<Transform>(actor)
                .map_or(DVec3::ZERO, |t| t.translation);
            world.write_message(engine::world::Interact {
                actor,
                target,
                from,
            });
        }
    };
    let player_to = |at: DVec3| {
        move |world: &mut World, _: &mut Editor| {
            if let Some(e) = crate::shots::find(world, "Player")
                && let Some(mut t) = world.get_mut::<Transform>(e)
            {
                t.translation = at;
            }
        }
    };
    // The pointer onto something drawn (a state: "state closing"), and the right button
    // pressed or let go there.
    let pointer = |what: &'static str, right: Option<bool>| {
        move |world: &mut World, _: &mut Editor| {
            let at = world
                .get_resource::<ScriptsTab>()
                .and_then(|t| t.drawn_at.get(what).map(|r| r.center()));
            if let Some(p) = at {
                let mut input = world.resource_mut::<crate::egui_bridge::EguiInput>();
                input.injected.push(egui::Event::PointerMoved(p));
                if let Some(pressed) = right {
                    input.injected.push(egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
        }
    };
    let gate_breaks = |on: bool| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_mut::<StateBreakpoints>().set(
                "examples/gate.luau",
                "closing",
                false,
                on,
            );
        }
    };
    let mut steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(1.3, 2.1, 6.4), DVec3::new(-0.3, 1.45, -0.5)),
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(open_file("examples/gate.luau", 18))),
        Step::Wait(40),
        Step::Shot("states_01_gate_and_its_states"),
        // Play, and E on the gate: it opens, the lamp blinking amber.
        Step::Select("Gate"),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(6),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Wait(20),
        // Frames take longer than a quarter second here, so the world steps 0.25 s each (its
        // longest step): the waits below are counted in those.
        Step::Do(Box::new(press("Gate"))),
        Step::Wait(4),
        Step::Shot("states_02_opening_live"),
        // Open: its `after hold` timer fills.
        Step::Wait(10),
        Step::Shot("states_03_open_waits_hold"),
        // Closing, someone steps under it: `when blocked` sends it back up.
        Step::Wait(7),
        Step::Do(Box::new(player_to(GATE_AT + DVec3::new(0.2, 0.88, 0.3)))),
        Step::Wait(5),
        Step::Shot("states_04_blocked_goes_back_up"),
        // The selected gate's Details: the same machine, live.
        Step::Do(Box::new(player_to(GATE_AT + DVec3::new(0.9, 0.88, 3.0)))),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<EditorDock>().0 = props_layout();
        })),
        Step::Wait(24),
        Step::Shot("states_05_details_panel"),
        // Stopped, two mistakes: a state name misspelt in code (the type checker underlines
        // it) and a transition to a state that isn't there (saved: the panel says where).
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed);
            world.resource_mut::<EditorDock>().0 = tests_layout();
        })),
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, "examples/gate.luau", Some(24));
                let i = tab.current;
                let doc = &mut tab.docs[i];
                doc.text = doc.saved.replace(
                    r#"closing = { done = "closed","#,
                    r#"closing = { done = "closd","#,
                );
                save(world, &mut tab, 0.0);
                let i = tab.current;
                let doc = &mut tab.docs[i];
                doc.text = doc.saved.replace(
                    r#"self:in_state("opening") or"#,
                    r#"self:in_state("openning") or"#,
                );
                tab.toast = None;
            });
        })),
        Step::Wait(60),
        Step::Do(Box::new(open_file("examples/gate.luau", 30))),
        Step::Wait(10),
        Step::Shot("states_06_mistakes"),
        // Put right, and every example's tests run in the game (the gate's among them).
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, "examples/gate.luau", Some(1));
                let i = tab.current;
                let doc = &mut tab.docs[i];
                doc.text = doc
                    .saved
                    .replace(r#"done = "closd","#, r#"done = "closed","#);
                save(world, &mut tab, 0.0);
                open(world, &mut tab, "examples/gate.test.luau", Some(1));
                tab.tests.run(json!({}));
                tab.toast = None;
            });
        })),
        Step::Wait(8),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Wait(700),
        Step::Shot("states_07_all_tests_pass"),
        // A breakpoint on `closing` (its red dot): Play, E, and the game pauses as the gate
        // starts to close, the panel saying what reached it.
        Step::Do(Box::new(gate_breaks(true))),
        Step::Do(Box::new(open_file("examples/gate.luau", 18))),
        Step::Select("Gate"),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(6),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Wait(4),
        Step::Do(Box::new(press("Gate"))),
        Step::Wait(34),
        Step::Shot("states_09_paused_as_it_starts_closing"),
        // Still paused: a click on an earlier change shows the machine as it was then.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            let Some(gate) = crate::shots::find(world, "Gate") else {
                return;
            };
            let opened = engine_script::states::live(world, gate)
                .and_then(|l| l.log.iter().rev().find(|c| c.to == "opening").cloned());
            if let Some(c) = opened {
                world.resource_mut::<ScriptsTab>().machine.scrub = Some((c.t, c.to));
            }
        })),
        Step::Wait(3),
        Step::Shot("states_10_back_to_an_earlier_change"),
        // Back to now; a right-click on `closing` shows its breakpoints, the one on its start
        // ticked.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<ScriptsTab>().machine.scrub = None;
        })),
        Step::Wait(2),
        Step::Do(Box::new(pointer("state closing", None))),
        Step::Wait(1),
        Step::Do(Box::new(pointer("state closing", Some(true)))),
        Step::Do(Box::new(pointer("state closing", Some(false)))),
        Step::Wait(3),
        Step::Shot("states_11_breakpoint_menu"),
        // Stopped, the breakpoint off. A sentry turret: its aim and its gun are two regions
        // that run at once (`parallel = true`); someone walks up and it turns and fires.
        Step::Do(Box::new(move |world: &mut World, ed: &mut Editor| {
            let mut input = world.resource_mut::<crate::egui_bridge::EguiInput>();
            input.injected.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            input.injected.push(egui::Event::PointerGone);
            gate_breaks(false)(world, ed);
        })),
        Step::Wait(2),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed);
            let turret = Script::new("examples/turret.luau")
                .with_props(json!({"target": "Player", "range": 4.5}));
            editor::spawn_with(
                world,
                ed,
                "Turret".into(),
                vec![("Script", serde_json::to_value(turret).unwrap_or_default())],
                // Right of the gate, facing the camera's side.
                Transform::from_translation(GATE_AT + DVec3::new(2.6, 0.0, 1.6))
                    .with_rotation(engine::core::math::Quat::from_rotation_y(2.6)),
            );
        })),
        Step::Do(Box::new(open_file("examples/turret.luau", 16))),
        Step::Select("Turret"),
        Step::Wait(4),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(6),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Wait(6),
        // Beside the gate, so the turret turns side on to the camera.
        Step::Do(Box::new(player_to(GATE_AT + DVec3::new(0.9, 0.88, 1.5)))),
        Step::Wait(9),
        Step::Shot("states_12_two_regions_at_once"),
        // Stopped. A rune seal in their place: each of its three runes is a region that ends in
        // a final state, and the seal opens once all three have (examples/seal.luau).
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed);
            for name in ["Turret", "Gate"] {
                ed.select(crate::shots::find(world, name));
                editor::delete_selection(world, ed);
            }
            editor::spawn_with(
                world,
                ed,
                "Seal".into(),
                vec![(
                    "Script",
                    serde_json::to_value(Script::new("examples/seal.luau")).unwrap_or_default(),
                )],
                Transform::from_translation(SEAL_AT),
            );
        })),
        Step::Camera(
            SEAL_AT + DVec3::new(1.6, 2.5, 7.6),
            SEAL_AT + DVec3::new(0.0, 1.2, 0.6),
        ),
        Step::Do(Box::new(open_file("examples/seal.luau", 18))),
        Step::Select("Seal"),
        Step::Wait(4),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(6),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world)
        })),
        Step::Do(Box::new(player_to(SEAL_AT + DVec3::new(1.2, 0.88, 3.6)))),
        Step::Wait(4),
        // E on the left rune, then the right: two of three regions done, the seal still shut.
        Step::Do(Box::new(press("Left rune"))),
        Step::Wait(3),
        Step::Do(Box::new(press("Right rune"))),
        Step::Wait(8),
        Step::Shot("states_13_two_runes_two_regions_done"),
        // The middle rune: every region is done, so `sealed`'s done opens it.
        Step::Do(Box::new(press("Middle rune"))),
        Step::Wait(14),
        Step::Shot("states_14_every_region_done_it_opens"),
        // Back to the first rune's change: the values as they were then, and now beside them.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            let Some(e) = crate::shots::find(world, "Seal") else {
                return;
            };
            let first = engine_script::states::live(world, e)
                .and_then(|l| l.log.into_iter().find(|c| c.to == "left_lit"));
            if let Some(c) = first {
                world.resource_mut::<ScriptsTab>().machine.scrub = Some((c.t, c.to));
            }
        })),
        Step::Wait(3),
        Step::Shot("states_15_values_then_and_now"),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<ScriptsTab>().machine.scrub = None;
        })),
    ];
    steps.extend(crate::graphs::states_steps());
    ShotScript::new(dir, steps)
}

/// Where the turret and the patrol drone stand in the gizmos shots (the seal at [`SEAL_AT`]).
const TURRET_AT: DVec3 = DVec3::new(3.9, 0.0, 1.2);
const PATROL_AT: DVec3 = DVec3::new(-5.6, 0.0, 2.4);

/// Screenshots of scripts in the editor's view (`--script gizmos`): the rune seal, a turret and
/// a patrol drone built by their `construct` in edit mode, with their gizmos; prop handles
/// dragged (the turret's range, the seal's spread, a waypoint), the parts building again as
/// they go; then playing, the gizmos live.
pub fn gizmos_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        dusk_sky(world);
        let from = engine_script::scripts_root(world).join(EXAMPLES);
        copy_dir(&from, &root.join(EXAMPLES));
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        let script = |path: &str, props: Value| {
            vec![(
                "Script",
                serde_json::to_value(Script::new(path).with_props(props)).unwrap_or_default(),
            )]
        };
        editor::spawn_with(
            world,
            editor,
            "Rune seal".into(),
            script("examples/seal.luau", json!({})),
            Transform::from_translation(SEAL_AT),
        );
        editor::spawn_with(
            world,
            editor,
            "Turret".into(),
            script(
                "examples/turret.luau",
                json!({"target": "Player", "range": 4.0}),
            ),
            Transform::from_translation(TURRET_AT)
                .with_rotation(engine::core::math::Quat::from_rotation_y(1.2)),
        );
        editor::spawn_with(
            world,
            editor,
            "Patrol".into(),
            script("examples/patrol.luau", json!({})),
            Transform::from_translation(PATROL_AT),
        );
        let body = MeshRenderer::new(
            Shape::Capsule {
                radius: 0.28,
                height: 1.2,
                segments: 0,
            },
            StandardMaterial::hex(0x3d6fb6),
        );
        editor::spawn_with(
            world,
            editor,
            "Player".into(),
            vec![(
                "MeshRenderer",
                serde_json::to_value(body).unwrap_or_default(),
            )],
            Transform::from_translation(DVec3::new(1.6, 0.88, 5.6)),
        );
        world.resource_mut::<EditorDock>().0 = props_layout();
        world.resource_mut::<ScriptsTab>().files_next = 0.0;
    };
    // The pointer onto a point in an entity's own space, the left button pressed or let go
    // there (a drag, as a hand would make it).
    let point = |name: &'static str, local: DVec3, press: Option<bool>| {
        move |world: &mut World, ed: &mut Editor| {
            let (Some(e), Some(view)) = (crate::shots::find(world, name), ed.view) else {
                return;
            };
            let Some(g) = world.get::<GlobalTransform>(e).map(|g| g.0) else {
                return;
            };
            let Some(p) = view.project(g.transform_point3(local)) else {
                return;
            };
            let mut input = world.resource_mut::<crate::egui_bridge::EguiInput>();
            input.injected.push(egui::Event::PointerMoved(p));
            if let Some(pressed) = press {
                input.injected.push(egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
        }
    };
    // A drag of a handle from `from` to `to` (both in the entity's space), stopping before the
    // button is let go so a shot can show it mid-drag.
    let drag = |name: &'static str, from: DVec3, to: DVec3| -> Vec<Step> {
        let mid = from.lerp(to, 0.3);
        vec![
            Step::Do(Box::new(point(name, from, None))),
            Step::Wait(1),
            Step::Do(Box::new(point(name, from, Some(true)))),
            Step::Wait(1),
            Step::Do(Box::new(point(name, mid, None))),
            Step::Wait(1),
            Step::Do(Box::new(point(name, to, None))),
            Step::Wait(3),
        ]
    };
    let open_file = |path: &'static str, line: u32| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<ScriptsTab>| {
                open(world, &mut tab, path, Some(line));
            });
        }
    };
    let mut steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(1.4, 6.4, 13.2), DVec3::new(-0.9, 0.9, -0.2)),
        Step::Select(""),
        Step::Wait(40),
        Step::Shot("gizmos_01_built_in_edit_mode"),
        // The turret selected: its range handle (four dots on the ring) and where it aims.
        Step::Select("Turret"),
        Step::Camera(
            TURRET_AT + DVec3::new(-1.0, 6.0, 7.5),
            TURRET_AT + DVec3::new(-0.6, 0.4, 0.0),
        ),
        Step::Wait(12),
        Step::Shot("gizmos_02_turret_range_handle"),
    ];
    // Its range dragged out from 4 m to 6.5 m.
    steps.extend(drag("Turret", DVec3::X * 4.0, DVec3::X * 6.5));
    steps.extend([
        Step::Shot("gizmos_03_dragging_the_range"),
        Step::Do(Box::new(point("Turret", DVec3::X * 6.5, Some(false)))),
        Step::Wait(2),
        // The seal: where each rune's touch reaches and the socket it lights; its spread
        // dragged wider, the runes rebuilt where it puts them as the drag goes.
        Step::Select("Rune seal"),
        Step::Camera(
            SEAL_AT + DVec3::new(1.2, 5.2, 9.4),
            SEAL_AT + DVec3::new(0.0, 1.0, 0.8),
        ),
        Step::Wait(12),
        Step::Shot("gizmos_04_seal_selected"),
    ]);
    steps.extend(drag("Rune seal", DVec3::X * 2.4, DVec3::X * 3.7));
    steps.extend([
        Step::Shot("gizmos_05_spread_dragged_runes_follow"),
        Step::Do(Box::new(point("Rune seal", DVec3::X * 3.7, Some(false)))),
        Step::Wait(2),
        // The drone's route: a handle on each waypoint; the second one dragged.
        Step::Select("Patrol"),
        Step::Camera(
            PATROL_AT + DVec3::new(3.0, 6.0, 9.0),
            PATROL_AT + DVec3::new(0.0, 1.8, -1.0),
        ),
        Step::Wait(12),
        Step::Shot("gizmos_06_patrol_waypoints"),
    ]);
    steps.extend(drag(
        "Patrol",
        DVec3::new(3.0, 2.6, -1.0),
        DVec3::new(4.2, 2.6, 2.4),
    ));
    steps.extend([
        Step::Shot("gizmos_07_waypoint_dragged"),
        Step::Do(Box::new(point(
            "Patrol",
            DVec3::new(4.2, 2.6, 2.4),
            Some(false),
        ))),
        Step::Wait(2),
        // Playing: the gizmos live (the turret's ring red once it tracks the player, the
        // drone's place on its route), with the code that draws them.
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<EditorDock>().0 = tests_layout();
        })),
        Step::Do(Box::new(open_file("examples/turret.luau", 186))),
        Step::Select("Turret"),
        Step::Camera(DVec3::new(2.4, 7.4, 13.6), DVec3::new(-0.4, 0.8, 0.2)),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Wait(6),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            dusk_sky(world);
            if let Some(e) = crate::shots::find(world, "Player")
                && let Some(mut t) = world.get_mut::<Transform>(e)
            {
                t.translation = TURRET_AT + DVec3::new(-2.6, 0.88, 2.0);
            }
        })),
        Step::Wait(14),
        Step::Shot("gizmos_08_playing_live"),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed)
        })),
        Step::Wait(2),
    ]);
    ShotScript::new(dir, steps)
}

const SCATTER_AT: DVec3 = DVec3::new(-2.6, 0.0, 1.8);
const CRATES_AT: DVec3 = DVec3::new(2.2, 0.0, 3.6);

/// `editor --headless --size 1920x1080 --shots <dir> --script tools`: tool buttons and tool
/// scripts. A stone scatter tuned, reseeded and baked into scene objects by the buttons in its
/// Details panel, a baked stone moved, both undone; then five crates lined up and varied by
/// the Tools menu's tool files.
pub fn tools_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        dusk_sky(world);
        let scripts = engine_script::scripts_root(world);
        copy_dir(&scripts.join(EXAMPLES), &root.join(EXAMPLES));
        copy_dir(
            &scripts.join(engine_script::TOOLS),
            &root.join(engine_script::TOOLS),
        );
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        editor::spawn_with(
            world,
            editor,
            "Stone scatter".into(),
            vec![(
                "Script",
                serde_json::to_value(
                    Script::new("examples/scatter.luau")
                        .with_props(json!({"count": 22, "radius": 3.0})),
                )
                .unwrap_or_default(),
            )],
            Transform::from_translation(SCATTER_AT),
        );
        // Five crates set down anyhow.
        let spots = [
            (0.0, 0.3, 0.2),
            (1.2, -0.5, -0.4),
            (2.9, 0.6, 0.9),
            (3.6, -0.2, -0.3),
            (5.0, 0.4, 0.5),
        ];
        for (i, (x, z, yaw)) in spots.into_iter().enumerate() {
            let crate_box = MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.8, 0.8, 0.8],
                },
                StandardMaterial::hex(0x8a6a44),
            );
            editor::spawn_with(
                world,
                editor,
                format!("Crate {}", i + 1),
                vec![(
                    "MeshRenderer",
                    serde_json::to_value(crate_box).unwrap_or_default(),
                )],
                Transform::from_translation(CRATES_AT + DVec3::new(x, 0.4, z))
                    .with_rotation(engine::core::math::Quat::from_rotation_y(yaw)),
            );
        }
        world.resource_mut::<EditorDock>().0 = tools_layout();
        world.resource_mut::<ScriptsTab>().files_next = 0.0;
    };
    // The pointer onto something the tab drew (a button, the Tools menu), still, so its
    // tooltip shows; with `click`, pressed and let go there.
    let pointer = |what: &'static str, click: bool| {
        move |world: &mut World, _: &mut Editor| {
            let Some(r) = world
                .get_resource::<ScriptsTab>()
                .and_then(|t| t.drawn_at.get(what).copied())
            else {
                return;
            };
            let input = &mut world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected;
            input.push(egui::Event::PointerMoved(r.center()));
            if click {
                for pressed in [true, false] {
                    input.push(egui::Event::PointerButton {
                        pos: r.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    });
                }
            }
        }
    };
    let button = |method: &'static str, label: &'static str| {
        move |world: &mut World, ed: &mut Editor| {
            if let Some(e) = crate::shots::find(world, "Stone scatter") {
                run_tool(
                    world,
                    ed,
                    ToolCall::Button {
                        entity: e,
                        method: method.into(),
                    },
                    &format!("{label} (scatter.luau)"),
                );
            }
        }
    };
    let crates = |world: &mut World, ed: &mut Editor| {
        ed.selection = (1..=5)
            .filter_map(|i| crate::shots::find(world, &format!("Crate {i}")))
            .collect();
    };
    let tool = |file: &'static str, title: &'static str| {
        move |world: &mut World, ed: &mut Editor| {
            run_tool(world, ed, ToolCall::File { path: file.into() }, title);
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Select("Stone scatter"),
        Step::Camera(
            SCATTER_AT + DVec3::new(0.6, 5.2, 7.4),
            SCATTER_AT + DVec3::new(0.0, 0.2, 0.2),
        ),
        Step::Wait(30),
        // Its buttons, under its props; the pointer resting on Bake shows what it does.
        Step::Do(Box::new(pointer("button bake", false))),
        Step::Wait(3),
        Step::Shot("tools_01_scatter_and_its_buttons"),
        // Reseed: another scatter, before Play.
        Step::Do(Box::new(button("reseed", "Reseed"))),
        Step::Wait(4),
        Step::Shot("tools_02_reseeded"),
        // Bake: the stones are scene objects now, listed under it; one step in History.
        Step::Do(Box::new(button("bake", "Bake"))),
        Step::Wait(4),
        Step::Select("Stone 7"),
        Step::Wait(3),
        Step::Shot("tools_03_baked_into_the_scene"),
        // A baked stone is the scene's own: moved like any object.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            let Some(e) = crate::shots::find(world, "Stone 7") else {
                return;
            };
            let Some(before) = world.get::<Transform>(e).copied() else {
                return;
            };
            let mut after = before;
            after.translation += DVec3::new(0.0, 0.9, 1.6);
            editor::apply(
                world,
                ed,
                Command::Component {
                    entity: e,
                    name: "Transform".into(),
                    before: serde_json::to_value(before).ok(),
                    after: serde_json::to_value(after).ok(),
                },
            );
        })),
        Step::Wait(3),
        Step::Shot("tools_04_a_baked_stone_moved"),
        // Ctrl+Z twice: the move, then the whole bake, undone.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::undo(world, ed);
            editor::undo(world, ed);
        })),
        Step::Select("Stone scatter"),
        Step::Wait(4),
        Step::Shot("tools_05_undone_back_to_the_scatter"),
        // The Tools menu, with the crates selected.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world.resource_mut::<EditorDock>().0 = tests_layout();
        })),
        Step::Tab(Tab::Scripts),
        Step::Do(Box::new(crates)),
        Step::Camera(
            CRATES_AT + DVec3::new(2.4, 4.6, 7.0),
            CRATES_AT + DVec3::new(2.4, 0.3, 0.0),
        ),
        Step::Wait(4),
        Step::Do(Box::new(pointer("tools menu", true))),
        Step::Wait(3),
        Step::Shot("tools_06_tools_menu"),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
        })),
        // Escape closed the menu and cleared the selection: the crates again.
        Step::Do(Box::new(crates)),
        Step::Wait(2),
        Step::Do(Box::new(tool("tools/line_up.luau", "Line up"))),
        Step::Wait(4),
        Step::Shot("tools_07_crates_lined_up"),
        Step::Do(Box::new(tool("tools/vary.luau", "Turn and size randomly"))),
        Step::Wait(4),
        Step::Shot("tools_08_turned_and_sized"),
    ];
    ShotScript::new(dir, steps)
}

/// The tools shots' layout: the Scene list and History on the left, the view, Details.
fn tools_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [view, _details] = surface.split_right(NodeIndex::root(), 0.7, vec![Tab::Inspector]);
    let [_list, left] = surface.split_left(view, 0.22, vec![Tab::Outliner]);
    let [_list, _history] = surface.split_below(left, 0.62, vec![Tab::History]);
    dock
}

// ----------------------------------------------------------------------------- rewind shots

const REWIND_PATROL_AT: DVec3 = DVec3::new(0.0, 0.0, 1.0);
const REWIND_TURRET_AT: DVec3 = DVec3::new(4.4, 0.0, 2.2);
const REWIND_SPOT_AT: DVec3 = DVec3::new(-3.8, 0.0, -0.8);

/// The rewind shots' spotlight: it turns on the drone while the drone is near, and has a bug
/// for the Rewind tab to find (it forgets its target a line before it says which it lost).
const SPOTLIGHT: &str = r#"-- A spotlight that lights the drone while the drone is near. It has a bug to find with the
-- Rewind tab: it forgets its target a line before it says which one it lost.
local Spot = {}
Spot.props = { target = "Drone", range = 3 }

local POST = { base_color = { 0.08, 0.085, 0.09, 1 }, metallic = 0.8, perceptual_roughness = 0.4 }
local LAMP = { base_color = { 0.05, 0.05, 0.04, 1 }, emissive = { 1, 0.85, 0.4 }, emissive_strength = 25 }

function Spot:construct()
	self.entity:spawn({
		name = "Post",
		components = {
			Transform = { translation = vec3(0, 0.75, 0) },
			MeshRenderer = { mesh = { shape = { Cylinder = { radius = 0.07, height = 1.5, segments = 16 } } }, material = { inline = POST } },
		},
	})
	self.lamp = self.entity:spawn({
		name = "Lamp",
		components = {
			Transform = { translation = vec3(0, 1.6, 0) },
			MeshRenderer = { mesh = { shape = { Sphere = { radius = 0.16, segments = 16 } } }, material = { inline = LAMP } },
			PointLight = { color = { 1, 0.85, 0.4 }, intensity_lm = 300, range = 5, radius = 0.05 },
		},
	})
end

function Spot:start()
	self.target = nil
	self.lost = 0
end

function Spot:update(dt)
	local who = world.find(self.props.target)
	local near = who ~= nil and (who.position - self.entity.position):flat():length() < self.props.range
	if near and not self.target then
		self.target = who
		print(`Spotlight: found {who.name}`)
	elseif not near and self.target then
		self.target = nil
		self.lost += 1
		print(`Spotlight: lost {self.target.name}`)
	end
	if self.target then
		draw.line(self.lamp.position, self.target.position, { color = "yellow", on_top = true })
	end
end

function Spot:draw_gizmos()
	draw.circle(vec3(0, 0.05, 0), self.props.range, { color = if self.target then "yellow" else "gray", alpha = 0.7 })
end

return Spot
"#;

/// The Rewind tab: a patrol drone, a turret watching it and a spotlight with a bug play for
/// nine seconds while everything is recorded; then scrubbing back (the game pauses and the
/// drone and the turret's head go back to where they were, with the drone's path), the
/// turret's values, the jump to the spotlight's error and the frame before it, and the
/// recording after Stop. `editor --headless --size 1920x1080 --shots <dir> --script rewind`.
pub fn rewind_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        dusk_sky(world);
        let scripts = engine_script::scripts_root(world);
        copy_dir(&scripts.join(EXAMPLES), &root.join(EXAMPLES));
        let _ = std::fs::write(root.join(EXAMPLES).join("spotlight.luau"), SPOTLIGHT);
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        let script = |path: &str, props: Value| {
            vec![(
                "Script",
                serde_json::to_value(Script::new(path).with_props(props)).unwrap_or_default(),
            )]
        };
        editor::spawn_with(
            world,
            editor,
            "Patrol drone".into(),
            script("examples/patrol.luau", json!({})),
            Transform::from_translation(REWIND_PATROL_AT),
        );
        editor::spawn_with(
            world,
            editor,
            "Turret".into(),
            script(
                "examples/turret.luau",
                json!({"target": "Drone", "range": 4.5, "sweep": 60}),
            ),
            Transform::from_translation(REWIND_TURRET_AT)
                .with_rotation(engine::core::math::Quat::from_rotation_y(1.35)),
        );
        editor::spawn_with(
            world,
            editor,
            "Spotlight".into(),
            script("examples/spotlight.luau", json!({})),
            Transform::from_translation(REWIND_SPOT_AT),
        );
        world.resource_mut::<EditorDock>().0 = rewind_layout();
        world.resource_mut::<ScriptsTab>().files_next = 0.0;
    };
    let open_track = |name: &'static str| {
        move |world: &mut World, _: &mut Editor| {
            if let Some(e) = crate::shots::find(world, name) {
                world
                    .resource_mut::<crate::rewind::RewindTab>()
                    .open
                    .insert(e);
            }
        }
    };
    // The pointer resting on something the Rewind tab drew; with `click`, pressed there.
    let pointer = |what: String, click: bool, along: f32| {
        move |world: &mut World, _: &mut Editor| {
            let Some(r) = world
                .get_resource::<crate::rewind::RewindTab>()
                .and_then(|t| t.drawn_at.get(&what).copied())
            else {
                return;
            };
            let at = Pos2::new(r.left() + r.width() * along, r.center().y);
            let input = &mut world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected;
            input.push(egui::Event::PointerMoved(at));
            if click {
                for pressed in [true, false] {
                    input.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    });
                }
            }
        }
    };
    let since_start = |secs: f64| {
        move |world: &mut World, ed: &mut Editor| {
            if let Some((from, _)) = world.resource::<engine_script::ScriptRewind>().span() {
                crate::rewind::show_at(world, ed, from + secs);
            }
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Select(""),
        Step::Camera(DVec3::new(0.4, 4.2, 7.0), DVec3::new(0.3, 1.0, -0.2)),
        Step::Wait(24),
        // Play: every script recorded as it runs (the Live playhead at the right).
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed);
        })),
        Step::Wait(2),
        Step::Do(Box::new(open_track("Patrol drone"))),
        Step::Wait(36),
        Step::Shot("rewind_01_recording_while_playing"),
        // A click back on the timeline: paused there, the drone and the turret's head where
        // they were, the drone's path, the gizmos as they were.
        Step::Do(Box::new(since_start(3.6))),
        Step::Wait(4),
        Step::Shot("rewind_02_scrubbed_back"),
        // The turret's values, its head's turn (yaw) as a curve under the pointer.
        Step::Select("Turret"),
        Step::Do(Box::new(open_track("Turret"))),
        Step::Wait(3),
        Step::Do(Box::new(pointer("value Turret yaw".into(), false, 0.55))),
        Step::Wait(3),
        Step::Shot("rewind_03_turret_values"),
        // The spotlight's error: the button jumps to it.
        Step::Select("Spotlight"),
        Step::Do(Box::new(open_track("Spotlight"))),
        Step::Wait(3),
        Step::Do(Box::new(pointer("errors".into(), true, 0.5))),
        Step::Wait(4),
        Step::Shot("rewind_04_at_the_error"),
        // A frame before: the target still there; it was cleared before the print read it.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            crate::rewind::step(world, ed, -1);
        })),
        Step::Wait(4),
        Step::Shot("rewind_05_a_frame_before"),
        // Stop: the scene is back, the recording stays to look through.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::stop(world, ed);
        })),
        Step::Select(""),
        Step::Wait(6),
        Step::Do(Box::new(since_start(5.4))),
        Step::Wait(4),
        Step::Shot("rewind_06_after_stop"),
    ];
    ShotScript::new(dir, steps)
}

/// The rewind shots' layout: the scene view over the Rewind tab, full width.
fn rewind_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [_view, _rewind] = surface.split_below(NodeIndex::root(), 0.52, vec![Tab::Rewind]);
    dock
}

// ----------------------------------------------------------------------------- tween shots

const TWEEN_BRIDGE_AT: DVec3 = DVec3::new(-2.6, 0.0, 0.4);
const TWEEN_RACE_AT: DVec3 = DVec3::new(1.4, 0.0, -6.2);

/// The Curves tab: a drawbridge on a timeline and the ease race play; the game paused part way
/// down shows the bridge's curve with the timeline's playhead and events, the landing key's
/// broken tangent, a key dragged (the bridge follows at once), a key's menu, the ease gallery
/// and the tweens running. `editor --headless --size 1920x1080 --shots <dir> --script tweens`.
pub fn tweens_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        editor.icons = false;
        dusk_sky(world);
        let scripts = engine_script::scripts_root(world);
        copy_dir(&scripts.join(EXAMPLES), &root.join(EXAMPLES));
        copy_dir(&scripts.join("curves"), &root.join("curves"));
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        let script = |path: &str, props: Value| {
            (
                "Script",
                serde_json::to_value(Script::new(path).with_props(props)).unwrap_or_default(),
            )
        };
        editor::spawn_with(
            world,
            editor,
            "Drawbridge".into(),
            vec![script(
                "examples/drawbridge.luau",
                json!({"span": 4.2, "width": 2.4}),
            )],
            Transform::from_translation(TWEEN_BRIDGE_AT).with_rotation(
                engine::core::math::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ),
        );
        editor::spawn_with(
            world,
            editor,
            "Ease race".into(),
            vec![script(
                "examples/ease_race.luau",
                json!({"length": 4.5, "secs": 1.4}),
            )],
            Transform::from_translation(TWEEN_RACE_AT),
        );
        world.resource_mut::<EditorDock>().0 = curves_layout();
        world.resource_mut::<ScriptsTab>().files_next = 0.0;
    };
    let eval = |code: &'static str| {
        move |world: &mut World, _: &mut Editor| {
            if let Err(e) = call_api(world, "script.eval", json!({ "code": code })) {
                engine::core::log::warn!("tween shots: {code}: {e}");
            }
        }
    };
    let curves = |f: fn(&mut crate::curves::CurvesTab)| {
        move |world: &mut World, _: &mut Editor| {
            f(&mut world.resource_mut::<crate::curves::CurvesTab>());
        }
    };
    // The pointer on something the Curves tab drew; with `button`, clicked there.
    let pointer = |what: &'static str, button: Option<egui::PointerButton>| {
        move |world: &mut World, _: &mut Editor| {
            let Some(r) = world
                .get_resource::<crate::curves::CurvesTab>()
                .and_then(|t| t.drawn_at.get(what).copied())
            else {
                return;
            };
            let at = r.center();
            let input = &mut world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected;
            input.push(egui::Event::PointerMoved(at));
            if let Some(button) = button {
                for pressed in [true, false] {
                    input.push(egui::Event::PointerButton {
                        pos: at,
                        button,
                        pressed,
                        modifiers: Modifiers::NONE,
                    });
                }
            }
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Select(""),
        Step::Camera(DVec3::new(2.2, 5.6, 8.4), DVec3::new(1.6, 0.9, -2.0)),
        Step::Wait(24),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed);
        })),
        Step::Wait(20),
        // Lowering, paused 1.15 s into its timeline: the deck part way down, the lamps lit.
        Step::Do(Box::new(eval(
            "local b = world.find('Drawbridge').script b:raise_now() b:toggle()",
        ))),
        Step::Wait(6),
        Step::Do(Box::new(eval(
            "world.find('Drawbridge').script:timeline('lower'):seek(1.15)",
        ))),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::pause(world, ed);
        })),
        Step::Select("Drawbridge"),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            if let Some(e) = crate::shots::find(world, "Drawbridge") {
                crate::curves::show_track(world, e, "lower", "swing");
            }
        })),
        Step::Wait(6),
        Step::Shot("tweens_01_timeline_on_its_curve"),
        // The landing key: its two tangents set apart (fast in, a bounce out).
        Step::Do(Box::new(curves(|t| t.picked = Some(2)))),
        Step::Wait(3),
        Step::Shot("tweens_02_landing_key_tangents"),
        // The second key raised: the deck starts faster, and the paused bridge moves with it.
        Step::Do(Box::new(curves(|t| {
            t.picked = Some(1);
            t.edit(|c| c.keys[1].v = 0.46);
        }))),
        Step::Wait(3),
        Step::Do(Box::new(eval(
            "world.find('Drawbridge').script:timeline('lower'):seek(1.15)",
        ))),
        Step::Wait(4),
        Step::Shot("tweens_03_key_dragged_live"),
        // A key's menu: how the curve leaves it, and its tangent.
        Step::Do(Box::new(pointer(
            "key 2",
            Some(egui::PointerButton::Secondary),
        ))),
        Step::Wait(3),
        Step::Shot("tweens_04_key_menu"),
        Step::Do(Box::new(pointer(
            "canvas",
            Some(egui::PointerButton::Primary),
        ))),
        Step::Wait(2),
        // Every named ease, moving.
        Step::Do(Box::new(curves(|t| t.view = crate::curves::View::Eases))),
        Step::Wait(8),
        Step::Shot("tweens_05_ease_gallery"),
        // Going on: the runners race, the orb springs after the ring, the list shows each.
        Step::Do(Box::new(curves(|t| t.view = crate::curves::View::Curve))),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed);
        })),
        Step::Select("Ease race"),
        Step::Wait(14),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::pause(world, ed);
        })),
        Step::Wait(3),
        Step::Shot("tweens_06_tweens_running"),
        // The winch's keys are in the script: shown, with Save as file.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            if let Some(e) = crate::shots::find(world, "Drawbridge") {
                crate::curves::show_track(world, e, "lower", "winch");
            }
        })),
        Step::Wait(4),
        Step::Shot("tweens_07_keys_in_the_script"),
    ];
    ShotScript::new(dir, steps)
}

/// Where the cutscene shots' drawbridge stands (as in the showcase's script courtyard).
const CINEMATIC_BRIDGE_AT: DVec3 = DVec3::new(-2.69, 0.0, 10.05);
const CINEMATIC_PATH: &str = "cutscenes/courtyard.cutscene";

/// `--script cutscenes`: the courtyard cutscene in the Sequencer, posed as it is scrubbed with
/// motion trails and the cut camera in the corner, axes unfolded, auto key, the add-track
/// menu, an axis in the Curves tab, and playing in the game.
pub fn cutscenes_shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = move |world: &mut World, editor: &mut Editor| {
        dusk_sky(world);
        let scripts = engine_script::scripts_root(world);
        for folder in [EXAMPLES, "curves", "cutscenes"] {
            copy_dir(&scripts.join(folder), &root.join(folder));
        }
        world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
        world.remove_resource::<ScriptRuntime>();
        editor::spawn_with(
            world,
            editor,
            "Drawbridge".into(),
            vec![(
                "Script",
                serde_json::to_value(
                    Script::new("examples/drawbridge.luau")
                        .with_props(json!({"span": 3.4, "width": 2.2})),
                )
                .unwrap_or_default(),
            )],
            Transform::from_translation(CINEMATIC_BRIDGE_AT)
                .with_rotation(engine::core::math::Quat::from_rotation_y(2.88)),
        );
        let orb = MeshRenderer::new(
            Shape::Sphere {
                radius: 0.24,
                segments: 24,
            },
            StandardMaterial::color(0.3, 0.22, 0.1).with_emissive([1.0, 0.72, 0.36], 2.0),
        )
        .without_shadows();
        editor::spawn_with(
            world,
            editor,
            "Herald".into(),
            vec![
                (
                    "MeshRenderer",
                    serde_json::to_value(orb).unwrap_or_default(),
                ),
                (
                    "PointLight",
                    json!({"color": [1.0, 0.72, 0.42], "intensity_lm": 0.0, "range": 9.0, "radius": 0.24}),
                ),
            ],
            Transform::from_xyz(0.0, 4.6, 0.0),
        );
        for (name, eye, target, fov) in [
            ("Cam wide", [9.5, 8.6, 13.5], [0.0, 4.6, 0.0], 50.0),
            ("Cam herald", [2.48, 4.84, 1.94], [-0.48, 4.18, 2.72], 44.0),
            ("Cam bridge", [-6.7, 1.05, 5.3], [-2.3, 1.2, 8.0], 40.0),
        ] {
            let camera = Camera {
                active: false,
                ..Camera::perspective(fov)
            };
            editor::spawn_with(
                world,
                editor,
                name.into(),
                vec![("Camera", serde_json::to_value(camera).unwrap_or_default())],
                Transform::from_xyz(eye[0], eye[1], eye[2])
                    .looking_at(DVec3::new(target[0], target[1], target[2]), Vec3::Y),
            );
        }
        editor::spawn_with(
            world,
            editor,
            "Courtyard cinematic".into(),
            vec![(
                "CutscenePlayer",
                json!({"path": "cutscenes/courtyard", "autoplay": false}),
            )],
            Transform::default(),
        );
        world.resource_mut::<EditorDock>().0 = sequencer_layout();
        crate::sequencer::open(world, CINEMATIC_PATH);
    };
    let seq = |f: fn(&mut crate::sequencer::SequencerTab)| {
        move |world: &mut World, _: &mut Editor| {
            f(&mut world.resource_mut::<crate::sequencer::SequencerTab>());
        }
    };
    // The index of the Herald's position track.
    let herald_position = |world: &World| {
        world
            .get_resource::<engine_script::CutsceneFiles>()
            .and_then(|f| f.loaded(CINEMATIC_PATH))
            .and_then(|c| {
                c.tracks
                    .iter()
                    .position(|t| t.bind() == "Herald" && t.label() == "position")
            })
    };
    // The pointer clicked on something the Sequencer drew.
    let click = |what: &'static str| {
        move |world: &mut World, _: &mut Editor| {
            let Some(r) = world
                .get_resource::<crate::sequencer::SequencerTab>()
                .and_then(|t| t.drawn_at.get(what).copied())
            else {
                return;
            };
            let at = r.center();
            let input = &mut world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected;
            input.push(egui::Event::PointerMoved(at));
            for pressed in [true, false] {
                input.push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
        }
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Tool(crate::editor::Tool::Select),
        Step::Select("Herald"),
        Step::Camera(DVec3::new(13.5, 7.4, 7.5), DVec3::new(-1.4, 2.4, 5.2)),
        Step::Wait(20),
        // 2.5 s in: the herald risen and lit, the wide camera in the corner, every object's
        // path drawn through the scene.
        Step::Do(Box::new(seq(|t| t.seek(2.5)))),
        Step::Wait(8),
        Step::Shot("cutscenes_01_sequencer"),
        // 5 s, in the blend to the herald's camera: its position unfolded into axes, the keys
        // at 5 s picked.
        Step::Do(Box::new(move |world: &mut World, _: &mut Editor| {
            let Some(i) = herald_position(world) else {
                return;
            };
            let mut tab = world.resource_mut::<crate::sequencer::SequencerTab>();
            tab.unfolded.insert(i);
            tab.selected = vec![crate::sequencer::Sel {
                track: i,
                axis: None,
                t: 5.0,
            }];
            tab.seek(5.0);
        })),
        Step::Wait(6),
        Step::Shot("cutscenes_02_axes_unfolded"),
        // Auto key at 7 s: the herald lifted 0.9 m keys only its height; the trail bends.
        Step::Do(Box::new(seq(|t| {
            t.auto_key = true;
            t.selected.clear();
            t.seek(7.0);
        }))),
        Step::Wait(4),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            if let Some(e) = crate::shots::find(world, "Herald")
                && let Some(mut t) = world.get_mut::<Transform>(e)
            {
                t.translation.y += 0.9;
            }
        })),
        Step::Wait(6),
        Step::Shot("cutscenes_03_auto_key"),
        // ➕ Track for the selection: its transform, its numbers, an event, a cut.
        Step::Do(Box::new(seq(|t| t.auto_key = false))),
        Step::Do(Box::new(click("add track"))),
        Step::Wait(4),
        Step::Shot("cutscenes_04_add_track"),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            world
                .resource_mut::<crate::egui_bridge::EguiInput>()
                .injected
                .push(egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
        })),
        Step::Wait(2),
        // The height axis in the Curves tab (a double-click on its row).
        Step::Do(Box::new(move |world: &mut World, _: &mut Editor| {
            if let Some(i) = herald_position(world) {
                crate::curves::show_channel(world, CINEMATIC_PATH, i, "y");
            }
        })),
        Step::Wait(6),
        Step::Shot("cutscenes_05_axis_in_curves"),
        // Playing in the game: the drawbridge lowers on its event, the corner shows the
        // cutscene's camera, the playhead follows.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            crate::sequencer::open(world, CINEMATIC_PATH);
            editor::play(world, ed);
        })),
        Step::Wait(4),
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| {
            let args =
                json!({"path": "cutscenes/courtyard", "play": true, "at": "Courtyard cinematic"});
            if let Err(e) = call_api(world, "script.cutscene", args) {
                engine::core::log::warn!("cutscene shots: {e}");
            }
        })),
        Step::Wait(27),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::pause(world, ed);
        })),
        Step::Wait(3),
        Step::Shot("cutscenes_06_playing"),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed);
        })),
        Step::Wait(10),
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::pause(world, ed);
        })),
        Step::Wait(3),
        Step::Shot("cutscenes_07_bridge_cut"),
    ];
    ShotScript::new(dir, steps)
}

/// The Sequencer shots' layout: the scene view over the Sequencer (the Curves tab behind it).
fn sequencer_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [_view, _seq] =
        surface.split_below(NodeIndex::root(), 0.5, vec![Tab::Sequencer, Tab::Curves]);
    dock
}

/// The Curves shots' layout: the scene view over the Curves tab, full width.
fn curves_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [_view, _curves] = surface.split_below(NodeIndex::root(), 0.46, vec![Tab::Curves]);
    dock
}

/// The evening sky, held (Play starts the sky from the scene's time).
fn dusk_sky(world: &mut World) {
    let sky = world
        .resource::<TypeRegistry>()
        .resource("Sky")
        .map(|r| r.patch);
    if let Some(patch) = sky {
        let _ = patch(world, json!({"hour": 19.6, "hours_per_second": 0.0}));
    }
}

/// The tests shots' layout: the Scripts tab wide, the game beside it, Details under that.
fn tests_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Scripts]);
    let surface = dock.main_surface_mut();
    let [_scripts, right] = surface.split_right(NodeIndex::root(), 0.62, vec![Tab::Viewport]);
    let [_view, _details] = surface.split_below(right, 0.62, vec![Tab::Inspector]);
    dock
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

    /// A tool's changes to the scene (an entity moved, one made under it and one at the root,
    /// one removed) are adopted into the scene and undone and redone as one step; what a
    /// script's `construct` built stays out of both.
    #[test]
    fn a_tools_changes_are_one_undo_step() {
        use crate::scene_ops::InScene;
        let mut app = App::new();
        app.update();
        let w = &mut app.world;
        let named = |w: &mut World, name: &str| {
            let mut q = w.query_filtered::<(Entity, &Name), With<InScene>>();
            q.iter(w).find(|(_, n)| n.as_str() == name).map(|(e, _)| e)
        };
        let a = w
            .spawn((
                InScene,
                SceneOrder::next(),
                Name::new("A"),
                Transform::default(),
            ))
            .id();
        let gone = w
            .spawn((
                InScene,
                SceneOrder::next(),
                Name::new("Gone"),
                Transform::from_xyz(0.0, 2.0, 0.0),
            ))
            .id();
        let before = scene_before(w);
        w.get_mut::<Transform>(a).unwrap().translation = DVec3::X;
        let kid = w
            .spawn((Name::new("Kid"), Transform::default(), ChildOf(a)))
            .id();
        w.entity_mut(gone).despawn();
        let root = w.spawn((Name::new("New"), Transform::default())).id();
        let part = w
            .spawn((
                Name::new("Part"),
                Transform::default(),
                ChildOf(a),
                engine_script::Constructed { by: a },
            ))
            .id();
        adopt_made(w, &before);
        assert!(w.get::<InScene>(kid).is_some() && w.get::<InScene>(root).is_some());
        assert!(
            w.get::<InScene>(part).is_none(),
            "built parts stay the script's"
        );
        let commands = scene_changes(w, &before);
        assert_eq!(commands.len(), 4, "{commands:?}");
        let mut history = crate::undo::History::default();
        history.record(w, Command::Group("Tool".into(), commands), 0.0);

        history.undo(w).unwrap();
        assert_eq!(w.get::<Transform>(a).unwrap().translation, DVec3::ZERO);
        assert!(w.get_entity(kid).is_err() && w.get_entity(root).is_err());
        let back = named(w, "Gone").expect("put back");
        assert_eq!(
            w.get::<Transform>(back).unwrap().translation,
            DVec3::new(0.0, 2.0, 0.0)
        );

        history.redo(w).unwrap();
        assert_eq!(w.get::<Transform>(a).unwrap().translation, DVec3::X);
        assert!(named(w, "Gone").is_none());
        assert!(named(w, "New").is_some());
        let kid = named(w, "Kid").expect("made again");
        assert_eq!(w.get::<ChildOf>(kid).map(|p| p.parent()), Some(a));
    }

    #[test]
    fn test_files_mark_their_groups_and_tests() {
        let text = "local x = 1\ndescribe(\"Lamp\", function()\n\tit(\"lights\", function(t)\n\
                    \tend)\n\tit.skip('later', function(t) end)\nend)\nit(\"alone\", function() end)\n";
        let lines = test_lines(text);
        let at: Vec<(u32, bool, u32)> = lines.iter().map(|t| (t.line, t.describe, t.end)).collect();
        assert_eq!(
            at,
            vec![(2, true, 6), (3, false, 3), (5, false, 5), (7, false, 7)]
        );
        let tests = TestsPanel {
            last: json!({"results": [
                {"file": "a.test.luau", "line": 3, "status": "passed", "secs": 0.1},
                {"file": "a.test.luau", "line": 5, "status": "failed", "failed_at": 4,
                 "message": "expected 1, got 2", "secs": 0.2},
            ]}),
            ..Default::default()
        };
        let status = |n: usize| line_status(&tests, "a.test.luau", &lines[n]);
        assert_eq!(
            status(0).as_deref(),
            Some("failed"),
            "a group shows its worst"
        );
        assert_eq!(status(1).as_deref(), Some("passed"));
        assert_eq!(status(3), None, "not run");
        assert_eq!(tests.counts("a.test.luau"), Some((1, 1)));
        let notes = test_notes(&tests, "a.test.luau");
        assert!(
            notes
                .iter()
                .any(|(l, t, _)| *l == 4 && t == "✖ expected 1, got 2"),
            "{notes:?}"
        );
        let listed: Vec<String> = functions(text).into_iter().map(|f| f.label).collect();
        assert_eq!(
            listed,
            [
                "describe Lamp",
                "  it lights",
                "  it.skip later",
                "it alone"
            ]
        );
    }

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
    fn the_checker_is_asked_where_the_word_list_knows_nothing() {
        let v = vocab();
        let at = |text: &str| typed_member_at(text, text.chars().count(), &v);
        // `self.props.` and a local's members: only the checker knows them.
        assert_eq!(at("\tlocal q = self.props.lu"), Some(22));
        assert_eq!(at("\tp.pulse."), Some(9));
        assert!(at("\ttask.w").is_none(), "the word list has task");
        assert!(at("-- p.pulse.").is_none() && at("print(\"p.").is_none());
        assert!(
            at("function Lamp:on_i").is_none(),
            "hooks come from the word list"
        );
        assert_eq!(
            receiver_text("\tlocal q = self.props.lu", 22),
            "self.props."
        );
        // A call's parameters: inside its parentheses, not in a string or after it closed.
        let caret = |t: &str| t.chars().count();
        assert!(in_call(
            "quat.axis_angle(vec3.up, ",
            caret("quat.axis_angle(vec3.up, ")
        ));
        assert!(!in_call("f(a)", caret("f(a)")));
        assert!(!in_call("print(\"(", caret("print(\"(")));
        // The word under the pointer, and positions both ways.
        assert_eq!(word_cols("\tself.home + x", 0, 8), Some((6, 10)));
        assert_eq!(word_cols("x = 12", 0, 5), None);
        let text = "ab\ncdé\nf";
        assert_eq!(char_at(text, 1, 2), 5);
        assert_eq!(char_at(text, 1, 99), 6, "clamped to the line");
        assert_eq!(line_col(text, 7), (2, 0));
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
