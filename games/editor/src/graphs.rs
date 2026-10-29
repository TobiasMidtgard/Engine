//! The Graphs tab: visual scripts, after Unreal's Blueprint editor and Unity's Visual Scripting
//! window. A `.graph` file is a script made of nodes and wires ([`engine_script::graph`]):
//! events where it starts (Start, Update, Interact, a message, Every so many seconds), flow
//! (Branch, Sequence, Delay, Do once, Flip flop, Gate, loops, Tween), the entity, variables and
//! props, math, and one node for each engine function and component the game has. It compiles
//! to Luau, so it runs, hot reloads and is profiled like any script; the code it runs as shows
//! beside the canvas under the node each line came from, and Convert to script turns it into a
//! `.luau` file to carry on in text.
//!
//! Mistakes show on their node before anything runs (Unity's predictive debugging): an unwired
//! condition, a value used where it doesn't exist yet, a loop with no Delay, a node nothing
//! runs. While playing, the graph is watched without stopping the game: the nodes that run
//! light up with pulses along their execution wires, each data wire shows the value it carries
//! (Blueprints' debugger), and a runtime error marks the node it came from. Saving (Ctrl+S)
//! hot reloads the running graph, which keeps its state.
//!
//! Breakpoints go on nodes, as Blueprints': F9 or a node's right-click menu puts a red octagon
//! on it, and when the graph gets there it waits (the other scripts and the world's time with
//! it) with the node outlined in yellow under an arrow and the locals' values over the canvas;
//! Next node (F10) runs on to the next node, Continue (F5) goes on.
//!
//! On the canvas: right-click or Space adds a node (a search of everything there is); drag from
//! a pin to wire it, or into empty space to add a node already wired; drag the background with
//! the right or middle button to pan, the wheel zooms, and a left drag selects a box. Delete,
//! Ctrl+D duplicates, Ctrl+C and Ctrl+V copy and paste (as text), Ctrl+Z and Ctrl+Y undo and
//! redo, F frames the graph, C boxes the selection in a comment, Alt+click breaks a pin's
//! wires.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use egui::emath::TSTransform;
use egui::{
    Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use egui_dock::{DockState, NodeIndex};
use engine::core::registry::call_api;
use engine::prelude::*;
use engine_script::debug::{self, Go};
use engine_script::editing::{self, FileMarks};
use engine_script::graph::{
    self, Comment, Compiled, Graph, Link, Live, Node, NodeSpec, PinType, Problem, look,
};
use engine_script::{Script, ScriptRuntime, ScriptSettings};
use serde_json::{Value, json};

use crate::editor::{self, Editor, Mode, Tab};
use crate::shots::{ShotScript, Step};
use crate::theme;
use crate::ui::EditorDock;

/// The Graphs tab's state.
pub struct GraphsTabPlugin;

impl Plugin for GraphsTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphsTab>();
    }
}

// ----------------------------------------------------------------------------- state

const CANVAS_BG: Color32 = Color32::from_rgb(0x15, 0x16, 0x19);
const GRID: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x23);
const GRID_MAJOR: Color32 = Color32::from_rgb(0x26, 0x28, 0x2e);
const NODE_BG: Color32 = Color32::from_rgb(0x1f, 0x21, 0x26);
const NODE_SELECTED: Color32 = Color32::from_rgb(0xff, 0xa8, 0x2e);
const FIELD_BG: Color32 = Color32::from_rgb(0x12, 0x13, 0x16);
const ERROR: Color32 = Color32::from_rgb(0xff, 0x5c, 0x56);
const WARN: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x3c);
const RAN: Color32 = Color32::from_rgb(0xff, 0xd0, 0x5a);
const VALUE: Color32 = Color32::from_rgb(0xff, 0xd6, 0x7a);
const VALUE_BG: Color32 = Color32::from_rgb(0x2e, 0x22, 0x0a);
const CODE_BG: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
const LINE_NO: Color32 = Color32::from_rgb(0x52, 0x57, 0x60);
const CODE_PICKED: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3a);
/// A node breakpoint's octagon, the node the graph waits at, and its values.
const BREAKPOINT: Color32 = Color32::from_rgb(0xe5, 0x3e, 0x3e);
const STOPPED: Color32 = Color32::from_rgb(0xff, 0xd8, 0x4c);
const STOP_STRIP: Color32 = Color32::from_rgb(0x3a, 0x31, 0x0e);
const STOP_VALUE: Color32 = Color32::from_rgb(0x9c, 0xdc, 0xfe);
/// Line steps "Next node" takes at most before it stops anyway.
const NODE_STEPS: u32 = 400;
/// Seconds a node stays lit after it ran.
const LIT_SECS: f64 = 0.6;
/// How often the runtime is asked what the graph did.
const POLL_SECS: f64 = 0.15;
const FILES_SECS: f64 = 1.0;
const FONT: f32 = 13.0;

/// The tab's state (a resource of its own, beside the editor's).
#[derive(Resource)]
pub struct GraphsTab {
    pub docs: Vec<GraphDoc>,
    pub current: usize,
    files: Vec<FileRow>,
    files_next: f64,
    /// Every node there is, without the open graph's own (built when first needed).
    library: Option<Vec<NodeSpec>>,
    /// The node search, while open.
    pub palette: Option<Palette>,
    /// Show the Luau the graph runs as.
    pub show_code: bool,
    /// Watch the graph while playing (lit nodes, wire values).
    pub live: bool,
    toast: Option<(String, f64, bool)>,
    playing: bool,
    new_name: Option<String>,
    /// Copied nodes, for Ctrl+V.
    clipboard: Option<Graph>,
    vocab: Option<crate::scripts::Vocab>,
    /// Next node: stepping lines until the graph is at another node (from this node, steps
    /// left).
    stepping: Option<(Option<u32>, u32)>,
}

impl Default for GraphsTab {
    fn default() -> Self {
        Self {
            docs: Vec::new(),
            current: 0,
            files: Vec::new(),
            files_next: 0.0,
            library: None,
            palette: None,
            show_code: true,
            live: true,
            toast: None,
            playing: false,
            new_name: None,
            clipboard: None,
            vocab: None,
            stepping: None,
        }
    }
}

/// An open graph.
pub struct GraphDoc {
    /// Relative to the scripts folder: `examples/pulse.graph`.
    pub path: String,
    pub graph: Graph,
    /// The graph as last read from or written to disk.
    saved: Graph,
    stamp: Option<std::time::SystemTime>,
    undo: Vec<Graph>,
    redo: Vec<Graph>,
    /// An edit that goes on over several frames (a drag, typing) makes one undo step.
    gesture: bool,
    pub selected: BTreeSet<u32>,
    /// The canvas's view (graph units), for `egui::Scene`.
    view: Rect,
    /// Frame every node next frame.
    pub frame_all: bool,
    /// Center the view on this point (graph units) at this zoom next frame.
    pub zoom_on: Option<(Pos2, f32)>,
    /// Where the canvas was on screen last frame.
    canvas: Rect,
    compiled: Compiled,
    compiled_of: Option<Graph>,
    marks: FileMarks,
    marks_next: f64,
    live: Live,
    /// When each node last ran (real seconds).
    ran: HashMap<u32, f64>,
    /// The loaded version the logpoints went on.
    watched: Option<u32>,
    /// A wire being drawn from a pin.
    wiring: Option<Wiring>,
    /// The corner a box selection started at (graph units).
    boxing: Option<Pos2>,
    /// A comment whose text is being edited.
    editing_comment: Option<usize>,
    /// Nodes with a breakpoint, and the lines last set for them in the running scripts.
    pub breakpoints: BTreeSet<u32>,
    applied: Option<Vec<u32>>,
}

impl GraphDoc {
    fn dirty(&self) -> bool {
        self.graph != self.saved
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    fn compiled(&mut self) -> &Compiled {
        if self.compiled_of.as_ref() != Some(&self.graph) {
            self.compiled = graph::compile(&self.graph, &self.path);
            self.compiled_of = Some(self.graph.clone());
        }
        &self.compiled
    }

    fn push_undo(&mut self, before: Graph) {
        self.undo.push(before);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}

/// A wire being drawn: from this pin, to the pointer.
#[derive(Clone, Debug)]
struct Wiring {
    node: u32,
    pin: String,
    output: bool,
    ty: PinType,
}

/// The node search.
pub struct Palette {
    /// Where on the screen it opened, and where the node goes (graph units).
    screen: Pos2,
    at: Pos2,
    pub query: String,
    /// Only nodes that can take this wire.
    wiring: Option<Wiring>,
    picked: usize,
    focus: bool,
}

struct FileRow {
    path: String,
    marks: FileMarks,
}

/// A pin on the canvas this frame.
struct PinAt {
    node: u32,
    pin: String,
    output: bool,
    ty: PinType,
    pos: Pos2,
}

// ----------------------------------------------------------------------------- the panel

pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let mut tab = world.remove_resource::<GraphsTab>().unwrap_or_default();
    let now = editor::now(world);
    poll(world, editor, &mut tab, now);
    if tab.library.is_none() {
        tab.library = Some(graph::library(world, None));
    }
    if tab.vocab.is_none() {
        tab.vocab = Some(crate::scripts::Vocab::new(world));
    }

    egui::Panel::left("graph files")
        .resizable(true)
        .default_size(262.0)
        .size_range(180.0..=380.0)
        .show(ui, |ui| sidebar(ui, world, &mut tab, now));
    if tab.show_code && !tab.docs.is_empty() {
        egui::Panel::right("graph code")
            .resizable(true)
            .default_size(330.0)
            .size_range(220.0..=760.0)
            .frame(egui::Frame::new().fill(CODE_BG))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                code_panel(ui, &mut tab);
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            tool_bar(ui, world, editor, &mut tab, now);
            stop_strip(ui, world, &mut tab);
            if tab.docs.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(theme::dim(
                        "Open a graph from the list, or make one with New. A graph is a script \
                         drawn as nodes and wires: put it on an entity with a Script component \
                         (path ending in .graph) and it runs when you press Play.",
                    ));
                });
                return;
            }
            tab.current = tab.current.min(tab.docs.len() - 1);
            let status_h = 24.0;
            let h = (ui.available_height() - status_h).max(80.0);
            ui.allocate_ui(Vec2::new(ui.available_width(), h), |ui| {
                canvas(ui, world, editor, &mut tab, now);
            });
            status_line(ui, editor, &mut tab);
        });
    palette(ui.ctx(), &mut tab);
    if let Some((text, until, error)) = &tab.toast {
        if now < *until {
            toast(ui, text, *error);
        } else {
            tab.toast = None;
        }
    }
    world.insert_resource(tab);
}

