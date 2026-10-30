//! The Story tab: conversations (`.yarn` files in Yarn Spinner's language,
//! [`engine_script::story`]) drawn as a map of their nodes, with the story's facts and quests
//! beside it, all live while the game plays (Yarn Spinner's graph view, articy:draft's flow
//! view, Unreal's gameplay debugger for quests).
//!
//! The map: a card per node with its title, its `when:` conditions (nodes that share a title
//! are a node group: of those whose conditions hold, the one that tests the most runs) and its
//! lines, options and commands; arrows show where it jumps (solid), detours (dashed) and where
//! each option leads (labeled with the option). Cards sit at their `position:` headers, or are
//! laid out by how far they are from the start; dragging one writes its `position:` back into
//! the file, as Yarn Spinner's editors do. Double-click a card to edit its text in the Scripts
//! tab, where `.yarn` files are colored and their mistakes marked like Luau's.
//!
//! While the game plays: the node a conversation is in glows and the ones it went through are
//! tinted; each card shows how many times it was left. The conversation's transcript and its
//! options are on the right (click one to pick it), under them the facts (editable, also
//! before Play: they are the scene's starting facts) and the quests, which can be started,
//! completed or failed by hand. ▶ Talk starts this file's conversation at the selected node,
//! a line at a time, its commands run by the script whose `conversation` prop is this file.
//!
//! Choices made by walking (options tagged `#spot:<name>`, [`engine_script::spots`]): their
//! cards mark each option's spot, and "Walked to" on the right is the plan of the choice from
//! above (Pendragon's board; Unreal's top view to lay things out):
//! the speaker, the people round them, the player and each spot's ring, to drag where it
//! belongs (one undo step), a button to place a spot that is missing, and one to look down on
//! it all in the scene view. In the scene view each spot is a dashed ring with its name; while
//! the choice is open it shows what the game shows (each option over its ring, filling as the
//! player stands in it, the speaker's prompt and the time left) and the corner preview looks
//! through the game's view from above.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::SystemTime;

use egui::text::LayoutJob;
use egui::{
    Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextFormat,
    Vec2,
};
use engine::core::registry::call_api;
use engine::prelude::*;
use engine_script::Script;
use engine_script::spots::{SpotChoice, SpotChoices, SpotView, StorySpot};
use engine_script::story::{
    self, Compiled, NodeInfo, OptionInfo, QuestBook, QuestState, RunInfo, StoryFacts, StoryRuns,
};
use serde_json::{Value, json};

use crate::camera::EditorCamera;
use crate::editor::{self, Editor, Mode, Tab};
use crate::scene_ops::{self, InScene, SceneOrder};
use crate::theme;
use crate::ui::EditorDock;
use crate::undo::Command;
use crate::view::ViewInfo;

pub struct StoryTabPlugin;

impl Plugin for StoryTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StoryTab>();
    }
}

/// A card's width, its title row's height and a line's.
const CARD_W: f32 = 260.0;
const TITLE_H: f32 = 30.0;
const LINE_H: f32 = 19.0;
/// The lines a card shows at most.
const CARD_LINES: usize = 9;
/// Laid out by depth: a column's width and the gap between cards.
const COLUMN_W: f32 = 360.0;
const GAP: f32 = 34.0;
const SIDE_W: f32 = 360.0;

const CANVAS_BG: Color32 = Color32::from_rgb(0x16, 0x17, 0x1a);
const GRID: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x23);
const CARD_BG: Color32 = Color32::from_rgb(0x24, 0x26, 0x2b);
const CARD_TITLE: Color32 = Color32::from_rgb(0x2e, 0x46, 0x6e);
const CARD_GROUP: Color32 = Color32::from_rgb(0x4a, 0x36, 0x6e);
const SPEAKER: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);
const OPTION: Color32 = Color32::from_rgb(0x5f, 0xb8, 0xff);
const GROUP_LINE: Color32 = Color32::from_rgb(0xc0, 0x8c, 0xff);
const COMMAND: Color32 = Color32::from_rgb(0xd0, 0x7c, 0xd8);
const FACT: Color32 = Color32::from_rgb(0x7e, 0xe0, 0xa8);
const WHEN: Color32 = Color32::from_rgb(0xff, 0xb3, 0x6b);
const JUMP: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xaa);
const DETOUR: Color32 = Color32::from_rgb(0xc0, 0x8c, 0xff);
const NOW: Color32 = Color32::from_rgb(0x4c, 0xe0, 0x8a);
const TAKEN: Color32 = Color32::from_rgb(0x2a, 0x5a, 0x3e);
const PICKED: Color32 = Color32::from_rgb(0xff, 0xd0, 0x5a);
const ERROR: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
const COMMENT: Color32 = Color32::from_rgb(0x6a, 0x73, 0x7d);
/// A spot's ring: gold for a choice, silver-blue for the one taken if the player waits (the
/// game's colors).
const SPOT_WARM: Color32 = Color32::from_rgb(0xff, 0xb0, 0x48);
const SPOT_COOL: Color32 = Color32::from_rgb(0x8c, 0xc0, 0xff);
const OWNER: Color32 = Color32::from_rgb(0xe8, 0x6a, 0x58);
const PERSON: Color32 = Color32::from_rgb(0x7c, 0x82, 0x8c);

/// The Story tab's state.
#[derive(Resource)]
pub struct StoryTab {
    /// The `.yarn` file shown.
    pub path: String,
    files: Vec<String>,
    next_poll: f64,
    doc: Option<Doc>,
    /// The selected card (a node's index in the file).
    pub selected: Option<usize>,
    /// The map's middle (in map units) and its zoom.
    pub center: Pos2,
    pub zoom: f32,
    pub fit: bool,
    drag: Option<Drag>,
    /// The conversation ▶ Talk started.
    pub talking: Option<u64>,
    fact_edit: Option<(String, String)>,
    new_fact: String,
    toast: Option<(String, f64, bool)>,
    /// Where things were drawn (shot scripts click them).
    pub drawn_at: HashMap<String, Rect>,
    /// The choice's camera this tab pinned in the corner preview.
    pinned: Option<Entity>,
    /// A spot dragged in the plan, and its transform before.
    plan_drag: Option<(Entity, Transform)>,
    /// The scene view's width over its height, as last drawn.
    view_aspect: f32,
}

impl Default for StoryTab {
    fn default() -> Self {
        Self {
            path: "dialogue/keeper.yarn".into(),
            files: Vec::new(),
            next_poll: 0.0,
            doc: None,
            selected: None,
            center: Pos2::ZERO,
            zoom: 1.0,
            fit: true,
            drag: None,
            talking: None,
            fact_edit: None,
            new_fact: String::new(),
            toast: None,
            drawn_at: HashMap::new(),
            pinned: None,
            plan_drag: None,
            view_aspect: 16.0 / 9.0,
        }
    }
}

enum Drag {
    /// A card, and how far it has moved (map units).
    Card(usize, Vec2),
    Pan,
}

/// The file shown, compiled, and where each card goes.
struct Doc {
    path: String,
    text: String,
    stamp: Option<SystemTime>,
    compiled: Compiled,
    places: Vec<Pos2>,
    sizes: Vec<Vec2>,
}

/// Shows `path` ("dialogue/keeper").
pub fn open(world: &mut World, path: &str) {
    let mut tab = world.resource_mut::<StoryTab>();
    tab.path = story::normalize_yarn(path);
    tab.doc = None;
    tab.next_poll = 0.0;
    tab.selected = None;
    tab.fit = true;
}

pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let mut tab = world.remove_resource::<StoryTab>().unwrap_or_default();
    let now = editor::now(world);
    poll(world, &mut tab, now);
    tab.drawn_at.clear();
    egui::Panel::right("story side")
        .resizable(true)
        .default_size(SIDE_W)
        .size_range(260.0..=560.0)
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| side(ui, world, editor, &mut tab));
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            tool_bar(ui, world, editor, &mut tab, now);
            map(ui, world, editor, &mut tab, now);
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

/// Lists the `.yarn` files and reads the one shown again when it changed on disk.
fn poll(world: &World, tab: &mut StoryTab, now: f64) {
    if now < tab.next_poll && tab.doc.is_some() {
        return;
    }
    tab.next_poll = now + 0.5;
    let root = engine_script::scripts_root(world);
    tab.files = story::find_files(&root, story::DIALOGUE_FOLDER, "yarn");
    let full = root.join(&tab.path);
    let stamp = std::fs::metadata(&full).and_then(|m| m.modified()).ok();
    if tab
        .doc
        .as_ref()
        .is_some_and(|d| d.path == tab.path && d.stamp == stamp)
    {
        return;
    }
    let text = std::fs::read_to_string(&full).unwrap_or_default();
    let compiled = story::compile_yarn(&text, &tab.path);
    let sizes: Vec<Vec2> = compiled.nodes.iter().map(card_size).collect();
    let places = lay_out(&compiled.nodes, &sizes);
    if tab.selected.is_some_and(|i| i >= compiled.nodes.len()) {
        tab.selected = None;
    }
    tab.doc = Some(Doc {
        path: tab.path.clone(),
        text,
        stamp,
        compiled,
        places,
        sizes,
    });
}

fn card_size(n: &NodeInfo) -> Vec2 {
    let when = if n.when.is_empty() { 0.0 } else { LINE_H + 4.0 };
    let lines = n.lines.len().min(CARD_LINES) as f32;
    let more = if n.lines.len() > CARD_LINES { 1.0 } else { 0.0 };
    Vec2::new(CARD_W, TITLE_H + when + (lines + more) * LINE_H + 12.0)
}

