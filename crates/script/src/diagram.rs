//! Where a state machine's diagram puts things: each group's states round a ring in the order
//! its transitions visit them, a parallel group's regions side by side, transitions as curves
//! that fan out when they share two states, and which of them glows after a change. The
//! editor's diagrams (Scripts tab, Details panel, Graphs tab) and [`crate::state_board`]'s
//! boards in the world both draw from it, so a machine looks the same everywhere.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Points and sizes (diagram units, y down).
pub use engine_core::glam::Vec2;

use crate::states::{Change, Live, Machine, Transition, Trigger};

/// A state's box (diagram units: pixels at zoom 1).
pub const STATE_W: f32 = 156.0;
pub const STATE_H: f32 = 58.0;
pub const GROUP_PAD: f32 = 22.0;
pub const GROUP_HEAD: f32 = 30.0;
const GAP_X: f32 = 96.0;
const GAP_Y: f32 = 92.0;
/// Room between transitions that share two states.
pub const BEND: f32 = 40.0;
/// World seconds a change glows.
pub const GLOW_SECS: f64 = 1.6;

/// An axis-aligned rectangle (diagram units, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub min: Vec2,
    pub max: Vec2,
}

impl Area {
    pub fn from_min_size(min: Vec2, size: Vec2) -> Area {
        Area {
            min,
            max: min + size,
        }
    }

    pub fn from_min_max(min: Vec2, max: Vec2) -> Area {
        Area { min, max }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    pub fn center(&self) -> Vec2 {
        (self.min + self.max) / 2.0
    }

    pub fn union(self, o: Area) -> Area {
        Area {
            min: self.min.min(o.min),
            max: self.max.max(o.max),
        }
    }

    pub fn translate(self, by: Vec2) -> Area {
        Area {
            min: self.min + by,
            max: self.max + by,
        }
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn contains_area(&self, o: Area) -> bool {
        self.contains(o.min) && self.contains(o.max)
    }

    pub fn intersects(&self, o: Area) -> bool {
        self.min.x <= o.max.x
            && o.min.x <= self.max.x
            && self.min.y <= o.max.y
            && o.min.y <= self.max.y
    }
}

/// Where everything goes (diagram units).
#[derive(Debug, Clone)]
pub struct Placed {
    pub rects: BTreeMap<String, Area>,
    /// The "any state" box the outer table's transitions start from.
    pub any: Option<Area>,
    pub bounds: Area,
}

/// Lays the machine out: each group's states round a ring in the order its transitions visit
/// them from the first (a cycle reads clockwise), a parallel group's regions in a row, groups
/// sized to hold theirs. States in `at` stay where it puts them (their top left corners) when
/// every state that isn't a group has a place there.
pub fn layout(m: &Machine, at: &BTreeMap<String, Vec2>) -> Placed {
    layout_with(m, at, false)
}

/// As [`layout`], for a view `aspect` times as wide as it is high: a parallel group's regions
/// in a row or one under another (UML draws either), whichever the machine fits that view
/// larger with. A machine of three regions side by side, each two states wide, is a strip too
/// thin to read in a panel beside the code; stacked, it fills it.
pub fn layout_for(m: &Machine, at: &BTreeMap<String, Vec2>, aspect: f32) -> Placed {
    let row = layout_with(m, at, false);
    if !m.states.iter().any(|s| s.parallel) {
        return row;
    }
    let column = layout_with(m, at, true);
    let fit =
        |p: &Placed| (aspect / p.bounds.width().max(1.0)).min(1.0 / p.bounds.height().max(1.0));
    if fit(&column) > fit(&row) * 1.05 {
        column
    } else {
        row
    }
}

/// Whether a parallel group's regions are one under another (else side by side).
pub fn stacked(m: &Machine, placed: &Placed, group: &str) -> bool {
    let mut regions = m
        .children(Some(group))
        .filter_map(|c| placed.rects.get(&c.name).copied());
    match (regions.next(), regions.next()) {
        (Some(a), Some(b)) => a.max.y <= b.min.y || b.max.y <= a.min.y,
        _ => false,
    }
}

fn layout_with(m: &Machine, at: &BTreeMap<String, Vec2>, stack: bool) -> Placed {
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
                Area::from_min_size(at[*l], Vec2::new(STATE_W, STATE_H)),
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
                .reduce(Area::union);
            if let Some(inner) = inner {
                rects.insert(
                    g.to_string(),
                    Area::from_min_max(
                        inner.min - Vec2::new(GROUP_PAD, GROUP_PAD + GROUP_HEAD),
                        inner.max + Vec2::splat(GROUP_PAD),
                    ),
                );
            }
        }
    } else {
        let (_, placed) = arrange(m, None, stack);
        rects = placed.into_iter().collect();
    }
    let mut bounds = rects
        .values()
        .copied()
        .reduce(Area::union)
        .unwrap_or(Area::from_min_size(Vec2::ZERO, Vec2::new(STATE_W, STATE_H)));
    let any =
        m.transitions.iter().any(|t| t.from.is_none()).then(|| {
            Area::from_min_size(bounds.min + Vec2::new(-8.0, -92.0), Vec2::new(112.0, 34.0))
        });
    if let Some(a) = any {
        bounds = bounds.union(a);
    }
    Placed { rects, any, bounds }
}