/// Opens `path` (or shows it if it is open).
pub fn open(world: &World, tab: &mut GraphsTab, path: &str) {
    let path = engine_script::normalize(path);
    if let Some(i) = tab.docs.iter().position(|d| d.path == path) {
        tab.current = i;
        return;
    }
    let full = engine_script::scripts_root(world).join(&path);
    let graph = std::fs::read_to_string(&full)
        .ok()
        .and_then(|t| Graph::parse(&t).ok())
        .unwrap_or_default();
    tab.docs.push(GraphDoc {
        marks: editing::file_marks(world, &path),
        path,
        saved: graph.clone(),
        graph,
        stamp: modified(&full),
        undo: Vec::new(),
        redo: Vec::new(),
        gesture: false,
        selected: BTreeSet::new(),
        view: Rect::NOTHING,
        frame_all: true,
        zoom_on: None,
        canvas: Rect::NOTHING,
        compiled: Compiled::default(),
        compiled_of: None,
        marks_next: 0.0,
        live: Live::default(),
        ran: HashMap::new(),
        watched: None,
        wiring: None,
        boxing: None,
        editing_comment: None,
        breakpoints: BTreeSet::new(),
        applied: None,
    });
    tab.current = tab.docs.len() - 1;
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The runtime and the disk, a few times a second: the file list, edits made elsewhere, and
/// what the running graphs did.
fn poll(world: &mut World, editor: &Editor, tab: &mut GraphsTab, now: f64) {
    let playing = editor.mode != Mode::Edit;
    if playing != tab.playing {
        tab.playing = playing;
        for doc in &mut tab.docs {
            doc.watched = None;
            doc.live = Live::default();
            doc.ran.clear();
            doc.applied = None;
        }
        tab.stepping = None;
    }
    if playing {
        breakpoints_and_steps(world, tab);
    }
    if now >= tab.files_next {
        tab.files_next = now + FILES_SECS;
        let root = engine_script::scripts_root(world);
        let mut paths = Vec::new();
        list_graphs(&root, &root, &mut paths);
        paths.sort();
        tab.files = paths
            .into_iter()
            .map(|path| FileRow {
                marks: editing::file_marks(world, &path),
                path,
            })
            .collect();
        // Changed on disk and not here: take the file's version.
        for doc in &mut tab.docs {
            let full = root.join(&doc.path);
            let stamp = modified(&full);
            if stamp != doc.stamp {
                doc.stamp = stamp;
                if !doc.dirty()
                    && let Some(g) = std::fs::read_to_string(&full)
                        .ok()
                        .and_then(|t| Graph::parse(&t).ok())
                    && g != doc.graph
                {
                    doc.graph = g.clone();
                    doc.saved = g;
                }
            }
        }
    }
    for doc in &mut tab.docs {
        if now < doc.marks_next {
            continue;
        }
        doc.marks_next = now + POLL_SECS;
        doc.marks = editing::file_marks(world, &doc.path);
        if !(playing && tab.live) || doc.dirty() {
            continue;
        }
        // Watch it once the runtime runs the code we compiled.
        doc.compiled();
        let Some(version) = graph::loaded_version(world, &doc.path, &doc.compiled) else {
            continue;
        };
        let lost = doc.marks.hits.is_empty() && !doc.compiled.node_lines.is_empty();
        if doc.watched != Some(version) || lost {
            graph::watch(world, &doc.path, &doc.compiled);
            doc.watched = Some(version);
            doc.live = Live::default();
            continue;
        }
        let live = graph::live(&doc.compiled, &doc.marks.hits, &doc.marks.values);
        for (node, runs) in &live.runs {
            if doc.live.runs.get(node).is_none_or(|old| runs > old) && *runs > 0 {
                doc.ran.insert(*node, now);
            }
        }
        doc.live = live;
    }
}

fn list_graphs(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            list_graphs(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "graph")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn save(world: &mut World, tab: &mut GraphsTab, now: f64) {
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    let text = doc.graph.to_text();
    match call_api(
        world,
        "script.write",
        json!({"path": doc.path, "source": text}),
    ) {
        Ok(_) => {
            doc.saved = doc.graph.clone();
            doc.stamp = modified(&engine_script::scripts_root(world).join(&doc.path));
            let errors = doc.compiled().errors().count();
            let msg = if errors > 0 {
                (
                    format!(
                        "Saved {}, but {} it from loading; the version before keeps running",
                        doc.name(),
                        if errors == 1 {
                            "a mistake keeps".to_string()
                        } else {
                            format!("{errors} mistakes keep")
                        }
                    ),
                    true,
                )
            } else if tab.playing {
                (
                    format!("Saved {}: the running game reloads it", doc.name()),
                    false,
                )
            } else {
                (format!("Saved {}", doc.name()), false)
            };
            tab.toast = Some((msg.0, now + 3.0, msg.1));
        }
        Err(err) => tab.toast = Some((format!("Can't save: {err}"), now + 5.0, true)),
    }
}

/// Writes the graph's Luau as a `.luau` file beside it and opens that in the Scripts tab.
fn convert(world: &mut World, tab: &mut GraphsTab, now: f64) {
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    let path = format!("{}.luau", doc.path.trim_end_matches(".graph"));
    let luau = doc.compiled().luau.clone();
    let mut lines = luau.lines();
    lines.next();
    let text = format!(
        "-- Converted from {} by the graph compiler: from here on, edit this code.\n{}\n",
        doc.path,
        lines.collect::<Vec<_>>().join("\n")
    );
    match call_api(world, "script.write", json!({"path": path, "source": text})) {
        Ok(_) => {
            world.resource_scope(|world, mut scripts: Mut<crate::scripts::ScriptsTab>| {
                crate::scripts::open(world, &mut scripts, &path, None)
            });
            let mut dock = world.resource_mut::<EditorDock>();
            if let Some(at) = dock.0.find_tab(&Tab::Scripts) {
                let _ = dock.0.set_active_tab(at);
            }
            tab.toast = Some((
                format!("Wrote {path}: put it on an entity in place of the graph"),
                now + 4.0,
                false,
            ));
        }
        Err(err) => tab.toast = Some((format!("Can't write {path}: {err}"), now + 5.0, true)),
    }
}

// ----------------------------------------------------------------------------- side panels

fn sidebar(ui: &mut egui::Ui, world: &mut World, tab: &mut GraphsTab, now: f64) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Graphs").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("New")
                .on_hover_text("Make a graph file")
                .clicked()
            {
                tab.new_name = Some(String::new());
            }
        });
    });
    if let Some(name) = &mut tab.new_name {
        let r = ui.add(
            egui::TextEdit::singleline(name)
                .hint_text("folder/name")
                .desired_width(f32::INFINITY),
        );
        r.request_focus();
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            tab.new_name = None;
        } else if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            let name = name.trim().trim_end_matches(".graph").to_string();
            tab.new_name = None;
            if !name.is_empty() {
                let path = format!("{name}.graph");
                let mut g = Graph {
                    doc: format!(
                        "{} (a visual script).",
                        name.rsplit('/').next().unwrap_or(&name)
                    ),
                    ..Default::default()
                };
                for (id, kind, y) in [(1, "event.start", 0), (2, "event.update", 140)] {
                    g.nodes.push(Node {
                        id,
                        kind: kind.into(),
                        of: String::new(),
                        at: [0, y],
                        values: BTreeMap::new(),
                    });
                }
                match call_api(
                    world,
                    "script.write",
                    json!({"path": path, "source": g.to_text()}),
                ) {
                    Ok(_) => {
                        open(world, tab, &path);
                        tab.files_next = 0.0;
                    }
                    Err(err) => tab.toast = Some((err, now + 4.0, true)),
                }
            }
        }
    }
    ui.add_space(2.0);
    let mut open_path = None;
    egui::ScrollArea::vertical()
        .id_salt("graph file list")
        .max_height((ui.available_height() * 0.38).max(80.0))
        .show(ui, |ui| {
            if tab.files.is_empty() {
                ui.label(theme::dim("No .graph files yet."));
            }
            for f in &tab.files {
                let current = tab.docs.get(tab.current).is_some_and(|d| d.path == f.path);
                let (dot, state) = if f.marks.error.is_some() || f.marks.fault.is_some() {
                    (ERROR, "has an error")
                } else if f.marks.instances > 0 {
                    (theme::PLAY, "running")
                } else {
                    (theme::TEXT_DIM, "not running")
                };
                let label = if f.marks.instances > 1 {
                    format!("      {}  ×{}", f.path, f.marks.instances)
                } else {
                    format!("      {}", f.path)
                };
                let r = ui
                    .add(egui::Button::selectable(current, label))
                    .on_hover_text(format!("{} ({state})", f.path));
                let at = Pos2::new(r.rect.left() + 10.0, r.rect.center().y);
                ui.painter().circle_filled(at, 3.5, dot);
                if r.clicked() {
                    open_path = Some(f.path.clone());
                }
            }
        });
    if let Some(p) = open_path {
        open(world, tab, &p);
    }
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    ui.separator();
    let center = doc.view.center();
    egui::ScrollArea::vertical()
        .id_salt("graph variables")
        .max_height((ui.available_height() * 0.62).max(100.0))
        .show(ui, |ui| variables(ui, doc, center));
    ui.separator();
    // Problems: click one to see its node.
    let problems: Vec<Problem> = doc.compiled().problems.clone();
    let fault = doc.marks.fault.clone();
    ui.label(RichText::new("Problems").strong());
    egui::ScrollArea::vertical()
        .id_salt("graph problems")
        .show(ui, |ui| {
            if problems.is_empty() && fault.is_none() {
                ui.label(theme::dim("None: it compiles to clean Luau."));
            }
            if let Some(f) = &fault {
                let node = f.line.and_then(|l| doc.compiled.node_at(l));
                let text = format!("• running: {}  ×{}", f.message, f.count);
                if ui
                    .add(egui::Label::new(RichText::new(text).color(ERROR)).sense(Sense::click()))
                    .clicked()
                    && let Some(n) = node
                {
                    doc.selected = BTreeSet::from([n]);
                    focus_node(doc, n);
                }
            }
            for p in &problems {
                let title = p
                    .node
                    .and_then(|n| doc.graph.node(n))
                    .map(node_title)
                    .unwrap_or_default();
                let text = if title.is_empty() {
                    p.message.clone()
                } else {
                    format!("{title}: {}", p.message)
                };
                let color = if p.error { ERROR } else { WARN };
                let r = ui.add(
                    egui::Label::new(RichText::new(format!("• {text}")).color(color))
                        .sense(Sense::click()),
                );
                if r.clicked()
                    && let Some(n) = p.node
                {
                    doc.selected = BTreeSet::from([n]);
                    focus_node(doc, n);
                }
            }
        });
}

fn node_title(n: &Node) -> String {
    let base = graph::builtins();
    base.iter()
        .find(|s| s.kind == n.kind)
        .map(|s| {
            if n.of.is_empty() {
                s.title.clone()
            } else {
                format!("{} {}", s.title, n.of)
            }
        })
        .unwrap_or_else(|| {
            if n.of.is_empty() {
                n.kind.clone()
            } else {
                n.of.clone()
            }
        })
}

fn focus_node(doc: &mut GraphDoc, id: u32) {
    if let Some(n) = doc.graph.node(id) {
        let at = Pos2::new(n.at[0] as f32 + 80.0, n.at[1] as f32 + 40.0);
        let size = if doc.view.is_positive() {
            doc.view.size()
        } else {
            Vec2::new(1200.0, 800.0)
        };
        doc.view = Rect::from_center_size(at, size);
    }
}

/// The graph's variables and props: add, rename, retype, set their starting values, and put
/// a Get or Set node on the canvas.
fn variables(ui: &mut egui::Ui, doc: &mut GraphDoc, center: Pos2) {
    for props in [false, true] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(if props { "Props" } else { "Variables" }).strong())
                .on_hover_text(if props {
                    "Settings per entity: a Script component's props override them"
                } else {
                    "This script's own state (self.name), set when it starts"
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+").clicked() {
                    let map = if props {
                        &mut doc.graph.props
                    } else {
                        &mut doc.graph.vars
                    };
                    let mut i = map.len() + 1;
                    let base = if props { "setting" } else { "value" };
                    while map.contains_key(&format!("{base}{i}")) {
                        i += 1;
                    }
                    map.insert(format!("{base}{i}"), json!(0));
                }
            });
        });
        let names: Vec<String> = if props {
            doc.graph.props.keys().cloned().collect()
        } else {
            doc.graph.vars.keys().cloned().collect()
        };
        let mut add: Option<(&str, String)> = None;
        let mut remove = None;
        let mut rename: Option<(String, String)> = None;
        for name in names {
            let id = ui.make_persistent_id(("graph var", props, &name));
            let map = if props {
                &mut doc.graph.props
            } else {
                &mut doc.graph.vars
            };
            let Some(v) = map.get_mut(&name) else {
                continue;
            };
            // A row for the name, type and buttons, and one under it for the value.
            let ty = PinType::of_value(v);
            let mut kind = ty;
            ui.horizontal(|ui| {
                let (r, p) = ui.allocate_painter(Vec2::new(10.0, 16.0), Sense::hover());
                p.circle_filled(r.rect.center(), 4.0, pin_rgb(ty));
                // Buttons and type from the right; the name takes what is left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✖").on_hover_text("Remove").clicked() {
                        remove = Some(name.clone());
                    }
                    if !props
                        && ui
                            .small_button("Set")
                            .on_hover_text("Add a Set node")
                            .clicked()
                    {
                        add = Some(("var.set", name.clone()));
                    }
                    if ui
                        .small_button("Get")
                        .on_hover_text("Add a Get node")
                        .clicked()
                    {
                        add = Some((if props { "prop.get" } else { "var.get" }, name.clone()));
                    }
                    egui::ComboBox::from_id_salt(("graph var type", props, &name))
                        .width(58.0)
                        .selected_text(type_name(ty))
                        .show_ui(ui, |ui| {
                            for t in [PinType::Number, PinType::Bool, PinType::Text, PinType::Vec3]
                            {
                                ui.selectable_value(&mut kind, t, type_name(t));
                            }
                        });
                    let mut text = ui
                        .data_mut(|d| d.get_temp::<String>(id))
                        .unwrap_or_else(|| name.clone());
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()),
                    );
                    if r.changed() {
                        ui.data_mut(|d| d.insert_temp(id, text.clone()));
                    }
                    if r.lost_focus() {
                        ui.data_mut(|d| d.remove::<String>(id));
                        if text != name && graph::is_ident(&text) {
                            rename = Some((name.clone(), text.clone()));
                        }
                    }
                });
            });
            if kind != ty {
                *v = match kind {
                    PinType::Bool => json!(false),
                    PinType::Text => json!(""),
                    PinType::Vec3 => json!([0, 0, 0]),
                    _ => json!(0),
                };
            }
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let w = ui.available_width() - 4.0;
                value_field(ui, v, kind, &[], w);
            });
            ui.add_space(2.0);
        }
        let map = if props {
            &mut doc.graph.props
        } else {
            &mut doc.graph.vars
        };
        if let Some(name) = remove {
            map.remove(&name);
        }
        if let Some((old, new)) = rename.filter(|(_, new)| !map.contains_key(new)) {
            if let Some(v) = map.remove(&old) {
                map.insert(new.clone(), v);
            }
            let kinds: &[&str] = if props {
                &["prop.get"]
            } else {
                &["var.get", "var.set"]
            };
            for n in &mut doc.graph.nodes {
                if kinds.contains(&n.kind.as_str()) && n.of == old {
                    n.of = new.clone();
                }
            }
        }
        if let Some((kind, of)) = add {
            let id = doc.graph.next_id();
            doc.graph.nodes.push(Node {
                id,
                kind: kind.into(),
                of,
                at: [center.x as i32 - 60, center.y as i32 - 20],
                values: BTreeMap::new(),
            });
            doc.selected = BTreeSet::from([id]);
        }
    }
}

