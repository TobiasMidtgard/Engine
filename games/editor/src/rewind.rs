//! The Rewind tab: what every script did while playing, on a timeline to scrub back through
//! (Unreal's Rewind Debugger and Visual Logger). Play records ([`engine_script::rewind`]); each
//! script gets a track with its state machine's states as colored bars and what happened as
//! marks (prints, messages sent and received, signals, errors, hot reloads); open a track for
//! its values as curves (numbers), bars (true or false) and spans (text, entities).
//!
//! Click or drag on the timeline to see a moment: the game pauses, and the scripts' entities
//! and the parts they hold go back to where they were then, with the path each moving part
//! took around it (bright behind, faint ahead, a dot each second), the shapes the scripts drew
//! then (`draw`, gizmos) and a label on each. The panel on the right has the picked script's
//! values at that moment (the ones that changed right then lit) and what it did up to it.
//! Arrows step a frame, the error button jumps to the next error, Live goes back to now, and
//! Play goes on from where the game was paused (nothing is replayed; the world isn't
//! recorded). After Stop the recording stays to look through; the next Play starts a new one.
//! A failed script test opens here at its moment ([`open_at`]).

use std::collections::{HashMap, HashSet};

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};
use engine::prelude::*;
use engine_script::ScriptRewind;
use engine_script::rewind::{EventKind, Track, Val};

use crate::editor::{self, Editor, Mode, Tab};
use crate::theme;
use crate::ui::EditorDock;

pub struct RewindTabPlugin;

impl Plugin for RewindTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RewindTab>()
            .insert_resource(ScriptRewind::default())
            .add_systems(Update, follow_play.before(UpdateSet::Logic))
            .add_systems(Update, pose_paused.after(UpdateSet::Logic));
    }
}

/// Names column width, track and value row heights, the ruler's height.
const NAMES_W: f32 = 230.0;
const TRACK_H: f32 = 36.0;
const VALUE_H: f32 = 22.0;
const RULER_H: f32 = 24.0;
/// The details panel's width.
const DETAILS_W: f32 = 330.0;
/// Seconds shown when following the recording.
const WINDOW_SECS: f64 = 20.0;

const PLAYHEAD: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
const ERROR: Color32 = Color32::from_rgb(0xf4, 0x47, 0x47);
const MESSAGE: Color32 = Color32::from_rgb(0x5a, 0xa9, 0xff);
const SIGNAL: Color32 = Color32::from_rgb(0x3f, 0xd8, 0xd0);
const RELOAD: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);
const CHANGED: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);
const ROW_BG: Color32 = Color32::from_rgb(0x19, 0x1a, 0x1d);
const ROW_ALT: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x21);
const AXES: [Color32; 3] = [
    Color32::from_rgb(0xf0, 0x5d, 0x5d),
    Color32::from_rgb(0x6c, 0xd0, 0x5c),
    Color32::from_rgb(0x5d, 0x8d, 0xf0),
];

/// The tab's own state.
#[derive(Resource)]
pub struct RewindTab {
    /// Record each Play.
    pub auto_record: bool,
    /// Seconds the timeline shows, and its right end (None: the newest frame).
    pub window: f64,
    pub end: Option<f64>,
    /// Tracks open to their values.
    pub open: HashSet<Entity>,
    /// The track the details panel shows (the editor's selection wins when it has one).
    pub picked: Option<Entity>,
    pub filter: String,
    /// Paths and shapes only for the selected scripts.
    pub selected_only: bool,
    /// Entities put back where they were, with where they really are.
    posed: HashMap<Entity, Transform>,
    last_mode: Option<Mode>,
    /// Where things were drawn (shot scripts point at them).
    pub drawn_at: HashMap<String, Rect>,
}

impl Default for RewindTab {
    fn default() -> Self {
        Self {
            auto_record: true,
            window: WINDOW_SECS,
            end: None,
            open: HashSet::new(),
            picked: None,
            filter: String::new(),
            selected_only: false,
            posed: HashMap::new(),
            last_mode: None,
            drawn_at: HashMap::new(),
        }
    }
}

/// Play starts a new recording; Stop keeps it to look through; going on from a pause, or
/// stepping a frame, goes back to now (before the frame runs, so it runs where things really
/// are).
fn follow_play(world: &mut World) {
    let Some((mode, step)) = world
        .get_resource::<Editor>()
        .map(|e| (e.mode, e.step_once))
    else {
        return;
    };
    let stepping = step || (mode == Mode::Paused && !world.resource::<Time>().is_paused());
    let (last, auto) = {
        let mut tab = world.resource_mut::<RewindTab>();
        (tab.last_mode.replace(mode), tab.auto_record)
    };
    match (last, mode) {
        (Some(Mode::Edit) | None, Mode::Play | Mode::Paused) => {
            restore(world);
            let mut rw = world.resource_mut::<ScriptRewind>();
            rw.clear();
            rw.recording = auto;
            world.resource_mut::<RewindTab>().end = None;
        }
        (Some(Mode::Play | Mode::Paused), Mode::Edit) => {
            restore(world);
            world.resource_mut::<ScriptRewind>().recording = false;
        }
        (Some(Mode::Paused), Mode::Play) => to_now(world),
        (_, Mode::Paused) if stepping => to_now(world),
        _ => {}
    }
}

/// Back to now: the entities where they are, the timeline following the recording.
fn to_now(world: &mut World) {
    restore(world);
    world.resource_mut::<ScriptRewind>().shown = None;
    world.resource_mut::<RewindTab>().end = None;
}

/// While paused on a moment, the scripts' entities and their parts stand where they were
/// then; otherwise where they are.
fn pose_paused(world: &mut World) {
    let Some(mode) = world.get_resource::<Editor>().map(|e| e.mode) else {
        return;
    };
    let shown = world.resource::<ScriptRewind>().shown;
    match (mode, shown) {
        (Mode::Paused, Some(t)) => pose(world, t),
        _ => restore(world),
    }
}

