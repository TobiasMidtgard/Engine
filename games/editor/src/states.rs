//! State machine diagrams: a script's `states` table drawn as boxes and arrows (Scripting's;
//! engine-design/scripting-research.md §19). While the game plays, the active states are lit
//! with their time, the change just made glows along its arrow, `after` timers fill and the
//! log lists each change with what caused it, as PlayMaker's graph view and FSM log, Unity's
//! state graphs and Unreal's StateTree debugger show them. The Scripts tab, the Details panel
//! and the Graphs tab's States view draw with it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use engine_script::states::{Live, Machine, Transition, Trigger};

use crate::theme;

/// A state's box (diagram units: pixels at zoom 1).
pub const STATE_W: f32 = 156.0;
pub const STATE_H: f32 = 58.0;
const GROUP_PAD: f32 = 22.0;
const GROUP_HEAD: f32 = 30.0;
const GAP_X: f32 = 96.0;
const GAP_Y: f32 = 78.0;
/// Room between transitions that share two states.
const BEND: f32 = 40.0;
/// World seconds the last change glows.
const GLOW_SECS: f64 = 1.6;

const BOX: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x32);
const GROUP: Color32 = Color32::from_rgb(0x1f, 0x22, 0x26);
const GROUP_EDGE: Color32 = Color32::from_rgb(0x3a, 0x3e, 0x46);
const CHIP: Color32 = Color32::from_rgb(0x17, 0x18, 0x1b);

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

/// Lays the machine out: each group's states round a ring in the order its transitions visit
/// them from the first (a cycle reads clockwise), groups sized to hold theirs. States in `at`
/// stay where it puts them when every state that isn't a group has a place there.
pub fn layout(m: &Machine, at: &BTreeMap<String, Pos2>) -> Placed {
    let leaves: Vec<&str> = m
        .states
        .iter()
        .filter(|s| !m.is_group(&s.name))
        .map(|s| s.name.as_str())
        .collect();
    let mut rects = BTreeMap::new();
    if !leaves.is_empty() && leaves.iter().all(|l| at.contains_key(*l)) {
        for l in &leaves {
            rects.insert(
                l.to_string(),
                Rect::from_min_size(at[*l], Vec2::new(STATE_W, STATE_H)),
            );
        }
        // Groups hold theirs, innermost first.
        let mut groups: Vec<(usize, &str)> = m
            .states
            .iter()
            .filter(|s| m.is_group(&s.name))
            .map(|s| (depth(m, &s.name), s.name.as_str()))
            .collect();
        groups.sort_by_key(|g| std::cmp::Reverse(g.0));
        for (_, g) in groups {
            let inner = m
                .children(Some(g))
                .filter_map(|c| rects.get(&c.name).copied())
                .reduce(|a, b| a.union(b));
            if let Some(inner) = inner {
                rects.insert(
                    g.to_string(),
                    Rect::from_min_max(
                        inner.min - Vec2::new(GROUP_PAD, GROUP_PAD + GROUP_HEAD),
                        inner.max + Vec2::splat(GROUP_PAD),
                    ),
                );
            }
        }
    } else {
        let (_, placed) = arrange(m, None);
        rects = placed.into_iter().collect();
    }
    let mut bounds = rects
        .values()
        .copied()
        .reduce(|a, b| a.union(b))
        .unwrap_or(Rect::from_min_size(Pos2::ZERO, Vec2::new(STATE_W, STATE_H)));
    let any =
        m.transitions.iter().any(|t| t.from.is_none()).then(|| {
            Rect::from_min_size(bounds.min + Vec2::new(-8.0, -92.0), Vec2::new(112.0, 34.0))
        });
    if let Some(a) = any {
        bounds = bounds.union(a);
    }
    Placed { rects, any, bounds }
}

fn depth(m: &Machine, name: &str) -> usize {
    let mut d = 0;
    let mut at = m.state(name).and_then(|s| s.parent.clone());
    while let Some(p) = at {
        d += 1;
        at = m.state(&p).and_then(|s| s.parent.clone());
    }
    d
}

/// The child of `group` that `name` is in (itself, or a group around it).
fn sibling_of<'a>(m: &'a Machine, group: Option<&str>, name: &'a str) -> Option<&'a str> {
    let mut at = name;
    loop {
        let s = m.state(at)?;
        if s.parent.as_deref() == group {
            return Some(&s.name);
        }
        at = s.parent.as_deref()?;
    }
}