fn type_name(t: PinType) -> &'static str {
    match t {
        PinType::Exec => "exec",
        PinType::Any => "any",
        PinType::Number => "number",
        PinType::Bool => "bool",
        PinType::Text => "text",
        PinType::Vec3 => "vec3",
        PinType::Entity => "entity",
        PinType::Table => "table",
    }
}

/// An editor for a JSON value of type `ty`.
fn value_field(ui: &mut egui::Ui, v: &mut Value, ty: PinType, choices: &[String], width: f32) {
    match ty {
        PinType::Bool => {
            let mut b = v.as_bool().unwrap_or(false);
            if ui.checkbox(&mut b, "").changed() {
                *v = json!(b);
            }
        }
        PinType::Vec3 => {
            let mut xyz = [0.0f64; 3];
            if let Some(a) = v.as_array() {
                for (k, x) in a.iter().take(3).enumerate() {
                    xyz[k] = x.as_f64().unwrap_or(0.0);
                }
            }
            let mut changed = false;
            for x in &mut xyz {
                changed |= ui
                    .add_sized(
                        [(width / 3.0 - 3.0).max(28.0), 16.0],
                        egui::DragValue::new(x).speed(0.02).max_decimals(2),
                    )
                    .changed();
            }
            if changed {
                *v = json!(xyz.map(|x| (x * 1000.0).round() / 1000.0));
            }
        }
        PinType::Text if !choices.is_empty() => {
            let mut s = v.as_str().unwrap_or("").to_string();
            egui::ComboBox::from_id_salt(ui.next_auto_id())
                .width(width - 8.0)
                .selected_text(s.clone())
                .show_ui(ui, |ui| {
                    for c in choices {
                        ui.selectable_value(&mut s, c.clone(), c);
                    }
                });
            if Some(s.as_str()) != v.as_str() {
                *v = json!(s);
            }
        }
        PinType::Text | PinType::Entity => {
            let mut s = v.as_str().unwrap_or("").to_string();
            let hint = if ty == PinType::Entity { "self" } else { "" };
            if ui
                .add_sized(
                    [width, 16.0],
                    egui::TextEdit::singleline(&mut s).hint_text(hint),
                )
                .changed()
            {
                *v = if s.is_empty() && ty == PinType::Entity {
                    Value::Null
                } else {
                    json!(s)
                };
            }
        }
        _ => {
            let mut x = v.as_f64().unwrap_or(0.0);
            if ui
                .add_sized(
                    [width, 16.0],
                    egui::DragValue::new(&mut x).speed(0.05).max_decimals(3),
                )
                .changed()
            {
                let r = (x * 1000.0).round() / 1000.0;
                *v = if r.fract() == 0.0 && r.abs() < 1e12 {
                    json!(r as i64)
                } else {
                    json!(r)
                };
            }
        }
    }
}

/// The Luau the graph runs as, lines of the selected nodes picked out.
fn code_panel(ui: &mut egui::Ui, tab: &mut GraphsTab) {
    let GraphsTab {
        docs,
        current,
        vocab,
        ..
    } = tab;
    let Some(doc) = docs.get_mut(*current) else {
        return;
    };
    let selected = doc.selected.clone();
    let c = doc.compiled().clone();
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new("Runs as Luau").strong());
        ui.label(theme::dim(format!("{} lines, generated", c.lines.len())));
    });
    let font = FontId::monospace(12.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let first_picked = c
        .lines
        .iter()
        .position(|n| n.is_some_and(|n| selected.contains(&n)));
    egui::ScrollArea::both()
        .id_salt(("graph code", &doc.path))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let job = match vocab {
                Some(v) => crate::scripts::highlight(&c.luau, &font, v, None),
                None => egui::text::LayoutJob::simple(
                    c.luau.clone(),
                    font.clone(),
                    theme::TEXT,
                    f32::INFINITY,
                ),
            };
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let gutter = 40.0;
            let size = Vec2::new(galley.size().x + gutter + 12.0, galley.size().y + 8.0);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let p = ui.painter();
            let origin = rect.min + Vec2::new(gutter, 4.0);
            for (i, node) in c.lines.iter().enumerate() {
                let y = origin.y + i as f32 * row_h;
                let line_rect =
                    Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), row_h));
                if node.is_some_and(|n| selected.contains(&n)) {
                    p.rect_filled(line_rect, 0.0, CODE_PICKED);
                }
                if c.problems
                    .iter()
                    .any(|pr| pr.error && pr.node.is_some() && pr.node == *node)
                {
                    p.rect_filled(
                        Rect::from_min_size(line_rect.min, Vec2::new(3.0, row_h)),
                        0.0,
                        ERROR,
                    );
                }
                p.text(
                    Pos2::new(rect.left() + gutter - 8.0, y),
                    Align2::RIGHT_TOP,
                    (i + 1).to_string(),
                    font.clone(),
                    LINE_NO,
                );
            }
            p.galley(origin, galley, theme::TEXT);
            if let Some(i) = first_picked {
                let y = origin.y + i as f32 * row_h;
                let target = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(1.0, row_h));
                let key = egui::Id::new(("graph code scrolled", &doc.path));
                let last: Option<BTreeSet<u32>> = ui.data(|d| d.get_temp(key));
                if last.as_ref() != Some(&selected) {
                    ui.scroll_to_rect(target, Some(egui::Align::Center));
                    ui.data_mut(|d| d.insert_temp(key, selected.clone()));
                }
            }
        });
}

// ----------------------------------------------------------------------------- tool bar

fn tool_bar(ui: &mut egui::Ui, world: &mut World, editor: &Editor, tab: &mut GraphsTab, now: f64) {
    let _ = editor;
    egui::Frame::new()
        .fill(theme::BG)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut close = None;
                for (i, doc) in tab.docs.iter_mut().enumerate() {
                    let current = i == tab.current;
                    let label = format!("{}{}", doc.name(), if doc.dirty() { " •" } else { "" });
                    let bad = doc.compiled().errors().next().is_some() || doc.marks.fault.is_some();
                    let mut text = RichText::new(label);
                    if bad {
                        text = text.color(ERROR);
                    } else if current {
                        text = text.color(Color32::WHITE);
                    }
                    let r = ui
                        .add(egui::Button::selectable(current, text))
                        .on_hover_text(format!("{}  (middle click closes)", doc.path));
                    if r.clicked() {
                        tab.current = i;
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
                        .selectable_label(tab.show_code, "Luau")
                        .on_hover_text("Show the code the graph runs as")
                        .clicked()
                    {
                        tab.show_code = !tab.show_code;
                    }
                    if ui
                        .selectable_label(tab.live, "Live")
                        .on_hover_text(
                            "While playing: light up the nodes that run and show each wire's \
                             value",
                        )
                        .clicked()
                    {
                        tab.live = !tab.live;
                    }
                    let has = !tab.docs.is_empty();
                    if ui
                        .add_enabled(has, egui::Button::new("To script"))
                        .on_hover_text("Write the graph's Luau as a .luau file to go on in text")
                        .clicked()
                    {
                        convert(world, tab, now);
                    }
                    if ui
                        .add_enabled(has, egui::Button::new("Frame"))
                        .on_hover_text("Show the whole graph (F)")
                        .clicked()
                        && let Some(d) = tab.docs.get_mut(tab.current)
                    {
                        d.frame_all = true;
                    }
                    let dirty = tab.docs.get(tab.current).is_some_and(GraphDoc::dirty);
                    if ui
                        .add_enabled(dirty, egui::Button::new("💾 Save"))
                        .on_hover_text("Save (Ctrl+S); the running game reloads it")
                        .clicked()
                    {
                        save(world, tab, now);
                    }
                });
            });
        });
}

fn status_line(ui: &mut egui::Ui, editor: &Editor, tab: &mut GraphsTab) {
    let playing = editor.mode != Mode::Edit;
    let live_on = tab.live;
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        return;
    };
    let (nodes, wires) = (doc.graph.nodes.len(), doc.graph.links.len());
    let errors = doc.compiled().errors().count();
    let warnings = doc.compiled.problems.len() - errors;
    let lines = doc.compiled.lines.len();
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(theme::dim(format!(
            "{nodes} nodes, {wires} wires: {lines} lines of Luau"
        )));
        if errors > 0 {
            ui.label(
                RichText::new(format!(
                    "{errors} mistake{} from loading",
                    if errors == 1 {
                        " keeps it"
                    } else {
                        "s keep it"
                    }
                ))
                .color(ERROR),
            );
        } else if warnings > 0 {
            ui.label(RichText::new(format!("{warnings} to look at")).color(WARN));
        }
        if let Some(e) = &doc.marks.error {
            ui.label(RichText::new(format!("didn't load: {}", e.message)).color(ERROR));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let state = if !playing {
                "Edit mode: Play runs it".to_string()
            } else if doc.dirty() {
                "Playing: save to run the edits".to_string()
            } else if live_on && doc.watched.is_some() {
                format!("Playing, live: v{}", doc.marks.version)
            } else if doc.marks.instances > 0 {
                format!("Playing: v{}", doc.marks.version)
            } else {
                "Playing: no entity runs it".to_string()
            };
            ui.label(theme::dim(state));
        });
    });
}