/// Each card's top-left: its `position:` header, or by its distance from the start nodes in
/// links (a column each), in file order down a column.
fn lay_out(nodes: &[NodeInfo], sizes: &[Vec2]) -> Vec<Pos2> {
    let mut depth: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut queue = VecDeque::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.title == "Start" || (i == 0 && !nodes.iter().any(|m| m.title == "Start")) {
            depth[i] = Some(0);
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let d = depth[i].unwrap_or(0);
        for (target, _, _) in &nodes[i].links {
            for (j, m) in nodes.iter().enumerate() {
                if &m.title == target && depth[j].is_none() {
                    depth[j] = Some(d + 1);
                    queue.push_back(j);
                }
            }
        }
    }
    let deepest = depth.iter().flatten().max().copied().unwrap_or(0);
    let mut column_y: BTreeMap<usize, f32> = BTreeMap::new();
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if let Some((x, y)) = n.position {
                return Pos2::new(x, y);
            }
            let d = depth[i].unwrap_or(deepest + 1);
            let y = column_y.entry(d).or_insert(0.0);
            let at = Pos2::new(d as f32 * COLUMN_W, *y);
            *y += sizes[i].y + GAP;
            at
        })
        .collect()
}

// ------------------------------------------------------------------------------ toolbar

fn tool_bar(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut StoryTab,
    now: f64,
) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let mut pick = None;
        egui::ComboBox::from_id_salt("story file")
            .selected_text(RichText::new(&tab.path).monospace())
            .width(260.0)
            .show_ui(ui, |ui| {
                for f in &tab.files {
                    if ui.selectable_label(*f == tab.path, f).clicked() {
                        pick = Some(f.clone());
                    }
                }
            });
        if let Some(p) = pick {
            tab.path = p;
            tab.doc = None;
            tab.next_poll = 0.0;
            tab.selected = None;
            tab.fit = true;
        }
        let playing = editor.mode != Mode::Edit;
        let node = tab
            .selected
            .and_then(|i| tab.doc.as_ref()?.compiled.nodes.get(i))
            .map(|n| n.title.clone())
            .unwrap_or_else(|| "Start".into());
        let talk = ui
            .add_enabled(playing, egui::Button::new(format!("▶ Talk from {node}")))
            .on_hover_text(
                "Starts this file's conversation at the selected node, a line at a time; its \
                 commands run on the script whose conversation prop is this file",
            )
            .on_disabled_hover_text("Press Play first: conversations run in the game");
        tab.drawn_at.insert("talk".into(), talk.rect);
        if talk.clicked() {
            talk_from(world, tab, &node, now);
        }
        if ui
            .button("📜 Edit text")
            .on_hover_text("Opens the file in the Scripts tab (at the selected node)")
            .clicked()
        {
            let line = tab
                .selected
                .and_then(|i| tab.doc.as_ref()?.compiled.nodes.get(i))
                .map(|n| n.line);
            show_in_scripts(world, &tab.path, line);
        }
        if ui
            .button("⛶ Fit")
            .on_hover_text("Fits the map in view")
            .clicked()
        {
            tab.fit = true;
        }
        if let Some(doc) = &tab.doc {
            ui.separator();
            let groups = group_sizes(&doc.compiled.nodes);
            ui.label(theme::dim(format!(
                "{} nodes · {} node groups · {} facts declared",
                doc.compiled.nodes.len(),
                groups.values().filter(|n| **n > 1).count(),
                doc.compiled.declares.len()
            )));
            if let Some(p) = doc.compiled.problems.first() {
                let more = doc.compiled.problems.len() - 1;
                let text = if more > 0 {
                    format!("⚠ line {}: {} (+{more})", p.line, p.message)
                } else {
                    format!("⚠ line {}: {}", p.line, p.message)
                };
                if ui
                    .add(egui::Label::new(RichText::new(text).color(ERROR)).sense(Sense::click()))
                    .on_hover_text("Click to see it in the Scripts tab")
                    .clicked()
                {
                    show_in_scripts(world, &tab.path, Some(p.line));
                }
            }
        }
    });
    ui.add_space(2.0);
}

/// Starts a conversation of this file at `node`, a line at a time.
fn talk_from(world: &mut World, tab: &mut StoryTab, node: &str, now: f64) {
    let owner = owner_of(world, &tab.path);
    let mut args = json!({"path": tab.path, "node": node, "headless": true, "step": true});
    if let Some(name) = owner {
        args["owner"] = json!(name);
    }
    match call_api(world, "story.start", args) {
        Ok(v) => tab.talking = v["id"].as_u64(),
        Err(e) => tab.toast = Some((e, now + 5.0, true)),
    }
}

/// The name of the entity whose script talks from `path` (its `conversation` prop, set on the
/// entity or the class's default).
fn owner_of(world: &mut World, path: &str) -> Option<String> {
    let want = story::normalize_yarn(path);
    let mut q = world.query::<(&Name, &Script)>();
    let scripted: Vec<(String, Script)> = q
        .iter(world)
        .map(|(n, s)| (n.as_str().to_string(), s.clone()))
        .collect();
    scripted.into_iter().find_map(|(name, s)| {
        let set = s
            .props
            .get("conversation")
            .and_then(Value::as_str)
            .map(str::to_string);
        let talks = set.or_else(|| {
            engine_script::props::of(world, &s.path)
                .ok()?
                .into_iter()
                .find(|p| p.name == "conversation")?
                .default
                .as_str()
                .map(str::to_string)
        });
        talks
            .is_some_and(|c| story::normalize_yarn(&c) == want)
            .then_some(name)
    })
}

pub fn show_in_scripts(world: &mut World, path: &str, line: Option<u32>) {
    world.resource_scope(|world, mut scripts: Mut<crate::scripts::ScriptsTab>| {
        crate::scripts::open(world, &mut scripts, path, line)
    });
    let mut dock = world.resource_mut::<EditorDock>();
    if let Some(at) = dock.0.find_tab(&Tab::Scripts) {
        let _ = dock.0.set_active_tab(at);
    }
}

fn group_sizes(nodes: &[NodeInfo]) -> HashMap<&str, usize> {
    let mut out: HashMap<&str, usize> = HashMap::new();
    for n in nodes {
        *out.entry(n.title.as_str()).or_default() += 1;
    }
    out
}

// ---------------------------------------------------------------------------------- map

/// Live state for the map: the conversation of this file to show, and how often each node
/// was left.
struct Live {
    run: Option<RunInfo>,
    visits: BTreeMap<String, u32>,
}

fn live(world: &World, tab: &StoryTab) -> Live {
    let runs = world.get_resource::<StoryRuns>();
    let run = runs.and_then(|r| {
        tab.talking
            .and_then(|id| r.get(id))
            .filter(|x| x.path == tab.path)
            .or_else(|| r.runs.iter().rev().find(|x| x.path == tab.path))
            .cloned()
    });
    let visits = world
        .get_resource::<StoryFacts>()
        .map(|f| f.visits.clone())
        .unwrap_or_default();
    Live { run, visits }
}

