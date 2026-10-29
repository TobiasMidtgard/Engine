//! State boards: a script's state machine ([`crate::states`]) drawn onto a mesh in the world
//! the way the editor draws it, live: the states it is in lit with their time, the change just
//! made glowing along its arrow, `after` timers filling and the last changes listed with what
//! caused each, as Unreal's StateTree debugger and PlayMaker's graph view show a running
//! machine. The showcase's Script courtyard stands one beside the portcullis it watches.
//!
//! Drawn on the CPU like code boards ([`crate::board`]), a few times a second at most, from the
//! same layout as the editor's diagrams ([`crate::diagram`]).

use std::collections::{BTreeSet, HashMap};

use bevy_ecs::prelude::*;
use engine_assets::{AssetServer, Assets, Handle};
use engine_core::glam::Vec2;
use engine_core::prelude::*;
use engine_render::{Image, MeshRenderer};

use crate::Script;
use crate::board::{BoardFont, Canvas, Rgb, show};
use crate::diagram::{self, Area, GROUP_HEAD, Placed};
use crate::graph_board::{BG, DIM, ERROR, HEADER_BG, LIVE, Label, TEXT, TITLE, finish, mix, srgb};
use crate::states::{Live, Machine};

/// Draws the state machine of the script on the entity named `target` on this entity's
/// [`MeshRenderer`] (a plane or box face whose UVs span the board), live while it runs.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateBoard {
    /// The name of the entity whose script's states it shows.
    pub target: String,
    /// Texture size, pixels (width, height); its aspect should match the board's.
    pub size: [u32; 2],
    /// Glow, cd/m² for the brightest parts.
    pub glow_cd_m2: f32,
    /// Redraws a second at most while the machine runs.
    pub fps: f32,
    /// Changes listed under the diagram, newest first.
    pub log_rows: u32,
}

impl Default for StateBoard {
    fn default() -> Self {
        Self {
            target: String::new(),
            size: [1600, 1000],
            glow_cd_m2: 60.0,
            fps: 4.0,
            log_rows: 4,
        }
    }
}

impl StateBoard {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            ..Default::default()
        }
    }
}

// ----------------------------------------------------------------------------- colors

/// The editor's diagram colors (games/editor/src/states.rs), linear.
struct Palette {
    state: Rgb,
    group: Rgb,
    group_head: Rgb,
    edge: Rgb,
    group_edge: Rgb,
    chip: Rgb,
    play: Rgb,
    any: Rgb,
}

fn palette() -> Palette {
    Palette {
        state: srgb([0x2b, 0x2d, 0x32]),
        group: srgb([0x1f, 0x22, 0x26]),
        group_head: srgb([0x27, 0x2a, 0x30]),
        edge: srgb([0x46, 0x4a, 0x52]),
        group_edge: srgb([0x3a, 0x3e, 0x46]),
        chip: srgb([0x17, 0x18, 0x1b]),
        play: srgb([0x4c, 0xc2, 0x6a]),
        any: srgb([0x22, 0x24, 0x29]),
    }
}

/// What a transition's trigger family is drawn in (the editor's `kind_color`).
fn kind_color(kind: &str) -> Rgb {
    match kind {
        "on" => srgb([120, 168, 255]),
        "after" => srgb([242, 182, 72]),
        "done" => srgb([98, 206, 128]),
        "when" => srgb([86, 206, 206]),
        _ => DIM,
    }
}

fn dimmed(c: Rgb, k: f32) -> Rgb {
    mix(BG, c, k)
}

// ----------------------------------------------------------------------------- shapes

impl Canvas {
    /// A filled triangle with soft edges.
    pub(crate) fn triangle(&mut self, p: [Vec2; 3], c: Rgb, glow: f32) {
        let lo = p[0].min(p[1]).min(p[2]) - Vec2::ONE;
        let hi = p[0].max(p[1]).max(p[2]) + Vec2::ONE;
        // Clockwise or not, the inside is where every edge's signed distance agrees.
        let area = (p[1] - p[0]).perp_dot(p[2] - p[0]);
        let sign = if area < 0.0 { -1.0 } else { 1.0 };
        for y in (lo.y.floor() as i32)..=(hi.y.ceil() as i32) {
            for x in (lo.x.floor() as i32)..=(hi.x.ceil() as i32) {
                let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let mut d = f32::MAX;
                for k in 0..3 {
                    let (a, b) = (p[k], p[(k + 1) % 3]);
                    let e = (b - a).normalize_or_zero();
                    d = d.min(sign * e.perp_dot(q - a));
                }
                self.blend(x, y, c, (d + 0.5).min(1.0), glow);
            }
        }
    }