fn toast(ui: &egui::Ui, text: &str, error: bool) {
    let rect = ui.max_rect();
    egui::Area::new(egui::Id::new("graph toast"))
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

// ----------------------------------------------------------------------------- the canvas

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn pin_rgb(ty: PinType) -> Color32 {
    rgb(look::pin_color(ty))
}

/// The library with the open graph's own variables and props.
fn specs_for(tab: &GraphsTab, g: &Graph) -> Vec<NodeSpec> {
    let mut lib = graph::own_specs(g);
    lib.extend(tab.library.iter().flatten().cloned());
    lib
}

fn canvas(ui: &mut egui::Ui, world: &mut World, editor: &Editor, tab: &mut GraphsTab, now: f64) {
    let lib = specs_for(tab, &tab.docs[tab.current].graph);
    let stopped_node = stopped_node(world, &mut tab.docs[tab.current]);
    let playing = editor.mode != Mode::Edit;
    let live_on = tab.live && playing;
    let clipboard = tab.clipboard.clone();
    let doc = &mut tab.docs[tab.current];
    let before = doc.graph.clone();
    let compiled = doc.compiled().clone();
    let fault_node = doc
        .marks
        .fault
        .as_ref()
        .and_then(|f| f.line)
        .and_then(|l| compiled.node_at(l));

    // Shapes of every node this frame.
    let wired_in: BTreeSet<(u32, String)> = doc.graph.links.iter().map(|l| l.to.clone()).collect();
    let wired_out: BTreeSet<(u32, String)> =
        doc.graph.links.iter().map(|l| l.from.clone()).collect();
    let shapes: BTreeMap<u32, (NodeSpec, look::Shape)> = doc
        .graph
        .nodes
        .iter()
        .map(|n| {
            let spec = graph::spec_for(&lib, n, &doc.graph);
            let shape = look::shape(&spec, n, &|p| wired_in.contains(&(n.id, p.to_string())));
            (n.id, (spec, shape))
        })
        .collect();
    let outer = ui.available_rect_before_wrap();
    ui.painter().rect_filled(outer, 0.0, CANVAS_BG);
    doc.canvas = outer;
    if doc.frame_all {
        doc.frame_all = false;
        let mut r = Rect::NOTHING;
        for n in &doc.graph.nodes {
            let size = shapes[&n.id].1.size;
            r = r.union(Rect::from_min_size(
                Pos2::new(n.at[0] as f32, n.at[1] as f32),
                Vec2::new(size[0], size[1]),
            ));
        }
        for c in &doc.graph.comments {
            r = r.union(Rect::from_min_size(
                Pos2::new(c.at[0] as f32, c.at[1] as f32),
                Vec2::new(c.size[0] as f32, c.size[1] as f32),
            ));
        }
        // Everything in view, but never blown up past 1:1.
        let r = if r.is_positive() {
            r.expand(40.0)
        } else {
            Rect::from_center_size(Pos2::ZERO, outer.size())
        };
        let zoom = (outer.width() / r.width())
            .min(outer.height() / r.height())
            .min(1.0);
        doc.zoom_on = Some((r.center(), zoom));
    }
    if let Some((at, zoom)) = doc.zoom_on.take() {
        doc.view = Rect::from_center_size(at, outer.size() / zoom.max(0.05));
    }
    let mut view = doc.view;
    let mut pins: Vec<PinAt> = Vec::new();
    let mut open_palette: Option<(Pos2, Pos2, Option<Wiring>)> = None;
    let mut hovered_node = false;
    let time = ui.input(|i| i.time);
    let mut texts = SharpText::default();
    let scene = egui::Scene::new()
        .zoom_range(0.15..=1.6)
        .max_inner_size(Vec2::splat(1.0e6))
        .drag_pan_buttons(egui::DragPanButtons::SECONDARY | egui::DragPanButtons::MIDDLE);
    let shown = scene.show(ui, &mut view, |ui| {
        let to_scene = ui
            .ctx()
            .layer_transform_from_global(ui.layer_id())
            .unwrap_or(TSTransform::IDENTITY);
        let clip = ui.clip_rect();
        let painter = ui.painter().clone();
        paint_grid(&painter, clip);
        // Comments, behind the nodes.
        comments(ui, doc, &shapes, &mut texts);
        // Wires.
        let socket = |id: u32, pin: &str, output: bool| -> Option<Pos2> {
            let n = doc.graph.node(id)?;
            let p = shapes.get(&id)?.1.socket(pin, output)?;
            Some(Pos2::new(n.at[0] as f32 + p[0], n.at[1] as f32 + p[1]))
        };
        let lit = |id: u32| {
            doc.ran
                .get(&id)
                .map(|t| (1.0 - (now - t) / LIT_SECS).clamp(0.0, 1.0) as f32)
                .unwrap_or(0.0)
        };
        let mut bubbles = Vec::new();
        let mut shown_outputs = BTreeSet::new();
        for exec_pass in [false, true] {
            for (i, l) in doc.graph.links.iter().enumerate() {
                let (Some((fs, _)), Some((ts, _))) = (shapes.get(&l.from.0), shapes.get(&l.to.0))
                else {
                    continue;
                };
                let out_ty = fs
                    .output_pin(&l.from.1)
                    .map(|p| p.ty)
                    .unwrap_or(PinType::Any);
                let in_ty = ts.input_pin(&l.to.1).map(|p| p.ty).unwrap_or(PinType::Any);
                let exec = out_ty == PinType::Exec;
                if exec != exec_pass {
                    continue;
                }
                let (Some(a), Some(b)) = (
                    socket(l.from.0, &l.from.1, true),
                    socket(l.to.0, &l.to.1, false),
                ) else {
                    continue;
                };
                let ty = if out_ty == PinType::Any {
                    in_ty
                } else {
                    out_ty
                };
                let mut color = pin_rgb(ty);
                let mut width = if exec { 3.0 } else { 2.2 };
                let k = if exec && live_on { lit(l.to.0) } else { 0.0 };
                if live_on && exec {
                    color = if k > 0.0 {
                        lerp_color(color, RAN, k)
                    } else {
                        color.gamma_multiply(0.45)
                    };
                    width *= 1.0 + 0.4 * k;
                }
                let pts = bezier(a, b);
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    pts,
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(width, color),
                ));
                // Pulses running along a wire that just carried execution.
                if k > 0.0 {
                    for j in 0..3 {
                        let t = ((time * 1.4 + j as f64 / 3.0) % 1.0) as f32;
                        let p = bezier_at(pts, t);
                        painter.circle_filled(p, 3.6, RAN.gamma_multiply(0.4 + 0.6 * k));
                    }
                }
                // One value per output, however many wires it feeds.
                if !exec
                    && live_on
                    && let Some(v) = doc.live.values.get(&i)
                    && shown_outputs.insert(l.from.clone())
                {
                    bubbles.push((bezier_at(pts, 0.5), v.clone()));
                }
            }
        }
        // The wire being drawn.
        if let Some(w) = &doc.wiring
            && let Some(from) = socket(w.node, &w.pin, w.output)
            && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        {
            let to = to_scene * p;
            let pts = if w.output {
                bezier(from, to)
            } else {
                bezier(to, from)
            };
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                pts,
                false,
                Color32::TRANSPARENT,
                Stroke::new(2.4, pin_rgb(w.ty)),
            ));
        }
        // Nodes.
        let mut drag_by = Vec2::ZERO;
        let mut drag_started = false;
        let mut clicked: Option<(u32, bool)> = None;
        let mut menu: Option<(u32, &str)> = None;
        let order: Vec<u32> = doc.graph.nodes.iter().map(|n| n.id).collect();
        for id in order {
            let Some((spec, shape)) = shapes.get(&id) else {
                continue;
            };
            let Some(n) = doc.graph.node(id).cloned() else {
                continue;
            };
            let rect = Rect::from_min_size(
                Pos2::new(n.at[0] as f32, n.at[1] as f32),
                Vec2::new(shape.size[0], shape.size[1]),
            );
            let selected = doc.selected.contains(&id);
            let problems: Vec<&Problem> = compiled
                .problems
                .iter()
                .filter(|p| p.node == Some(id))
                .collect();
            let k = if live_on { lit(id) } else { 0.0 };
            let resp = ui.interact(
                rect,
                ui.id().with(("graph node", id)),
                Sense::click_and_drag(),
            );
            hovered_node |= resp.hovered();
            paint_node(
                &painter,
                rect,
                spec,
                k,
                selected,
                &problems,
                fault_node == Some(id),
            );
            if doc.breakpoints.contains(&id) {
                paint_breakpoint(&painter, rect);
            }
            if stopped_node == Some(id) {
                paint_stopped(&painter, rect);
            }
            if resp.drag_started() {
                drag_started = true;
                if !selected {
                    clicked = Some((id, ui.input(|i| i.modifiers.shift || i.modifiers.command)));
                }
            }
            if resp.dragged() {
                drag_by += resp.drag_delta();
            }
            if resp.clicked() {
                clicked = Some((id, ui.input(|i| i.modifiers.shift || i.modifiers.command)));
            }
            resp.context_menu(|ui| {
                if ui.button("Delete").clicked() {
                    menu = Some((id, "delete"));
                }
                if ui.button("Duplicate").clicked() {
                    menu = Some((id, "duplicate"));
                }
                if ui.button("Break its wires").clicked() {
                    menu = Some((id, "break"));
                }
                ui.separator();
                let label = if doc.breakpoints.contains(&id) {
                    "Remove the breakpoint (F9)"
                } else {
                    "Add a breakpoint (F9)"
                };
                if ui.button(label).clicked() {
                    menu = Some((id, "breakpoint"));
                }
            });
            let mut tip = format!("{}\n{}", spec.title, spec.doc);
            if !spec.of.is_empty() && spec.kind != "var.get" && spec.kind != "prop.get" {
                tip.push_str(&format!("\n{} {}", spec.kind, spec.of));
            }
            for p in &problems {
                tip.push_str(&format!(
                    "\n\n{}: {}",
                    if p.error { "Error" } else { "Warning" },
                    p.message
                ));
            }
            if fault_node == Some(id)
                && let Some(f) = &doc.marks.fault
            {
                tip.push_str(&format!(
                    "\n\nRunning, it failed: {} (×{})",
                    f.message, f.count
                ));
            }
            // Title over the header.
            texts.add(
                Pos2::new(rect.left() + look::PAD, rect.top() + look::HEADER / 2.0),
                Align2::LEFT_CENTER,
                &spec.title,
                FontId::proportional(FONT),
                Color32::WHITE,
            );
            // Pins and their fields.
            for row in &shape.inputs {
                let y = rect.top() + row.y;
                let mut x = rect.left() + 6.0;
                if row.socket {
                    let at = Pos2::new(rect.left() + 9.0, y);
                    let on = wired_in.contains(&(id, row.pin.name.clone()));
                    paint_pin(&painter, at, row.pin.ty, on);
                    pins.push(PinAt {
                        node: id,
                        pin: row.pin.name.clone(),
                        output: false,
                        ty: row.pin.ty,
                        pos: at,
                    });
                    x = rect.left() + look::SOCKET;
                }
                if !row.label.is_empty() {
                    let font = FontId::proportional(FONT - 1.0);
                    let w = painter
                        .layout_no_wrap(row.label.clone(), font.clone(), theme::TEXT_DIM)
                        .size()
                        .x;
                    texts.add(
                        Pos2::new(x, y),
                        Align2::LEFT_CENTER,
                        &row.label,
                        font,
                        theme::TEXT_DIM,
                    );
                    x += w + 6.0;
                }
                let unset = row.field > 0.0
                    && look::patches(&n.kind)
                    && row.pin.name != "entity"
                    && !n.values.contains_key(&row.pin.name);
                if unset {
                    // Left as it is: a dash until clicked.
                    let field =
                        Rect::from_min_size(Pos2::new(x, y - 8.0), Vec2::new(row.field, 16.0));
                    let r = ui
                        .put(
                            field,
                            egui::Button::new(RichText::new("–").color(theme::TEXT_DIM))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::new(1.0, FIELD_BG)),
                        )
                        .on_hover_text("Left as it is. Click to set it.");
                    if r.clicked()
                        && let Some(node) = doc.graph.node_mut(id)
                    {
                        node.values
                            .insert(row.pin.name.clone(), row.pin.default.clone());
                    }
                } else if row.field > 0.0 {
                    let field =
                        Rect::from_min_size(Pos2::new(x, y - 8.0), Vec2::new(row.field, 16.0));
                    let mut v = n
                        .values
                        .get(&row.pin.name)
                        .cloned()
                        .unwrap_or_else(|| row.pin.default.clone());
                    let old = v.clone();
                    ui.scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.spacing_mut().interact_size.y = 16.0;
                        ui.style_mut().override_font_id = Some(FontId::proportional(FONT - 1.0));
                        ui.visuals_mut().widgets.inactive.bg_fill = FIELD_BG;
                        ui.visuals_mut().widgets.inactive.weak_bg_fill = FIELD_BG;
                        ui.horizontal(|ui| {
                            value_field(ui, &mut v, row.pin.ty, &row.pin.choices, row.field)
                        });
                    });
                    if v != old
                        && let Some(node) = doc.graph.node_mut(id)
                    {
                        // A patch keeps what was set, even the default.
                        if v == row.pin.default && !look::patches(&node.kind) {
                            node.values.remove(&row.pin.name);
                        } else {
                            node.values.insert(row.pin.name.clone(), v);
                        }
                    }
                }
            }
            for row in &shape.outputs {
                let y = rect.top() + row.y;
                let at = Pos2::new(rect.right() - 9.0, y);
                let on = wired_out.contains(&(id, row.pin.name.clone()));
                paint_pin(&painter, at, row.pin.ty, on);
                pins.push(PinAt {
                    node: id,
                    pin: row.pin.name.clone(),
                    output: true,
                    ty: row.pin.ty,
                    pos: at,
                });
                if !row.label.is_empty() {
                    texts.add(
                        Pos2::new(rect.right() - look::SOCKET, y),
                        Align2::RIGHT_CENTER,
                        &row.label,
                        FontId::proportional(FONT - 1.0),
                        theme::TEXT_DIM,
                    );
                }
            }
            if let Some(p) = problems.iter().find(|p| p.error).or(problems.first()) {
                let c = if p.error { ERROR } else { WARN };
                let badge = Pos2::new(rect.right() - 2.0, rect.top() + 2.0);
                painter.circle_filled(badge, 8.0, c);
                texts.add(
                    badge,
                    Align2::CENTER_CENTER,
                    "!",
                    FontId::proportional(12.0),
                    Color32::BLACK,
                );
                texts.add(
                    Pos2::new(rect.left(), rect.bottom() + 4.0),
                    Align2::LEFT_TOP,
                    &p.message,
                    FontId::proportional(FONT - 1.0),
                    c,
                );
            }
            resp.on_hover_text(tip);
        }
        // Pin interaction: start a wire, Alt+click to break.
        for p in &pins {
            let r = ui.interact(
                Rect::from_center_size(p.pos, Vec2::splat(14.0)),
                ui.id().with(("graph pin", p.node, &p.pin, p.output)),
                Sense::click_and_drag(),
            );
            if r.hovered() {
                painter.circle_stroke(p.pos, 7.0, Stroke::new(1.5, Color32::WHITE));
            }
            if r.clicked() && ui.input(|i| i.modifiers.alt) {
                doc.graph.links.retain(|l| {
                    let at = if p.output { &l.from } else { &l.to };
                    !(at.0 == p.node && at.1 == p.pin)
                });
            } else if r.drag_started() {
                doc.wiring = Some(Wiring {
                    node: p.node,
                    pin: p.pin.clone(),
                    output: p.output,
                    ty: p.ty,
                });
            }
        }
        // Values along the wires.
        for (at, v) in bubbles {
            let text: String = if v.chars().count() > 22 {
                v.chars().take(21).chain(['…']).collect()
            } else {
                v
            };
            let font = FontId::monospace(11.0);
            let size = painter
                .layout_no_wrap(text.clone(), font.clone(), VALUE)
                .size();
            let r = Rect::from_center_size(at, size + Vec2::new(10.0, 4.0));
            painter.rect(
                r,
                4.0,
                VALUE_BG,
                Stroke::new(1.0, VALUE.gamma_multiply(0.5)),
                egui::StrokeKind::Inside,
            );
            texts.add(r.center(), Align2::CENTER_CENTER, &text, font, VALUE);
        }
        // Moving nodes (the selection with them).
        if let Some((id, add)) = clicked {
            if add {
                if !doc.selected.remove(&id) {
                    doc.selected.insert(id);
                }
            } else {
                doc.selected = BTreeSet::from([id]);
            }
        }
        if drag_by != Vec2::ZERO || drag_started {
            let moving = doc.selected.clone();
            for n in &mut doc.graph.nodes {
                if moving.contains(&n.id) {
                    n.at[0] += drag_by.x.round() as i32;
                    n.at[1] += drag_by.y.round() as i32;
                }
            }
        }
        if let Some((id, what)) = menu {
            match what {
                "delete" => {
                    if !doc.selected.contains(&id) {
                        doc.selected = BTreeSet::from([id]);
                    }
                    delete_selected(doc);
                }
                "duplicate" => {
                    if !doc.selected.contains(&id) {
                        doc.selected = BTreeSet::from([id]);
                    }
                    let frag = fragment(&doc.graph, &doc.selected);
                    paste(doc, &frag, Vec2::new(30.0, 30.0));
                }
                "breakpoint" => {
                    if !doc.breakpoints.remove(&id) {
                        doc.breakpoints.insert(id);
                    }
                }
                _ => doc.graph.links.retain(|l| l.from.0 != id && l.to.0 != id),
            }
        }
        // A wire dropped: on a pin it joins, in empty space it asks what to add.
        if doc.wiring.is_some() && ui.input(|i| i.pointer.any_released()) {
            let w = doc.wiring.take().unwrap_or_else(|| unreachable!());
            if let Some(global) = ui.input(|i| i.pointer.interact_pos()) {
                let at = to_scene * global;
                let target = pins
                    .iter()
                    .filter(|p| p.output != w.output && p.node != w.node)
                    .filter(|p| p.pos.distance(at) < 14.0)
                    .min_by(|a, b| a.pos.distance(at).total_cmp(&b.pos.distance(at)));
                match target {
                    Some(t) => {
                        let (from, to) = if w.output {
                            ((w.node, w.pin.clone(), w.ty), (t.node, t.pin.clone(), t.ty))
                        } else {
                            ((t.node, t.pin.clone(), t.ty), (w.node, w.pin.clone(), w.ty))
                        };
                        connect(doc, from, to);
                    }
                    None if !hovered_node => {
                        let on_pin = pins.iter().any(|p| p.pos.distance(at) < 14.0);
                        if !on_pin {
                            open_palette = Some((global, at, Some(w)));
                        }
                    }
                    None => {}
                }
            }
        }
        // Context-menu click and Space on empty canvas: the node search.
        to_scene
    });
    let to_scene = shown.inner;
    let bg = shown.response;
    doc.view = view;
    texts.draw(ui, outer, to_scene.inverse());
    overlay(
        ui,
        outer,
        live_on,
        doc.watched.is_some(),
        view.width() / outer.width().max(1.0),
    );
    if bg.secondary_clicked()
        && let Some(global) = ui.input(|i| i.pointer.interact_pos())
    {
        open_palette = Some((global, to_scene * global, None));
    }
    if bg.clicked() && !ui.input(|i| i.modifiers.shift) {
        doc.selected.clear();
    }
    // Box selection with the left button on the background.
    if bg.drag_started_by(egui::PointerButton::Primary)
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
    {
        doc.boxing = Some(to_scene * o);
    }
    if let Some(start) = doc.boxing {
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            let end = to_scene * p;
            let r = Rect::from_two_pos(start, end);
            let to_global = to_scene.inverse();
            let screen = Rect::from_two_pos(to_global * r.min, to_global * r.max);
            ui.painter().rect(
                screen,
                2.0,
                theme::ACCENT.gamma_multiply(0.12),
                Stroke::new(1.0, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
            if ui.input(|i| i.pointer.any_released()) {
                let add = ui.input(|i| i.modifiers.shift);
                if !add {
                    doc.selected.clear();
                }
                for n in &doc.graph.nodes {
                    let size = shapes.get(&n.id).map(|s| s.1.size).unwrap_or([100.0, 40.0]);
                    let nr = Rect::from_min_size(
                        Pos2::new(n.at[0] as f32, n.at[1] as f32),
                        Vec2::new(size[0], size[1]),
                    );
                    if r.intersects(nr) {
                        doc.selected.insert(n.id);
                    }
                }
                doc.boxing = None;
            }
        } else {
            doc.boxing = None;
        }
    }
    // Keys, while the pointer is over the canvas and nothing is being typed.
    let over = ui.rect_contains_pointer(outer);
    if over && !ui.ctx().egui_wants_keyboard_input() {
        // F9: a breakpoint on each selected node, or off again.
        if ui.input(|i| i.key_pressed(Key::F9)) {
            let ids: Vec<u32> = doc.selected.iter().copied().collect();
            let all_on = ids.iter().all(|id| doc.breakpoints.contains(id));
            for id in ids {
                if all_on {
                    doc.breakpoints.remove(&id);
                } else {
                    doc.breakpoints.insert(id);
                }
            }
        }
        let (del, dup, undo, redo, frame, copy, pastes, space, comment, all) = ui.input(|i| {
            let cmd = i.modifiers.command;
            (
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                cmd && i.key_pressed(Key::D),
                cmd && i.key_pressed(Key::Z) && !i.modifiers.shift,
                cmd && (i.key_pressed(Key::Y) || (i.key_pressed(Key::Z) && i.modifiers.shift)),
                !cmd && i.key_pressed(Key::F),
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
                i.key_pressed(Key::Space),
                !cmd && i.key_pressed(Key::C),
                cmd && i.key_pressed(Key::A),
            )
        });
        if del {
            delete_selected(doc);
        }
        if dup && !doc.selected.is_empty() {
            let frag = fragment(&doc.graph, &doc.selected);
            paste(doc, &frag, Vec2::new(30.0, 30.0));
        }
        if copy && !doc.selected.is_empty() {
            let frag = fragment(&doc.graph, &doc.selected);
            ui.ctx().copy_text(frag.to_text());
            tab_clipboard_set(&mut tab.clipboard, frag);
        }
        let doc = &mut tab.docs[tab.current];
        if pastes && let Some(frag) = &clipboard {
            let at = ui
                .input(|i| i.pointer.hover_pos())
                .map(|p| to_scene * p)
                .unwrap_or(doc.view.center());
            let min = frag
                .nodes
                .iter()
                .map(|n| Pos2::new(n.at[0] as f32, n.at[1] as f32))
                .fold(Pos2::new(f32::MAX, f32::MAX), |a, b| a.min(b));
            paste(doc, frag, at - min);
        }
        if undo && let Some(g) = doc.undo.pop() {
            doc.redo.push(std::mem::replace(&mut doc.graph, g));
        }
        if redo && let Some(g) = doc.redo.pop() {
            doc.undo.push(std::mem::replace(&mut doc.graph, g));
        }
        if frame {
            doc.frame_all = true;
        }
        if all {
            doc.selected = doc.graph.nodes.iter().map(|n| n.id).collect();
        }
        if comment && !doc.selected.is_empty() {
            let mut r = Rect::NOTHING;
            for n in &doc.graph.nodes {
                if doc.selected.contains(&n.id) {
                    let size = shapes.get(&n.id).map(|s| s.1.size).unwrap_or([100.0, 40.0]);
                    r = r.union(Rect::from_min_size(
                        Pos2::new(n.at[0] as f32, n.at[1] as f32),
                        Vec2::new(size[0], size[1]),
                    ));
                }
            }
            let r = r.expand2(Vec2::new(24.0, 20.0));
            doc.graph.comments.push(Comment {
                at: [r.left() as i32, r.top() as i32 - 24],
                size: [r.width() as i32, r.height() as i32 + 24],
                text: "Comment".into(),
                color: [70, 90, 120],
            });
            doc.editing_comment = Some(doc.graph.comments.len() - 1);
        }
        if space && let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            open_palette = Some((p, to_scene * p, None));
        }
    }
    if ui.input(|i| i.modifiers.command && i.key_pressed(Key::S)) {
        save(world, tab, now);
    }
    let doc = &mut tab.docs[tab.current];
    // One undo step per gesture.
    if doc.graph != before && !doc.gesture {
        doc.push_undo(before);
        doc.gesture = true;
    }
    if !ui.input(|i| i.pointer.any_down()) && !ui.ctx().egui_wants_keyboard_input() {
        doc.gesture = false;
    }
    if let Some((screen, at, wiring)) = open_palette {
        tab.palette = Some(Palette {
            screen,
            at,
            query: String::new(),
            wiring,
            picked: 0,
            focus: true,
        });
    }
}