fn map(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut StoryTab, now: f64) {
    let outer = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(outer, Sense::click_and_drag());
    let painter = ui.painter_at(outer);
    painter.rect_filled(outer, 0.0, CANVAS_BG);
    let Some(doc) = tab.doc.as_ref() else {
        return;
    };
    let nodes = &doc.compiled.nodes;
    if nodes.is_empty() {
        painter.text(
            outer.center(),
            Align2::CENTER_CENTER,
            if doc.text.is_empty() {
                format!("{} is empty or missing: pick a file above", tab.path)
            } else {
                "No nodes: a node is headers (title: Name), ---, its lines, then ===".into()
            },
            FontId::proportional(14.0),
            theme::TEXT_DIM,
        );
        return;
    }
    let places: Vec<Pos2> = (0..nodes.len())
        .map(|i| match &tab.drag {
            Some(Drag::Card(j, by)) if *j == i => doc.places[i] + *by,
            _ => doc.places[i],
        })
        .collect();
    let bounds = places
        .iter()
        .zip(&doc.sizes)
        .fold(Rect::NOTHING, |r, (p, s)| {
            r.union(Rect::from_min_size(*p, *s))
        });
    if tab.fit {
        tab.fit = false;
        let r = bounds.expand(50.0);
        tab.center = r.center();
        tab.zoom = (outer.width() / r.width())
            .min(outer.height() / r.height())
            .clamp(0.3, 1.0);
    }
    // Wheel zooms about the pointer; dragging the background (or with the right or middle
    // button anywhere) pans.
    if let Some(hover) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let before = to_map(outer, tab, hover);
            tab.zoom = (tab.zoom * (scroll * 0.0015).exp()).clamp(0.25, 2.0);
            let after = to_map(outer, tab, hover);
            tab.center += before - after;
        }
    }
    let zoom = tab.zoom;
    let to_screen = |p: Pos2| outer.center() + (p - tab.center) * zoom;
    let rects: Vec<Rect> = places
        .iter()
        .zip(&doc.sizes)
        .map(|(p, s)| Rect::from_min_size(to_screen(*p), *s * zoom))
        .collect();

    // The grid.
    let step = 40.0 * zoom;
    if step > 6.0 {
        let o = to_screen(Pos2::ZERO);
        let mut x = outer.left() + (o.x - outer.left()).rem_euclid(step);
        while x < outer.right() {
            painter.line_segment(
                [Pos2::new(x, outer.top()), Pos2::new(x, outer.bottom())],
                Stroke::new(1.0, GRID),
            );
            x += step;
        }
        let mut y = outer.top() + (o.y - outer.top()).rem_euclid(step);
        while y < outer.bottom() {
            painter.line_segment(
                [Pos2::new(outer.left(), y), Pos2::new(outer.right(), y)],
                Stroke::new(1.0, GRID),
            );
            y += step;
        }
    }

    let live = live(world, tab);
    let current = live
        .run
        .as_ref()
        .filter(|r| !r.done)
        .map(|r| (r.node.clone(), r.node_line));
    let taken: HashSet<&str> = live
        .run
        .as_ref()
        .map(|r| r.taken.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let groups = group_sizes(nodes);
    let spot_names = engine_script::spots::spot_names(world);

    // Arrows first, under the cards.
    for (i, n) in nodes.iter().enumerate() {
        for (k, (target, how, line)) in n.links.iter().enumerate() {
            // The row it leaves from: the last one shown at or before its line (a jump under
            // an option leaves from the option's row).
            let row = n.lines.iter().rposition(|(l, _)| l <= line);
            let from_y = match row {
                Some(r) if r < CARD_LINES => {
                    rects[i].top() + (TITLE_H + when_h(n) + (r as f32 + 0.5) * LINE_H) * zoom
                }
                _ => rects[i].center().y,
            };
            for (j, m) in nodes.iter().enumerate() {
                if &m.title != target {
                    continue;
                }
                let a = Pos2::new(rects[i].right(), from_y);
                let b = Pos2::new(rects[j].left(), rects[j].top() + TITLE_H * 0.5 * zoom);
                let color = match how.as_str() {
                    "jump" => JUMP,
                    "detour" => DETOUR,
                    _ => OPTION,
                };
                let lit = current.as_ref().is_some_and(|(t, _)| t == &m.title)
                    && taken.contains(n.title.as_str());
                let stroke = Stroke::new(if lit { 2.6 } else { 1.7 } * zoom.max(0.6), color);
                let pts = arrow_path(a, b, i == j, k);
                if how == "detour" {
                    let line: Vec<Pos2> = flatten(&pts);
                    painter.extend(Shape::dashed_line(&line, stroke, 7.0, 5.0));
                } else {
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        pts,
                        false,
                        Color32::TRANSPARENT,
                        stroke,
                    ));
                }
                arrow_head(
                    &painter,
                    pts[3],
                    pts[3] - pts[2],
                    9.0 * zoom.max(0.6),
                    color,
                );
                if how != "jump" && how != "detour" && row.is_none_or(|r| r >= CARD_LINES) {
                    // The option's text, near where the arrow leaves.
                    painter.text(
                        a + Vec2::new(8.0, -8.0),
                        Align2::LEFT_BOTTOM,
                        short(how, 28),
                        FontId::proportional(11.0 * zoom.max(0.7)),
                        color,
                    );
                }
            }
        }
    }

    // The cards.
    let mut clicked_card = None;
    let mut double_card = None;
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let over_card = pointer.and_then(|p| rects.iter().rposition(|r| r.contains(p)));
    for (i, n) in nodes.iter().enumerate() {
        let r = rects[i];
        if !r.intersects(outer) {
            continue;
        }
        tab.drawn_at.insert(format!("node {}", n.title), r);
        let is_now = current
            .as_ref()
            .is_some_and(|(t, line)| *t == n.title && (*line == 0 || *line == n.line));
        let was = taken.contains(n.title.as_str());
        let selected = tab.selected == Some(i);
        let round = 6.0 * zoom;
        if is_now {
            let pulse = 0.5 + 0.5 * (now * 4.0).sin() as f32;
            painter.rect_filled(
                r.expand(6.0 * zoom),
                round + 4.0,
                NOW.gamma_multiply(0.18 + 0.2 * pulse),
            );
        }
        painter.rect_filled(
            r,
            round,
            if was {
                mix(CARD_BG, TAKEN, 0.45)
            } else {
                CARD_BG
            },
        );
        let in_group = groups.get(n.title.as_str()).copied().unwrap_or(1) > 1;
        let title_rect = Rect::from_min_size(r.min, Vec2::new(r.width(), TITLE_H * zoom));
        painter.rect_filled(
            title_rect,
            egui::CornerRadius {
                nw: round as u8,
                ne: round as u8,
                sw: 0,
                se: 0,
            },
            if in_group { CARD_GROUP } else { CARD_TITLE },
        );
        let font = |size: f32| FontId::proportional(size * zoom);
        let mono = |size: f32| FontId::monospace(size * zoom);
        painter.text(
            title_rect.left_center() + Vec2::new(10.0 * zoom, 0.0),
            Align2::LEFT_CENTER,
            &n.title,
            font(14.5),
            Color32::WHITE,
        );
        // Right of the title: which of a group it is, and how often it was left.
        let mut right = title_rect.right() - 8.0 * zoom;
        let left_n = live.visits.get(&n.title).copied().unwrap_or(0);
        if left_n > 0 {
            let g = painter.text(
                Pos2::new(right, title_rect.center().y),
                Align2::RIGHT_CENTER,
                format!("×{left_n}"),
                mono(12.0),
                FACT,
            );
            right = g.left() - 8.0 * zoom;
        }
        if in_group {
            let k = nodes[..=i].iter().filter(|m| m.title == n.title).count();
            painter.text(
                Pos2::new(right, title_rect.center().y),
                Align2::RIGHT_CENTER,
                format!("{k} of {}", groups[n.title.as_str()]),
                font(11.5),
                Color32::from_rgb(0xd8, 0xc8, 0xff),
            );
        }
        let mut y = title_rect.bottom() + 5.0 * zoom;
        if !n.when.is_empty() {
            let mut x = r.left() + 8.0 * zoom;
            for w in &n.when {
                let text = if w == "once" || w == "always" {
                    w.clone()
                } else {
                    short(w, 30)
                };
                let g = painter.layout_no_wrap(format!("when {text}"), mono(11.0), WHEN);
                let chip =
                    Rect::from_min_size(Pos2::new(x, y), g.size() + Vec2::new(10.0, 4.0) * zoom);
                painter.rect_filled(chip, 4.0 * zoom, WHEN.gamma_multiply(0.16));
                painter.galley(chip.min + Vec2::new(5.0, 2.0) * zoom, g, WHEN);
                x = chip.right() + 5.0 * zoom;
            }
            y += (LINE_H + 4.0) * zoom;
        }
        for (k, (line, text)) in n.lines.iter().enumerate() {
            if k >= CARD_LINES {
                painter.text(
                    Pos2::new(r.left() + 10.0 * zoom, y + LINE_H * 0.5 * zoom),
                    Align2::LEFT_CENTER,
                    format!("… {} more", n.lines.len() - CARD_LINES),
                    font(11.5),
                    theme::TEXT_DIM,
                );
                break;
            }
            let said_now = live
                .run
                .as_ref()
                .filter(|run| !run.done)
                .and_then(|run| run.lines.last())
                .is_some_and(|l| l.line == *line && is_now);
            if said_now {
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(r.left() + 3.0 * zoom, y),
                        Vec2::new(r.width() - 6.0 * zoom, LINE_H * zoom),
                    ),
                    3.0 * zoom,
                    NOW.gamma_multiply(0.25),
                );
            }
            // An option walked to: its spot on the right ("○ ear"), red when the scene has
            // no spot of that name.
            let mut width = r.width() - 20.0 * zoom;
            if let Some(o) = n.options.iter().find(|o| o.line == *line)
                && let Some(name) = &o.spot
            {
                let missing = !spot_names.contains(name);
                let (mark, color) = match (missing, o.fallback) {
                    (true, _) => ("⚠", ERROR),
                    (false, true) => ("○", SPOT_COOL),
                    (false, false) => ("○", SPOT_WARM),
                };
                let g = painter.layout_no_wrap(format!("{mark} {name}"), mono(10.5), color);
                let size = g.size() + Vec2::new(10.0, 3.0) * zoom;
                let chip = Rect::from_min_size(
                    Pos2::new(
                        r.right() - 7.0 * zoom - size.x,
                        y + (LINE_H * zoom - size.y) * 0.5,
                    ),
                    size,
                );
                painter.rect_filled(chip, 4.0 * zoom, color.gamma_multiply(0.15));
                painter.galley(chip.min + Vec2::new(5.0, 1.5) * zoom, g, color);
                width = chip.left() - r.left() - 16.0 * zoom;
            }
            let job = card_line(text, 12.0 * zoom, width.max(20.0));
            let g = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(
                Pos2::new(
                    r.left() + 10.0 * zoom,
                    y + (LINE_H * zoom - g.size().y) * 0.5,
                ),
                g,
                theme::TEXT,
            );
            y += LINE_H * zoom;
        }
        let edge = if is_now {
            Stroke::new(2.4, NOW)
        } else if selected {
            Stroke::new(2.0, theme::ACCENT)
        } else {
            Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3d, 0x44))
        };
        painter.rect_stroke(r, round, edge, StrokeKind::Outside);
        if over_card == Some(i) {
            if response.double_clicked() {
                double_card = Some(i);
            } else if response.clicked() || response.drag_started_by(egui::PointerButton::Primary) {
                clicked_card = Some(i);
            }
        }
    }

    // Dragging: a card moves (written to the file when let go); the background pans.
    if response.drag_started_by(egui::PointerButton::Primary) {
        tab.drag = Some(match over_card {
            Some(i) => Drag::Card(i, Vec2::ZERO),
            None => Drag::Pan,
        });
    } else if response.drag_started() {
        tab.drag = Some(Drag::Pan);
    }
    if response.dragged() {
        let d = response.drag_delta() / zoom;
        match &mut tab.drag {
            Some(Drag::Card(_, by)) => *by += d,
            Some(Drag::Pan) => tab.center -= d,
            None => {}
        }
    }
    if response.drag_stopped() {
        if let Some(Drag::Card(i, by)) = tab.drag.take()
            && by.length() > 1.0
        {
            let at = places[i];
            place_card(world, tab, i, at, now);
        }
        tab.drag = None;
    }
    if let Some(i) = clicked_card {
        tab.selected = Some(i);
    } else if response.clicked() && over_card.is_none() {
        tab.selected = None;
    }
    if let Some(i) = double_card
        && let Some(n) = tab.doc.as_ref().and_then(|d| d.compiled.nodes.get(i))
    {
        let line = n.line;
        let path = tab.path.clone();
        show_in_scripts(world, &path, Some(line));
    }
    let _ = editor;

    // Corner: what the map shows.
    let hint = if live.run.as_ref().is_some_and(|r| !r.done) {
        "LIVE · the green card is where the conversation is"
    } else {
        "Drag a card to move it (saved in its position: header) · double-click to edit · wheel zooms"
    };
    painter.text(
        outer.left_bottom() + Vec2::new(10.0, -10.0),
        Align2::LEFT_BOTTOM,
        hint,
        FontId::proportional(12.0),
        theme::TEXT_DIM,
    );
}

