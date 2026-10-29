//! State machine diagrams: a script's `states` table drawn as boxes and arrows (Scripting's;
//! engine-design/scripting-research.md §19). While the game plays, the active states are lit
//! with their time, the change just made glows along its arrow, `after` timers fill and the
//! log lists each change with what caused it, as PlayMaker's graph view and FSM log, Unity's
//! state graphs and Unreal's StateTree debugger show them. The Scripts tab, the Details panel
//! and the Graphs tab's States view draw with it.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use engine_script::diagram::{self as geo, Area, GROUP_HEAD};
use engine_script::states::{Breaks, Live, Machine};

use crate::theme;

/// A state's box (diagram units: pixels at zoom 1).
pub use engine_script::diagram::{STATE_H, STATE_W};

const BOX: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x32);
const GROUP: Color32 = Color32::from_rgb(0x1f, 0x22, 0x26);
const GROUP_EDGE: Color32 = Color32::from_rgb(0x3a, 0x3e, 0x46);
const CHIP: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);
/// Breakpoints' red.
pub const BREAK: Color32 = Color32::from_rgb(0xe5, 0x48, 0x3c);

/// What a transition's trigger family is drawn in.
pub fn kind_color(kind: &str) -> Color32 {
    match kind {
        "on" => Color32::from_rgb(120, 168, 255),
        "after" => Color32::from_rgb(242, 182, 72),
        "done" => Color32::from_rgb(98, 206, 128),
        "when" => Color32::from_rgb(86, 206, 206),
        _ => theme::TEXT_DIM,
    }
}

/// Something on the diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    State(String),
    /// By index in the machine's transitions.
    Transition(usize),
}

/// Where everything goes (diagram units).
#[derive(Debug, Clone)]
pub struct Placed {
    pub rects: BTreeMap<String, Rect>,
    /// The "any state" box the outer table's transitions start from.
    pub any: Option<Rect>,
    pub bounds: Rect,
}

fn pos(v: geo::Vec2) -> Pos2 {
    Pos2::new(v.x, v.y)
}

fn rect(a: Area) -> Rect {
    Rect::from_min_max(pos(a.min), pos(a.max))
}

fn area(r: Rect) -> Area {
    Area::from_min_max(
        geo::Vec2::new(r.min.x, r.min.y),
        geo::Vec2::new(r.max.x, r.max.y),
    )
}

/// Lays the machine out ([`geo::layout`], which [`engine_script::state_board`]'s boards share):
/// each group's states round a ring in the order its transitions visit them, a parallel group's
/// regions in a row. States in `at` stay where it puts them when every state that isn't a group
/// has a place there.
pub fn layout(m: &Machine, at: &BTreeMap<String, Pos2>) -> Placed {
    layout_for(m, at, None)
}

/// As [`layout`], for a view `aspect` times as wide as high: a parallel group's regions side
/// by side or stacked, whichever draws larger there ([`geo::layout_for`]).
pub fn layout_for(m: &Machine, at: &BTreeMap<String, Pos2>, aspect: Option<f32>) -> Placed {
    let at = at
        .iter()
        .map(|(k, p)| (k.clone(), geo::Vec2::new(p.x, p.y)))
        .collect();
    let placed = match aspect {
        Some(a) => geo::layout_for(m, &at, a),
        None => geo::layout(m, &at),
    };
    Placed {
        rects: placed
            .rects
            .into_iter()
            .map(|(k, a)| (k, rect(a)))
            .collect(),
        any: placed.any.map(rect),
        bounds: rect(placed.bounds),
    }
}

/// The curves of the machine's transitions, in its order, as points (diagram units).
fn paths(m: &Machine, placed: &Placed) -> Vec<Option<geo::Route>> {
    let placed = geo::Placed {
        rects: placed
            .rects
            .iter()
            .map(|(k, r)| (k.clone(), area(*r)))
            .collect(),
        any: placed.any.map(area),
        bounds: area(placed.bounds),
    };
    geo::paths(m, &placed)
}

/// Diagram units to screen points: `origin + p * scale`.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub origin: Vec2,
    pub scale: f32,
}

impl View {
    pub fn at(&self, p: Pos2) -> Pos2 {
        (p.to_vec2() * self.scale + self.origin).to_pos2()
    }