    /// A line through `points`, `width` thick.
    fn polyline(&mut self, points: &[Vec2], width: f32, c: Rgb, glow: f32) {
        for pair in points.windows(2) {
            self.segment(pair[0].into(), pair[1].into(), width, c, glow);
        }
    }

    /// An arrow head with its tip at `tip`, pointing along `dir`.
    fn head(&mut self, tip: Vec2, dir: Vec2, size: f32, c: Rgb, glow: f32) {
        let d = dir.try_normalize().unwrap_or(Vec2::X);
        let n = d.perp();
        let back = tip - d * size;
        self.triangle([tip, back + n * size * 0.5, back - n * size * 0.5], c, glow);
    }
}

fn rect4(a: Area) -> [f32; 4] {
    [a.min.x, a.min.y, a.max.x, a.max.y]
}

/// A check mark `r` across, centered on `c`.
fn check(cv: &mut Canvas, c: Vec2, r: f32, color: Rgb) {
    let w = (r * 0.32).max(1.2);
    let a = c + Vec2::new(-r, 0.0);
    let b = c + Vec2::new(-r * 0.3, r * 0.7);
    let e = c + Vec2::new(r, -r * 0.8);
    cv.segment(a.into(), b.into(), w, color, 0.8);
    cv.segment(b.into(), e.into(), w, color, 0.8);
}

// ----------------------------------------------------------------------------- drawing

/// What a board shows besides the machine.
pub(crate) struct View<'a> {
    /// The target's name and its script file.
    pub title: &'a str,
    pub path: &'a str,
    /// Why there's no machine to draw (no such entity, no states table, a mistake).
    pub error: Option<&'a str>,
    pub live: Option<&'a Live>,
}

/// Draws a board: the header, the machine fitted in the middle and its last changes below.
pub(crate) fn draw(
    board: &StateBoard,
    font: &mut BoardFont,
    machine: Option<&Machine>,
    view: &View,
) -> (Image, Image) {
    let (w, h) = (
        board.size[0].max(64) as usize,
        board.size[1].max(64) as usize,
    );
    let (wf, hf) = (w as f32, h as f32);
    let mut cv = Canvas::new(w, h);
    for p in cv.px.iter_mut() {
        *p = BG;
    }
    let pal = palette();
    let mut labels: Vec<Label> = Vec::new();
    let (head, area, log_h, line) = areas(board);
    let head_px = head * 0.48;
    cv.fill([0.0, 0.0, wf, head], 0.0, HEADER_BG, 0.08);
    font.set_size(head_px);
    let adv = font.advance();
    let pad = head * 0.4;
    labels.push(Label {
        px: head_px,
        x: pad,
        baseline: head * 0.66,
        text: view.title.to_string(),
        color: TITLE,
    });
    let file = view.path.rsplit('/').next().unwrap_or(view.path);
    let mut x = pad + adv * (view.title.chars().count() as f32 + 1.5);
    labels.push(Label {
        px: head_px,
        x,
        baseline: head * 0.66,
        text: file.to_string(),
        color: DIM,
    });
    x += adv * (file.chars().count() as f32 + 2.0);
    // Where it is now: "in open 2.4 s" (with regions, each).
    if let Some(l) = view.live {
        let now: Vec<String> = l
            .current
            .iter()
            .map(|c| match l.secs_in(c) {
                Some(s) => format!("{c} {s:.1} s"),
                None => c.clone(),
            })
            .collect();
        if !now.is_empty() {
            // A machine that reached a final state at the outer level has ended there.
            let word = if l.over { "ended in" } else { "in" };
            let gap = word.len() as f32 + 1.0;
            labels.push(Label {
                px: head_px,
                x,
                baseline: head * 0.66,
                text: word.into(),
                color: DIM,
            });
            let text = Canvas::fitted(
                &now.join(" + "),
                ((wf - x - adv * (gap + 6.0) - pad) / adv).max(4.0) as usize,
            );
            labels.push(Label {
                px: head_px,
                x: x + adv * gap,
                baseline: head * 0.66,
                text,
                color: LIVE,
            });
        }
        let lx = wf - pad - adv * 4.0;
        cv.disc([lx - head * 0.3, head * 0.5], head * 0.13, LIVE, 1.0);
        labels.push(Label {
            px: head_px,
            x: lx,
            baseline: head * 0.66,
            text: "LIVE".into(),
            color: LIVE,
        });
    }

    let margin = head * 0.5;
    let rows = board.log_rows as usize;
    if let Some(e) = view.error {
        font.set_size(head_px);
        let per_line = ((area.width()) / font.advance()).max(8.0) as usize;
        for (k, chunk) in wrap(e, per_line).iter().take(6).enumerate() {
            labels.push(Label {
                px: head_px,
                x: area.min.x,
                baseline: area.min.y + head_px * (1.5 + 1.4 * k as f32),
                text: chunk.clone(),
                color: ERROR,
            });
        }
    }
    if let Some(m) = machine {
        let placed = diagram::layout_for(
            m,
            &Default::default(),
            area.width() / area.height().max(1.0),
        );
        machine_drawing(
            &mut cv,
            &mut labels,
            font,
            &pal,
            m,
            &placed,
            view.live,
            area,
        );
    }
    if rows > 0 {
        log_drawing(
            &mut cv,
            &mut labels,
            font,
            view.live,
            Area::from_min_max(
                Vec2::new(margin, hf - log_h),
                Vec2::new(wf - margin, hf - margin),
            ),
            line,
            rows,
        );
    }
    finish(&mut cv, font, labels);
    cv.into_images()
}