fn when_h(n: &NodeInfo) -> f32 {
    if n.when.is_empty() { 0.0 } else { LINE_H + 4.0 }
}

fn to_map(outer: Rect, tab: &StoryTab, screen: Pos2) -> Pos2 {
    tab.center + (screen - outer.center()) / tab.zoom
}

/// A curve from `a` (a card's right side) to `b` (another's left side); a loop back to its
/// own card goes round above it. `k` spreads the arrows leaving one card.
fn arrow_path(a: Pos2, b: Pos2, own: bool, k: usize) -> [Pos2; 4] {
    if own {
        let up = 40.0 + 10.0 * k as f32;
        return [a, a + Vec2::new(60.0, -up), b + Vec2::new(-60.0, -up), b];
    }
    let dx = ((b.x - a.x).abs() * 0.5).max(60.0);
    [a, a + Vec2::new(dx, 0.0), b - Vec2::new(dx, 0.0), b]
}

fn flatten(pts: &[Pos2; 4]) -> Vec<Pos2> {
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let p = pts[0].to_vec2() * u * u * u
                + pts[1].to_vec2() * 3.0 * u * u * t
                + pts[2].to_vec2() * 3.0 * u * t * t
                + pts[3].to_vec2() * t * t * t;
            p.to_pos2()
        })
        .collect()
}

fn arrow_head(p: &egui::Painter, tip: Pos2, dir: Vec2, size: f32, color: Color32) {
    let d = if dir.length() < 1e-3 {
        Vec2::X
    } else {
        dir.normalized()
    };
    let n = Vec2::new(-d.y, d.x);
    p.add(Shape::convex_polygon(
        vec![
            tip,
            tip - d * size + n * size * 0.5,
            tip - d * size - n * size * 0.5,
        ],
        color,
        Stroke::NONE,
    ));
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn short(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(n - 1).collect::<String>())
    }
}

/// A card's line: `Speaker: text` with the speaker colored, options and line groups with
/// their arrows, commands in their own color, `{expressions}` and `$facts` picked out.
fn card_line(text: &str, size: f32, width: f32) -> LayoutJob {
    let font = FontId::proportional(size);
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let add = |job: &mut LayoutJob, s: &str, color: Color32| {
        job.append(s, 0.0, TextFormat::simple(font.clone(), color));
    };
    if text.starts_with("<<") {
        add(&mut job, text, COMMAND);
        return job;
    }
    let (mark, rest, color) = if let Some(r) = text.strip_prefix("-> ") {
        ("› ", r, OPTION)
    } else if let Some(r) = text.strip_prefix("=> ") {
        ("» ", r, GROUP_LINE)
    } else {
        ("", text, theme::TEXT)
    };
    if !mark.is_empty() {
        add(&mut job, mark, color);
    }
    let rest = match rest.split_once(": ") {
        Some((who, said))
            if !who.is_empty() && who.len() <= 40 && !who.contains(['{', '<', '$', '"']) =>
        {
            add(&mut job, who, SPEAKER);
            add(&mut job, ": ", theme::TEXT_DIM);
            said
        }
        _ => rest,
    };
    // `{...}` and `$name` in the text.
    let mut plain = String::new();
    let mut chars = rest.chars().peekable();
    let base = if mark.is_empty() { theme::TEXT } else { color };
    while let Some(c) = chars.next() {
        if c == '{' {
            add(&mut job, &std::mem::take(&mut plain), base);
            let mut inner = String::from("{");
            for d in chars.by_ref() {
                inner.push(d);
                if d == '}' {
                    break;
                }
            }
            add(&mut job, &inner, FACT);
        } else {
            plain.push(c);
        }
    }
    add(&mut job, &plain, base);
    job
}

/// Writes the card's place into its node's `position:` header (adding the header after its
/// title line if it has none).
fn place_card(world: &mut World, tab: &mut StoryTab, i: usize, at: Pos2, now: f64) {
    let Some(doc) = tab.doc.as_mut() else {
        return;
    };
    let Some(n) = doc.compiled.nodes.get(i) else {
        return;
    };
    let mut lines: Vec<String> = doc.text.lines().map(str::to_string).collect();
    let title_line = (n.line as usize).saturating_sub(1);
    // The node's headers: from its first header line to its `---`.
    let mut start = title_line;
    while start > 0 {
        let l = lines[start - 1].trim();
        if l.is_empty() || l == "===" || l.starts_with("//") || !l.contains(':') {
            break;
        }
        start -= 1;
    }
    let mut end = title_line;
    while end < lines.len() && lines[end].trim() != "---" {
        end += 1;
    }
    let header = format!("position: {},{}", at.x.round() as i64, at.y.round() as i64);
    match (start..end).find(|k| lines[*k].trim_start().starts_with("position:")) {
        Some(k) => lines[k] = header,
        None => {
            let k = (start..end)
                .find(|k| lines[*k].trim_start().starts_with("title:"))
                .unwrap_or(start);
            lines.insert(k + 1, header);
        }
    }
    let mut text = lines.join("\n");
    if doc.text.ends_with('\n') {
        text.push('\n');
    }
    match call_api(
        world,
        "script.write",
        json!({"path": doc.path, "source": text}),
    ) {
        Ok(_) => {
            doc.places[i] = at;
            doc.text = text.clone();
            doc.compiled = story::compile_yarn(&text, &doc.path);
            doc.stamp = std::fs::metadata(engine_script::scripts_root(world).join(&doc.path))
                .and_then(|m| m.modified())
                .ok();
        }
        Err(e) => tab.toast = Some((format!("Can't save the card's place: {e}"), now + 5.0, true)),
    }
}

// --------------------------------------------------------------------------------- side

fn side(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut StoryTab) {
    let playing = editor.mode != Mode::Edit;
    conversation(ui, world, tab, playing);
    ui.add_space(8.0);
    walk_plan(ui, world, editor, tab);
    facts(ui, world, tab);
    ui.add_space(8.0);
    quests(ui, world, playing);
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong().size(14.0).color(theme::TEXT));
    ui.add_space(2.0);
}

fn conversation(ui: &mut egui::Ui, world: &mut World, tab: &mut StoryTab, playing: bool) {
    heading(ui, "💬 Conversation");
    let run = live(world, tab).run;
    let Some(run) = run else {
        ui.label(theme::dim(if playing {
            "None yet. ▶ Talk starts this file at the selected node; talking to its owner in \
             the game starts one too."
        } else {
            "Conversations run while the game plays: press Play, then ▶ Talk (or talk to the \
             character in the game)."
        }));
        return;
    };
    let state = if let Some(e) = &run.error {
        RichText::new(format!("stopped: {e}")).color(ERROR)
    } else if run.done {
        RichText::new("ended").color(theme::TEXT_DIM)
    } else {
        RichText::new(format!("in {} · {}", run.node, waiting_text(&run.waiting))).color(NOW)
    };
    ui.label(state);
    egui::Frame::new()
        .fill(Color32::from_rgb(0x18, 0x19, 0x1c))
        .corner_radius(6.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let shown = run.lines.len().saturating_sub(14);
            if shown > 0 {
                ui.label(theme::dim(format!("… {shown} lines before")));
            }
            for l in &run.lines[shown..] {
                if l.choice {
                    ui.label(RichText::new(format!("➡ {}", l.text)).color(PICKED));
                } else if l.speaker.is_empty() {
                    ui.label(RichText::new(&l.text).italics().color(theme::TEXT));
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new(&l.speaker).strong().color(SPEAKER));
                        ui.label(RichText::new(&l.text).color(theme::TEXT));
                    });
                }
            }
        });
    if run.done {
        return;
    }
    let mut pick = None;
    // Picked by walking: each option's spot, how far the player is into its ring, the time.
    let walk = world
        .get_resource::<SpotChoices>()
        .and_then(|c| c.of_run(run.id).cloned());
    if !run.options.is_empty() {
        if let Some(c) = &walk {
            let left = c
                .seconds_left()
                .map(|s| format!(" · {} s left", s.ceil() as i32))
                .unwrap_or_default();
            ui.label(
                RichText::new(format!(
                    "Walk to choose: stand in a ring or press E there (or click){left}"
                ))
                .small()
                .color(SPOT_WARM),
            );
        }
        for (i, o) in run.options.iter().enumerate() {
            let spot = walk.as_ref().and_then(|c| c.options.get(i));
            let mut job = LayoutJob::default();
            let font = FontId::proportional(14.0);
            job.append(
                &format!("{}. {o}", i + 1),
                0.0,
                TextFormat::simple(font.clone(), OPTION),
            );
            if let Some(name) = spot.and_then(|s| s.spot_name.as_ref()) {
                let color = if spot.is_some_and(|s| s.fallback) {
                    SPOT_COOL
                } else {
                    SPOT_WARM
                };
                job.append(
                    &format!("○ {name}"),
                    10.0,
                    TextFormat::simple(FontId::monospace(11.5), color),
                );
            }
            let b = ui.add(egui::Button::new(job).min_size(Vec2::new(ui.available_width(), 26.0)));
            if let Some(s) = spot.filter(|s| s.progress > 0.0) {
                let color = if s.fallback { SPOT_COOL } else { SPOT_WARM };
                let bar = Rect::from_min_size(
                    b.rect.left_bottom() + Vec2::new(3.0, -4.0),
                    Vec2::new((b.rect.width() - 6.0) * s.progress, 3.0),
                );
                ui.painter().rect_filled(bar, 1.5, color);
            }
            tab.drawn_at.insert(format!("option {}", i + 1), b.rect);
            if b.clicked() {
                pick = Some(i + 1);
            }
        }
    } else if run.waiting == "reading" {
        let b = ui.button("Continue ›");
        tab.drawn_at.insert("continue".into(), b.rect);
        if b.clicked() {
            pick = Some(0);
        }
    }
    ui.horizontal(|ui| {
        if ui.small_button("■ Stop").clicked() {
            let _ = call_api(world, "story.stop", json!({"id": run.id}));
        }
    });
    if let Some(i) = pick {
        let _ = call_api(world, "story.choose", json!({"id": run.id, "index": i}));
    }
}