    pub fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.at(r.min), self.at(r.max))
    }

    /// Screen point to diagram units.
    pub fn back(&self, p: Pos2) -> Pos2 {
        ((p.to_vec2() - self.origin) / self.scale).to_pos2()
    }

    /// Fits `bounds` in `rect`, no bigger than `max_scale`.
    pub fn fit(bounds: Rect, rect: Rect, max_scale: f32) -> View {
        let pad = 26.0;
        let scale = ((rect.width() - 2.0 * pad) / bounds.width().max(1.0))
            .min((rect.height() - 2.0 * pad) / bounds.height().max(1.0))
            .clamp(0.2, max_scale);
        let origin = rect.center().to_vec2() - bounds.center().to_vec2() * scale;
        View { origin, scale }
    }
}

/// What to draw.
pub struct Diagram<'a> {
    pub machine: &'a Machine,
    pub placed: &'a Placed,
    pub live: Option<&'a Live>,
    pub selected: Option<&'a Pick>,
    /// A state the pointer is over (drawn lighter).
    pub hovered: Option<&'a Pick>,
    /// The script's state breakpoints (a red dot: on entry, a red ring: on exit).
    pub breaks: Option<&'a Breaks>,
}

fn text(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, color: Color32) -> Rect {
    p.text(at, align, s, FontId::proportional(size), color)
}