/// The canvas's text, drawn after it at its size on screen. `egui::Scene` scales text the way
/// it scales shapes, so zoomed out, glyphs drawn at their 1:1 size lose strokes; laid out at
/// the zoomed size they stay sharp.
#[derive(Default)]
struct SharpText(Vec<(Pos2, Align2, String, FontId, Color32)>);

impl SharpText {
    fn add(&mut self, at: Pos2, align: Align2, text: &str, font: FontId, color: Color32) {
        self.0.push((at, align, text.to_string(), font, color));
    }

    /// On a layer just above the canvas's, so it covers the nodes but not menus or popups.
    fn draw(self, ui: &egui::Ui, clip: Rect, to_global: TSTransform) {
        let parent = ui.layer_id();
        let layer = egui::LayerId::new(parent.order, parent.id.with("graph text"));
        ui.ctx().set_sublayer(parent, layer);
        let p = egui::Painter::new(ui.ctx().clone(), layer, clip);
        for (at, align, text, font, color) in self.0 {
            // Half-pixel steps, so zooming doesn't fill the font atlas with sizes.
            let size = (font.size * to_global.scaling * 2.0).round() / 2.0;
            if size >= 4.0 {
                p.text(
                    to_global * at,
                    align,
                    text,
                    FontId::new(size, font.family),
                    color,
                );
            }
        }
    }
}

/// What sits on the canvas, not in the graph: the corner mark (LIVE while the game runs and
/// is watched, with a border in the play color), the zoom and the mouse hints.
fn overlay(ui: &egui::Ui, outer: Rect, live: bool, watched: bool, units_per_px: f32) {
    let p = ui.painter_at(outer);
    let (mark, color) = if live && watched {
        ("LIVE", theme::PLAY)
    } else {
        ("VISUAL SCRIPT", Color32::from_white_alpha(28))
    };
    p.text(
        outer.right_bottom() - Vec2::new(18.0, 12.0),
        Align2::RIGHT_BOTTOM,
        mark,
        FontId::proportional(30.0),
        if live && watched {
            color.gamma_multiply(0.75)
        } else {
            color
        },
    );
    if live && watched {
        p.rect_stroke(
            outer.shrink(1.0),
            0.0,
            Stroke::new(2.0, color),
            egui::StrokeKind::Inside,
        );
    }
    let zoom = 1.0 / units_per_px.max(1e-3);
    let hint = if live {
        format!("zoom {:.0}%", zoom * 100.0)
    } else {
        format!(
            "zoom {:.0}%   right-click: add a node   drag a pin: wire   right-drag: pan   F: frame   C: comment",
            zoom * 100.0
        )
    };
    p.text(
        outer.left_bottom() + Vec2::new(10.0, -8.0),
        Align2::LEFT_BOTTOM,
        hint,
        FontId::proportional(12.0),
        Color32::from_white_alpha(70),
    );
}