fn waiting_text(w: &str) -> &'static str {
    match w {
        "options" => "waiting for a pick",
        "reading" => "waiting to go on",
        "time" => "waiting (<<wait>>)",
        "until" => "waiting for a command to finish",
        "closing" => "the last lines on screen",
        "spots" => "waiting for a walk",
        _ => "running",
    }
}

fn facts(ui: &mut egui::Ui, world: &mut World, tab: &mut StoryTab) {
    heading(ui, "🏷 Facts");
    let facts = world
        .get_resource::<StoryFacts>()
        .cloned()
        .unwrap_or_default();
    if facts.values.is_empty() {
        ui.label(theme::dim(
            "None set. $name in a .yarn file and facts.name in a script read and write them; \
             set one here (name = value) to start the scene with it.",
        ));
    }
    let mut set: Option<(String, Value)> = None;
    egui::Grid::new("story facts")
        .num_columns(2)
        .striped(true)
        .spacing(Vec2::new(10.0, 3.0))
        .show(ui, |ui| {
            for (k, v) in &facts.values {
                ui.label(RichText::new(format!("${k}")).monospace().color(FACT));
                let editing = tab.fact_edit.as_ref().is_some_and(|(name, _)| name == k);
                if editing {
                    let (_, text) = tab.fact_edit.as_mut().unwrap();
                    let r = ui.add(
                        egui::TextEdit::singleline(text)
                            .desired_width(140.0)
                            .code_editor(),
                    );
                    r.request_focus();
                    if r.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            set = Some((k.clone(), parse_value(text)));
                        }
                        tab.fact_edit = None;
                    }
                } else {
                    let shown = match v {
                        Value::String(s) => format!("\"{s}\""),
                        other => other.to_string(),
                    };
                    let r = ui.add(
                        egui::Label::new(RichText::new(shown).monospace().color(theme::TEXT))
                            .sense(Sense::click()),
                    );
                    if r.on_hover_text("Click to change it").clicked() {
                        tab.fact_edit = Some((k.clone(), v.to_string()));
                    }
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut tab.new_fact)
                .hint_text("name = value")
                .desired_width(ui.available_width() - 40.0)
                .code_editor(),
        );
        let go = ui.button("+").clicked()
            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if go && let Some((k, v)) = tab.new_fact.split_once('=') {
            let k = k.trim().trim_start_matches('$').to_string();
            if !k.is_empty() {
                set = Some((k, parse_value(v)));
                tab.new_fact.clear();
            }
        }
    });
    if let Some((k, v)) = set {
        let mut m = serde_json::Map::new();
        m.insert(k, v);
        let _ = call_api(world, "story.facts", json!({ "set": m }));
    }
    let visits: Vec<(&String, &u32)> = facts
        .visits
        .iter()
        .filter(|(k, _)| !k.starts_with('@'))
        .collect();
    if !visits.is_empty() {
        egui::CollapsingHeader::new(theme::dim(format!("Nodes left ({})", visits.len())))
            .id_salt("story visits")
            .show(ui, |ui| {
                for (k, n) in visits {
                    ui.label(
                        RichText::new(format!("{k} ×{n}"))
                            .monospace()
                            .color(theme::TEXT_DIM),
                    );
                }
            });
    }
}

/// `3`, `true`, `"text"` or bare text.
fn parse_value(text: &str) -> Value {
    let t = text.trim();
    serde_json::from_str(t).unwrap_or_else(|_| json!(t))
}

fn quests(ui: &mut egui::Ui, world: &mut World, playing: bool) {
    heading(ui, "📜 Quests");
    let Some(book) = world.get_resource::<QuestBook>().cloned() else {
        return;
    };
    if book.defs.is_empty() {
        ui.label(theme::dim(
            "No quests yet: a quest is a .quest file in scripts/quests, its objectives \
             conditions on the facts (done_when, progress), so it moves on by itself.",
        ));
    }
    let mut act: Option<(&str, String)> = None;
    for d in &book.defs {
        let run = book.runs.get(&d.id).cloned().unwrap_or_default();
        egui::Frame::new()
            .fill(Color32::from_rgb(0x1f, 0x21, 0x25))
            .corner_radius(6.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&d.title).strong().color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (text, color) = match run.state {
                            QuestState::Inactive => ("inactive", theme::TEXT_DIM),
                            QuestState::Active => ("active", theme::ACCENT),
                            QuestState::Completed => ("completed", NOW),
                            QuestState::Failed => ("failed", ERROR),
                        };
                        ui.label(RichText::new(text).small().strong().color(color));
                    });
                });
                if let Some(w) = &d.start_when
                    && run.state == QuestState::Inactive
                {
                    ui.label(
                        RichText::new(format!("starts when {w}"))
                            .monospace()
                            .small()
                            .color(WHEN),
                    );
                }
                for (i, o) in d.objectives.iter().enumerate() {
                    let done = run.done.get(i).copied().unwrap_or(false);
                    let n = run.progress.get(i).copied().unwrap_or(0);
                    let count = o.count.max(1);
                    ui.horizontal(|ui| {
                        ui.label(if done {
                            RichText::new("✔").color(NOW)
                        } else {
                            RichText::new("○").color(theme::TEXT_DIM)
                        });
                        let text = if o.optional {
                            format!("{} (optional)", o.text)
                        } else {
                            o.text.clone()
                        };
                        ui.label(RichText::new(text).color(if done {
                            theme::TEXT_DIM
                        } else {
                            theme::TEXT
                        }));
                    });
                    if count > 1 {
                        let k = if done {
                            1.0
                        } else {
                            n.min(count) as f32 / count as f32
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.add(
                                egui::ProgressBar::new(k)
                                    .desired_width(ui.available_width() - 6.0)
                                    .desired_height(12.0)
                                    .fill(if done { NOW } else { theme::ACCENT })
                                    .text(
                                        RichText::new(format!(
                                            "{} / {count}",
                                            if done { count } else { n }
                                        ))
                                        .small(),
                                    ),
                            );
                        });
                    }
                    let cond = o.done_when.as_deref().or(o.progress.as_deref());
                    if let Some(c) = cond {
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(RichText::new(c).monospace().small().color(COMMENT));
                        });
                    }
                }
                if let Some(e) = &run.error {
                    ui.label(RichText::new(e).small().color(ERROR));
                }
                if playing {
                    ui.horizontal(|ui| {
                        if run.state == QuestState::Inactive && ui.small_button("Start").clicked() {
                            act = Some(("start", d.id.clone()));
                        }
                        if run.state == QuestState::Active {
                            if ui.small_button("Complete").clicked() {
                                act = Some(("complete", d.id.clone()));
                            }
                            if ui.small_button("Fail").clicked() {
                                act = Some(("fail", d.id.clone()));
                            }
                        }
                    });
                }
            });
        ui.add_space(4.0);
    }
    for (file, p) in &book.problems {
        ui.label(
            RichText::new(format!("{file}: {}", p.message))
                .small()
                .color(ERROR),
        );
    }
    if let Some((what, id)) = act {
        let _ = call_api(world, "story.quests", json!({ what: id }));
    }
}