/// Puts each recorded entity that still exists where it was at `t`.
pub fn pose(world: &mut World, t: f64) {
    let wanted: Vec<(Entity, Transform)> = {
        let rw = world.resource::<ScriptRewind>();
        let Some(frame) = rw.frame_at(t) else {
            return;
        };
        rw.tracks()
            .iter()
            .filter(|tr| tr.alive_at(frame))
            .flat_map(|tr| tr.parts.iter())
            .filter_map(|p| p.pose_at(frame).map(|pose| (p.entity, pose.local)))
            .collect()
    };
    for (e, local) in wanted {
        let Some(now) = world.get::<Transform>(e).copied() else {
            continue;
        };
        let mut tab = world.resource_mut::<RewindTab>();
        tab.posed.entry(e).or_insert(now);
        if now != local
            && let Some(mut t) = world.get_mut::<Transform>(e)
        {
            *t = local;
        }
    }
}

/// Puts every posed entity back where it really is.
pub fn restore(world: &mut World) {
    let posed = std::mem::take(&mut world.resource_mut::<RewindTab>().posed);
    for (e, live) in posed {
        if let Some(mut t) = world.get_mut::<Transform>(e) {
            *t = live;
        }
    }
}

/// Opens the tab on a stretch of the recording with `at` shown (a failed test's run).
pub fn open_at(world: &mut World, from: f64, to: f64, at: f64) {
    {
        let mut rw = world.resource_mut::<ScriptRewind>();
        rw.shown = Some(at);
    }
    {
        let mut tab = world.resource_mut::<RewindTab>();
        tab.window = (to - from).max(1.0) * 1.15;
        tab.end = Some(to + (to - from).max(1.0) * 0.05);
    }
    let mut dock = world.resource_mut::<EditorDock>();
    if let Some(at) = dock.0.find_tab(&Tab::Rewind) {
        let _ = dock.0.set_active_tab(at);
    }
}

fn color(c: [f32; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        (c[3] * 255.0) as u8,
    )
}

/// A state's color: one of a few muted hues, by its name.
fn state_color(name: &str) -> Color32 {
    const HUES: [Color32; 8] = [
        Color32::from_rgb(0x3b, 0x6e, 0xa8),
        Color32::from_rgb(0xa8, 0x5b, 0x3b),
        Color32::from_rgb(0x4b, 0x8a, 0x4f),
        Color32::from_rgb(0x8a, 0x4b, 0x86),
        Color32::from_rgb(0x9a, 0x84, 0x36),
        Color32::from_rgb(0x3b, 0x8a, 0x86),
        Color32::from_rgb(0x6a, 0x5b, 0xa8),
        Color32::from_rgb(0xa8, 0x3b, 0x5b),
    ];
    let h = name
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    HUES[(h as usize) % HUES.len()]
}

fn event_color(kind: EventKind) -> Color32 {
    match kind {
        EventKind::Print => theme::TEXT,
        EventKind::Error => ERROR,
        EventKind::Sent | EventKind::Got => MESSAGE,
        EventKind::Signal => SIGNAL,
        EventKind::Reload => RELOAD,
    }
}

/// The time span the timeline shows.
fn view_span(tab: &RewindTab, rw: &ScriptRewind) -> Option<(f64, f64)> {
    let (from, to) = rw.span()?;
    let window = tab.window.max(0.5);
    // A recording shorter than the window shows from its start; a longer one never shows
    // time before it starts or long after it ends.
    let end = if to - from + 0.5 < window {
        from - 0.25 + window
    } else {
        tab.end
            .unwrap_or(to + 0.25)
            .clamp(from - 0.25 + window, to + 0.25)
    };
    Some((end - window, end))
}

/// The track whose details show: the selected scripted entity, else the picked one, else the
/// first.
fn picked_track(world: &World, editor: &Editor, tab: &RewindTab) -> Option<Entity> {
    let rw = world.resource::<ScriptRewind>();
    editor
        .primary()
        .filter(|e| rw.track(*e).is_some())
        .or(tab.picked.filter(|e| rw.track(*e).is_some()))
        .or(rw.tracks().first().map(|t| t.entity))
}

/// The tab.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let Some(mut tab) = world.remove_resource::<RewindTab>() else {
        return;
    };
    tab.drawn_at.clear();
    toolbar(ui, world, editor, &mut tab);
    ui.separator();
    let empty = world.resource::<ScriptRewind>().frame_count() == 0;
    if empty {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Nothing recorded yet")
                    .size(16.0)
                    .color(theme::TEXT),
            );
            ui.add_space(4.0);
            ui.label(theme::dim(
                "Press Play: every script's values, states, prints, messages, errors, shapes and \
                 parts' positions are kept, frame by frame, to scrub back through here.",
            ));
        });
        world.insert_resource(tab);
        return;
    }
    let picked = picked_track(world, editor, &tab);
    let full = ui.available_rect_before_wrap();
    let details_w = DETAILS_W.min(full.width() * 0.4);
    let timeline_rect = Rect::from_min_max(
        full.min,
        Pos2::new(full.right() - details_w - 6.0, full.bottom()),
    );
    let details_rect =
        Rect::from_min_max(Pos2::new(full.right() - details_w, full.top()), full.max);
    let mut timeline_ui = ui.new_child(egui::UiBuilder::new().max_rect(timeline_rect));
    timeline_ui.set_clip_rect(timeline_rect.intersect(ui.clip_rect()));
    timeline(&mut timeline_ui, world, editor, &mut tab, picked);
    ui.painter().vline(
        full.right() - details_w - 3.0,
        full.y_range(),
        Stroke::new(1.0, theme::BORDER),
    );
    let mut details_ui =
        ui.new_child(egui::UiBuilder::new().max_rect(details_rect.shrink2(Vec2::new(6.0, 2.0))));
    details_ui.set_clip_rect(details_rect.intersect(ui.clip_rect()));
    details(&mut details_ui, world, editor, &mut tab, picked);
    ui.allocate_rect(full, Sense::hover());
    world.insert_resource(tab);
}

/// Shows the moment `t` (pausing a game that runs).
pub fn show_at(world: &mut World, editor: &mut Editor, t: f64) {
    let Some((from, to)) = world.resource::<ScriptRewind>().span() else {
        return;
    };
    if editor.mode == Mode::Play {
        editor::pause(world, editor);
    }
    world.resource_mut::<ScriptRewind>().shown = Some(t.clamp(from, to));
}