fn tab_clipboard_set(slot: &mut Option<Graph>, g: Graph) {
    *slot = Some(g);
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn bezier(a: Pos2, b: Pos2) -> [Pos2; 4] {
    let d = ((b.x - a.x).abs() * 0.5).max(50.0);
    [a, Pos2::new(a.x + d, a.y), Pos2::new(b.x - d, b.y), b]
}

fn bezier_at(p: [Pos2; 4], t: f32) -> Pos2 {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    Pos2::new(
        w[0] * p[0].x + w[1] * p[1].x + w[2] * p[2].x + w[3] * p[3].x,
        w[0] * p[0].y + w[1] * p[1].y + w[2] * p[2].y + w[3] * p[3].y,
    )
}

fn paint_grid(p: &egui::Painter, clip: Rect) {
    let step = 32.0;
    let x0 = (clip.left() / step).floor() as i64;
    let x1 = (clip.right() / step).ceil() as i64;
    let y0 = (clip.top() / step).floor() as i64;
    let y1 = (clip.bottom() / step).ceil() as i64;
    if (x1 - x0) > 600 || (y1 - y0) > 600 {
        return;
    }
    for i in x0..=x1 {
        let x = i as f32 * step;
        let c = if i % 8 == 0 { GRID_MAJOR } else { GRID };
        p.line_segment(
            [Pos2::new(x, clip.top()), Pos2::new(x, clip.bottom())],
            Stroke::new(1.0, c),
        );
    }
    for i in y0..=y1 {
        let y = i as f32 * step;
        let c = if i % 8 == 0 { GRID_MAJOR } else { GRID };
        p.line_segment(
            [Pos2::new(clip.left(), y), Pos2::new(clip.right(), y)],
            Stroke::new(1.0, c),
        );
    }
}

fn paint_pin(p: &egui::Painter, at: Pos2, ty: PinType, on: bool) {
    let c = pin_rgb(ty);
    if ty == PinType::Exec {
        let pts = vec![
            at + Vec2::new(-5.0, -5.5),
            at + Vec2::new(1.0, -5.5),
            at + Vec2::new(6.0, 0.0),
            at + Vec2::new(1.0, 5.5),
            at + Vec2::new(-5.0, 5.5),
        ];
        if on {
            p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
        } else {
            p.add(Shape::closed_line(pts, Stroke::new(1.5, c)));
        }
    } else if on {
        p.circle_filled(at, 5.0, c);
    } else {
        p.circle_stroke(at, 4.5, Stroke::new(1.6, c));
    }
}

/// Unreal's breakpoint mark: a red octagon on the node's top-left corner.
fn paint_breakpoint(p: &egui::Painter, rect: Rect) {
    let c = rect.left_top() + Vec2::new(2.0, 2.0);
    let r = 8.5;
    let points: Vec<Pos2> = (0..8)
        .map(|i| {
            let a = std::f32::consts::TAU * (i as f32 + 0.5) / 8.0;
            c + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::convex_polygon(
        points,
        BREAKPOINT,
        Stroke::new(1.5, Color32::from_black_alpha(200)),
    ));
}

/// The node the graph waits at: outlined in yellow, under a big arrow (as Blueprints mark the
/// node a breakpoint stopped on).
fn paint_stopped(p: &egui::Painter, rect: Rect) {
    p.rect_stroke(
        rect.expand(3.0),
        9.0,
        Stroke::new(3.5, STOPPED),
        egui::StrokeKind::Outside,
    );
    let tip = Pos2::new(rect.center().x, rect.top() - 8.0);
    let (w, h, stem) = (16.0, 16.0, 7.0);
    p.add(Shape::convex_polygon(
        vec![
            tip,
            tip + Vec2::new(w, -h),
            tip + Vec2::new(stem, -h),
            tip + Vec2::new(stem, -h - 18.0),
            tip + Vec2::new(-stem, -h - 18.0),
            tip + Vec2::new(-stem, -h),
            tip + Vec2::new(-w, -h),
        ],
        STOPPED,
        Stroke::new(1.5, Color32::from_black_alpha(200)),
    ));
}

/// The node of `doc` the scripts are stopped at (the innermost frame in its code).
fn stopped_node(world: &World, doc: &mut GraphDoc) -> Option<u32> {
    let stop = debug::stopped(world)?;
    let frame = stop.frames.iter().find(|f| f.path == doc.path)?;
    doc.compiled().node_at(frame.line)
}

/// Puts each graph's node breakpoints on the lines their nodes run from (again when the code
/// moves), and runs Next node: line steps until the graph is at another node.
fn breakpoints_and_steps(world: &mut World, tab: &mut GraphsTab) {
    for doc in &mut tab.docs {
        if doc.dirty() || (doc.breakpoints.is_empty() && doc.applied.is_none()) {
            continue;
        }
        doc.compiled();
        let lines: Vec<u32> = doc
            .breakpoints
            .iter()
            .filter_map(|n| doc.compiled.node_lines.get(n).copied())
            .collect();
        if doc.applied.as_ref() != Some(&lines) {
            debug::set_breakpoints(world, &doc.path, &lines);
            doc.applied = Some(lines);
        }
    }
    let Some((from, left)) = tab.stepping else {
        return;
    };
    let Some(stop) = debug::stopped(world) else {
        return;
    };
    let Some(doc) = tab.docs.get_mut(tab.current) else {
        tab.stepping = None;
        return;
    };
    let here = stop
        .frames
        .first()
        .filter(|f| f.path == doc.path)
        .map(|f| doc.compiled().node_at(f.line));
    match here {
        // Still in the node it left from, or in the graph's own scaffolding: on.
        Some(node) if (node == from || node.is_none()) && left > 0 && stop.live => {
            if debug::go(world, Go::StepOver) {
                tab.stepping = Some((from, left - 1));
            }
        }
        _ => tab.stepping = None,
    }
}

/// While the scripts are stopped: where, its locals, and Continue and the steps (F5, F10 for
/// the next node, F11, Shift+F11).
fn stop_strip(ui: &mut egui::Ui, world: &mut World, tab: &mut GraphsTab) {
    let Some(stop) = debug::stopped(world) else {
        return;
    };
    let node = tab
        .docs
        .get_mut(tab.current)
        .and_then(|doc| stopped_node(world, doc));
    let in_graph = tab
        .docs
        .get(tab.current)
        .is_some_and(|d| stop.frames.iter().any(|f| f.path == d.path));
    let stepping = tab.stepping.is_some();
    let mut go = None;
    let key = |mods: egui::Modifiers, k: Key| ui.input_mut(|i| i.consume_key(mods, k));
    if !stepping {
        if key(egui::Modifiers::NONE, Key::F5) {
            go = Some(Go::Continue);
        } else if key(egui::Modifiers::NONE, Key::F10) {
            go = Some(Go::StepOver);
        } else if key(egui::Modifiers::SHIFT, Key::F11) {
            go = Some(Go::StepOut);
        } else if key(egui::Modifiers::NONE, Key::F11) {
            go = Some(Go::StepIn);
        }
    }
    egui::Frame::new()
        .fill(STOP_STRIP)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let buttons = [
                    ("▶ Continue", "F5", Go::Continue),
                    ("Next node", "F10: run on to the next node", Go::StepOver),
                    ("Step into", "F11", Go::StepIn),
                    ("Step out", "Shift+F11", Go::StepOut),
                ];
                for (label, tip, g) in buttons {
                    let enabled = !stepping && (stop.live || g == Go::Continue);
                    let text = if g == Go::Continue {
                        RichText::new(label).color(theme::PLAY).strong()
                    } else {
                        RichText::new(label)
                    };
                    if ui
                        .add_enabled(enabled, egui::Button::new(text))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        go = Some(g);
                    }
                }
                ui.add_space(8.0);
                let title = crate::scripts::stop_title(&stop);
                ui.label(RichText::new(title).color(STOPPED).strong());
                if let Some(e) = &stop.exception {
                    ui.label(RichText::new(e).color(ERROR));
                }
            });
            // The locals where it waits: the graph's variables and the values on its wires.
            if in_graph && let Some(doc) = tab.docs.get_mut(tab.current) {
                let luau = doc.compiled().luau.clone();
                if let Some(at) = debug::stop_in(world, &doc.path, &luau) {
                    let values: Vec<String> = at.values.into_values().collect();
                    if !values.is_empty() {
                        ui.label(
                            RichText::new(values.join("   "))
                                .monospace()
                                .color(STOP_VALUE),
                        );
                    }
                }
            }
        });
    if let Some(g) = go
        && debug::go(world, g)
        && g == Go::StepOver
        && in_graph
    {
        tab.stepping = Some((node, NODE_STEPS));
    }
}

fn paint_node(
    p: &egui::Painter,
    rect: Rect,
    spec: &NodeSpec,
    lit: f32,
    selected: bool,
    problems: &[&Problem],
    failed: bool,
) {
    let r = 7.0;
    p.add(
        egui::epaint::Shadow {
            offset: [2, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(150),
        }
        .as_shape(rect, r),
    );
    p.rect_filled(rect, r, NODE_BG);
    // The header: its category's color fading to the right, as Blueprints draw it.
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), look::HEADER));
    let base = rgb(look::category_color(&spec.category));
    let left = if lit > 0.0 {
        lerp_color(base, Color32::WHITE, 0.35 * lit)
    } else {
        base
    };
    let right = lerp_color(base, NODE_BG, 0.7);
    let mut mesh = egui::Mesh::default();
    let steps = 12;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = head.left() + head.width() * t;
        let c = lerp_color(left, right, t);
        mesh.colored_vertex(Pos2::new(x, head.top() + r * 0.5), c);
        mesh.colored_vertex(Pos2::new(x, head.bottom()), c);
        if i > 0 {
            let k = (i as u32) * 2;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k, k + 1);
        }
    }
    p.rect_filled(
        Rect::from_min_size(head.min, Vec2::new(head.width(), r + 1.0)),
        CornerRadius {
            nw: r as u8,
            ne: r as u8,
            sw: 0,
            se: 0,
        },
        lerp_color(left, right, 0.5),
    );
    p.add(Shape::mesh(mesh));
    p.line_segment(
        [head.left_bottom(), head.right_bottom()],
        Stroke::new(1.0, Color32::from_black_alpha(90)),
    );
    let outline = if problems.iter().any(|p| p.error) || failed {
        Some(Stroke::new(2.5, ERROR))
    } else if selected {
        Some(Stroke::new(2.0, NODE_SELECTED))
    } else if !problems.is_empty() {
        Some(Stroke::new(1.6, WARN))
    } else if lit > 0.0 {
        Some(Stroke::new(
            1.0 + 1.5 * lit,
            RAN.gamma_multiply(0.4 + 0.6 * lit),
        ))
    } else {
        None
    };
    if let Some(s) = outline {
        p.rect_stroke(rect, r, s, egui::StrokeKind::Outside);
    }
}