fn toast(ui: &egui::Ui, text: &str, error: bool) {
    let rect = ui.max_rect();
    egui::Area::new(egui::Id::new("story toast"))
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

// ---------------------------------------------------------------- choices made by walking

/// A [`StorySpot`] in the world: where (world meters) and how big.
struct SpotAt {
    entity: Entity,
    name: String,
    at: DVec3,
    radius: f32,
}

fn spots_in(world: &mut World) -> Vec<SpotAt> {
    let mut q = world.query::<(Entity, &StorySpot, &GlobalTransform)>();
    q.iter(world)
        .map(|(entity, s, g)| SpotAt {
            entity,
            name: s.name.clone(),
            at: g.0.translation,
            radius: s.radius_m.max(0.2),
        })
        .collect()
}

fn find_named(world: &mut World, name: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Name)>();
    q.iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

fn world_pos(world: &World, e: Entity) -> Option<DVec3> {
    world.get::<GlobalTransform>(e).map(|g| g.0.translation)
}

/// The heading (yaw about up) a camera turned by `rotation` looks along.
fn heading_of(rotation: Quat) -> f32 {
    let f = rotation * Vec3::NEG_Z;
    (-f.x).atan2(-f.z)
}

/// Right and ahead on the ground for heading `yaw`.
fn ground_axes(yaw: f32) -> (DVec3, DVec3) {
    let (s, c) = (yaw.sin() as f64, yaw.cos() as f64);
    (DVec3::new(c, 0.0, -s), DVec3::new(-s, 0.0, -c))
}

/// A ring on the ground round `at` from `from` to `to` turns, clockwise seen from above and
/// starting on its far side for a camera heading `yaw`; in pieces where it goes behind the
/// camera.
fn ground_arc(view: &ViewInfo, yaw: f32, at: DVec3, r: f64, from: f32, to: f32) -> Vec<Vec<Pos2>> {
    let (right, ahead) = ground_axes(yaw);
    let n = (((to - from).abs() * 72.0).ceil() as usize).max(2);
    let mut runs = vec![Vec::new()];
    for i in 0..=n {
        let a = ((from + (to - from) * i as f32 / n as f32) * std::f32::consts::TAU) as f64;
        let p = at + (right * a.sin() + ahead * a.cos()) * r + DVec3::new(0.0, 0.03, 0.0);
        match view.project(p) {
            Some(q) => runs.last_mut().expect("one run").push(q),
            None if runs.last().is_some_and(|r| !r.is_empty()) => runs.push(Vec::new()),
            None => {}
        }
    }
    runs.retain(|r| r.len() > 1);
    runs
}

/// The scene view's part, drawn with the script handles: each spot a dashed ring on the ground
/// with its name (an editor helper, so not in Game view); while a walk-to choice is open, what
/// the game shows over the world, as the UI markers don't draw in the editor's view.
pub fn overlay(ui: &egui::Ui, world: &mut World, editor: &Editor, view: &ViewInfo) {
    if let Some(mut tab) = world.get_resource_mut::<StoryTab>() {
        tab.view_aspect = view.aspect();
    }
    let choice = world
        .get_resource::<SpotChoices>()
        .and_then(|c| c.choices.first().cloned());
    pin_view(world, editor, choice.as_ref());
    let p = ui.painter().with_clip_rect(view.screen);
    let yaw = heading_of(view.rotation);
    if let Some(c) = &choice {
        choice_overlay(&p, world, view, yaw, c);
        return;
    }
    if editor.game_view {
        return;
    }
    for s in spots_in(world) {
        let lit = editor.selection.contains(&s.entity);
        let color = if lit {
            SPOT_WARM
        } else {
            SPOT_WARM.gamma_multiply(0.7)
        };
        let stroke = Stroke::new(if lit { 2.4 } else { 1.6 }, color);
        for run in ground_arc(view, yaw, s.at, s.radius as f64, 0.0, 1.0) {
            p.extend(Shape::dashed_line(&run, stroke, 7.0, 5.0));
        }
        if let Some(c) = view.project(s.at + DVec3::new(0.0, 0.03, 0.0)) {
            let g = p.layout_no_wrap(s.name.clone(), FontId::proportional(12.0), color);
            let r = Rect::from_center_size(c, g.size() + Vec2::new(10.0, 4.0));
            p.rect_filled(r, 4.0, Color32::from_black_alpha(160));
            p.galley(r.min + Vec2::new(5.0, 2.0), g, color);
        }
    }
}

/// An open (or closing) walk-to choice over the world: each ring, filling as the chooser stands
/// in it; each option over its ring (the fallback with the time left); the prompt over the
/// speaker. The labels go at the pick and the rings shrink, as in the game.
fn choice_overlay(p: &egui::Painter, world: &World, view: &ViewInfo, yaw: f32, c: &SpotChoice) {
    let k = c.closing.map_or(0.0, |t| (t / 0.45).clamp(0.0, 1.0));
    for (i, o) in c.options.iter().enumerate() {
        if o.spot.is_none() {
            continue;
        }
        let picked = c.picked == Some(i);
        let fade = if c.closing.is_some() && !picked {
            1.0 - k
        } else {
            1.0
        };
        if fade < 0.02 {
            continue;
        }
        let color = if o.fallback { SPOT_COOL } else { SPOT_WARM }.gamma_multiply(fade);
        let r = o.radius as f64;
        for run in ground_arc(view, yaw, o.at, r, 0.0, 1.0) {
            p.add(Shape::line(
                run,
                Stroke::new(if o.inside { 3.0 } else { 2.0 }, color),
            ));
        }
        let fill = if picked && c.closing.is_some() {
            1.0
        } else {
            o.progress
        };
        if fill > 0.01 {
            for run in ground_arc(view, yaw, o.at, r * 0.8, 0.0, fill) {
                p.add(Shape::line(run, Stroke::new(5.0, color)));
            }
        }
        if c.closing.is_some() {
            continue;
        }
        let Some(at) = view.project(o.at + DVec3::new(0.0, 0.3, 0.0)) else {
            continue;
        };
        let note = o.fallback.then(|| match c.seconds_left() {
            Some(s) => format!("if you wait · {} s", s.ceil() as i32),
            None => "where you stand".to_string(),
        });
        pill(p, at, &o.text, note.as_deref(), color, o.inside);
    }
    if c.is_open()
        && let Some(sp) = c.speaker
        && let Some(head) = world_pos(world, sp)
        && let Some(at) = view.project(head + DVec3::new(0.0, 2.35, 0.0))
    {
        let who = if c.prompt_speaker.is_empty() {
            world
                .get::<Name>(sp)
                .map(|n| n.as_str().to_string())
                .unwrap_or_default()
        } else {
            c.prompt_speaker.clone()
        };
        bubble(p, at, &who, &c.prompt);
    }
}

/// A label in the scene view standing on `at`: a title and a line under it in `color`.
fn pill(p: &egui::Painter, at: Pos2, title: &str, sub: Option<&str>, color: Color32, lit: bool) {
    let t = p.layout(
        title.to_string(),
        FontId::proportional(13.0),
        Color32::WHITE,
        230.0,
    );
    let s = sub.map(|s| p.layout_no_wrap(s.to_string(), FontId::proportional(11.0), color));
    let (tw, th) = (t.size().x, t.size().y);
    let (sw, sh) = s
        .as_ref()
        .map_or((0.0, 0.0), |g| (g.size().x, g.size().y + 1.0));
    let size = Vec2::new(tw.max(sw) + 18.0, th + sh + 8.0);
    let rect = Rect::from_min_size(at - Vec2::new(size.x / 2.0, size.y), size);
    p.rect_filled(
        rect,
        5.0,
        Color32::from_black_alpha(if lit { 225 } else { 190 }),
    );
    p.rect_stroke(
        rect,
        5.0,
        Stroke::new(
            if lit { 2.0 } else { 1.0 },
            color.gamma_multiply(if lit { 1.0 } else { 0.75 }),
        ),
        StrokeKind::Inside,
    );
    p.galley(
        Pos2::new(rect.center().x - tw / 2.0, rect.top() + 4.0),
        t,
        Color32::WHITE,
    );
    if let Some(g) = s {
        p.galley(
            Pos2::new(rect.center().x - sw / 2.0, rect.top() + 5.0 + th),
            g,
            color,
        );
    }
}

/// The speaker's prompt over their head, as the game's marker shows it: who, and what they ask.
fn bubble(p: &egui::Painter, at: Pos2, who: &str, text: &str) {
    let name = p.layout_no_wrap(who.to_string(), FontId::proportional(12.0), SPEAKER);
    let said = p.layout(
        text.to_string(),
        FontId::proportional(14.0),
        Color32::WHITE,
        300.0,
    );
    let (nh, size) = (
        name.size().y,
        Vec2::new(
            name.size().x.max(said.size().x) + 22.0,
            name.size().y + said.size().y + 15.0,
        ),
    );
    let rect = Rect::from_min_size(at - Vec2::new(size.x / 2.0, size.y + 9.0), size);
    let fill = Color32::from_rgba_unmultiplied(0x22, 0x1d, 0x18, 232);
    p.rect_filled(rect, 7.0, fill);
    p.add(Shape::convex_polygon(
        vec![
            rect.center_bottom() + Vec2::new(-8.0, -0.5),
            rect.center_bottom() + Vec2::new(8.0, -0.5),
            at,
        ],
        fill,
        Stroke::NONE,
    ));
    p.rect_stroke(
        rect,
        7.0,
        Stroke::new(1.0, Color32::from_rgb(0x6a, 0x5c, 0x4a)),
        StrokeKind::Inside,
    );
    p.galley(rect.min + Vec2::new(11.0, 6.0), name, SPEAKER);
    p.galley(rect.min + Vec2::new(11.0, 9.0 + nh), said, Color32::WHITE);
}

/// While a walk-to choice is on in the game, the corner preview looks through its camera (what
/// the player sees from above), unless something else is pinned there.
fn pin_view(world: &mut World, editor: &Editor, choice: Option<&SpotChoice>) {
    let want = choice.filter(|_| editor.mode != Mode::Edit).and_then(|c| {
        let mut q = world.query::<(Entity, &SpotView)>();
        q.iter(world)
            .find(|(_, v)| v.choice == c.id)
            .map(|(e, _)| e)
    });
    let ours = world.get_resource::<StoryTab>().and_then(|t| t.pinned);
    if want.is_none() && ours.is_none() {
        return;
    }
    let took = {
        let Some(mut preview) = world.get_resource_mut::<crate::camera_preview::CameraPreview>()
        else {
            return;
        };
        let take = preview.pinned == want || preview.pinned.is_none() || preview.pinned == ours;
        if take {
            preview.pinned = want;
        }
        take
    };
    if took && let Some(mut tab) = world.get_resource_mut::<StoryTab>() {
        tab.pinned = want;
    }
}

/// "Walked to": the plan of a choice made by walking, from above and turned like the scene view
/// (up is where it looks): the speaker (the script that talks from this file), the people round
/// them, the player and each option's spot (the open choice live: rings filling, time left).
/// Before Play a spot is dragged to move it (one undo step) and a missing one placed.
fn walk_plan(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut StoryTab) {
    let Some(doc) = tab.doc.as_ref() else {
        return;
    };
    let nodes = &doc.compiled.nodes;
    let walked = |n: &NodeInfo| n.options.iter().any(|o| o.spot.is_some());
    if !nodes.iter().any(walked) {
        return;
    }
    let run = live(world, tab).run.filter(|r| !r.done);
    let node = tab
        .selected
        .filter(|&i| nodes.get(i).is_some_and(walked))
        .or_else(|| {
            let r = run.as_ref()?;
            nodes.iter().position(|n| n.title == r.node && walked(n))
        })
        .or_else(|| nodes.iter().position(walked));
    let Some(n) = node.map(|i| &nodes[i]) else {
        return;
    };
    let set = n
        .options
        .iter()
        .find(|o| o.spot.is_some())
        .map_or(0, |o| o.set);
    let options: Vec<OptionInfo> = n.options.iter().filter(|o| o.set == set).cloned().collect();
    let title = n.title.clone();
    let path = tab.path.clone();
    let editing = editor.mode == Mode::Edit;
    let choice = run
        .as_ref()
        .filter(|r| r.node == title)
        .and_then(|r| world.get_resource::<SpotChoices>()?.of_run(r.id).cloned());

    heading(ui, "👣 Walked to");
    ui.label(theme::dim(format!(
        "{title}'s options are picked by walking to their spots (#spot:name), the view from \
         above. {}",
        if editing {
            "Drag a spot to move it."
        } else {
            "Live: the rings fill as you stand in them."
        }
    )));

    // Who and what it shows.
    let owner = owner_of(world, &path).and_then(|name| find_named(world, &name));
    let owner_at = owner.and_then(|e| world_pos(world, e));
    let chooser = match &choice {
        Some(c) => Some(c.chooser),
        None => engine_script::spots::find_chooser(world, None),
    };
    let chooser_at = chooser.and_then(|e| world_pos(world, e));
    let all = spots_in(world);
    let near = owner_at.or(chooser_at).unwrap_or(DVec3::ZERO);
    // Each option's spot: the one of its name nearest the speaker.
    let placed: Vec<(usize, &SpotAt)> = options
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            let name = o.spot.as_ref()?;
            let s = all.iter().filter(|s| &s.name == name).min_by(|a, b| {
                a.at.distance_squared(near)
                    .total_cmp(&b.at.distance_squared(near))
            })?;
            Some((i, s))
        })
        .collect();
    let people: Vec<DVec3> = match owner {
        Some(o) => {
            let mut q = world.query_filtered::<(&ChildOf, &GlobalTransform), Without<StorySpot>>();
            q.iter(world)
                .filter(|(c, _)| c.parent() == o)
                .map(|(_, g)| g.0.translation)
                .collect()
        }
        None => Vec::new(),
    };
    let mut points: Vec<(DVec3, f64)> = placed
        .iter()
        .map(|(_, s)| (s.at, s.radius as f64))
        .collect();
    points.extend(owner_at.map(|p| (p, 0.4)));
    points.extend(chooser_at.map(|p| (p, 0.35)));
    points.extend(people.iter().map(|p| (*p, 0.25)));

    let yaw = world
        .get::<Transform>(editor.camera)
        .map_or(0.0, |t| heading_of(t.rotation));
    let (right, ahead) = ground_axes(yaw);
    let w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(w, (w * 0.72).clamp(170.0, 300.0)),
        Sense::click_and_drag(),
    );
    tab.drawn_at.insert("walk plan".into(), rect);
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, Color32::from_rgb(0x18, 0x19, 0x1c));
    if points.is_empty() {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No spots in the scene yet",
            FontId::proportional(13.0),
            theme::TEXT_DIM,
        );
    }
    let origin = points.first().map_or(DVec3::ZERO, |p| p.0);
    let (mut lo, mut hi) = (egui::Vec2::splat(f32::MAX), egui::Vec2::splat(f32::MIN));
    for (q, r) in &points {
        let d = *q - origin;
        let (u, v) = (d.dot(right) as f32, d.dot(ahead) as f32);
        lo = lo.min(Vec2::new(u, v) - Vec2::splat(*r as f32));
        hi = hi.max(Vec2::new(u, v) + Vec2::splat(*r as f32));
    }
    let margin = 0.7;
    let span = (hi - lo + Vec2::splat(2.0 * margin)).max(Vec2::splat(1.0));
    let scale = (rect.width() / span.x)
        .min(rect.height() / span.y)
        .min(90.0);
    let mid_uv = (lo + hi) * 0.5;
    let mid = origin + right * mid_uv.x as f64 + ahead * mid_uv.y as f64;
    let to_screen = |q: DVec3| {
        let d = q - mid;
        Pos2::new(
            rect.center().x + d.dot(right) as f32 * scale,
            rect.center().y - d.dot(ahead) as f32 * scale,
        )
    };
    // A meter grid, for size.
    if scale > 12.0 {
        let grid = Stroke::new(1.0, GRID);
        let mut x = rect.center().x % scale;
        while x < rect.right() {
            p.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                grid,
            );
            x += scale;
        }
        let mut y = rect.center().y % scale;
        while y < rect.bottom() {
            p.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                grid,
            );
            y += scale;
        }
    }
    for q in &people {
        p.circle_filled(to_screen(*q), (0.17 * scale).clamp(2.5, 7.0), PERSON);
    }
    // The spots, under the speaker and the player.
    let pointer = response.hover_pos();
    let mut over = None;
    for (i, s) in &placed {
        let o = &options[*i];
        let live_opt = choice.as_ref().and_then(|c| c.options.get(*i));
        let c = to_screen(s.at);
        let r = s.radius * scale;
        let color = if o.fallback { SPOT_COOL } else { SPOT_WARM };
        if pointer.is_some_and(|q| q.distance(c) <= r + 5.0) {
            over = Some(s.entity);
        }
        let lit = live_opt.is_some_and(|l| l.inside)
            || editor.selection.contains(&s.entity)
            || tab.plan_drag.is_some_and(|(e, _)| e == s.entity);
        p.circle_filled(c, r, color.gamma_multiply(0.09));
        if let Some(l) = live_opt.filter(|l| l.progress > 0.0) {
            p.circle_filled(c, r * l.progress, color.gamma_multiply(0.4));
        }
        p.circle_stroke(c, r, Stroke::new(if lit { 2.6 } else { 1.6 }, color));
        p.text(
            c,
            Align2::CENTER_CENTER,
            &s.name,
            FontId::monospace(11.5),
            color,
        );
        p.text(
            c + Vec2::new(0.0, r + 3.0),
            Align2::CENTER_TOP,
            short(&o.text, 30),
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        tab.drawn_at.insert(
            format!("plan spot {}", s.name),
            Rect::from_center_size(c, Vec2::splat(2.0 * r)),
        );
    }
    if let (Some(e), Some(q)) = (owner, owner_at) {
        let c = to_screen(q);
        p.circle_filled(c, (0.24 * scale).clamp(4.0, 10.0), OWNER);
        let name = world
            .get::<Name>(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        p.text(
            c + Vec2::new(0.0, -(0.24 * scale).clamp(4.0, 10.0) - 2.0),
            Align2::CENTER_BOTTOM,
            name,
            FontId::proportional(11.5),
            OWNER,
        );
    }
    if let (Some(e), Some(q)) = (chooser, chooser_at) {
        let c = to_screen(q);
        p.circle_filled(c, (0.2 * scale).clamp(3.5, 8.0), NOW);
        let who = if world.get::<engine::world::Interactor>(e).is_some() {
            "you".to_string()
        } else {
            world
                .get::<Name>(e)
                .map(|n| n.as_str().to_string())
                .unwrap_or_default()
        };
        p.text(
            c + Vec2::new((0.2 * scale).clamp(3.5, 8.0) + 3.0, 0.0),
            Align2::LEFT_CENTER,
            who,
            FontId::proportional(11.5),
            NOW,
        );
    }
    let corner = match choice.as_ref().and_then(SpotChoice::seconds_left) {
        Some(s) => format!("{title} · {} s left", s.ceil() as i32),
        None => format!("{title} · 1 square = 1 m"),
    };
    p.text(
        rect.left_top() + Vec2::new(8.0, 6.0),
        Align2::LEFT_TOP,
        corner,
        FontId::proportional(11.0),
        theme::TEXT_DIM,
    );

    // Dragging a spot (before Play): it follows the pointer on the ground; letting go is one
    // undo step. A click selects it.
    if editing {
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(e) = over
            && let Some(t) = world.get::<Transform>(e).copied()
        {
            tab.plan_drag = Some((e, t));
            editor.select(Some(e));
        }
        if response.dragged()
            && let (Some((e, _)), Some(q)) = (tab.plan_drag, response.interact_pointer_pos())
            && let Some(now_at) = world_pos(world, e)
        {
            let u = ((q.x - rect.center().x) / scale) as f64;
            let v = ((rect.center().y - q.y) / scale) as f64;
            let mut target = mid + right * u + ahead * v;
            target.y = now_at.y;
            let local = match world
                .get::<ChildOf>(e)
                .and_then(|c| world.get::<GlobalTransform>(c.parent()))
            {
                Some(g) => g.0.inverse().transform_point3(target),
                None => target,
            };
            if let Some(mut t) = world.get_mut::<Transform>(e) {
                t.translation = local;
            }
        }
        if response.drag_stopped()
            && let Some((e, before)) = tab.plan_drag.take()
            && let Some(after) = world.get::<Transform>(e).copied()
            && after != before
            && let (Ok(b), Ok(a)) = (serde_json::to_value(before), serde_json::to_value(after))
        {
            let t = editor::now(world);
            editor.history.record(
                world,
                Command::Component {
                    entity: e,
                    name: "Transform".into(),
                    before: Some(b),
                    after: Some(a),
                },
                t,
            );
        }
        if response.clicked()
            && let Some(e) = over
        {
            editor.select(Some(e));
        }
    } else {
        tab.plan_drag = None;
    }

    // Spots the options name that the scene hasn't got.
    let mut place = None;
    for (k, o) in options.iter().enumerate() {
        let Some(name) = &o.spot else { continue };
        if all.iter().any(|s| &s.name == name) {
            continue;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("⚠ no spot '{name}' in the scene")).color(ERROR));
            if editing
                && ui
                    .small_button("+ Place it")
                    .on_hover_text(match owner {
                        Some(_) => "A StorySpot beside the speaker, as their child (so it goes where they go)",
                        None => "A StorySpot where the scene view looks",
                    })
                    .clicked()
            {
                place = Some((name.clone(), k));
            }
        });
    }
    ui.horizontal(|ui| {
        let b = ui
            .button("👁 View from above")
            .on_hover_text("The scene view looks down on the spots, as the game's view does");
        tab.drawn_at.insert("view from above".into(), b.rect);
        if b.clicked() {
            look_down_on(world, editor, &points, tab.view_aspect);
        }
    });
    if let Some((name, k)) = place {
        let at = match owner {
            Some(_) => DVec3::new(1.2 + 0.9 * k as f64, 0.0, 0.0),
            None => world
                .get::<Transform>(editor.camera)
                .map_or(DVec3::ZERO, |t| {
                    let mut q = t.translation + t.forward().as_dvec3() * 6.0;
                    q.y = 0.0;
                    q
                }),
        };
        place_spot(world, editor, owner, &name, at, 0.6);
    }
    ui.add_space(8.0);
}