/// Steps `by` frames from the shown one.
pub fn step(world: &mut World, editor: &mut Editor, by: i64) {
    let rw = world.resource::<ScriptRewind>();
    let Some((first, last)) = rw.frame_span() else {
        return;
    };
    let now = rw.shown.and_then(|t| rw.frame_at(t)).unwrap_or(last);
    let to = (now as i64 + by).clamp(first as i64, last as i64) as u64;
    if let Some(t) = rw.time_of(to) {
        show_at(world, editor, t);
    }
}

fn toolbar(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut RewindTab) {
    ui.horizontal(|ui| {
        let recording = world.resource::<ScriptRewind>().recording;
        let rec_text = if recording {
            "⏺ Recording"
        } else {
            "⏺ Record"
        };
        let rec = ui
            .add(
                egui::Button::new(RichText::new(rec_text).color(if recording {
                    ERROR
                } else {
                    theme::TEXT
                }))
                .selected(recording),
            )
            .on_hover_text(
                "Record what the scripts do while playing (each Play starts a new recording)",
            );
        if rec.clicked() {
            let mut rw = world.resource_mut::<ScriptRewind>();
            rw.recording = !rw.recording;
            tab.auto_record = rw.recording;
        }
        ui.separator();
        if ui
            .button("⏮")
            .on_hover_text("The first frame kept")
            .clicked()
            && let Some((from, _)) = world.resource::<ScriptRewind>().span()
        {
            show_at(world, editor, from);
        }
        if ui
            .button("◀")
            .on_hover_text("One frame back (Left arrow)")
            .clicked()
        {
            step(world, editor, -1);
        }
        if ui
            .button("▶")
            .on_hover_text("One frame on (Right arrow)")
            .clicked()
        {
            step(world, editor, 1);
        }
        let live = world.resource::<ScriptRewind>().shown.is_none();
        if ui
            .add(egui::Button::new("⏭ Live").selected(live))
            .on_hover_text("Back to now (Play goes on from where the game was paused)")
            .clicked()
        {
            world.resource_mut::<ScriptRewind>().shown = None;
            tab.end = None;
        }
        let rw = world.resource::<ScriptRewind>();
        let (from, to) = rw.span().unwrap_or((0.0, 0.0));
        let (first, last) = rw.frame_span().unwrap_or((0, 0));
        let t = rw.shown.unwrap_or(to);
        let frame = rw.frame_at(t).unwrap_or(last);
        ui.label(
            RichText::new(format!("{:.2} s", t))
                .font(FontId::monospace(14.0))
                .color(if live { theme::TEXT } else { PLAYHEAD }),
        );
        ui.label(theme::dim(format!(
            "frame {} of {}",
            frame - first + 1,
            last - first + 1
        )));
        ui.separator();
        // The errors, to jump between.
        let errors: Vec<f64> = rw
            .tracks()
            .iter()
            .flat_map(|tr| tr.events.iter())
            .filter(|e| e.kind == EventKind::Error)
            .filter_map(|e| rw.time_of(e.frame))
            .collect();
        if !errors.is_empty() {
            let mut sorted = errors.clone();
            sorted.sort_by(f64::total_cmp);
            let next = sorted
                .iter()
                .copied()
                .find(|&e| e > t + 1e-6)
                .unwrap_or(sorted[0]);
            let label = format!(
                "⚠ {} error{} ⏵",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            );
            let r = ui
                .add(egui::Button::new(RichText::new(label).color(ERROR)))
                .on_hover_text("Jump to the next error");
            tab.drawn_at.insert("errors".into(), r.rect);
            if r.clicked() {
                show_at(world, editor, next);
                let (w, end) = (tab.window, tab.end);
                if end.is_none_or(|end| next < end - w || next > end) {
                    tab.end = Some(next + w * 0.3);
                }
            }
            ui.separator();
        }
        let mut rw = world.resource_mut::<ScriptRewind>();
        ui.checkbox(&mut rw.trails, "Paths")
            .on_hover_text("Each moving part's path around the moment shown");
        ui.checkbox(&mut rw.shapes, "Shapes")
            .on_hover_text("What the scripts drew then (draw, gizmos)");
        ui.checkbox(&mut tab.selected_only, "Selected only")
            .on_hover_text("Paths and shapes of the selected scripts only");
        rw.focus = if tab.selected_only {
            editor.selection.clone()
        } else {
            Vec::new()
        };
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut tab.filter)
                .hint_text("Filter scripts")
                .desired_width(130.0),
        );
        if ui
            .button("Fit")
            .on_hover_text("Show the whole recording")
            .clicked()
        {
            tab.window = (to - from).max(1.0) + 0.5;
            tab.end = Some(to + 0.25);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::dim(format!(
                "{:.1} MB · {:.1} s kept",
                rw.bytes() as f64 / (1024.0 * 1024.0),
                to - from
            )));
        });
    });
}

/// One row of the timeline.
#[derive(Clone, Copy)]
enum Row {
    Track(usize),
    Value(usize, usize),
}

fn rows(world: &World, tab: &RewindTab) -> Vec<Row> {
    let rw = world.resource::<ScriptRewind>();
    let filter = tab.filter.to_lowercase();
    let mut out = Vec::new();
    for (i, tr) in rw.tracks().iter().enumerate() {
        if !filter.is_empty()
            && !tr.name.to_lowercase().contains(&filter)
            && !tr.path.to_lowercase().contains(&filter)
        {
            continue;
        }
        out.push(Row::Track(i));
        if tab.open.contains(&tr.entity) {
            out.extend((0..tr.values.len()).map(|k| Row::Value(i, k)));
        }
    }
    out
}

fn row_h(row: Row) -> f32 {
    match row {
        Row::Track(_) => TRACK_H,
        Row::Value(..) => VALUE_H,
    }
}

/// Seconds between ruler ticks for `px_per_sec`.
fn tick_step(px_per_sec: f64) -> f64 {
    [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0]
        .into_iter()
        .find(|s| s * px_per_sec >= 70.0)
        .unwrap_or(300.0)
}