/// Comment boxes: drag the title to move one with the nodes in it, the corner to size it,
/// double-click the title to rename it.
fn comments(
    ui: &mut egui::Ui,
    doc: &mut GraphDoc,
    shapes: &BTreeMap<u32, (NodeSpec, look::Shape)>,
    texts: &mut SharpText,
) {
    let p = ui.painter().clone();
    let mut moved: Option<(usize, Vec2)> = None;
    let mut sized: Option<(usize, Vec2)> = None;
    let mut remove = None;
    for (i, c) in doc.graph.comments.iter().enumerate() {
        let rect = Rect::from_min_size(
            Pos2::new(c.at[0] as f32, c.at[1] as f32),
            Vec2::new(c.size[0] as f32, c.size[1] as f32),
        );
        let color = rgb(c.color);
        p.rect_filled(rect, 8.0, color.gamma_multiply(0.18));
        p.rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0, color.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        let title = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 30.0));
        p.rect_filled(
            title,
            CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            color.gamma_multiply(0.55),
        );
        let r = ui.interact(
            title,
            ui.id().with(("graph comment", i)),
            Sense::click_and_drag(),
        );
        if doc.editing_comment == Some(i) {
            continue;
        }
        texts.add(
            title.left_center() + Vec2::new(10.0, 0.0),
            Align2::LEFT_CENTER,
            &c.text,
            FontId::proportional(17.0),
            Color32::WHITE,
        );
        if r.dragged() {
            moved = Some((i, r.drag_delta()));
        }
        if r.double_clicked() {
            doc.editing_comment = Some(i);
        }
        r.context_menu(|ui| {
            if ui.button("Remove the comment").clicked() {
                remove = Some(i);
            }
        });
        let corner =
            Rect::from_center_size(rect.right_bottom() - Vec2::splat(6.0), Vec2::splat(14.0));
        let cr = ui.interact(
            corner,
            ui.id().with(("graph comment size", i)),
            Sense::drag(),
        );
        p.line_segment(
            [
                rect.right_bottom() - Vec2::new(12.0, 3.0),
                rect.right_bottom() - Vec2::new(3.0, 12.0),
            ],
            Stroke::new(1.2, color),
        );
        if cr.dragged() {
            sized = Some((i, cr.drag_delta()));
        }
    }
    if let Some(i) = doc.editing_comment
        && let Some(c) = doc.graph.comments.get_mut(i)
    {
        let title = Rect::from_min_size(
            Pos2::new(c.at[0] as f32 + 6.0, c.at[1] as f32 + 4.0),
            Vec2::new((c.size[0] as f32 - 12.0).max(80.0), 22.0),
        );
        let r = ui.put(
            title,
            egui::TextEdit::singleline(&mut c.text).font(FontId::proportional(16.0)),
        );
        r.request_focus();
        if r.lost_focus() {
            doc.editing_comment = None;
        }
    }
    if let Some((i, d)) = moved {
        let c = &mut doc.graph.comments[i];
        let rect = Rect::from_min_size(
            Pos2::new(c.at[0] as f32, c.at[1] as f32),
            Vec2::new(c.size[0] as f32, c.size[1] as f32),
        );
        c.at[0] += d.x.round() as i32;
        c.at[1] += d.y.round() as i32;
        for n in &mut doc.graph.nodes {
            let size = shapes.get(&n.id).map(|s| s.1.size).unwrap_or([0.0, 0.0]);
            let nr = Rect::from_min_size(
                Pos2::new(n.at[0] as f32, n.at[1] as f32),
                Vec2::new(size[0], size[1]),
            );
            if rect.contains_rect(nr) {
                n.at[0] += d.x.round() as i32;
                n.at[1] += d.y.round() as i32;
            }
        }
    }
    if let Some((i, d)) = sized {
        let c = &mut doc.graph.comments[i];
        c.size[0] = (c.size[0] + d.x.round() as i32).max(120);
        c.size[1] = (c.size[1] + d.y.round() as i32).max(60);
    }
    if let Some(i) = remove {
        doc.graph.comments.remove(i);
    }
}

/// Joins two pins, replacing what they can only have one of.
fn connect(doc: &mut GraphDoc, from: (u32, String, PinType), to: (u32, String, PinType)) {
    if !from.2.fits(to.2) {
        return;
    }
    let exec = from.2 == PinType::Exec;
    doc.graph.links.retain(|l| {
        if exec {
            // An execution output leads to one place.
            !(l.from.0 == from.0 && l.from.1 == from.1)
        } else {
            // A value input takes one wire.
            !(l.to.0 == to.0 && l.to.1 == to.1)
        }
    });
    doc.graph.links.push(Link {
        from: (from.0, from.1),
        to: (to.0, to.1),
    });
}

fn delete_selected(doc: &mut GraphDoc) {
    let gone = std::mem::take(&mut doc.selected);
    doc.graph.nodes.retain(|n| !gone.contains(&n.id));
    doc.graph
        .links
        .retain(|l| !gone.contains(&l.from.0) && !gone.contains(&l.to.0));
}

/// The selected nodes and the wires between them, as a graph of their own.
fn fragment(g: &Graph, ids: &BTreeSet<u32>) -> Graph {
    Graph {
        nodes: g
            .nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect(),
        links: g
            .links
            .iter()
            .filter(|l| ids.contains(&l.from.0) && ids.contains(&l.to.0))
            .cloned()
            .collect(),
        ..Default::default()
    }
}

/// Adds a fragment's nodes under new ids, moved by `by`, and selects them.
fn paste(doc: &mut GraphDoc, frag: &Graph, by: Vec2) {
    let mut ids = HashMap::new();
    doc.selected.clear();
    for (next, n) in (doc.graph.next_id()..).zip(&frag.nodes) {
        let mut m = n.clone();
        ids.insert(n.id, next);
        m.id = next;
        m.at[0] += by.x.round() as i32;
        m.at[1] += by.y.round() as i32;
        doc.selected.insert(next);
        doc.graph.nodes.push(m);
    }
    for l in &frag.links {
        if let (Some(a), Some(b)) = (ids.get(&l.from.0), ids.get(&l.to.0)) {
            doc.graph.links.push(Link {
                from: (*a, l.from.1.clone()),
                to: (*b, l.to.1.clone()),
            });
        }
    }
}

// ----------------------------------------------------------------------------- node search

/// Whether a node of `spec` can take the wire being drawn, and on which pin.
fn takes(spec: &NodeSpec, w: &Wiring) -> Option<String> {
    if w.output {
        spec.inputs
            .iter()
            .find(|p| {
                !p.fixed && w.ty.fits(p.ty) && (w.ty == PinType::Exec) == (p.ty == PinType::Exec)
            })
            .map(|p| p.name.clone())
    } else {
        spec.outputs
            .iter()
            .find(|p| p.ty.fits(w.ty) && (w.ty == PinType::Exec) == (p.ty == PinType::Exec))
            .map(|p| p.name.clone())
    }
}

fn matches(spec: &NodeSpec, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let title = spec.title.to_lowercase();
    if title.starts_with(&q) {
        return Some(3);
    }
    if title.contains(&q) || spec.of.to_lowercase().contains(&q) {
        return Some(2);
    }
    let all = format!("{} {} {}", spec.category, spec.kind, spec.doc).to_lowercase();
    q.split_whitespace()
        .all(|w| all.contains(w) || title.contains(w))
        .then_some(1)
}

/// The node search: type to filter, Enter or a click adds the node (wired, when a wire was
/// dropped into empty space), Escape closes it.
fn palette(ctx: &egui::Context, tab: &mut GraphsTab) {
    let Some(mut pal) = tab.palette.take() else {
        return;
    };
    let Some(doc) = tab.docs.get(tab.current) else {
        return;
    };
    let lib = specs_for(tab, &doc.graph);
    let mut found: Vec<(i32, &NodeSpec, Option<String>)> = lib
        .iter()
        .filter(|s| {
            !(s.kind == "var.get" && s.of.is_empty()) && !(s.kind == "var.set" && s.of.is_empty())
        })
        .filter(|s| !(s.kind == "prop.get" && s.of.is_empty()))
        .filter_map(|s| {
            let pin = match &pal.wiring {
                Some(w) => Some(takes(s, w)?),
                None => None,
            };
            matches(s, &pal.query).map(|score| (score, s, pin))
        })
        .collect();
    // Best match first; among equals, what the graph already uses, then by category.
    let used: BTreeSet<(&str, &str)> = doc
        .graph
        .nodes
        .iter()
        .map(|n| (n.kind.as_str(), n.of.as_str()))
        .collect();
    let in_use =
        |s: &NodeSpec| !s.of.is_empty() && used.contains(&(s.kind.as_str(), s.of.as_str()));
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| in_use(b.1).cmp(&in_use(a.1)))
            .then_with(|| category_rank(&a.1.category).cmp(&category_rank(&b.1.category)))
    });
    let searching = !pal.query.trim().is_empty();
    let mut close = false;
    let mut add: Option<(NodeSpec, Option<String>)> = None;
    let title = match &pal.wiring {
        Some(w) => format!(
            "Add a node for {} ({})",
            w.pin.replace('_', " "),
            type_name(w.ty)
        ),
        None => "Add a node".to_string(),
    };
    egui::Area::new(egui::Id::new("graph palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(pal.screen)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(330.0);
                ui.label(RichText::new(title).strong());
                let r = ui.add(
                    egui::TextEdit::singleline(&mut pal.query)
                        .hint_text("Search: light, delay, fire, branch...")
                        .desired_width(f32::INFINITY),
                );
                if pal.focus {
                    r.request_focus();
                    pal.focus = false;
                }
                if r.changed() {
                    pal.picked = 0;
                }
                let (down, up, enter, esc) = ui.input(|i| {
                    (
                        i.key_pressed(Key::ArrowDown),
                        i.key_pressed(Key::ArrowUp),
                        i.key_pressed(Key::Enter),
                        i.key_pressed(Key::Escape),
                    )
                });
                if down {
                    pal.picked = (pal.picked + 1).min(found.len().saturating_sub(1));
                }
                if up {
                    pal.picked = pal.picked.saturating_sub(1);
                }
                if esc {
                    close = true;
                }
                if enter && let Some((_, s, pin)) = found.get(pal.picked) {
                    add = Some(((*s).clone(), pin.clone()));
                }
                ui.add_space(2.0);
                egui::ScrollArea::vertical()
                    .max_height(340.0)
                    .show(ui, |ui| {
                        // Searching: one list, best first, each with its category. Browsing:
                        // grouped under the categories.
                        let mut category = "";
                        for (i, (_, s, pin)) in found.iter().enumerate().take(120) {
                            let top = s.category.split(':').next().unwrap_or(&s.category);
                            if !searching && top != category {
                                category = top;
                                ui.add_space(3.0);
                                ui.label(RichText::new(category).small().color(theme::TEXT_DIM));
                            }
                            let picked = i == pal.picked;
                            let dot = rgb(look::category_color(&s.category));
                            let r = ui
                                .horizontal(|ui| {
                                    let (r, p) =
                                        ui.allocate_painter(Vec2::new(8.0, 16.0), Sense::hover());
                                    p.circle_filled(r.rect.center(), 3.5, dot);
                                    let r = ui.add(egui::Button::selectable(picked, &s.title));
                                    if searching {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(top)
                                                        .small()
                                                        .color(theme::TEXT_DIM),
                                                )
                                            },
                                        );
                                    }
                                    r
                                })
                                .inner;
                            let r = r.on_hover_text(if s.doc.is_empty() {
                                s.kind.clone()
                            } else {
                                s.doc.clone()
                            });
                            if picked && (down || up) {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                add = Some(((*s).clone(), pin.clone()));
                            }
                        }
                        if found.is_empty() {
                            ui.label(theme::dim("Nothing matches."));
                        }
                    });
            });
        });
    if ctx.input(|i| i.pointer.any_pressed())
        && !ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| Rect::from_min_size(pal.screen, Vec2::new(350.0, 420.0)).contains(p))
    {
        close = true;
    }
    if let Some((spec, pin)) = add {
        let doc = &mut tab.docs[tab.current];
        let id = doc.graph.next_id();
        let mut at = pal.at;
        if let Some(w) = &pal.wiring {
            // Put the new node beside the pin it wires.
            at.y -= look::HEADER + look::ROW * 0.5;
            if !w.output {
                at.x -= 160.0;
            }
        }
        doc.graph.nodes.push(Node {
            id,
            kind: spec.kind.clone(),
            of: spec.of.clone(),
            at: [at.x.round() as i32, at.y.round() as i32],
            values: BTreeMap::new(),
        });
        if let (Some(w), Some(pin)) = (&pal.wiring, pin) {
            let ty = if w.output {
                spec.input_pin(&pin).map(|p| p.ty)
            } else {
                spec.output_pin(&pin).map(|p| p.ty)
            }
            .unwrap_or(PinType::Any);
            let (from, to) = if w.output {
                ((w.node, w.pin.clone(), w.ty), (id, pin, ty))
            } else {
                ((id, pin, ty), (w.node, w.pin.clone(), w.ty))
            };
            connect(doc, from, to);
        }
        doc.selected = BTreeSet::from([id]);
        close = true;
    }
    if !close {
        tab.palette = Some(pal);
    }
}

fn category_rank(c: &str) -> usize {
    let top = c.split(':').next().unwrap_or(c);
    [
        "Events",
        "Flow",
        "Variables",
        "Props",
        "Entity",
        "Math",
        "Text",
        "Components",
        "Engine",
    ]
    .iter()
    .position(|x| *x == top)
    .unwrap_or(9)
}

// ----------------------------------------------------------------------------- shots

const EXAMPLES: &str = "examples";