/// A [`StorySpot`] named `name` in the scene (one undo step), under `parent` when given (`at`
/// in its space then), selected.
pub fn place_spot(
    world: &mut World,
    editor: &mut Editor,
    parent: Option<Entity>,
    name: &str,
    at: DVec3,
    radius_m: f32,
) -> Entity {
    let mut e = world.spawn((
        InScene,
        SceneOrder::next(),
        Name::new(format!("Spot {name}")),
        Transform::from_translation(at),
        StorySpot {
            name: name.to_string(),
            radius_m,
        },
    ));
    if let Some(p) = parent {
        e.insert(ChildOf(p));
    }
    let id = e.id();
    let saved = scene_ops::save_entity(world, id);
    let t = editor::now(world);
    editor
        .history
        .record(world, Command::Spawned { saved, parent }, t);
    editor.select(Some(id));
    editor.info(t, format!("Placed spot '{name}'"));
    id
}

/// The scene view looks down on `points` (each with room round it) as the game's view from
/// above does: 64 degrees down, keeping its heading.
fn look_down_on(world: &mut World, editor: &Editor, points: &[(DVec3, f64)], aspect: f32) {
    let Some(mut t) = world.get::<Transform>(editor.camera).copied() else {
        return;
    };
    if points.is_empty() {
        return;
    }
    let yaw = heading_of(t.rotation);
    let (right, ahead) = ground_axes(yaw);
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for (q, _) in points {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    let mut mid = (lo + hi) * 0.5;
    mid.y = lo.y;
    let (mut across, mut along) = (0.0f64, 0.0f64);
    for (q, r) in points {
        let d = *q - mid;
        across = across.max(d.dot(right).abs() + r);
        along = along.max(d.dot(ahead).abs() + r);
    }
    let fov = match world.get::<Camera>(editor.camera).map(|c| c.projection) {
        Some(Projection::Perspective { fov_y_deg, .. }) => fov_y_deg,
        _ => 60.0,
    };
    let pitch = -64f32.to_radians();
    let half = (fov.to_radians() as f64 * 0.5).tan();
    let dist = ((along + 1.1) * (-pitch as f64).sin() / half)
        .max((across + 1.1) / (half * aspect.max(0.3) as f64))
        .max(4.0);
    let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
    let at = mid - (rotation * Vec3::NEG_Z).as_dvec3() * dist;
    if let Some(mut cam) = world.get_mut::<EditorCamera>(editor.camera) {
        cam.jump(&mut t, at, yaw, pitch, dist);
    }
    if let Some(mut tr) = world.get_mut::<Transform>(editor.camera) {
        *tr = t;
    }
}

// --------------------------------------------------------------- .yarn in the Scripts tab

/// A `.yarn` file's text colored for the Scripts tab: headers, `---` and `===`, speakers,
/// options and line groups, commands, `{expressions}`, `$facts`, `#tags` and comments.
pub fn highlight_yarn(text: &str, font: &FontId, find: Option<&str>) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut in_body = false;
    let find_bg = Color32::from_rgb(0x5a, 0x4a, 0x10);
    let mut push = |job: &mut LayoutJob, s: &str, color: Color32| {
        if s.is_empty() {
            return;
        }
        match find.filter(|f| !f.is_empty() && s.contains(*f)) {
            Some(f) => {
                let mut rest = s;
                while let Some(i) = rest.find(f) {
                    job.append(&rest[..i], 0.0, TextFormat::simple(font.clone(), color));
                    let mut fmt = TextFormat::simple(font.clone(), color);
                    fmt.background = find_bg;
                    job.append(&rest[i..i + f.len()], 0.0, fmt);
                    rest = &rest[i + f.len()..];
                }
                job.append(rest, 0.0, TextFormat::simple(font.clone(), color));
            }
            None => job.append(s, 0.0, TextFormat::simple(font.clone(), color)),
        }
    };
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let end = &line[body.len()..];
        let t = body.trim_start();
        let indent = &body[..body.len() - t.len()];
        push(&mut job, indent, theme::TEXT);
        if t.starts_with("//") {
            push(&mut job, t, COMMENT);
        } else if t == "---" {
            in_body = true;
            push(&mut job, t, theme::TEXT_DIM);
        } else if t == "===" {
            in_body = false;
            push(&mut job, t, theme::TEXT_DIM);
        } else if !in_body {
            match t.split_once(':') {
                Some((k, v)) => {
                    push(&mut job, k, if k.trim() == "title" { OPTION } else { WHEN });
                    push(&mut job, ":", theme::TEXT_DIM);
                    push(
                        &mut job,
                        v,
                        if k.trim() == "title" {
                            Color32::WHITE
                        } else {
                            theme::TEXT
                        },
                    );
                }
                None => push(&mut job, t, theme::TEXT),
            }
        } else {
            yarn_body_line(&mut job, t, &mut push);
        }
        push(&mut job, end, theme::TEXT);
    }
    job
}