/// Paints the diagram; returns what is where on screen, for clicks and hovers (the last that
/// contains the pointer wins).
pub fn paint(p: &Painter, d: &Diagram, view: View) -> Vec<(Pick, Rect)> {
    let m = d.machine;
    let s = view.scale;
    let mut hits: Vec<(Pick, Rect)> = Vec::new();
    let active: BTreeSet<&str> = d
        .live
        .map(|l| l.active.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let glow = d.live.map(|l| geo::glowing(m, l)).unwrap_or_default();
    let is_selected = |pick: &Pick| d.selected == Some(pick);

    // Groups, outermost first.
    let mut groups: Vec<&engine_script::states::State> =
        m.states.iter().filter(|st| m.is_group(&st.name)).collect();
    groups.sort_by_key(|g| geo::depth(m, &g.name));
    for g in groups {
        let Some(r) = d.placed.rects.get(&g.name).map(|r| view.rect(*r)) else {
            continue;
        };
        let on = active.contains(g.name.as_str());
        let pick = Pick::State(g.name.clone());
        let edge = if is_selected(&pick) {
            Stroke::new(2.0, theme::ACCENT)
        } else if on {
            Stroke::new(1.6, theme::PLAY.gamma_multiply(0.8))
        } else {
            Stroke::new(1.0, GROUP_EDGE)
        };
        p.rect(r, 10.0 * s, GROUP, edge, StrokeKind::Inside);
        let head = Rect::from_min_size(r.min, Vec2::new(r.width(), GROUP_HEAD * s));
        p.rect_filled(
            head.shrink(1.0),
            egui::CornerRadius {
                nw: (9.0 * s) as u8,
                ne: (9.0 * s) as u8,
                sw: 0,
                se: 0,
            },
            if on {
                theme::PLAY.gamma_multiply(0.16)
            } else {
                Color32::from_rgb(0x27, 0x2a, 0x30)
            },
        );
        let title = text(
            p,
            head.left_center() + Vec2::new(10.0 * s, 0.0),
            Align2::LEFT_CENTER,
            &g.name,
            13.5 * s,
            if on { theme::PLAY } else { theme::TEXT },
        );
        if g.parallel {
            // Its regions run at once: dashed lines between them (UML's orthogonal regions).
            text(
                p,
                title.right_center() + Vec2::new(8.0 * s, 0.0),
                Align2::LEFT_CENTER,
                "all at once",
                11.0 * s,
                theme::TEXT_DIM,
            );
            let mut regions: Vec<Rect> = m
                .children(Some(&g.name))
                .filter_map(|c| d.placed.rects.get(&c.name).map(|r| view.rect(*r)))
                .collect();
            // Side by side, or one under another (`geo::layout_for` picks for the view).
            let stacked = regions.len() > 1
                && (regions[0].bottom() <= regions[1].top()
                    || regions[1].bottom() <= regions[0].top());
            if stacked {
                regions.sort_by(|a, b| a.top().total_cmp(&b.top()));
            } else {
                regions.sort_by(|a, b| a.left().total_cmp(&b.left()));
            }
            for pair in regions.windows(2) {
                let line = if stacked {
                    let y = (pair[0].bottom() + pair[1].top()) / 2.0;
                    [
                        Pos2::new(r.left() + 6.0 * s, y),
                        Pos2::new(r.right() - 6.0 * s, y),
                    ]
                } else {
                    let x = (pair[0].right() + pair[1].left()) / 2.0;
                    [
                        Pos2::new(x, head.bottom() + 6.0 * s),
                        Pos2::new(x, r.bottom() - 6.0 * s),
                    ]
                };
                p.add(Shape::dashed_line(
                    &line,
                    Stroke::new(1.2, GROUP_EDGE.gamma_multiply(1.6)),
                    7.0 * s,
                    5.0 * s,
                ));
            }
        }
        break_marks(p, d.breaks, &g.name, head, s);
        // Its right end: the history mark (H, H* for deep), then whether it is done.
        let mut right = head.right() - 8.0 * s;
        if g.history {
            let c = Pos2::new(right - 8.5 * s, head.center().y);
            p.circle_stroke(c, 8.5 * s, Stroke::new(1.2, theme::TEXT_DIM));
            let mark = if g.deep { "H*" } else { "H" };
            text(p, c, Align2::CENTER_CENTER, mark, 10.5 * s, theme::TEXT_DIM);
            right -= 24.0 * s;
        }
        if d.live.is_some_and(|l| l.done.contains(&g.name)) {
            text(
                p,
                Pos2::new(right, head.center().y),
                Align2::RIGHT_CENTER,
                "✔ done",
                12.0 * s,
                theme::PLAY,
            );
        }
        hits.push((pick, head));
    }

    // The outer table's transitions start from "any state".
    if let Some(a) = d.placed.any.map(|r| view.rect(r)) {
        p.rect(
            a,
            a.height() / 2.0,
            Color32::from_rgb(0x22, 0x24, 0x29),
            Stroke::new(1.0, GROUP_EDGE),
            StrokeKind::Inside,
        );
        text(
            p,
            a.center(),
            Align2::CENTER_CENTER,
            "any state",
            12.0 * s,
            theme::TEXT_DIM,
        );
    }

    // Start markers: a dot and an arrow into each first state (UML's initial state).
    for first in geo::firsts(m) {
        let Some(r) = d.placed.rects.get(first).map(|r| view.rect(*r)) else {
            continue;
        };
        let to = r.left_center() - Vec2::new(2.0, 0.0);
        let dot = to - Vec2::new(26.0 * s, 0.0);
        p.circle_filled(dot, 5.0 * s, theme::TEXT);
        p.line_segment([dot, to], Stroke::new(1.4, theme::TEXT));
        arrow_head(p, to, Vec2::X, 7.0 * s, theme::TEXT);
    }

    // Transitions under the states, their labels over them.
    let paths = paths(m, d.placed);
    let mut chips: Vec<(usize, Pos2, Vec2)> = Vec::new();
    for (k, t) in m.transitions.iter().enumerate() {
        let Some(route) = &paths[k] else { continue };
        let path = &route.path;
        let color = kind_color(t.kind());
        let pts: Vec<Pos2> = path
            .points(24)
            .into_iter()
            .map(|q| view.at(pos(q)))
            .collect();
        let from_on = t.from.as_deref().is_none_or(|f| active.contains(f));
        let lit = glow.iter().find(|(g, _)| *g == k).map(|(_, a)| *a);
        let selected = is_selected(&Pick::Transition(k));
        let base = if selected {
            theme::ACCENT
        } else if from_on || active.is_empty() {
            color.gamma_multiply(0.85)
        } else {
            color.gamma_multiply(0.38)
        };
        let width = if selected { 2.4 } else { 1.6 };
        if let Some(a) = lit {
            p.add(Shape::line(
                pts.clone(),
                Stroke::new(width + 7.0 * a, color.gamma_multiply(0.35 * a)),
            ));
        }
        p.add(Shape::line(pts.clone(), Stroke::new(width, base)));
        let n = pts.len();
        let tip = pts[n - 1];
        let dir = tip - pts[n - 3];
        arrow_head(p, tip, dir, 9.0 * s, base);
        if let Some(a) = lit {
            // A spark runs along it as it fires.
            let along = (1.0 - a) * 2.4;
            if along < 1.0 {
                let q = view.at(pos(path.point(along.min(1.0))));
                p.circle_filled(q, 4.5 * s.max(0.6), Color32::WHITE.gamma_multiply(0.9));
            }
        }
        let mid = view.at(pos(path.point(route.label)));
        chips.push((k, mid, dir));
    }

    // States.
    for st in m.states.iter().filter(|st| !m.is_group(&st.name)) {
        let Some(r) = d.placed.rects.get(&st.name).map(|r| view.rect(*r)) else {
            continue;
        };
        let on = active.contains(st.name.as_str());
        let is_current = d.live.is_some_and(|l| l.is_current(&st.name));
        let pick = Pick::State(st.name.clone());
        if let Some(a) = d.live.and_then(|l| geo::just_entered(l, &st.name)) {
            p.rect_filled(
                r.expand(6.0 * a * s),
                12.0 * s,
                theme::PLAY.gamma_multiply(0.22 * a),
            );
        }
        let fill = if is_current {
            lerp(BOX, theme::PLAY, 0.2)
        } else if d.hovered == Some(&pick) {
            theme::HOVER
        } else {
            BOX
        };
        let edge = if is_selected(&pick) {
            Stroke::new(2.4, theme::ACCENT)
        } else if is_current {
            Stroke::new(2.6, theme::PLAY)
        } else {
            Stroke::new(1.0, Color32::from_rgb(0x46, 0x4a, 0x52))
        };
        p.rect(r, 8.0 * s, fill, edge, StrokeKind::Inside);
        if st.is_final {
            // A final state: a second edge inside the first (where its group ends).
            p.rect_stroke(
                r.shrink(4.0 * s),
                5.0 * s,
                Stroke::new(edge.width * 0.8, edge.color),
                StrokeKind::Inside,
            );
        }
        text(
            p,
            r.left_top() + Vec2::new(12.0 * s, 10.0 * s),
            Align2::LEFT_TOP,
            &st.name,
            15.0 * s,
            if is_current {
                Color32::WHITE
            } else {
                theme::TEXT
            },
        );
        // What it does: its hooks, or that its enter task is still going.
        let running = d.live.is_some_and(|l| l.running.contains(&st.name));
        let (line, color) = if running {
            ("⏳ enter running".to_string(), kind_color("after"))
        } else if st.is_final {
            let mut what = vec!["final".to_string()];
            what.extend(st.hooks.iter().cloned());
            (
                what.join(" · "),
                if is_current {
                    theme::PLAY
                } else {
                    theme::TEXT_DIM
                },
            )
        } else if st.hooks.is_empty() {
            ("—".to_string(), theme::TEXT_DIM.gamma_multiply(0.7))
        } else {
            (st.hooks.join(" · "), theme::TEXT_DIM)
        };
        text(
            p,
            r.left_bottom() + Vec2::new(12.0 * s, -10.0 * s),
            Align2::LEFT_BOTTOM,
            &line,
            11.5 * s,
            color,
        );
        break_marks(p, d.breaks, &st.name, r, s);
        if on && let Some(secs) = d.live.and_then(|l| l.secs_in(&st.name)) {
            text(
                p,
                r.right_top() + Vec2::new(-10.0 * s, 10.0 * s),
                Align2::RIGHT_TOP,
                &format!("{secs:.1} s"),
                12.0 * s,
                theme::PLAY,
            );
        }
        hits.push((pick, r));
    }

    // Transition labels, with `after` timers filling while their state is active.
    for (k, mid, _) in chips {
        let t = &m.transitions[k];
        let color = kind_color(t.kind());
        let label = t.label();
        let font = FontId::proportional(11.5 * s.max(0.75));
        let galley = p.layout_no_wrap(label.clone(), font.clone(), color);
        let size = galley.size() + Vec2::new(12.0, 6.0);
        let chip = Rect::from_center_size(mid, size);
        let selected = is_selected(&Pick::Transition(k));
        p.rect(
            chip,
            4.0,
            CHIP,
            Stroke::new(
                1.0,
                if selected {
                    theme::ACCENT
                } else {
                    color.gamma_multiply(0.55)
                },
            ),
            StrokeKind::Inside,
        );
        if let Some(f) = d.live.and_then(|l| geo::timer(t, l)) {
            let bar = Rect::from_min_size(
                chip.left_bottom() + Vec2::new(2.0, -3.0),
                Vec2::new((chip.width() - 4.0) * f, 2.0),
            );
            p.rect_filled(bar, 1.0, color);
        }
        p.galley(chip.center() - galley.size() / 2.0, galley, color);
        hits.push((Pick::Transition(k), chip));
    }
    hits
}

/// A state's breakpoints on its box's top left corner: a red dot for its entry, a red ring for
/// its exit.
fn break_marks(p: &Painter, breaks: Option<&Breaks>, name: &str, r: Rect, s: f32) {
    let Some(b) = breaks else {
        return;
    };
    let at = r.left_top() + Vec2::new(1.0, 1.0);
    if b.enter.contains(name) {
        p.circle_filled(at, 6.5 * s.max(0.6), BREAK);
        p.circle_stroke(
            at,
            6.5 * s.max(0.6),
            Stroke::new(1.0, Color32::from_rgb(0x2a, 0x0c, 0x0a)),
        );
    }
    if b.exit.contains(name) {
        let at = at + Vec2::new(16.0 * s.max(0.6), 0.0);
        p.circle_filled(at, 6.0 * s.max(0.6), Color32::from_rgb(0x2a, 0x0c, 0x0a));
        p.circle_stroke(at, 5.0 * s.max(0.6), Stroke::new(2.2, BREAK));
    }
}

fn arrow_head(p: &Painter, tip: Pos2, dir: Vec2, size: f32, color: Color32) {
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

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// The pick under `pointer` (the topmost: labels over states over groups).
pub fn hit(hits: &[(Pick, Rect)], pointer: Pos2) -> Option<Pick> {
    hits.iter()
        .rev()
        .find(|(_, r)| r.contains(pointer))
        .map(|(p, _)| p.clone())
}

/// The log of changes, newest first: the time, from, to and why. `shown` (an index in the log)
/// is highlighted; returns the row clicked, to show the machine as it was after it.
pub fn log_rows(
    ui: &mut egui::Ui,
    live: &Live,
    rows: usize,
    shown: Option<usize>,
) -> Option<usize> {
    if live.log.is_empty() {
        ui.label(theme::dim("No changes yet."));
        return None;
    }
    let mut clicked = None;
    let n = live.log.len();
    for (k, c) in live.log.iter().enumerate().rev().take(rows) {
        let under = ui.painter().add(Shape::Noop);
        let row = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(
                    egui::RichText::new(format!("{:>7.2} s", c.t))
                        .monospace()
                        .color(theme::TEXT_DIM),
                );
                if c.from.is_empty() {
                    ui.label(egui::RichText::new("start").color(theme::TEXT_DIM));
                } else {
                    ui.label(egui::RichText::new(&c.from).color(theme::TEXT));
                }
                ui.label(egui::RichText::new("➡").color(theme::TEXT_DIM));
                ui.label(egui::RichText::new(&c.to).color(theme::PLAY).strong());
                let color = kind_color(geo::why_kind(&c.why));
                ui.label(egui::RichText::new(&c.why).color(color));
                ui.allocate_space(Vec2::new(ui.available_width(), 0.0));
            })
            .response;
        let row = ui.interact(
            row.rect,
            ui.id().with(("state log row", k)),
            egui::Sense::click(),
        );
        if shown == Some(k) {
            ui.painter().set(
                under,
                egui::epaint::RectShape::filled(
                    row.rect.expand(2.0),
                    3.0,
                    theme::ACCENT.gamma_multiply(0.22),
                ),
            );
        } else if row.hovered() {
            ui.painter().set(
                under,
                egui::epaint::RectShape::filled(row.rect.expand(2.0), 3.0, theme::HOVER),
            );
        }
        if row
            .on_hover_text(format!(
                "Show the machine as it was after this change ({} of {n})",
                k + 1
            ))
            .clicked()
        {
            clicked = Some(k);
        }
    }
    clicked
}