/// Lays out `group`'s states (None: the outer level): its size and their rects, from (0, 0).
fn arrange(m: &Machine, group: Option<&str>) -> (Vec2, Vec<(String, Rect)>) {
    let kids: Vec<&str> = m.children(group).map(|s| s.name.as_str()).collect();
    if kids.is_empty() {
        return (Vec2::new(STATE_W, STATE_H), Vec::new());
    }
    // Visit order: from the first state along the transitions between these states.
    let first = match group {
        Some(g) => m.state(g).and_then(|s| s.initial.clone()),
        None => Some(m.initial.clone()),
    }
    .filter(|f| kids.contains(&f.as_str()))
    .unwrap_or_else(|| kids[0].to_string());
    let mut order: Vec<String> = Vec::new();
    let mut queue = VecDeque::from([first]);
    let mut seen = BTreeSet::new();
    while let Some(s) = queue.pop_front() {
        if !seen.insert(s.clone()) {
            continue;
        }
        order.push(s.clone());
        for t in &m.transitions {
            let Some(from) = t.from.as_deref() else {
                continue;
            };
            if sibling_of(m, group, from) == Some(s.as_str())
                && let Some(to) = sibling_of(m, group, &t.to)
                && !seen.contains(to)
            {
                queue.push_back(to.to_string());
            }
        }
    }
    for k in &kids {
        if !seen.contains(*k) {
            order.push(k.to_string());
        }
    }
    // Each state's size (a group's from its own).
    let mut inner: BTreeMap<String, Vec<(String, Rect)>> = BTreeMap::new();
    let sizes: Vec<Vec2> = order
        .iter()
        .map(|s| {
            if m.is_group(s) {
                let (size, placed) = arrange(m, Some(s));
                inner.insert(s.clone(), placed);
                size + Vec2::new(2.0 * GROUP_PAD, 2.0 * GROUP_PAD + GROUP_HEAD)
            } else {
                Vec2::new(STATE_W, STATE_H)
            }
        })
        .collect();
    let cells = ring(order.len());
    let cols = cells.iter().map(|c| c.0).max().unwrap_or(0) + 1;
    let rows = cells.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let mut widths = vec![0f32; cols];
    let mut heights = vec![0f32; rows];
    for (k, (c, r)) in cells.iter().enumerate() {
        widths[*c] = widths[*c].max(sizes[k].x);
        heights[*r] = heights[*r].max(sizes[k].y);
    }
    let xs: Vec<f32> = widths
        .iter()
        .scan(0.0, |x, w| {
            let at = *x;
            *x += w + GAP_X;
            Some(at)
        })
        .collect();
    let ys: Vec<f32> = heights
        .iter()
        .scan(0.0, |y, h| {
            let at = *y;
            *y += h + GAP_Y;
            Some(at)
        })
        .collect();
    let mut out = Vec::new();
    for (k, name) in order.iter().enumerate() {
        let (c, r) = cells[k];
        // Centered in its cell.
        let min = Pos2::new(
            xs[c] + (widths[c] - sizes[k].x) / 2.0,
            ys[r] + (heights[r] - sizes[k].y) / 2.0,
        );
        let rect = Rect::from_min_size(min, sizes[k]);
        out.push((name.clone(), rect));
        if let Some(placed) = inner.remove(name) {
            let offset = rect.min.to_vec2() + Vec2::new(GROUP_PAD, GROUP_PAD + GROUP_HEAD);
            out.extend(placed.into_iter().map(|(n, r)| (n, r.translate(offset))));
        }
    }
    let size = Vec2::new(
        widths.iter().sum::<f32>() + GAP_X * (cols as f32 - 1.0),
        heights.iter().sum::<f32>() + GAP_Y * (rows as f32 - 1.0),
    );
    (size, out)
}