fn timeline(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut RewindTab,
    picked: Option<Entity>,
) {
    let Some((t0, t1)) = view_span(tab, world.resource::<ScriptRewind>()) else {
        return;
    };
    let outer = ui.available_rect_before_wrap();
    let canvas_x = outer.left() + NAMES_W;
    let width = (outer.right() - canvas_x - 8.0).max(40.0);
    let x_of = |t: f64| canvas_x + ((t - t0) / (t1 - t0)) as f32 * width;
    let t_of = |x: f32| t0 + ((x - canvas_x) / width) as f64 * (t1 - t0);
    let ruler = Rect::from_min_size(Pos2::new(canvas_x, outer.top()), Vec2::new(width, RULER_H));
    let all_rows = rows(world, tab);
    let shown = world.resource::<ScriptRewind>().shown;
    let (span_from, span_to) = world.resource::<ScriptRewind>().span().unwrap_or((t0, t1));
    let head = shown.unwrap_or(span_to);

    // The ruler: seconds, the recorded stretch, the playhead.
    {
        let p = ui.painter();
        p.rect_filled(ruler, 0.0, theme::PANEL);
        let rec = Rect::from_x_y_ranges(
            x_of(span_from).max(ruler.left())..=x_of(span_to).min(ruler.right()),
            ruler.bottom() - 4.0..=ruler.bottom(),
        );
        p.rect_filled(rec, 0.0, theme::ACCENT_DIM);
        let step = tick_step(width as f64 / (t1 - t0));
        let mut t = (t0 / step).ceil() * step;
        while t <= t1 {
            let x = x_of(t);
            p.vline(
                x,
                ruler.bottom() - 7.0..=ruler.bottom(),
                Stroke::new(1.0, theme::TEXT_DIM),
            );
            p.text(
                Pos2::new(x + 3.0, ruler.top() + 2.0),
                Align2::LEFT_TOP,
                format!(
                    "{} s",
                    engine_script::rewind::num((t * 100.0).round() / 100.0)
                ),
                FontId::proportional(11.0),
                theme::TEXT_DIM,
            );
            t += step;
        }
    }
    let ruler_resp = ui.interact(ruler, ui.id().with("rewind ruler"), Sense::click_and_drag());
    tab.drawn_at.insert("ruler".into(), ruler);

    // The rows, in a scroll area under the ruler.
    let below = Rect::from_min_max(Pos2::new(outer.left(), ruler.bottom()), outer.max);
    let mut rows_ui = ui.new_child(egui::UiBuilder::new().max_rect(below));
    let mut hovered: Option<(f64, String)> = None;
    let mut toggle: Option<Entity> = None;
    let mut pick: Option<Entity> = None;
    let mut jump: Option<f64> = None;
    egui::ScrollArea::vertical()
        .id_salt("rewind rows")
        .auto_shrink([false, false])
        .show(&mut rows_ui, |ui| {
            let total: f32 = all_rows.iter().map(|r| row_h(*r)).sum();
            let (area, resp) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), total.max(below.height())),
                Sense::click_and_drag(),
            );
            let rw = world.resource::<ScriptRewind>();
            let p = ui.painter().clone();
            let pointer = resp.hover_pos();
            let mut y = area.top();
            for (n, row) in all_rows.iter().enumerate() {
                let h = row_h(*row);
                let r = Rect::from_min_size(Pos2::new(area.left(), y), Vec2::new(area.width(), h));
                y += h;
                if !ui.is_rect_visible(r) {
                    continue;
                }
                let lane = Rect::from_x_y_ranges(canvas_x..=canvas_x + width, r.y_range());
                let names = Rect::from_x_y_ranges(r.left()..=canvas_x - 4.0, r.y_range());
                let bg = if n % 2 == 0 { ROW_BG } else { ROW_ALT };
                p.rect_filled(r, 0.0, bg);
                match *row {
                    Row::Track(i) => {
                        let tr = &rw.tracks()[i];
                        let is_picked = picked == Some(tr.entity);
                        if is_picked {
                            p.rect_filled(names, 0.0, theme::ACCENT_DIM.gamma_multiply(0.6));
                        }
                        let open = tab.open.contains(&tr.entity);
                        tab.drawn_at.insert(format!("track {}", tr.name), names);
                        track_name(&p, names, tr, open);
                        track_lane(&p, lane, tr, rw, &x_of, t0, t1);
                        // Hover: the event under the pointer, else the states then.
                        if let Some(pos) = pointer.filter(|p| lane.contains(*p)) {
                            let t = t_of(pos.x);
                            hovered = Some((t, hover_track(tr, rw, &x_of, pos, t)));
                        }
                        if let Some(pos) =
                            resp.interact_pointer_pos().filter(|p| names.contains(*p))
                            && resp.clicked()
                        {
                            if pos.x < names.left() + 22.0 {
                                toggle = Some(tr.entity);
                            } else {
                                pick = Some(tr.entity);
                            }
                        }
                    }
                    Row::Value(i, k) => {
                        let tr = &rw.tracks()[i];
                        let series = &tr.values[k];
                        tab.drawn_at
                            .insert(format!("value {} {}", tr.name, series.name), lane);
                        let frame = rw.frame_at(head).unwrap_or(tr.last);
                        let now = series.at(frame);
                        let changed = series
                            .points
                            .iter()
                            .any(|(f, _)| *f == frame && *f != tr.first);
                        p.text(
                            Pos2::new(names.left() + 26.0, names.center().y),
                            Align2::LEFT_CENTER,
                            &series.name,
                            FontId::proportional(12.0),
                            theme::TEXT_DIM,
                        );
                        if let Some(v) = now {
                            let text = clip(&v.show(), 14);
                            p.text(
                                Pos2::new(names.right() - 4.0, names.center().y),
                                Align2::RIGHT_CENTER,
                                text,
                                FontId::monospace(11.5),
                                if changed { CHANGED } else { theme::TEXT },
                            );
                        }
                        value_lane(
                            &p,
                            lane.shrink2(Vec2::new(0.0, 3.0)),
                            tr,
                            k,
                            rw,
                            &x_of,
                            t0,
                            t1,
                        );
                        if let Some(pos) = pointer.filter(|p| lane.contains(*p)) {
                            let t = t_of(pos.x);
                            let v = rw
                                .frame_at(t)
                                .and_then(|f| series.at(f))
                                .map_or("unset".to_string(), Val::show);
                            hovered = Some((t, format!("{}  {v}", series.name)));
                        }
                    }
                }
            }
            // Clicking or dragging on the lanes shows that moment.
            if let Some(pos) = resp.interact_pointer_pos()
                && pos.x >= canvas_x
                && (resp.clicked() || resp.dragged())
            {
                jump = Some(t_of(pos.x));
            }
            // Ctrl+wheel (or a pinch) zooms around the pointer; Shift+wheel pans.
            if let Some(pos) = pointer.filter(|p| p.x >= canvas_x) {
                let (zoom, pan) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.x));
                if zoom != 1.0 {
                    let at = t_of(pos.x);
                    let window = (tab.window / zoom as f64).clamp(0.5, 3600.0);
                    let right = at + (t1 - at) * window / (t1 - t0);
                    tab.window = window;
                    tab.end = Some(right);
                } else if pan != 0.0 {
                    tab.end = Some(t1 - pan as f64 * (t1 - t0) / width as f64);
                }
            }
            // The playhead, over every row.
            let x = x_of(head);
            if (canvas_x..=canvas_x + width).contains(&x) {
                p.vline(x, area.y_range(), Stroke::new(1.5, PLAYHEAD));
            }
            if let Some((t, text)) = hovered.take() {
                let _ = resp.clone().on_hover_ui_at_pointer(|ui| {
                    ui.label(
                        RichText::new(format!("{t:.2} s"))
                            .font(FontId::monospace(12.0))
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(text);
                });
            }
        });
    if let Some(e) = toggle
        && !tab.open.remove(&e)
    {
        tab.open.insert(e);
    }
    if let Some(e) = pick {
        tab.picked = Some(e);
        if world.get_entity(e).is_ok() {
            editor.select(Some(e));
        }
    }
    if let Some(pos) = ruler_resp.interact_pointer_pos()
        && (ruler_resp.clicked() || ruler_resp.dragged())
    {
        jump = Some(t_of(pos.x));
    }
    if let Some(t) = jump {
        if tab.end.is_none() {
            tab.end = Some(t1);
        }
        show_at(world, editor, t);
    }
    // The playhead's head on the ruler, with its time.
    let x = x_of(head);
    if (ruler.left()..=ruler.right()).contains(&x) {
        let p = ui.painter();
        p.add(Shape::convex_polygon(
            vec![
                Pos2::new(x - 6.0, ruler.top() + 12.0),
                Pos2::new(x + 6.0, ruler.top() + 12.0),
                Pos2::new(x + 6.0, ruler.bottom() - 6.0),
                Pos2::new(x, ruler.bottom()),
                Pos2::new(x - 6.0, ruler.bottom() - 6.0),
            ],
            PLAYHEAD,
            Stroke::NONE,
        ));
        p.vline(x, ruler.y_range(), Stroke::new(1.5, PLAYHEAD));
    }
    // Arrow keys step while the tab is under the pointer.
    if ui.rect_contains_pointer(outer) {
        let (left, right) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
            )
        });
        if left {
            step(world, editor, -1);
        } else if right {
            step(world, editor, 1);
        }
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// A track's name cell: the open arrow, its color, name and file.
fn track_name(p: &egui::Painter, r: Rect, tr: &Track, open: bool) {
    p.text(
        Pos2::new(r.left() + 6.0, r.center().y),
        Align2::LEFT_CENTER,
        if open { "⏷" } else { "⏵" },
        FontId::proportional(12.0),
        theme::TEXT_DIM,
    );
    let swatch = Rect::from_center_size(Pos2::new(r.left() + 27.0, r.center().y), Vec2::splat(9.0));
    p.rect_filled(swatch, 2.0, color(tr.color));
    p.text(
        Pos2::new(r.left() + 38.0, r.top() + 4.0),
        Align2::LEFT_TOP,
        clip(&tr.name, 24),
        FontId::proportional(13.5),
        theme::TEXT,
    );
    p.text(
        Pos2::new(r.left() + 38.0, r.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        clip(&tr.path, 30),
        FontId::proportional(10.5),
        theme::TEXT_DIM,
    );
}

/// A track's lane: its lifetime, its states as bars (one lane per parallel region) and what
/// happened as marks along the top.
#[allow(clippy::too_many_arguments)]
fn track_lane(
    p: &egui::Painter,
    lane: Rect,
    tr: &Track,
    rw: &ScriptRewind,
    x_of: &dyn Fn(f64) -> f32,
    t0: f64,
    t1: f64,
) {
    let (Some(a), Some(b)) = (rw.time_of(tr.first), rw.time_of(tr.last)) else {
        return;
    };
    let alive = Rect::from_x_y_ranges(
        x_of(a.max(t0)).max(lane.left())..=x_of(b.min(t1)).min(lane.right()),
        lane.top() + 11.0..=lane.bottom() - 3.0,
    );
    if alive.width() <= 0.0 {
        return;
    }
    let regions = tr
        .states
        .iter()
        .map(|(_, s)| s.len())
        .max()
        .unwrap_or(0)
        .min(3);
    if regions == 0 {
        p.rect_filled(alive, 2.0, color(tr.color).gamma_multiply(0.28));
    } else {
        let lane_h = alive.height() / regions as f32;
        for k in 0..regions {
            let band = Rect::from_x_y_ranges(
                alive.x_range(),
                alive.top() + lane_h * k as f32..=alive.top() + lane_h * (k + 1) as f32 - 1.0,
            );
            // Spans of one state in this region.
            let mut spans: Vec<(f64, f64, &str)> = Vec::new();
            for (i, (f, names)) in tr.states.iter().enumerate() {
                let start = rw.time_of(*f).unwrap_or(a);
                let end = tr
                    .states
                    .get(i + 1)
                    .and_then(|(g, _)| rw.time_of(*g))
                    .unwrap_or(b);
                let Some(name) = names.get(k) else { continue };
                match spans.last_mut() {
                    Some(last) if last.2 == name && (last.1 - start).abs() < 1e-9 => last.1 = end,
                    _ => spans.push((start, end, name)),
                }
            }
            for (s, e, name) in spans {
                if e < t0 || s > t1 {
                    continue;
                }
                let r = Rect::from_x_y_ranges(
                    x_of(s).max(band.left())..=x_of(e).min(band.right()),
                    band.y_range(),
                );
                if r.width() <= 0.5 {
                    continue;
                }
                p.rect_filled(r, 2.0, state_color(name));
                let font = FontId::proportional(if regions > 1 { 10.0 } else { 11.5 });
                let galley = p.layout_no_wrap(name.to_string(), font, theme::TEXT);
                if galley.size().x + 6.0 < r.width() {
                    p.galley(
                        Pos2::new(r.left() + 4.0, r.center().y - galley.size().y / 2.0),
                        galley,
                        theme::TEXT,
                    );
                }
            }
        }
    }
    // What happened, along the top.
    let y = lane.top() + 6.0;
    for e in &tr.events {
        let Some(t) = rw.time_of(e.frame) else {
            continue;
        };
        if t < t0 || t > t1 {
            continue;
        }
        let x = x_of(t);
        mark(p, Pos2::new(x, y), e.kind);
    }
}

/// An event's mark: a dot for a print, triangles for messages (out and in), a diamond for a
/// signal, a bigger red diamond for an error, a bar for a reload.
fn mark(p: &egui::Painter, at: Pos2, kind: EventKind) {
    let c = event_color(kind);
    match kind {
        EventKind::Print => {
            p.circle_filled(at, 2.6, c);
        }
        EventKind::Error => {
            let s = 5.0;
            p.add(Shape::convex_polygon(
                vec![
                    at + Vec2::new(0.0, -s),
                    at + Vec2::new(s, 0.0),
                    at + Vec2::new(0.0, s),
                    at + Vec2::new(-s, 0.0),
                ],
                c,
                Stroke::new(1.0, Color32::BLACK),
            ));
        }
        EventKind::Signal => {
            let s = 3.5;
            p.add(Shape::convex_polygon(
                vec![
                    at + Vec2::new(0.0, -s),
                    at + Vec2::new(s, 0.0),
                    at + Vec2::new(0.0, s),
                    at + Vec2::new(-s, 0.0),
                ],
                c,
                Stroke::NONE,
            ));
        }
        EventKind::Sent | EventKind::Got => {
            let d = if kind == EventKind::Sent { 1.0 } else { -1.0 };
            p.add(Shape::convex_polygon(
                vec![
                    at + Vec2::new(-3.5 * d, -3.5),
                    at + Vec2::new(3.5 * d, 0.0),
                    at + Vec2::new(-3.5 * d, 3.5),
                ],
                c,
                Stroke::NONE,
            ));
        }
        EventKind::Reload => {
            p.rect_filled(Rect::from_center_size(at, Vec2::new(3.0, 9.0)), 1.0, c);
        }
    }
}

/// What the pointer is over in a track's lane: an event within a few points, else the states
/// then.
fn hover_track(
    tr: &Track,
    rw: &ScriptRewind,
    x_of: &dyn Fn(f64) -> f32,
    pos: Pos2,
    t: f64,
) -> String {
    let near: Vec<String> = tr
        .events
        .iter()
        .filter(|e| {
            rw.time_of(e.frame)
                .is_some_and(|et| (x_of(et) - pos.x).abs() <= 5.0)
        })
        .take(6)
        .map(|e| format!("{}: {}", e.kind.name(), e.text))
        .collect();
    if !near.is_empty() {
        return format!("{}\n{}", tr.name, near.join("\n"));
    }
    let states = rw
        .frame_at(t)
        .map(|f| tr.states_at(f).join(" | "))
        .unwrap_or_default();
    if states.is_empty() {
        tr.name.clone()
    } else {
        format!("{}: {states}", tr.name)
    }
}

/// A value's lane: numbers (and true or false) as a line from the lowest to the highest seen
/// here, a `vec3` as three (x red, y green, z blue), text and entities as spans.
#[allow(clippy::too_many_arguments)]
fn value_lane(
    p: &egui::Painter,
    lane: Rect,
    tr: &Track,
    k: usize,
    rw: &ScriptRewind,
    x_of: &dyn Fn(f64) -> f32,
    t0: f64,
    t1: f64,
) {
    let series = &tr.values[k];
    let end_t = rw.time_of(tr.last).unwrap_or(t1);
    // The points in view, each with its time and the time it held until.
    let start = series
        .points
        .partition_point(|(f, _)| rw.time_of(*f).is_some_and(|t| t < t0))
        .saturating_sub(1);
    let mut spans: Vec<(f64, f64, Option<&Val>)> = Vec::new();
    for i in start..series.points.len() {
        let (f, v) = &series.points[i];
        let Some(s) = rw.time_of(*f) else { continue };
        if s > t1 {
            break;
        }
        let e = series
            .points
            .get(i + 1)
            .and_then(|(g, _)| rw.time_of(*g))
            .unwrap_or(end_t);
        spans.push((s, e, v.as_ref()));
    }
    let c = color(tr.color);
    let numbers: Vec<(f64, f64, f64)> = spans
        .iter()
        .filter_map(|(s, e, v)| v.and_then(|v| v.number()).map(|n| (*s, *e, n)))
        .collect();
    let first = spans.iter().find_map(|(_, _, v)| *v);
    match first {
        Some(Val::Bool(_)) => {
            for (s, e, v) in &spans {
                if *v == Some(&Val::Bool(true)) {
                    let r = Rect::from_x_y_ranges(
                        x_of(*s).max(lane.left())..=x_of(*e).min(lane.right()),
                        lane.top() + 3.0..=lane.bottom() - 3.0,
                    );
                    p.rect_filled(r, 2.0, c.gamma_multiply(0.55));
                }
            }
        }
        Some(Val::Num(_)) => line(p, lane, &numbers, x_of, c),
        Some(Val::Vec3(_)) => {
            for (axis, &axis_color) in AXES.iter().enumerate() {
                let pts: Vec<(f64, f64, f64)> = spans
                    .iter()
                    .filter_map(|(s, e, v)| match v {
                        Some(Val::Vec3(v)) => Some((*s, *e, v.to_array()[axis])),
                        _ => None,
                    })
                    .collect();
                line(p, lane, &pts, x_of, axis_color);
            }
        }
        Some(Val::Quat(_)) => {
            let pts: Vec<(f64, f64, f64)> = spans
                .iter()
                .filter_map(|(s, e, v)| match v {
                    Some(Val::Quat(q)) => {
                        Some((*s, *e, q.to_euler(EulerRot::YXZ).0.to_degrees() as f64))
                    }
                    _ => None,
                })
                .collect();
            line(p, lane, &pts, x_of, c);
        }
        _ => {
            // Text, entities, tables: a span per value, labelled where it fits.
            for (n, (s, e, v)) in spans.iter().enumerate() {
                let Some(v) = v else { continue };
                let r = Rect::from_x_y_ranges(
                    x_of(*s).max(lane.left())..=x_of(*e).min(lane.right()),
                    lane.top() + 2.0..=lane.bottom() - 2.0,
                );
                if r.width() <= 0.5 {
                    continue;
                }
                let fill = if n % 2 == 0 { 0.30 } else { 0.18 };
                p.rect_filled(r, 2.0, c.gamma_multiply(fill));
                let galley = p.layout_no_wrap(v.show(), FontId::proportional(10.5), theme::TEXT);
                if galley.size().x + 6.0 < r.width() {
                    p.galley(
                        Pos2::new(r.left() + 3.0, r.center().y - galley.size().y / 2.0),
                        galley,
                        theme::TEXT,
                    );
                }
            }
        }
    }
}

/// Values held from each start to the next as a stepped line, scaled to the lane (at most a
/// few points per pixel: the lowest and highest in each).
fn line(
    p: &egui::Painter,
    lane: Rect,
    pts: &[(f64, f64, f64)],
    x_of: &dyn Fn(f64) -> f32,
    c: Color32,
) {
    if pts.is_empty() {
        return;
    }
    let (lo, hi) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), (_, _, v)| {
            (lo.min(*v), hi.max(*v))
        });
    let y_of = |v: f64| {
        if hi - lo < 1e-12 {
            lane.center().y
        } else {
            lane.bottom() - ((v - lo) / (hi - lo)) as f32 * lane.height()
        }
    };
    let mut points: Vec<Pos2> = Vec::with_capacity(pts.len() * 2);
    let mut last_x = f32::MIN;
    for &(s, e, v) in pts {
        let (xs, xe) = (x_of(s).max(lane.left()), x_of(e).min(lane.right()));
        if xe < lane.left() || xs > lane.right() {
            continue;
        }
        let y = y_of(v);
        if xe - last_x < 0.5 && points.len() >= 2 {
            // Within the same pixel: one vertical stroke covers it.
            points.push(Pos2::new(xs, y));
            continue;
        }
        points.push(Pos2::new(xs, y));
        points.push(Pos2::new(xe, y));
        last_x = xe;
    }
    if points.len() >= 2 {
        p.add(Shape::line(points, Stroke::new(1.4, c)));
    }
    // Its range, faint, at the lane's left.
    if hi > lo {
        p.text(
            Pos2::new(lane.left() + 2.0, lane.top() - 1.0),
            Align2::LEFT_TOP,
            engine_script::rewind::num((hi * 100.0).round() / 100.0),
            FontId::proportional(9.0),
            theme::TEXT_DIM.gamma_multiply(0.7),
        );
    }
}