/// The script's values right after the change `shown` (None: now), as PlayMaker's Debug Flow
/// shows them: at an earlier change, the ones it changed are lit and what they are now stands
/// beside them.
pub fn value_rows(ui: &mut egui::Ui, live: &Live, shown: Option<usize>) {
    let (values, before) = match shown {
        Some(k) => (
            &live.log[k].values,
            k.checked_sub(1).map(|j| &live.log[j].values),
        ),
        None => (&live.values, None),
    };
    if values.is_empty() {
        ui.label(theme::dim("No values to show."));
        return;
    }
    let was = |list: &Vec<(String, String)>, name: &str| {
        list.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    };
    egui::Grid::new("state values")
        .num_columns(3)
        .spacing(Vec2::new(10.0, 2.0))
        .show(ui, |ui| {
            for (name, value) in values {
                let changed = before.is_some_and(|b| was(b, name).as_ref() != Some(value));
                ui.label(egui::RichText::new(name).monospace().color(theme::TEXT_DIM));
                let v = egui::RichText::new(value).monospace();
                ui.label(if changed {
                    v.color(theme::ACCENT).strong()
                } else {
                    v.color(theme::TEXT)
                })
                .on_hover_text(if changed {
                    "this change set it"
                } else {
                    "as it was then"
                });
                match was(&live.values, name).filter(|now| shown.is_some() && now != value) {
                    Some(now) => {
                        ui.label(theme::dim(format!("now {now}")));
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
}

/// A diagram filling `rect` (fitted, no larger than life), with its hits.
pub fn fitted(ui: &egui::Ui, rect: Rect, d: &Diagram) -> (Vec<(Pick, Rect)>, View) {
    let view = View::fit(d.placed.bounds, rect, 1.0);
    let painter = ui.painter_at(rect);
    (paint(&painter, d, view), view)
}