/// Grid cells (column, row) for `n` states, clockwise round the edge from the top left, then
/// the middle.
fn ring(n: usize) -> Vec<(usize, usize)> {
    let cols = match n {
        0..=2 => n.max(1),
        3 | 4 => 2,
        5 | 6 => 3,
        7..=9 => 3,
        _ => 4,
    };
    let rows = n.div_ceil(cols);
    let mut edge = Vec::new();
    if rows == 1 {
        edge.extend((0..cols).map(|c| (c, 0)));
    } else {
        edge.extend((0..cols).map(|c| (c, 0)));
        edge.extend((1..rows - 1).map(|r| (cols - 1, r)));
        edge.extend((0..cols).rev().map(|c| (c, rows - 1)));
        edge.extend((1..rows - 1).rev().map(|r| (0, r)));
    }
    let mut cells: Vec<(usize, usize)> = edge;
    for r in 1..rows.saturating_sub(1) {
        for c in 1..cols.saturating_sub(1) {
            cells.push((c, r));
        }
    }
    cells.truncate(n);
    // A short last row: its cells as far left as the ring allows.
    cells
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
}

/// Where a transition runs: a quadratic curve, or for a loop a cubic.
enum Path {
    Quad([Pos2; 3]),
    Cubic([Pos2; 4]),
}

impl Path {
    fn point(&self, t: f32) -> Pos2 {
        let u = 1.0 - t;
        match self {
            Path::Quad([a, b, c]) => {
                (a.to_vec2() * u * u + b.to_vec2() * 2.0 * u * t + c.to_vec2() * t * t).to_pos2()
            }
            Path::Cubic([a, b, c, d]) => (a.to_vec2() * u * u * u
                + b.to_vec2() * 3.0 * u * u * t
                + c.to_vec2() * 3.0 * u * t * t
                + d.to_vec2() * t * t * t)
                .to_pos2(),
        }
    }

    fn points(&self, n: usize) -> Vec<Pos2> {
        (0..=n).map(|k| self.point(k as f32 / n as f32)).collect()
    }
}

/// Where the ray from `r`'s middle along `dir` leaves it.
fn exit_point(r: Rect, dir: Vec2) -> Pos2 {
    let c = r.center();
    let d = if dir.length() < 1e-3 {
        Vec2::X
    } else {
        dir.normalized()
    };
    let tx = if d.x.abs() > 1e-4 {
        (r.width() / 2.0) / d.x.abs()
    } else {
        f32::INFINITY
    };
    let ty = if d.y.abs() > 1e-4 {
        (r.height() / 2.0) / d.y.abs()
    } else {
        f32::INFINITY
    };
    c + d * tx.min(ty)
}

/// The curves of the machine's transitions, in its order (diagram units).
fn paths(m: &Machine, placed: &Placed) -> Vec<Option<Path>> {
    let key = |t: &Transition| t.from.clone().unwrap_or_else(|| "*".into());
    let rect_of = |name: &str| -> Option<Rect> {
        if name == "*" {
            placed.any
        } else {
            placed.rects.get(name).copied()
        }
    };
    // Transitions sharing two states fan out.
    let mut pairs: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (k, t) in m.transitions.iter().enumerate() {
        let (a, b) = (key(t), t.to.clone());
        let pair = if a <= b { (a, b) } else { (b, a) };
        pairs.entry(pair).or_default().push(k);
    }
    let mut out: Vec<Option<Path>> = (0..m.transitions.len()).map(|_| None).collect();
    for ((a, b), list) in pairs {
        let (Some(ra), Some(rb)) = (rect_of(&a), rect_of(&b)) else {
            continue;
        };
        if a == b {
            for (i, k) in list.iter().enumerate() {
                let lift = 46.0 + 26.0 * i as f32;
                let p0 = Pos2::new(ra.left() + ra.width() * 0.32, ra.top());
                let p3 = Pos2::new(ra.left() + ra.width() * 0.68, ra.top());
                out[*k] = Some(Path::Cubic([
                    p0,
                    p0 + Vec2::new(-26.0, -lift),
                    p3 + Vec2::new(26.0, -lift),
                    p3,
                ]));
            }
            continue;
        }
        let along = rb.center() - ra.center();
        let normal = Vec2::new(-along.y, along.x).normalized();
        let n = list.len() as f32;
        for (i, k) in list.iter().enumerate() {
            let t = &m.transitions[*k];
            let off = (i as f32 - (n - 1.0) / 2.0) * BEND;
            let (from, to) = if key(t) == a { (ra, rb) } else { (rb, ra) };
            let mid = from.center() + (to.center() - from.center()) / 2.0 + normal * off;
            let p0 = exit_point(from, mid - from.center());
            let p2 = exit_point(to, mid - to.center());
            // The control point that makes the curve pass through `mid`.
            let c = (mid.to_vec2() * 2.0 - (p0.to_vec2() + p2.to_vec2()) / 2.0).to_pos2();
            out[*k] = Some(Path::Quad([p0, c, p2]));
        }
    }
    out
}