/// Splits text into lines of at most `n` characters, at spaces where it can.
fn wrap(s: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split(' ') {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > n {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// The scale and offset that fit `bounds` (diagram units) in `area` (pixels), centered.
fn fit(bounds: Area, area: Area) -> (f32, Vec2) {
    let s = (area.width() / bounds.width().max(1.0))
        .min(area.height() / bounds.height().max(1.0))
        .min(3.0);
    (s, area.center() - bounds.center() * s)
}

/// Where a board's header, diagram and log go: the header's height, the diagram's area, the
/// log's height and its line height.
fn areas(board: &StateBoard) -> (f32, Area, f32, f32) {
    let (wf, hf) = (board.size[0].max(64) as f32, board.size[1].max(64) as f32);
    let head = (hf * 0.085).max(18.0);
    let margin = head * 0.5;
    let line = (hf * 0.042).max(12.0);
    let rows = board.log_rows as f32;
    let log_h = if rows > 0.0 {
        line * (rows + 1.0) + margin
    } else {
        0.0
    };
    let area = Area::from_min_max(
        Vec2::new(margin, head + margin),
        Vec2::new(wf - margin, hf - margin - log_h),
    );
    (head, area, log_h, line)
}

#[allow(clippy::too_many_arguments)]
fn machine_drawing(
    cv: &mut Canvas,
    labels: &mut Vec<Label>,
    font: &mut BoardFont,
    pal: &Palette,
    m: &Machine,
    placed: &Placed,
    live: Option<&Live>,
    area: Area,
) {
    let (s, off) = fit(placed.bounds, area);
    let at = |p: Vec2| off + p * s;
    let rect = |r: Area| Area::from_min_max(at(r.min), at(r.max));
    let active: BTreeSet<&str> = live
        .map(|l| l.active.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let glow = live.map(|l| diagram::glowing(m, l)).unwrap_or_default();
    // Text sizes, and how many characters fit across a box.
    let mut size_of = |px: f32| {
        font.set_size(px);
        font.advance()
    };
    let name_px = 15.0 * s;
    let small_px = 11.5 * s;
    let name_adv = size_of(name_px);
    let small_adv = size_of(small_px);

    // Groups, outermost first.
    let mut groups: Vec<&crate::states::State> =
        m.states.iter().filter(|st| m.is_group(&st.name)).collect();
    groups.sort_by_key(|g| diagram::depth(m, &g.name));
    for g in groups {
        let Some(r) = placed.rects.get(&g.name).map(|r| rect(*r)) else {
            continue;
        };
        let on = active.contains(g.name.as_str());
        cv.fill(rect4(r), 10.0 * s, pal.group, 0.1);
        let edge = if on {
            mix(pal.group_edge, pal.play, 0.8)
        } else {
            pal.group_edge
        };
        cv.outline(
            rect4(r),
            10.0 * s,
            if on { 1.6 * s.max(1.0) } else { 1.2 },
            edge,
            0.5,
        );
        let head = Area::from_min_size(r.min, Vec2::new(r.width(), GROUP_HEAD * s));
        let head_fill = if on {
            mix(pal.group, pal.play, 0.16)
        } else {
            pal.group_head
        };
        // The header strip: rounded on top only (a square strip under its lower half).
        cv.fill(
            [
                head.min.x + 1.0,
                head.min.y + 1.0,
                head.max.x - 1.0,
                head.max.y,
            ],
            9.0 * s,
            head_fill,
            0.2,
        );
        cv.fill(
            [
                head.min.x + 1.0,
                head.center().y,
                head.max.x - 1.0,
                head.max.y,
            ],
            0.0,
            head_fill,
            0.2,
        );
        let title_px = 13.5 * s;
        labels.push(Label {
            px: title_px,
            x: head.min.x + 10.0 * s,
            baseline: head.center().y + title_px * 0.35,
            text: g.name.clone(),
            color: if on { pal.play } else { TEXT },
        });
        if g.parallel {
            // Its regions run at once: dashed lines between them (UML's orthogonal regions).
            labels.push(Label {
                px: small_px,
                x: head.min.x + 10.0 * s + title_px * 0.6 * (g.name.chars().count() as f32 + 1.0),
                baseline: head.center().y + small_px * 0.35,
                text: "all at once".into(),
                color: DIM,
            });
            let mut regions: Vec<Area> = m
                .children(Some(&g.name))
                .filter_map(|c| placed.rects.get(&c.name).map(|r| rect(*r)))
                .collect();
            // Side by side, or one under another (`diagram::layout_for` picks for the board).
            let stacked = diagram::stacked(m, placed, &g.name);
            if stacked {
                regions.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
            } else {
                regions.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
            }
            for pair in regions.windows(2) {
                // A dashed line between them, the whole way across the group.
                let (a, b) = if stacked {
                    let y = (pair[0].max.y + pair[1].min.y) / 2.0;
                    (
                        Vec2::new(r.min.x + 6.0 * s, y),
                        Vec2::new(r.max.x - 6.0 * s, y),
                    )
                } else {
                    let x = (pair[0].max.x + pair[1].min.x) / 2.0;
                    (
                        Vec2::new(x, head.max.y + 6.0 * s),
                        Vec2::new(x, r.max.y - 6.0 * s),
                    )
                };
                let len = (b - a).length();
                let dir = (b - a) / len.max(1e-3);
                let mut at = 0.0;
                while at < len {
                    let to = (at + 7.0 * s).min(len);
                    cv.segment(
                        (a + dir * at).into(),
                        (a + dir * to).into(),
                        1.4 * s.max(1.0),
                        mix(pal.group_edge, TEXT, 0.3),
                        0.3,
                    );
                    at = to + 5.0 * s;
                }
            }
        }
        // Its right end: the history mark (H, H* for deep), then whether it is done.
        let mut right = head.max.x - 8.0 * s;
        if g.history {
            let c = Vec2::new(right - 8.5 * s, head.center().y);
            cv.outline(
                rect4(Area::from_min_max(
                    c - Vec2::splat(8.5 * s),
                    c + Vec2::splat(8.5 * s),
                )),
                8.5 * s,
                1.2 * s.max(1.0),
                DIM,
                0.4,
            );
            let mark = if g.deep { "H*" } else { "H" };
            labels.push(Label {
                px: 10.5 * s,
                x: c.x - 10.5 * s * 0.3 * mark.len() as f32,
                baseline: c.y + 10.5 * s * 0.35,
                text: mark.into(),
                color: DIM,
            });
            right -= 24.0 * s;
        }
        if live.is_some_and(|l| l.done.contains(&g.name)) {
            // Reached a final state (every region has): a check and "done".
            let px = small_px;
            let w = px * 0.6 * 4.0;
            labels.push(Label {
                px,
                x: right - w,
                baseline: head.center().y + px * 0.35,
                text: "done".into(),
                color: pal.play,
            });
            check(
                cv,
                Vec2::new(right - w - 10.0 * s, head.center().y),
                6.0 * s,
                pal.play,
            );
        }
    }

    // The outer table's transitions start from "any state".
    if let Some(a) = placed.any.map(rect) {
        cv.fill(rect4(a), a.height() / 2.0, pal.any, 0.1);
        cv.outline(rect4(a), a.height() / 2.0, 1.2, pal.group_edge, 0.4);
        labels.push(Label {
            px: 12.0 * s,
            x: a.center().x - 12.0 * s * 0.3 * 9.0,
            baseline: a.center().y + 12.0 * s * 0.35,
            text: "any state".into(),
            color: DIM,
        });
    }

    // Start markers: a dot and an arrow into each first state (UML's initial state).
    for first in diagram::firsts(m) {
        let Some(r) = placed.rects.get(first).map(|r| rect(*r)) else {
            continue;
        };
        let to = Vec2::new(r.min.x - 2.0, r.center().y);
        let dot = to - Vec2::new(26.0 * s, 0.0);
        cv.disc(dot.into(), 5.0 * s, TEXT, 0.6);
        cv.segment(dot.into(), to.into(), 1.4 * s.max(1.0), TEXT, 0.6);
        cv.head(to, Vec2::X, 7.0 * s, TEXT, 0.6);
    }

    // Transitions under the states, their labels over them.
    let paths = diagram::paths(m, placed);
    let under = |name: Option<&str>| {
        if name
            .and_then(|n| m.state(n))
            .is_some_and(|st| st.parent.is_some())
        {
            pal.group
        } else {
            BG
        }
    };
    let mut chips: Vec<(usize, Vec2)> = Vec::new();
    for (k, t) in m.transitions.iter().enumerate() {
        let Some(route) = &paths[k] else { continue };
        let path = &route.path;
        let color = kind_color(t.kind());
        let pts: Vec<Vec2> = path.points(24).into_iter().map(at).collect();
        let from_on = t.from.as_deref().is_none_or(|f| active.contains(f));
        let lit = glow.iter().find(|(g, _)| *g == k).map(|(_, a)| *a);
        let base = if from_on || active.is_empty() {
            dimmed(color, 0.85)
        } else {
            dimmed(color, 0.38)
        };
        let width = 1.6 * s.max(1.0) * 0.8;
        if let Some(a) = lit {
            let halo = mix(under(t.from.as_deref()), color, 0.4 * a);
            cv.polyline(&pts, width + 7.0 * s * a, halo, 0.8 * a);
        }
        cv.polyline(&pts, width, base, if lit.is_some() { 1.0 } else { 0.45 });
        let n = pts.len();
        cv.head(pts[n - 1], pts[n - 1] - pts[n - 3], 9.0 * s, base, 0.5);
        if let Some(a) = lit {
            // A spark runs along it as it fires.
            let along = (1.0 - a) * 2.4;
            if along < 1.0 {
                cv.disc(at(path.point(along)).into(), 4.5 * s, [1.0, 1.0, 1.0], 1.2);
            }
        }
        chips.push((k, at(path.point(route.label))));
    }

    // States.
    for st in m.states.iter().filter(|st| !m.is_group(&st.name)) {
        let Some(r) = placed.rects.get(&st.name).map(|r| rect(*r)) else {
            continue;
        };
        let on = active.contains(st.name.as_str());
        let current = live.is_some_and(|l| l.is_current(&st.name));
        if let Some(a) = live.and_then(|l| diagram::just_entered(l, &st.name)) {
            let grow = Vec2::splat(6.0 * a * s);
            let halo = mix(under(Some(&st.name)), pal.play, 0.3 * a);
            cv.fill(
                rect4(Area::from_min_max(r.min - grow, r.max + grow)),
                12.0 * s,
                halo,
                0.6 * a,
            );
        }
        let fill = if current {
            mix(pal.state, pal.play, 0.2)
        } else {
            pal.state
        };
        cv.fill(rect4(r), 8.0 * s, fill, if current { 0.5 } else { 0.18 });
        let (edge, width) = if current {
            (pal.play, 2.6 * s.max(1.0) * 0.8)
        } else {
            (pal.edge, 1.2)
        };
        cv.outline(
            rect4(r),
            8.0 * s,
            width,
            edge,
            if current { 1.0 } else { 0.4 },
        );
        if st.is_final {
            // A final state: a second edge inside the first (where its group ends).
            let inset = Vec2::splat(4.0 * s);
            cv.outline(
                rect4(Area::from_min_max(r.min + inset, r.max - inset)),
                5.0 * s,
                width * 0.8,
                edge,
                if current { 1.0 } else { 0.4 },
            );
        }
        let inner = r.width() - 24.0 * s;
        let secs = on
            .then(|| live.and_then(|l| l.secs_in(&st.name)))
            .flatten()
            .map(|t| format!("{t:.1} s"));
        let secs_w = secs
            .as_ref()
            .map_or(0.0, |t| (t.chars().count() as f32 + 1.0) * small_adv);
        labels.push(Label {
            px: name_px,
            x: r.min.x + 12.0 * s,
            baseline: r.min.y + 10.0 * s + name_px * 0.8,
            text: Canvas::fitted(&st.name, ((inner - secs_w) / name_adv).max(3.0) as usize),
            color: if current { [1.0, 1.0, 1.0] } else { TEXT },
        });
        // What it does: its hooks, or that its enter task is still going.
        let running = live.is_some_and(|l| l.running.contains(&st.name));
        let (what, color) = if running {
            ("enter running".to_string(), kind_color("after"))
        } else if st.is_final {
            let mut what = vec!["final".to_string()];
            what.extend(st.hooks.iter().cloned());
            (what.join(" · "), if current { pal.play } else { DIM })
        } else if st.hooks.is_empty() {
            ("-".to_string(), dimmed(DIM, 0.7))
        } else {
            (st.hooks.join(" · "), DIM)
        };
        labels.push(Label {
            px: small_px,
            x: r.min.x + 12.0 * s,
            baseline: r.max.y - 10.0 * s,
            text: Canvas::fitted(&what, (inner / small_adv).max(3.0) as usize),
            color,
        });
        if let Some(t) = secs {
            labels.push(Label {
                px: small_px,
                x: r.max.x - 10.0 * s - t.chars().count() as f32 * small_adv,
                baseline: r.min.y + 10.0 * s + small_px * 0.8,
                text: t,
                color: pal.play,
            });
        }
    }

    // Transition labels, with `after` timers filling while their state is active.
    for (k, mid) in chips {
        let t = &m.transitions[k];
        let color = kind_color(t.kind());
        let text = t.label();
        let size = Vec2::new(
            text.chars().count() as f32 * small_adv + 12.0 * s,
            small_px * 1.25 + 6.0 * s,
        );
        let chip = Area::from_min_max(mid - size / 2.0, mid + size / 2.0);
        cv.fill(rect4(chip), 4.0 * s, pal.chip, 0.1);
        cv.outline(rect4(chip), 4.0 * s, 1.2, dimmed(color, 0.55), 0.4);
        let timer = live.and_then(|l| diagram::timer(t, l));
        if let Some(f) = timer {
            let y = chip.max.y - 3.0 * s;
            cv.fill(
                [
                    chip.min.x + 2.0 * s,
                    y,
                    chip.min.x + 2.0 * s + (chip.width() - 4.0 * s) * f,
                    y + 2.0 * s,
                ],
                1.0 * s,
                color,
                1.0,
            );
        }
        let lit = timer.is_some() || glow.iter().any(|(g, _)| *g == k);
        labels.push(Label {
            px: small_px,
            x: chip.min.x + 6.0 * s,
            baseline: chip.center().y + small_px * 0.35,
            text,
            color: if lit { color } else { dimmed(color, 0.9) },
        });
    }
}

/// The last changes, newest first: time, from, an arrow, to, and what caused it.
fn log_drawing(
    cv: &mut Canvas,
    labels: &mut Vec<Label>,
    font: &mut BoardFont,
    live: Option<&Live>,
    area: Area,
    line: f32,
    rows: usize,
) {
    let px = line * 0.62;
    font.set_size(px);
    let adv = font.advance();
    cv.segment(
        [area.min.x, area.min.y],
        [area.max.x, area.min.y],
        1.0,
        mix(BG, DIM, 0.35),
        0.1,
    );
    labels.push(Label {
        px,
        x: area.min.x,
        baseline: area.min.y + line * 0.8,
        text: "CHANGES".into(),
        color: DIM,
    });
    let Some(l) = live else {
        labels.push(Label {
            px,
            x: area.min.x + adv * 9.0,
            baseline: area.min.y + line * 0.8,
            text: "not running".into(),
            color: DIM,
        });
        return;
    };
    let room = (area.width() / adv) as usize;
    for (row, c) in l.log.iter().rev().take(rows).enumerate() {
        let y = area.min.y + line * (row as f32 + 1.8);
        // The newest is brightest.
        let fade = 1.0 - row as f32 * 0.16;
        let mut x = area.min.x;
        let mut put = |text: String, color: Rgb, x: &mut f32| {
            let n = text.chars().count();
            labels.push(Label {
                px,
                x: *x,
                baseline: y,
                text,
                color: dimmed(color, fade),
            });
            *x += (n as f32 + 1.0) * adv;
        };
        put(format!("{:>7.2} s", c.t), DIM, &mut x);
        let from = if c.from.is_empty() { "start" } else { &c.from };
        put(
            from.to_string(),
            if c.from.is_empty() { DIM } else { TEXT },
            &mut x,
        );
        let mid = y - px * 0.32;
        cv.segment(
            [x, mid],
            [x + adv * 1.4, mid],
            px * 0.09,
            dimmed(DIM, fade),
            0.4,
        );
        cv.head(
            Vec2::new(x + adv * 1.9, mid),
            Vec2::X,
            px * 0.42,
            dimmed(DIM, fade),
            0.4,
        );
        x += adv * 3.0;
        put(c.to.clone(), LIVE, &mut x);
        let used = ((x - area.min.x) / adv) as usize;
        put(
            Canvas::fitted(&c.why, room.saturating_sub(used).max(4)),
            kind_color(diagram::why_kind(&c.why)),
            &mut x,
        );
    }
    if l.log.is_empty() {
        labels.push(Label {
            px,
            x: area.min.x,
            baseline: area.min.y + line * 1.8,
            text: "No changes yet.".into(),
            color: DIM,
        });
    }
}

// ----------------------------------------------------------------------------- the system

/// One board's state.
pub(crate) struct Drawn {
    image: Option<(Handle<Image>, Handle<Image>)>,
    drawn_at: f64,
    /// What it showed last: the board, the machine (or why none) and the live state.
    shown: Option<(StateBoard, Result<Machine, String>, Option<Live>)>,
}

#[derive(Default)]
pub(crate) struct StateBoards {
    drawn: HashMap<Entity, Drawn>,
    font: Option<BoardFont>,
}

/// Reads each board's target, its script's machine and where it is now, and redraws the board
/// when that changed, a few times a second at most (`Update`, after the scripts ran).
pub(crate) fn draw_state_boards(world: &mut World, mut state: Local<StateBoards>) {
    let mut q = world.query::<(Entity, &StateBoard)>();
    let boards: Vec<(Entity, StateBoard)> = q.iter(world).map(|(e, b)| (e, b.clone())).collect();
    let StateBoards { drawn, font } = &mut *state;
    drawn.retain(|e, _| boards.iter().any(|(b, _)| b == e));
    if boards.is_empty() || !world.contains_resource::<Assets<Image>>() {
        return;
    }
    let now = world
        .get_resource::<Time>()
        .map_or(0.0, |t| t.real_elapsed_secs_f64());
    let mut names = world.query::<(Entity, &Name, &Script)>();
    for (e, board) in boards {
        let d = drawn.entry(e).or_insert_with(|| Drawn {
            image: None,
            drawn_at: f64::MIN,
            shown: None,
        });
        if now - d.drawn_at < 1.0 / board.fps.max(0.1) as f64 {
            continue;
        }
        let target = names
            .iter(world)
            .find(|(_, n, _)| n.as_str() == board.target)
            .map(|(t, _, s)| (t, s.path.clone()));
        let (machine, live, path) = match target {
            None => (
                Err(format!("Nothing named \"{}\" runs a script.", board.target)),
                None,
                String::new(),
            ),
            Some((t, path)) => {
                let machine = match crate::props::declared(world, &path) {
                    Err(e) => Err(e),
                    Ok(d) => match d.machine {
                        None => Err(format!("{path} has no states table.")),
                        Some(Err(p)) => Err(format!("{path} line {}: {}", p.line, p.message)),
                        Some(Ok(m)) => Ok(m),
                    },
                };
                (
                    machine,
                    crate::states::live(world, t),
                    crate::normalize(&path),
                )
            }
        };
        // While it runs, the time in each state counts up (and glows fade, timers fill): it
        // redraws at the board's rate; otherwise only when what it shows changed.
        let moving = machine.is_ok() && live.is_some();
        let shown = (board.clone(), machine, live);
        if !moving && d.shown.as_ref() == Some(&shown) {
            continue;
        }
        d.drawn_at = now;
        let font = font.get_or_insert_with(|| {
            BoardFont::load(world.get_resource::<AssetServer>().map(|s| s.root()))
        });
        let (board, machine, live) = &shown;
        let pictures = draw(
            board,
            font,
            machine.as_ref().ok(),
            &View {
                title: &board.target,
                path: &path,
                error: machine.as_ref().err().map(String::as_str),
                live: live.as_ref(),
            },
        );
        let old = d.image;
        world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
            if let Some(mut renderer) = world.get_mut::<MeshRenderer>(e) {
                d.image = Some(show(
                    &mut images,
                    old,
                    pictures,
                    &mut renderer,
                    board.glow_cd_m2,
                ));
            }
        });
        d.shown = Some(shown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{Change, State, Transition, Trigger};
    use std::collections::BTreeMap;

    /// The portcullis's machine, a second into `open` with its timer running.
    fn gate() -> (Machine, Live) {
        let st = |name: &str, parent: Option<&str>, hooks: &[&str]| State {
            name: name.into(),
            parent: parent.map(str::to_string),
            hooks: hooks.iter().map(|h| h.to_string()).collect(),
            ..Default::default()
        };
        let tr = |from: &str, to: &str, trigger: Trigger| Transition {
            from: Some(from.into()),
            to: to.into(),
            trigger,
            line: 0,
        };
        let on = |e: &str| Trigger::On {
            event: e.into(),
            guard: None,
        };
        let mut powered = st("powered", None, &["update"]);
        powered.initial = Some("closed".into());
        powered.history = true;
        let m = Machine {
            initial: "powered".into(),
            states: vec![
                powered,
                st("closed", Some("powered"), &["enter"]),
                st("opening", Some("powered"), &["enter"]),
                st("open", Some("powered"), &["enter"]),
                st("closing", Some("powered"), &["enter"]),
                st("unpowered", None, &["enter"]),
            ],
            transitions: vec![
                tr("closed", "opening", on("interact")),
                tr("opening", "open", Trigger::Done),
                tr(
                    "open",
                    "closing",
                    Trigger::After {
                        secs: 4.0,
                        prop: Some("hold".into()),
                    },
                ),
                tr("closing", "closed", Trigger::Done),
                tr("closing", "opening", Trigger::When("blocked".into())),
                tr("powered", "unpowered", on("power")),
                tr("unpowered", "powered", on("power")),
            ],
            line: 0,
        };
        let change = |t: f64, from: &str, to: &str, why: &str, active: &[&str]| Change {
            t,
            from: from.into(),
            to: to.into(),
            why: why.into(),
            active: active.iter().map(|a| a.to_string()).collect(),
            values: Vec::new(),
        };
        let live = Live {
            active: vec!["powered".into(), "open".into()],
            current: vec!["open".into()],
            since: BTreeMap::from([("powered".into(), 0.0), ("open".into(), 9.5)]),
            running: BTreeSet::new(),
            clock: 10.5,
            log: vec![
                change(0.0, "", "closed", "start", &["powered", "closed"]),
                change(
                    7.5,
                    "closed",
                    "opening",
                    "interact by Player",
                    &["powered", "opening"],
                ),
                change(9.5, "opening", "open", "done", &["powered", "open"]),
            ],
            history: BTreeMap::new(),
            ..Default::default()
        };
        (m, live)
    }

    #[test]
    fn a_state_board_lights_the_current_state_and_lists_changes() {
        let (m, live) = gate();
        let mut font = BoardFont::load(None);
        let board = StateBoard {
            size: [800, 500],
            ..StateBoard::new("Gate")
        };
        let view = View {
            title: "Gate",
            path: "examples/gate.luau",
            error: None,
            live: Some(&live),
        };
        let (img, glow) = draw(&board, &mut font, Some(&m), &view);
        assert_eq!((img.width, img.height), (800, 500));
        let count =
            |i: &Image, f: &dyn Fn(&[u8]) -> bool| i.data.chunks(4).filter(|p| f(p)).count();
        // The current state is filled green, the others grey; the gold `after` chip's text.
        let (_, area, _, _) = areas(&board);
        let placed = diagram::layout(&m, &BTreeMap::new());
        let (s, off) = fit(placed.bounds, area);
        let px = |i: &Image, state: &str| {
            let r = placed.rects[state];
            let p = off + Vec2::new(r.max.x - 14.0, r.center().y) * s;
            let k = (p.y as usize * i.width as usize + p.x as usize) * 4;
            [i.data[k], i.data[k + 1], i.data[k + 2]]
        };
        let greenish =
            |c: [u8; 3]| c[1] as i32 > c[0] as i32 + 30 && c[1] as i32 > c[2] as i32 + 20;
        assert!(greenish(px(&img, "open")), "{:?}", px(&img, "open"));
        assert!(!greenish(px(&img, "closed")), "{:?}", px(&img, "closed"));
        assert!(count(&img, &|p| p[0] > 200 && p[1] > 140 && p[2] < 110) > 20);
        // Not running: nothing lit.
        let still = View { live: None, ..view };
        let (img2, _) = draw(&board, &mut font, Some(&m), &still);
        assert!(!greenish(px(&img2, "open")), "{:?}", px(&img2, "open"));
        // The glow is mostly dark.
        let lit = |i: &Image| {
            i.data
                .chunks(4)
                .map(|p| p[0] as u64 + p[1] as u64)
                .sum::<u64>()
        };
        assert!(lit(&glow) < lit(&img));
    }

    #[test]
    fn a_state_board_says_what_is_wrong() {
        let mut font = BoardFont::load(None);
        let board = StateBoard {
            size: [640, 400],
            ..StateBoard::new("Gate")
        };
        let view = View {
            title: "Gate",
            path: "examples/gate.luau",
            error: Some("examples/gate.luau line 24: no state closd"),
            live: None,
        };
        let (img, _) = draw(&board, &mut font, None, &view);
        let red = img
            .data
            .chunks(4)
            .filter(|p| {
                p[0] > 200 && p[0] as u32 > p[1] as u32 + 50 && p[0] as u32 > p[2] as u32 + 50
            })
            .count();
        assert!(red > 30, "{red}");
        assert_eq!(wrap("a bb ccc dddd", 6), vec!["a bb", "ccc", "dddd"]);
    }

    /// The gate's board at full size, as a picture to check a layout before a render:
    /// `cargo test -p engine_script -- --ignored --nocapture state_board_picture`.
    #[test]
    #[ignore]
    fn state_board_picture() {
        let (m, live) = gate();
        let mut font = BoardFont::load(Some(std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        ))));
        let view = View {
            title: "Gate",
            path: "examples/gate.luau",
            error: None,
            live: Some(&live),
        };
        let (img, _) = draw(&StateBoard::new("Gate"), &mut font, Some(&m), &view);
        let path = std::env::temp_dir().join("state_board.ppm");
        let mut ppm = format!("P6\n{} {}\n255\n", img.width, img.height).into_bytes();
        ppm.extend(img.data.chunks(4).flat_map(|p| [p[0], p[1], p[2]]));
        std::fs::write(&path, ppm).unwrap();
        println!("{}", path.display());
    }
}