/// `editor --headless --size 1920x1080 --shots <dir> --script graphs`: the Graphs tab in use,
/// with the starter courtyard's lamp and stone running the example graphs. The scripts are
/// copied into `<dir>/scripts`, so the run edits copies and leaves the repo's files alone.
pub fn shots(dir: String) -> ShotScript {
    let _ = std::fs::create_dir_all(&dir);
    let root = std::fs::canonicalize(&dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&dir))
        .join("scripts");
    let setup = {
        let root = root.clone();
        move |world: &mut World, editor: &mut Editor| {
            editor.icons = false;
            dusk(world);
            let from = engine_script::scripts_root(world).join(EXAMPLES);
            copy_dir(&from, &root.join(EXAMPLES));
            world.resource_mut::<ScriptSettings>().folder = root.to_string_lossy().to_string();
            world.remove_resource::<ScriptRuntime>();
            for (entity, path) in [
                ("Lamp", "examples/pulse.graph"),
                ("Stone", "examples/hover.graph"),
            ] {
                if let Some(e) = crate::shots::find(world, entity) {
                    world.entity_mut(e).insert(Script::new(path));
                }
            }
            world.resource_mut::<EditorDock>().0 = shot_layout();
            let mut tab = world.resource_mut::<GraphsTab>();
            tab.files_next = 0.0;
            tab.library = None;
        }
    };
    let with_tab = |f: fn(&mut World, &mut GraphsTab)| {
        move |world: &mut World, _: &mut Editor| {
            world.resource_scope(|world, mut tab: Mut<GraphsTab>| f(world, &mut tab));
        }
    };
    let select = |kind: &'static str| {
        move |world: &mut World, _: &mut Editor| {
            let mut tab = world.resource_mut::<GraphsTab>();
            let i = tab.current;
            if let Some(doc) = tab.docs.get_mut(i) {
                doc.selected = doc
                    .graph
                    .nodes
                    .iter()
                    .filter(|n| n.kind == kind)
                    .map(|n| n.id)
                    .take(1)
                    .collect();
            }
        }
    };
    // The lamp's action row (Update, the light, the material) and the values feeding it.
    let close_up = |world: &mut World, _: &mut Editor| {
        let mut tab = world.resource_mut::<GraphsTab>();
        let i = tab.current;
        zoom_to(
            &mut tab.docs[i],
            &["event.update", "comp.set", "entity.material", "math.lerp"],
            0.8,
        );
    };
    let steps = vec![
        Step::Do(Box::new(setup)),
        Step::Select("Lamp"),
        Step::Tool(crate::editor::Tool::Select),
        Step::Camera(DVec3::new(-2.0, 2.1, 7.5), DVec3::new(1.6, 1.5, 1.6)),
        Step::Do(Box::new(with_tab(|world, tab| {
            open(world, tab, "examples/hover.graph");
            open(world, tab, "examples/pulse.graph");
        }))),
        Step::Tab(Tab::Graphs),
        Step::Wait(30),
        // The whole graph, the light node picked: its lines of Luau light up on the right.
        Step::Do(Box::new(select("comp.set"))),
        Step::Wait(10),
        Step::Shot("graphs_01_edit_mode"),
        // Play: the graph runs and is watched; values ride the wires, the nodes that run
        // light up and pulses run along their exec wires.
        Step::Do(Box::new(|world: &mut World, ed: &mut Editor| {
            editor::play(world, ed)
        })),
        Step::Do(Box::new(select(""))),
        Step::Do(Box::new(close_up)),
        Step::Wait(10),
        // Play starts the sky from the scene's time; back to the evening.
        Step::Do(Box::new(|world: &mut World, _: &mut Editor| dusk(world))),
        Step::Wait(140),
        Step::Shot("graphs_02_playing_live"),
        // A wire dragged from Update's exec pin into empty space: the search for what can
        // take it, typed "light".
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &tab.docs[tab.current];
            let Some(update) = doc.graph.nodes.iter().find(|n| n.kind == "event.update") else {
                return;
            };
            let wiring = Wiring {
                node: update.id,
                pin: "then".into(),
                output: true,
                ty: PinType::Exec,
            };
            let at = Pos2::new(update.at[0] as f32 + 150.0, update.at[1] as f32 + 150.0);
            let scale = doc.canvas.width() / doc.view.width().max(1.0);
            let screen = doc.canvas.min + (at - doc.view.min) * scale;
            tab.palette = Some(Palette {
                screen,
                at,
                query: "light".into(),
                wiring: Some(wiring),
                picked: 0,
                focus: true,
            });
        }))),
        Step::Wait(8),
        Step::Shot("graphs_03_node_search"),
        // Mistakes, before saving: a Branch with nothing on its condition, and a Print of
        // Update's dt on Interact's path, where dt doesn't exist.
        Step::Do(Box::new(with_tab(|_, tab| {
            tab.palette = None;
            let doc = &mut tab.docs[tab.current];
            let before = doc.graph.clone();
            let id = doc.graph.next_id();
            let (interact, branch, print) = (id, id + 1, id + 2);
            let y = doc.graph.nodes.iter().map(|n| n.at[1]).max().unwrap_or(0) + 140;
            let x = doc
                .graph
                .nodes
                .iter()
                .find(|n| n.kind == "event.update")
                .map_or(0, |n| n.at[0]);
            for (nid, kind, dx) in [
                (interact, "event.interact", 0),
                (branch, "flow.branch", 220),
                (print, "debug.print", 440),
            ] {
                doc.graph.nodes.push(Node {
                    id: nid,
                    kind: kind.into(),
                    of: String::new(),
                    at: [x + dx, y],
                    values: BTreeMap::new(),
                });
            }
            doc.graph.links.push(Link {
                from: (interact, "then".into()),
                to: (branch, "in".into()),
            });
            doc.graph.links.push(Link {
                from: (branch, "true".into()),
                to: (print, "in".into()),
            });
            if let Some(update) = doc.graph.nodes.iter().find(|n| n.kind == "event.update") {
                let uid = update.id;
                doc.graph.links.push(Link {
                    from: (uid, "dt".into()),
                    to: (print, "text".into()),
                });
            }
            doc.push_undo(before);
            zoom_to(doc, &["event.interact", "flow.branch", "debug.print"], 0.95);
            doc.selected = BTreeSet::from([print]);
        }))),
        Step::Wait(20),
        Step::Shot("graphs_04_mistakes"),
        // Undone, then an edit saved while playing: the lamp turns blue and pulses faster,
        // without restarting.
        Step::Do(Box::new(with_tab(|world, tab| {
            let doc = &mut tab.docs[tab.current];
            if let Some(g) = doc.undo.pop() {
                doc.graph = g;
            }
            doc.graph
                .props
                .insert("color".into(), json!([0.45, 0.75, 1.0]));
            doc.graph.props.insert("speed".into(), json!(6));
            doc.selected.clear();
            let now = editor::now(world);
            save(world, tab, now);
        }))),
        Step::Do(Box::new(close_up)),
        Step::Wait(90),
        Step::Shot("graphs_05_hot_reload"),
        // A breakpoint on the light node: the graph waits there (with the world's time), the
        // node outlined under an arrow and the values it has over the canvas.
        Step::Do(Box::new(with_tab(|_, tab| {
            let doc = &mut tab.docs[tab.current];
            if let Some(n) = doc.graph.nodes.iter().find(|n| n.kind == "comp.set") {
                doc.breakpoints.insert(n.id);
            }
        }))),
        Step::Wait(40),
        Step::Shot("graphs_06_breakpoint"),
        // Next node (F10): line steps until the graph is at the material node.
        Step::Do(Box::new(with_tab(|world, tab| {
            let node = tab
                .docs
                .get_mut(tab.current)
                .and_then(|doc| stopped_node(world, doc));
            if debug::go(world, Go::StepOver) {
                tab.stepping = Some((node, NODE_STEPS));
            }
        }))),
        Step::Wait(40),
        Step::Shot("graphs_07_next_node"),
        Step::Do(Box::new(with_tab(|world, tab| {
            for doc in &mut tab.docs {
                doc.breakpoints.clear();
            }
            breakpoints_and_steps(world, tab);
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

/// Evening light, held still, so the lamp shows.
fn dusk(world: &mut World) {
    let sky = world
        .resource::<TypeRegistry>()
        .resource("Sky")
        .map(|r| r.patch);
    if let Some(patch) = sky {
        let _ = patch(world, json!({"hour": 19.3, "hours_per_second": 0.0}));
    }
}

/// Centers `doc`'s view on its nodes of these kinds, at `zoom`.
fn zoom_to(doc: &mut GraphDoc, kinds: &[&str], zoom: f32) {
    let mut r = Rect::NOTHING;
    for n in doc
        .graph
        .nodes
        .iter()
        .filter(|n| kinds.contains(&n.kind.as_str()))
    {
        let at = Pos2::new(n.at[0] as f32, n.at[1] as f32);
        r = r.union(Rect::from_min_size(at, Vec2::new(200.0, 120.0)));
    }
    if r.is_positive() {
        doc.zoom_on = Some((r.center(), zoom));
    }
}

/// The shots' layout: the Graphs tab across the top (graphs are wide), the scene view and
/// Details under it.
fn shot_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Graphs]);
    let surface = dock.main_surface_mut();
    let [_graphs, below] = surface.split_below(NodeIndex::root(), 0.6, vec![Tab::Viewport]);
    let [_view, _details] = surface.split_right(below, 0.62, vec![Tab::Inspector]);
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

    fn doc_with(g: Graph) -> GraphDoc {
        GraphDoc {
            path: "t.graph".into(),
            saved: g.clone(),
            graph: g,
            stamp: None,
            undo: Vec::new(),
            redo: Vec::new(),
            gesture: false,
            selected: BTreeSet::new(),
            view: Rect::NOTHING,
            frame_all: false,
            zoom_on: None,
            canvas: Rect::NOTHING,
            compiled: Compiled::default(),
            compiled_of: None,
            marks: FileMarks::default(),
            marks_next: 0.0,
            live: Live::default(),
            ran: HashMap::new(),
            watched: None,
            wiring: None,
            boxing: None,
            editing_comment: None,
            breakpoints: BTreeSet::new(),
            applied: None,
        }
    }

    fn node(id: u32, kind: &str) -> Node {
        Node {
            id,
            kind: kind.into(),
            of: String::new(),
            at: [id as i32 * 200, 0],
            values: BTreeMap::new(),
        }
    }

    #[test]
    fn wiring_replaces_what_a_pin_takes_one_of() {
        let mut doc = doc_with(Graph {
            nodes: vec![
                node(1, "event.update"),
                node(2, "debug.print"),
                node(3, "debug.print"),
            ],
            ..Default::default()
        });
        connect(
            &mut doc,
            (1, "then".into(), PinType::Exec),
            (2, "in".into(), PinType::Exec),
        );
        connect(
            &mut doc,
            (1, "then".into(), PinType::Exec),
            (3, "in".into(), PinType::Exec),
        );
        assert_eq!(
            doc.graph.links.len(),
            1,
            "an exec output leads to one place"
        );
        assert_eq!(doc.graph.links[0].to.0, 3);
        connect(
            &mut doc,
            (1, "dt".into(), PinType::Number),
            (2, "text".into(), PinType::Text),
        );
        connect(
            &mut doc,
            (1, "dt".into(), PinType::Number),
            (3, "text".into(), PinType::Text),
        );
        assert_eq!(doc.graph.links.len(), 3, "a value goes to many inputs");
        // A bool doesn't fit a number.
        connect(
            &mut doc,
            (1, "x".into(), PinType::Bool),
            (2, "n".into(), PinType::Number),
        );
        assert_eq!(doc.graph.links.len(), 3);
    }

    #[test]
    fn copies_paste_under_new_ids_with_their_wires() {
        let mut doc = doc_with(Graph {
            nodes: vec![node(1, "event.update"), node(2, "debug.print")],
            links: vec![Link {
                from: (1, "then".into()),
                to: (2, "in".into()),
            }],
            ..Default::default()
        });
        let frag = fragment(&doc.graph, &BTreeSet::from([1, 2]));
        paste(&mut doc, &frag, Vec2::new(30.0, 30.0));
        assert_eq!(doc.graph.nodes.len(), 4);
        assert_eq!(doc.selected, BTreeSet::from([3, 4]));
        assert!(doc.graph.links.iter().any(|l| l.from.0 == 3 && l.to.0 == 4));
        delete_selected(&mut doc);
        assert_eq!(doc.graph.nodes.len(), 2);
        assert_eq!(doc.graph.links.len(), 1);
    }

    #[test]
    fn the_search_finds_nodes_that_take_a_wire() {
        let lib = graph::builtins();
        let w = Wiring {
            node: 1,
            pin: "dt".into(),
            output: true,
            ty: PinType::Number,
        };
        let sine = lib.iter().find(|s| s.kind == "math.sin").unwrap();
        assert_eq!(takes(sine, &w), Some("x".into()));
        let branch = lib.iter().find(|s| s.kind == "flow.branch").unwrap();
        assert_eq!(takes(branch, &w), None, "a number isn't a condition");
        assert!(matches(sine, "sin").is_some());
        assert!(matches(sine, "delay").is_none());
    }
}