/// The transition behind the last change in the log, if it is still glowing: its index and how
/// much (1 down to 0).
fn glowing(m: &Machine, live: &Live) -> Option<(usize, f32)> {
    let last = live.log.last()?;
    let age = live.clock - last.t;
    if !(0.0..GLOW_SECS).contains(&age) || last.from.is_empty() {
        return None;
    }
    let within = |inner: &str, outer: &str| {
        let mut at = Some(inner.to_string());
        while let Some(s) = at {
            if s == outer {
                return true;
            }
            at = m.state(&s).and_then(|s| s.parent.clone());
        }
        false
    };
    let fits = |t: &Transition| {
        let from_ok = t.from.as_deref().is_none_or(|f| within(&last.from, f));
        let to_ok = within(&last.to, &t.to);
        let why_ok = match &t.trigger {
            Trigger::On { event, .. } => {
                last.why == *event || last.why.starts_with(&format!("{event} by "))
            }
            Trigger::After { .. } => last.why.starts_with("after "),
            Trigger::Done => last.why == "done",
            Trigger::When(c) => last.why == format!("when {c}"),
        };
        from_ok && to_ok && why_ok
    };
    let k = m.transitions.iter().position(fits)?;
    Some((k, (1.0 - age / GLOW_SECS) as f32))
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
    let current = d.live.and_then(|l| l.current());
    let glow = d.live.and_then(|l| glowing(m, l));
    let is_selected = |pick: &Pick| d.selected == Some(pick);

    // Groups, outermost first.
    let mut groups: Vec<&engine_script::states::State> =
        m.states.iter().filter(|st| m.is_group(&st.name)).collect();
    groups.sort_by_key(|g| depth(m, &g.name));
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
        text(
            p,
            head.left_center() + Vec2::new(10.0 * s, 0.0),
            Align2::LEFT_CENTER,
            &g.name,
            13.5 * s,
            if on { theme::PLAY } else { theme::TEXT },
        );
        if g.history {
            let c = head.right_center() - Vec2::new(16.0 * s, 0.0);
            p.circle_stroke(c, 8.5 * s, Stroke::new(1.2, theme::TEXT_DIM));
            text(p, c, Align2::CENTER_CENTER, "H", 10.5 * s, theme::TEXT_DIM);
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
    let mut firsts: Vec<(Option<&str>, &str)> = vec![(None, m.initial.as_str())];
    for st in &m.states {
        if let Some(i) = &st.initial {
            firsts.push((Some(&st.name), i));
        }
    }
    for (_, first) in firsts {
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
        let Some(path) = &paths[k] else { continue };
        let color = kind_color(t.kind());
        let pts: Vec<Pos2> = path.points(24).into_iter().map(|q| view.at(q)).collect();
        let from_on = t.from.as_deref().is_none_or(|f| active.contains(f));
        let lit = glow.filter(|(g, _)| *g == k).map(|(_, a)| a);
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
                let q = view.at(path.point(along.min(1.0)));
                p.circle_filled(q, 4.5 * s.max(0.6), Color32::WHITE.gamma_multiply(0.9));
            }
        }
        let mid = view.at(path.point(0.5));
        chips.push((k, mid, dir));
    }

    // States.
    for st in m.states.iter().filter(|st| !m.is_group(&st.name)) {
        let Some(r) = d.placed.rects.get(&st.name).map(|r| view.rect(*r)) else {
            continue;
        };
        let on = active.contains(st.name.as_str());
        let is_current = current == Some(st.name.as_str());
        let pick = Pick::State(st.name.clone());
        let just = d.live.and_then(|l| {
            let last = l.log.last()?;
            let age = l.clock - last.t;
            (last.to == st.name && (0.0..GLOW_SECS).contains(&age))
                .then_some((1.0 - age / GLOW_SECS) as f32)
        });
        if let Some(a) = just {
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
        if let (Trigger::After { secs, .. }, Some(from), Some(l)) = (&t.trigger, &t.from, d.live)
            && let Some(inside) = l.secs_in(from)
            && *secs > 0.0
        {
            let f = (inside / secs).clamp(0.0, 1.0) as f32;
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

/// The log of changes, newest first: the time, from, to and why.
pub fn log_rows(ui: &mut egui::Ui, live: &Live, rows: usize) {
    if live.log.is_empty() {
        ui.label(theme::dim("No changes yet."));
        return;
    }
    for c in live.log.iter().rev().take(rows) {
        ui.horizontal(|ui| {
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
            let kind = c.why.split_whitespace().next().unwrap_or("");
            let color = match kind {
                "after" => kind_color("after"),
                "done" => kind_color("done"),
                "when" => kind_color("when"),
                "go" | "start" | "reload" => theme::TEXT_DIM,
                _ => kind_color("on"),
            };
            ui.label(egui::RichText::new(&c.why).color(color));
        });
    }
}

/// A diagram filling `rect` (fitted, no larger than life), with its hits.
pub fn fitted(
    ui: &egui::Ui,
    rect: Rect,
    machine: &Machine,
    placed: &Placed,
    live: Option<&Live>,
    selected: Option<&Pick>,
    hovered: Option<&Pick>,
) -> (Vec<(Pick, Rect)>, View) {
    let view = View::fit(placed.bounds, rect, 1.0);
    let painter = ui.painter_at(rect);
    let d = Diagram {
        machine,
        placed,
        live,
        selected,
        hovered,
    };
    (paint(&painter, &d, view), view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_script::states::{Machine, State, Transition, Trigger};

    fn gate() -> Machine {
        let st = |name: &str, parent: Option<&str>| State {
            name: name.into(),
            parent: parent.map(str::to_string),
            ..Default::default()
        };
        let on = |from: &str, to: &str, event: &str| Transition {
            from: Some(from.into()),
            to: to.into(),
            trigger: Trigger::On {
                event: event.into(),
                guard: None,
            },
            line: 0,
        };
        let mut powered = st("powered", None);
        powered.initial = Some("closed".into());
        Machine {
            initial: "powered".into(),
            states: vec![
                powered,
                st("closed", Some("powered")),
                st("opening", Some("powered")),
                st("open", Some("powered")),
                st("closing", Some("powered")),
                st("unpowered", None),
            ],
            transitions: vec![
                on("closed", "opening", "interact"),
                Transition {
                    from: Some("opening".into()),
                    to: "open".into(),
                    trigger: Trigger::Done,
                    line: 0,
                },
                on("open", "closing", "interact"),
                Transition {
                    from: Some("closing".into()),
                    to: "closed".into(),
                    trigger: Trigger::Done,
                    line: 0,
                },
                on("powered", "unpowered", "power"),
                on("unpowered", "powered", "power"),
            ],
            line: 0,
        }
    }

    #[test]
    fn a_cycle_goes_round_clockwise_inside_its_group() {
        let m = gate();
        let placed = layout(&m, &BTreeMap::new());
        let r = |n: &str| placed.rects[n];
        // closed top left, opening top right, open bottom right, closing bottom left.
        assert!(r("closed").center().x < r("opening").center().x);
        assert!((r("closed").center().y - r("opening").center().y).abs() < 1.0);
        assert!(r("open").center().y > r("opening").center().y);
        assert!(r("closing").center().x < r("open").center().x);
        // The group holds its states; the other state sits beside it.
        let g = r("powered");
        for s in ["closed", "opening", "open", "closing"] {
            assert!(g.contains_rect(r(s)), "{s} outside its group");
        }
        assert!(!g.intersects(r("unpowered")));
        assert!(placed.any.is_none());
    }

    #[test]
    fn transitions_between_the_same_states_bend_apart() {
        let m = gate();
        let placed = layout(&m, &BTreeMap::new());
        let ps = paths(&m, &placed);
        let mid = |k: usize| ps[k].as_ref().unwrap().point(0.5);
        // powered -> unpowered and back: two curves, not one line.
        assert!((mid(4) - mid(5)).length() > BEND * 0.8);
    }

    #[test]
    fn rings_cover_each_cell_once() {
        for n in 1..14 {
            let cells = ring(n);
            assert_eq!(cells.len(), n);
            let unique: BTreeSet<_> = cells.iter().collect();
            assert_eq!(unique.len(), n, "{n}: {cells:?}");
        }
        assert_eq!(ring(4), vec![(0, 0), (1, 0), (1, 1), (0, 1)]);
    }
}