/// The picked script at the moment shown: its states, values (the ones that changed right
/// then lit) and what it did up to then.
fn details(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut RewindTab,
    picked: Option<Entity>,
) {
    let mut jump = None;
    {
        let rw = world.resource::<ScriptRewind>();
        let Some(tr) = picked.and_then(|e| rw.track(e)) else {
            ui.label(theme::dim("Pick a script on the left."));
            return;
        };
        let t = rw.shown.or(rw.span().map(|s| s.1)).unwrap_or(0.0);
        let frame = rw.frame_at(t).unwrap_or(tr.last);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, color(tr.color));
            ui.label(
                RichText::new(&tr.name)
                    .strong()
                    .size(15.0)
                    .color(theme::TEXT),
            );
            ui.label(theme::dim(&tr.path));
        });
        ui.horizontal(|ui| {
            let at = rw.time_of(frame).unwrap_or(t);
            ui.label(
                RichText::new(format!("at {:.2} s", at))
                    .font(FontId::monospace(12.5))
                    .color(if rw.shown.is_some() {
                        PLAYHEAD
                    } else {
                        theme::TEXT_DIM
                    }),
            );
            if !tr.alive_at(frame) {
                ui.label(theme::dim("(not running then)"));
            }
            for s in tr.states_at(frame) {
                egui::Frame::new()
                    .fill(state_color(s))
                    .corner_radius(3.0)
                    .inner_margin(egui::Margin::symmetric(5, 1))
                    .show(ui, |ui| {
                        ui.label(RichText::new(s).size(11.5).color(theme::TEXT));
                    });
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("rewind details")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Values")
                        .small()
                        .strong()
                        .color(theme::TEXT_DIM),
                );
                egui::Grid::new("rewind values")
                    .num_columns(2)
                    .spacing([10.0, 2.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (name, v, changed) in tr.values_at(frame) {
                            ui.label(RichText::new(name).size(12.0).color(theme::TEXT_DIM));
                            let text =
                                RichText::new(clip(&v.show(), 32)).font(FontId::monospace(12.0));
                            ui.label(if changed {
                                text.color(Color32::BLACK).background_color(CHANGED)
                            } else {
                                text.color(theme::TEXT)
                            });
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.label(
                    RichText::new("What happened")
                        .small()
                        .strong()
                        .color(theme::TEXT_DIM),
                );
                let from = rw.frame_at(t - 6.0).unwrap_or(tr.first);
                let events = tr.events_in(from, frame);
                if events.is_empty() {
                    ui.label(theme::dim("Nothing in the 6 s before."));
                }
                for e in events.iter().rev().take(14) {
                    let et = rw.time_of(e.frame).unwrap_or(0.0);
                    let row = ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{:6.2}", et))
                                .font(FontId::monospace(11.5))
                                .color(if e.frame == frame {
                                    PLAYHEAD
                                } else {
                                    theme::TEXT_DIM
                                }),
                        );
                        ui.label(
                            RichText::new(format!("{:<6}", e.kind.name()))
                                .font(FontId::monospace(11.5))
                                .color(event_color(e.kind)),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(&e.text).size(12.0).color(
                                if e.kind == EventKind::Error {
                                    ERROR
                                } else {
                                    theme::TEXT
                                },
                            ))
                            .truncate(),
                        )
                    });
                    if row
                        .response
                        .interact(Sense::click())
                        .on_hover_text(&e.text)
                        .clicked()
                    {
                        jump = Some(et);
                    }
                }
            });
    }
    if let Some(t) = jump {
        show_at(world, editor, t);
    }
    let _ = tab;
}