/// How many groups `name` is inside.
pub fn depth(m: &Machine, name: &str) -> usize {
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
/// `stack`: parallel groups' regions one under another.
fn arrange(m: &Machine, group: Option<&str>, stack: bool) -> (Vec2, Vec<(String, Area)>) {
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
    let mut inner: BTreeMap<String, Vec<(String, Area)>> = BTreeMap::new();
    let sizes: Vec<Vec2> = order
        .iter()
        .map(|s| {
            if m.is_group(s) {
                let (size, placed) = arrange(m, Some(s), stack);
                inner.insert(s.clone(), placed);
                size + Vec2::new(2.0 * GROUP_PAD, 2.0 * GROUP_PAD + GROUP_HEAD)
            } else {
                Vec2::new(STATE_W, STATE_H)
            }
        })
        .collect();
    // A parallel group's regions side by side (or one under another); other groups round a
    // ring.
    let parallel = group.is_some_and(|g| m.state(g).is_some_and(|s| s.parallel));
    let cells = if parallel && stack {
        (0..order.len()).map(|r| (0, r)).collect()
    } else if parallel {
        (0..order.len()).map(|c| (c, 0)).collect()
    } else {
        ring(order.len())
    };
    let cols = cells.iter().map(|c| c.0).max().unwrap_or(0) + 1;
    let rows = cells.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let mut widths = vec![0f32; cols];
    let mut heights = vec![0f32; rows];
    for (k, (c, r)) in cells.iter().enumerate() {
        widths[*c] = widths[*c].max(sizes[k].x);
        heights[*r] = heights[*r].max(sizes[k].y);
    }
    let starts = |sizes: &[f32], gap: f32| -> Vec<f32> {
        sizes
            .iter()
            .scan(0.0, |at, w| {
                let start = *at;
                *at += w + gap;
                Some(start)
            })
            .collect()
    };
    let (xs, ys) = (starts(&widths, GAP_X), starts(&heights, GAP_Y));
    let mut out = Vec::new();
    for (k, name) in order.iter().enumerate() {
        let (c, r) = cells[k];
        let rect = if parallel && m.is_group(name) {
            // A region fills its cell across the group (so the lines between them run the
            // whole way); its states stay at its top left.
            let size = if stack {
                Vec2::new(widths[c], sizes[k].y)
            } else {
                Vec2::new(sizes[k].x, heights[r])
            };
            Area::from_min_size(Vec2::new(xs[c], ys[r]), size)
        } else {
            // Centered in its cell; regions side by side share a top.
            let down = if parallel {
                0.0
            } else {
                (heights[r] - sizes[k].y) / 2.0
            };
            let min = Vec2::new(xs[c] + (widths[c] - sizes[k].x) / 2.0, ys[r] + down);
            Area::from_min_size(min, sizes[k])
        };
        out.push((name.clone(), rect));
        if let Some(placed) = inner.remove(name) {
            let offset = rect.min + Vec2::new(GROUP_PAD, GROUP_PAD + GROUP_HEAD);
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
pub fn ring(n: usize) -> Vec<(usize, usize)> {
    let cols = match n {
        0..=2 => n.max(1),
        3 | 4 => 2,
        5..=9 => 3,
        _ => 4,
    };
    let rows = n.div_ceil(cols);
    let mut cells: Vec<(usize, usize)> = (0..cols).map(|c| (c, 0)).collect();
    if rows > 1 {
        cells.extend((1..rows - 1).map(|r| (cols - 1, r)));
        cells.extend((0..cols).rev().map(|c| (c, rows - 1)));
        cells.extend((1..rows - 1).rev().map(|r| (0, r)));
    }
    for r in 1..rows.saturating_sub(1) {
        for c in 1..cols.saturating_sub(1) {
            cells.push((c, r));
        }
    }
    cells.truncate(n);
    cells
}

/// Where a transition runs: a quadratic curve, or for a loop a cubic.
#[derive(Debug, Clone, Copy)]
pub enum Path {
    Quad([Vec2; 3]),
    Cubic([Vec2; 4]),
}

impl Path {
    pub fn point(&self, t: f32) -> Vec2 {
        let u = 1.0 - t;
        match *self {
            Path::Quad([a, b, c]) => a * u * u + b * 2.0 * u * t + c * t * t,
            Path::Cubic([a, b, c, d]) => {
                a * u * u * u + b * 3.0 * u * u * t + c * 3.0 * u * t * t + d * t * t * t
            }
        }
    }

    pub fn points(&self, n: usize) -> Vec<Vec2> {
        (0..=n).map(|k| self.point(k as f32 / n as f32)).collect()
    }
}

/// A transition's curve and where its label goes.
#[derive(Debug, Clone, Copy)]
pub struct Route {
    pub path: Path,
    /// How far along the curve its label sits (0 to 1).
    pub label: f32,
}

/// Where the ray from `r`'s middle along `dir` leaves it.
fn exit_point(r: Area, dir: Vec2) -> Vec2 {
    let d = dir.try_normalize().unwrap_or(Vec2::X);
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
    r.center() + d * tx.min(ty)
}

/// The curves of the machine's transitions, in its order (diagram units). Transitions between
/// the same two states bend apart; when those states are one above the other, their labels
/// (side by side, they would overlap) also step along the curves.
pub fn paths(m: &Machine, placed: &Placed) -> Vec<Option<Route>> {
    let key = |t: &Transition| t.from.clone().unwrap_or_else(|| "*".into());
    let rect_of = |name: &str| -> Option<Area> {
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
    let mut out: Vec<Option<Route>> = vec![None; m.transitions.len()];
    for ((a, b), list) in pairs {
        let (Some(ra), Some(rb)) = (rect_of(&a), rect_of(&b)) else {
            continue;
        };
        if a == b {
            for (i, k) in list.iter().enumerate() {
                let lift = 46.0 + 26.0 * i as f32;
                let p0 = Vec2::new(ra.min.x + ra.width() * 0.32, ra.min.y);
                let p3 = Vec2::new(ra.min.x + ra.width() * 0.68, ra.min.y);
                out[*k] = Some(Route {
                    path: Path::Cubic([
                        p0,
                        p0 + Vec2::new(-26.0, -lift),
                        p3 + Vec2::new(26.0, -lift),
                        p3,
                    ]),
                    label: 0.5,
                });
            }
            continue;
        }
        let along = rb.center() - ra.center();
        let normal = Vec2::new(-along.y, along.x)
            .try_normalize()
            .unwrap_or(Vec2::Y);
        let n = list.len() as f32;
        let stacked = along.y.abs() > along.x.abs() && n > 1.0;
        let step = (0.6 / (n - 1.0).max(1.0)).min(0.3);
        for (i, k) in list.iter().enumerate() {
            let t = &m.transitions[*k];
            let rank = i as f32 - (n - 1.0) / 2.0;
            let off = rank * BEND;
            let forward = key(t) == a;
            let (from, to) = if forward { (ra, rb) } else { (rb, ra) };
            // How far from `a` its label sits, measured along its own direction.
            let from_a = if stacked { 0.5 + rank * step } else { 0.5 };
            let mid = from.center() + (to.center() - from.center()) / 2.0 + normal * off;
            let p0 = exit_point(from, mid - from.center());
            let p2 = exit_point(to, mid - to.center());
            // The control point that makes the curve pass through `mid`.
            let c = mid * 2.0 - (p0 + p2) / 2.0;
            out[*k] = Some(Route {
                path: Path::Quad([p0, c, p2]),
                label: if forward { from_a } else { 1.0 - from_a },
            });
        }
    }
    out
}

/// Whether `inner` is `outer` or inside it.
fn within(m: &Machine, inner: &str, outer: &str) -> bool {
    let mut at = Some(inner.to_string());
    while let Some(s) = at {
        if s == outer {
            return true;
        }
        at = m.state(&s).and_then(|s| s.parent.clone());
    }
    false
}

/// The transitions behind the changes still glowing (the last, or with parallel regions the
/// last few at once): index and how much (1 down to 0).
pub fn glowing(m: &Machine, live: &Live) -> Vec<(usize, f32)> {
    let last = live.log.last().map_or(f64::NEG_INFINITY, |c| c.t);
    live.log
        .iter()
        .rev()
        .take_while(|c| c.t == last)
        .filter_map(|c| glowing_change(m, live, c))
        .collect()
}

fn glowing_change(m: &Machine, live: &Live, last: &Change) -> Option<(usize, f32)> {
    let age = live.clock - last.t;
    if !(0.0..GLOW_SECS).contains(&age) || last.from.is_empty() {
        return None;
    }
    let fits = |t: &Transition| {
        let from_ok = t.from.as_deref().is_none_or(|f| within(m, &last.from, f));
        let to_ok = within(m, &last.to, &t.to);
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

/// How much `name` glows for having just been entered (1 down to 0; None: it wasn't).
pub fn just_entered(live: &Live, name: &str) -> Option<f32> {
    let last = live.log.last()?.t;
    let age = live.clock - last;
    let entered = live
        .log
        .iter()
        .rev()
        .take_while(|c| c.t == last)
        .any(|c| c.to == name);
    (entered && (0.0..GLOW_SECS).contains(&age)).then_some((1.0 - age / GLOW_SECS) as f32)
}

/// The first states: the machine's, and each group's that has one (the start markers).
pub fn firsts(m: &Machine) -> Vec<&str> {
    std::iter::once(m.initial.as_str())
        .chain(m.states.iter().filter_map(|s| s.initial.as_deref()))
        .collect()
}

/// How full an `after` timer is (0 to 1), while the state it leaves is active.
pub fn timer(t: &Transition, live: &Live) -> Option<f32> {
    let (Trigger::After { secs, .. }, Some(from)) = (&t.trigger, &t.from) else {
        return None;
    };
    let inside = live.secs_in(from)?;
    (*secs > 0.0).then(|| (inside / secs).clamp(0.0, 1.0) as f32)
}

/// The family a log entry's cause is in (`on`, `after`, `done`, `when`, or `other` for `go`,
/// `start` and `reload`), for its color.
pub fn why_kind(why: &str) -> &'static str {
    match why.split_whitespace().next().unwrap_or("") {
        "after" => "after",
        "done" => "done",
        "when" => "when",
        "go" | "start" | "reload" => "other",
        _ => "on",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::State;

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
        let done = |from: &str, to: &str| Transition {
            from: Some(from.into()),
            to: to.into(),
            trigger: Trigger::Done,
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
                done("opening", "open"),
                on("open", "closing", "interact"),
                done("closing", "closed"),
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
            assert!(g.contains_area(r(s)), "{s} outside its group");
        }
        assert!(!g.intersects(r("unpowered")));
        assert!(placed.any.is_none());
    }

    #[test]
    fn transitions_between_the_same_states_bend_apart() {
        let m = gate();
        let placed = layout(&m, &BTreeMap::new());
        let ps = paths(&m, &placed);
        let mid = |k: usize| ps[k].as_ref().unwrap().path.point(0.5);
        // powered -> unpowered and back: two curves, not one line.
        assert!((mid(4) - mid(5)).length() > BEND * 0.8);
        // Three between two states one above the other: their labels step down the curves.
        let mut m = gate();
        m.transitions.push(Transition {
            from: Some("open".into()),
            to: "opening".into(),
            trigger: Trigger::On {
                event: "interact".into(),
                guard: None,
            },
            line: 0,
        });
        m.transitions.push(Transition {
            from: Some("opening".into()),
            to: "open".into(),
            trigger: Trigger::When("ready".into()),
            line: 0,
        });
        let placed = layout(&m, &BTreeMap::new());
        let (a, b) = (placed.rects["opening"], placed.rects["open"]);
        assert!(b.min.y > a.max.y && (a.center().x - b.center().x).abs() < 1.0);
        let ps = paths(&m, &placed);
        let spot = |k: usize| {
            let r = ps[k].as_ref().unwrap();
            r.path.point(r.label)
        };
        let mut ys = [spot(1).y, spot(6).y, spot(7).y];
        ys.sort_by(f32::total_cmp);
        assert!(ys[1] - ys[0] > 20.0 && ys[2] - ys[1] > 20.0, "{ys:?}");
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

    #[test]
    fn a_parallel_groups_regions_sit_in_a_row() {
        let mut m = gate();
        // powered becomes two regions: the bars, and a lamp beside them.
        m.states[0].parallel = true;
        m.states[0].initial = None;
        for s in &mut m.states[1..5] {
            s.parent = Some("bars".into());
        }
        let bars = State {
            name: "bars".into(),
            parent: Some("powered".into()),
            initial: Some("closed".into()),
            ..Default::default()
        };
        let lamp = State {
            name: "lamp".into(),
            parent: Some("powered".into()),
            ..Default::default()
        };
        m.states.insert(1, bars);
        m.states.insert(2, lamp);
        let placed = layout(&m, &BTreeMap::new());
        let r = |n: &str| placed.rects[n];
        assert!((r("bars").min.y - r("lamp").min.y).abs() < 1.0);
        assert!(r("bars").max.x < r("lamp").min.x);
        assert!(r("powered").contains_area(r("bars")) && r("powered").contains_area(r("lamp")));
        assert!(r("bars").contains_area(r("open")));
    }

    /// The seal: three regions of two states each. In a wide view they sit side by side; in a
    /// tall one they stack, each region as wide as the group, and the machine draws larger.
    #[test]
    fn a_parallel_groups_regions_stack_for_a_tall_view() {
        let st = |name: &str, parent: Option<&str>, initial: Option<&str>| State {
            name: name.into(),
            parent: parent.map(str::to_string),
            initial: initial.map(str::to_string),
            ..Default::default()
        };
        let mut sealed = st("sealed", None, None);
        sealed.parallel = true;
        let mut m = Machine {
            initial: "sealed".into(),
            states: vec![sealed],
            ..Default::default()
        };
        for r in ["left", "middle", "right"] {
            let (dark, lit) = (format!("{r}_dark"), format!("{r}_lit"));
            m.states.push(st(r, Some("sealed"), Some(&dark)));
            m.states.push(st(&dark, Some(r), None));
            m.states.push(st(&lit, Some(r), None));
            m.transitions.push(Transition {
                from: Some(dark),
                to: lit,
                trigger: Trigger::On {
                    event: "touch".into(),
                    guard: None,
                },
                line: 0,
            });
        }
        m.states.push(st("open", None, None));
        let wide = layout_for(&m, &BTreeMap::new(), 4.0);
        assert!(!stacked(&m, &wide, "sealed"));
        let tall = layout_for(&m, &BTreeMap::new(), 1.0);
        assert!(stacked(&m, &tall, "sealed"));
        let r = |n: &str| tall.rects[n];
        assert!(r("left").max.y <= r("middle").min.y && r("middle").max.y <= r("right").min.y);
        assert_eq!(r("left").width(), r("right").width());
        assert!(r("sealed").contains_area(r("right")) && r("right").contains_area(r("right_lit")));
        let fit = |p: &Placed| (1.0 / p.bounds.width()).min(1.0 / p.bounds.height());
        assert!(fit(&tall) > fit(&wide));
    }
}