fn yarn_body_line(
    job: &mut LayoutJob,
    t: &str,
    push: &mut impl FnMut(&mut LayoutJob, &str, Color32),
) {
    let (lead, rest, base) = if let Some(r) = t.strip_prefix("->") {
        ("->", r, OPTION)
    } else if let Some(r) = t.strip_prefix("=>") {
        ("=>", r, GROUP_LINE)
    } else {
        ("", t, theme::TEXT)
    };
    push(job, lead, base);
    let mut rest = rest;
    // A speaker before the first ": ".
    if let Some((who, _)) = rest.split_once(": ")
        && !who.trim().is_empty()
        && who.len() <= 40
        && !who.contains(['{', '<', '$', '"', '#'])
    {
        push(job, who, SPEAKER);
        push(job, ":", theme::TEXT_DIM);
        rest = &rest[who.len() + 1..];
    }
    // Commands, {expressions}, $facts and #tags, the rest in the line's color.
    let mut plain_start = 0;
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let (end, color) = if b[i] == b'<' && b.get(i + 1) == Some(&b'<') {
            (
                rest[i..].find(">>").map_or(rest.len(), |k| i + k + 2),
                COMMAND,
            )
        } else if b[i] == b'{' {
            (rest[i..].find('}').map_or(rest.len(), |k| i + k + 1), FACT)
        } else if b[i] == b'$' {
            let mut e = i + 1;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            (e, FACT)
        } else if b[i] == b'#' && (i == 0 || b[i - 1] == b' ') {
            let e = rest[i..].find(' ').map_or(rest.len(), |k| i + k);
            (e, COMMENT)
        } else {
            i += 1;
            continue;
        };
        push(job, &rest[plain_start..i], base);
        let inner = &rest[i..end];
        if color == COMMAND {
            // `$facts` inside commands too.
            let mut last = 0;
            let ib = inner.as_bytes();
            let mut k = 0;
            while k < ib.len() {
                if ib[k] == b'$' {
                    push(job, &inner[last..k], COMMAND);
                    let mut e = k + 1;
                    while e < ib.len() && (ib[e].is_ascii_alphanumeric() || ib[e] == b'_') {
                        e += 1;
                    }
                    push(job, &inner[k..e], FACT);
                    last = e;
                    k = e;
                } else {
                    k += 1;
                }
            }
            push(job, &inner[last..], COMMAND);
        } else {
            push(job, inner, color);
        }
        i = end;
        plain_start = end;
    }
    push(job, &rest[plain_start..], base);
}

/// Stop: the conversations ended with the scripts that ran them (the facts and quests go back
/// to the scene's with the rest of its resources).
pub fn reset(world: &mut World) {
    engine_script::spots::reset(world);
    if let Some(mut runs) = world.get_resource_mut::<StoryRuns>() {
        runs.runs.clear();
        runs.version += 1;
    }
    if let Some(mut tab) = world.get_resource_mut::<StoryTab>() {
        tab.talking = None;
        tab.fact_edit = None;
        tab.plan_drag = None;
    }
}