/// Labels in the scene view at the moment shown: each script's name with its states (or its
/// first value) beside where it was.
pub fn overlay(ui: &egui::Ui, world: &World, view: &crate::view::ViewInfo) {
    let Some(rw) = world.get_resource::<ScriptRewind>() else {
        return;
    };
    let Some(t) = rw.shown else {
        return;
    };
    let Some(frame) = rw.frame_at(t) else {
        return;
    };
    let p = ui.painter().with_clip_rect(view.screen);
    for tr in rw.tracks().iter().filter(|tr| tr.alive_at(frame)) {
        if !rw.focus.is_empty() && !rw.focus.contains(&tr.entity) {
            continue;
        }
        // Beside the part that moves most (a drone, not its post), else the script's entity.
        let part = tr
            .parts
            .iter()
            .filter(|p| p.pose_at(frame).is_some())
            .max_by_key(|p| p.poses.len())
            .or(tr.parts.first());
        let Some(pose) = part.and_then(|p| p.pose_at(frame)) else {
            continue;
        };
        let Some(at) = view.project(pose.position + DVec3::new(0.0, 0.55, 0.0)) else {
            continue;
        };
        let states = tr.states_at(frame).join(" | ");
        // Its states, else its first value that tells something (not a part it holds).
        let detail = if !states.is_empty() {
            states
        } else {
            let values = tr.values_at(frame);
            values
                .iter()
                .find(|(_, v, _)| !matches!(v, Val::Entity(..) | Val::Table(_)))
                .or(values.first())
                .map(|(k, v, _)| format!("{k} {}", clip(&v.show(), 16)))
                .unwrap_or_default()
        };
        let font = FontId::proportional(13.0);
        let name = p.layout_no_wrap(tr.name.clone(), font.clone(), color(tr.color));
        let rest = p.layout_no_wrap(detail, FontId::proportional(12.0), theme::TEXT);
        let w = name.size().x + rest.size().x + 18.0;
        let back = Rect::from_min_size(at + Vec2::new(-w / 2.0, -22.0), Vec2::new(w, 20.0));
        p.rect_filled(back, 4.0, Color32::from_black_alpha(185));
        p.rect_stroke(
            back,
            4.0,
            Stroke::new(1.0, color(tr.color).gamma_multiply(0.7)),
            StrokeKind::Inside,
        );
        let y = back.center().y;
        let name_w = name.size().x;
        p.galley(
            Pos2::new(back.left() + 6.0, y - name.size().y / 2.0),
            name,
            theme::TEXT,
        );
        p.galley(
            Pos2::new(back.left() + 12.0 + name_w, y - rest.size().y / 2.0),
            rest,
            theme::TEXT,
        );
        // A tick down to it.
        p.line_segment(
            [Pos2::new(at.x, back.bottom()), at + Vec2::new(0.0, 6.0)],
            Stroke::new(1.0, color(tr.color)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::time::TimeSource;
    use engine_script::{Script, ScriptSettings};

    const MOVER: &str = r#"
local Mover = {}
function Mover:construct()
	self.arm = self.entity:spawn({ name = "Arm", components = { Transform = { translation = vec3(0, 1, 0) } } })
end
function Mover:update(dt)
	self.entity.position += vec3(2 * dt, 0, 0)
	self.arm.position = self.entity.position + vec3(0, 1 + self.entity.position.x, 0)
end
return Mover
"#;

    /// A moment puts a script's entity and the part it holds back where they were then; back
    /// to now puts them where they really are.
    #[test]
    fn a_moment_puts_entities_back_and_now_where_they_are() {
        let dir = std::env::temp_dir().join(format!("editor_rewind_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mover.luau"), MOVER).unwrap();
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs: 0.1 });
        app.insert_resource(ScriptSettings {
            folder: dir.display().to_string(),
            ..Default::default()
        });
        app.add_plugins(engine_script::ScriptPlugin);
        app.init_resource::<RewindTab>();
        app.insert_resource(ScriptRewind::recording());
        let mover = app
            .world
            .spawn((
                Name::new("Mover"),
                Transform::default(),
                Script::new("mover.luau"),
            ))
            .id();
        app.update_n(20);
        let x = |w: &World, e: Entity| w.get::<Transform>(e).unwrap().translation.x;
        let (from, to) = app.world.resource::<ScriptRewind>().span().unwrap();
        let arm = app
            .world
            .resource::<ScriptRewind>()
            .track(mover)
            .and_then(|t| t.parts.iter().find(|p| p.field == "arm"))
            .map(|p| p.entity)
            .expect("the arm is a part");
        let (mover_now, arm_now) = (
            *app.world.get::<Transform>(mover).unwrap(),
            *app.world.get::<Transform>(arm).unwrap(),
        );
        assert!(to - from > 1.5);

        pose(&mut app.world, from + 0.5);
        let then = x(&app.world, mover);
        assert!(
            then < mover_now.translation.x - 2.0,
            "{then} vs {}",
            mover_now.translation.x
        );
        let arm_then = app.world.get::<Transform>(arm).unwrap().translation;
        assert!(
            (arm_then.y - (1.0 + then)).abs() < 0.25,
            "the arm as it was then: {arm_then}"
        );

        restore(&mut app.world);
        assert_eq!(*app.world.get::<Transform>(mover).unwrap(), mover_now);
        assert_eq!(*app.world.get::<Transform>(arm).unwrap(), arm_now);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
