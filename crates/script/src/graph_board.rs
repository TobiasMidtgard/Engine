//! Graph boards: a visual script ([`crate::graph`]) drawn onto a mesh in the world the way the
//! editor's Graphs tab draws it: nodes with colored headers, white execution wires, data wires
//! colored by type, comment boxes, and the problems the compiler found marked on their nodes.
//! While the graph runs, the board watches it (logpoints on its lines, [`graph::watch`]): the
//! nodes that just ran light up with the wires into them, and every data wire shows its newest
//! value, as Blueprints' debugger does in Unreal's editor. The showcase's Script courtyard
//! stands one beside the crystal its graph drives.
//!
//! Drawn on the CPU like code boards ([`crate::board`]), a few times a second at most.

use std::collections::{BTreeMap, HashMap};

use bevy_ecs::prelude::*;
use engine_assets::{AssetServer, Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, MeshRenderer};

use crate::ScriptRuntime;
use crate::board::{BoardFont, Canvas, Rgb, scaled, show};
use crate::graph::{self, Compiled, Graph, Live, PinType, look};
use crate::runtime::normalize;

/// Draws a visual script (a `.graph` file) on this entity's [`MeshRenderer`] (a plane or box
/// face whose UVs span the board), live while it runs.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphBoard {
    /// The graph file, relative to the scripts folder.
    pub path: String,
    /// Texture size, pixels (width, height); its aspect should match the board's.
    pub size: [u32; 2],
    /// Glow, cd/m² for the brightest parts.
    pub glow_cd_m2: f32,
    /// Watches the graph while it runs: lights the nodes that run and shows wire values.
    pub live: bool,
    /// Redraws a second at most while live.
    pub fps: f32,
}

impl Default for GraphBoard {
    fn default() -> Self {
        Self {
            path: String::new(),
            size: [2048, 1280],
            glow_cd_m2: 60.0,
            live: true,
            fps: 3.0,
        }
    }
}

impl GraphBoard {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }
}

// ----------------------------------------------------------------------------- colors

const BG: Rgb = [0.045, 0.05, 0.062];
const GRID: Rgb = [0.065, 0.072, 0.088];
const GRID_MAJOR: Rgb = [0.085, 0.094, 0.115];
const HEADER_BG: Rgb = [0.09, 0.1, 0.125];
const NODE_BG: Rgb = [0.085, 0.09, 0.105];
const FIELD_BG: Rgb = [0.035, 0.038, 0.046];
const TEXT: Rgb = [0.86, 0.88, 0.92];
const DIM: Rgb = [0.52, 0.56, 0.63];
const TITLE: Rgb = [0.98, 0.80, 0.42];
const LIVE: Rgb = [0.45, 0.88, 0.5];
const ERROR: Rgb = [1.0, 0.36, 0.33];
const WARN: Rgb = [1.0, 0.7, 0.25];
const RAN: Rgb = [1.0, 0.82, 0.35];
const VALUE: Rgb = [1.0, 0.84, 0.48];
const VALUE_BG: Rgb = [0.15, 0.11, 0.035];

/// Seconds a node stays lit after it ran.
const LIT_SECS: f64 = 0.9;

fn srgb(c: [u8; 3]) -> Rgb {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2])]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

// ----------------------------------------------------------------------------- shapes

impl Canvas {
    /// Blends `c` into a pixel with coverage `a` (its glow at `glow` of its color).
    fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32, glow: f32) {
        if a <= 0.0 || x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        let a = a.min(1.0);
        let g = scaled(c, glow);
        for k in 0..3 {
            self.px[i][k] += (c[k] - self.px[i][k]) * a;
            self.glow[i][k] += (g[k] - self.glow[i][k]) * a;
        }
    }

    /// A rounded rectangle with soft edges, colored per pixel by `color(x, y)`.
    fn rounded(
        &mut self,
        [x0, y0, x1, y1]: [f32; 4],
        r: f32,
        color: &dyn Fn(f32, f32) -> Rgb,
        glow: f32,
    ) {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hx, hy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
        let r = r.min(hx).min(hy).max(0.0);
        for y in (y0.floor() as i32)..=(y1.ceil() as i32) {
            for x in (x0.floor() as i32)..=(x1.ceil() as i32) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let qx = (px - cx).abs() - (hx - r);
                let qy = (py - cy).abs() - (hy - r);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = outside + qx.max(qy).min(0.0) - r;
                self.blend(x, y, color(px, py), 0.5 - d, glow);
            }
        }
    }

    fn fill(&mut self, rect: [f32; 4], r: f32, c: Rgb, glow: f32) {
        self.rounded(rect, r, &|_, _| c, glow);
    }

    /// The outline of a rounded rectangle, `width` thick.
    fn outline(&mut self, [x0, y0, x1, y1]: [f32; 4], r: f32, width: f32, c: Rgb, glow: f32) {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hx, hy) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
        let r = r.min(hx).min(hy).max(0.0);
        let pad = width + 1.0;
        for y in ((y0 - pad).floor() as i32)..=((y1 + pad).ceil() as i32) {
            for x in ((x0 - pad).floor() as i32)..=((x1 + pad).ceil() as i32) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let qx = (px - cx).abs() - (hx - r);
                let qy = (py - cy).abs() - (hy - r);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = (outside + qx.max(qy).min(0.0) - r).abs() - width / 2.0;
                self.blend(x, y, c, 0.5 - d, glow);
            }
        }
    }

    fn disc(&mut self, [cx, cy]: [f32; 2], r: f32, c: Rgb, glow: f32) {
        for y in ((cy - r - 1.0).floor() as i32)..=((cy + r + 1.0).ceil() as i32) {
            for x in ((cx - r - 1.0).floor() as i32)..=((cx + r + 1.0).ceil() as i32) {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() - r;
                self.blend(x, y, c, 0.5 - d, glow);
            }
        }
    }

    /// A line `width` thick with round ends.
    fn segment(&mut self, a: [f32; 2], b: [f32; 2], width: f32, c: Rgb, glow: f32) {
        let h = width / 2.0;
        let (minx, maxx) = (a[0].min(b[0]) - h - 1.0, a[0].max(b[0]) + h + 1.0);
        let (miny, maxy) = (a[1].min(b[1]) - h - 1.0, a[1].max(b[1]) + h + 1.0);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        for y in (miny.floor() as i32)..=(maxy.ceil() as i32) {
            for x in (minx.floor() as i32)..=(maxx.ceil() as i32) {
                let (px, py) = (x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]);
                let t = ((px * dx + py * dy) / len2).clamp(0.0, 1.0);
                let d = ((px - dx * t).powi(2) + (py - dy * t).powi(2)).sqrt() - h;
                self.blend(x, y, c, 0.5 - d, glow);
            }
        }
    }

    /// A triangle pointing right (an execution pin), filled or hollow.
    fn arrow(&mut self, [cx, cy]: [f32; 2], r: f32, c: Rgb, filled: bool, inside: Rgb) {
        // Coverage of the triangle (-r, -r), (-r, r), (r, 0) around the center.
        let tri = |px: f32, py: f32, r: f32| {
            let (x, y) = (px - cx, (py - cy).abs());
            let edge = (y + x / 2.0 - r / 2.0) / 1.118;
            0.5 - (-r - x).max(edge)
        };
        for y in ((cy - r - 1.0).floor() as i32)..=((cy + r + 1.0).ceil() as i32) {
            for x in ((cx - r - 1.0).floor() as i32)..=((cx + r + 1.0).ceil() as i32) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                self.blend(x, y, c, tri(px, py, r), 0.6);
                if !filled {
                    self.blend(x, y, inside, tri(px, py, r * 0.55), 0.1);
                }
            }
        }
    }
}

/// Points along a wire from `a` to `b`: a curve leaving and entering sideways, as in
/// Blueprints.
fn wire_points(a: [f32; 2], b: [f32; 2], s: f32) -> Vec<[f32; 2]> {
    let d = ((b[0] - a[0]).abs() * 0.5).max(50.0 * s);
    let (c1, c2) = ([a[0] + d, a[1]], [b[0] - d, b[1]]);
    let n = 28;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            [
                w[0] * a[0] + w[1] * c1[0] + w[2] * c2[0] + w[3] * b[0],
                w[0] * a[1] + w[1] * c1[1] + w[2] * c2[1] + w[3] * b[1],
            ]
        })
        .collect()
}

/// Text drawn after the shapes, grouped by size (the font redraws its glyphs at each size).
struct Label {
    px: f32,
    x: f32,
    baseline: f32,
    text: String,
    color: Rgb,
}

// ----------------------------------------------------------------------------- drawing

/// What a board shows besides the graph.
pub(crate) struct View<'a> {
    pub path: &'a str,
    pub error: Option<&'a str>,
    /// The graph runs (and is watched).
    pub running: bool,
    pub live: &'a Live,
    /// Seconds since each node last ran.
    pub ran: &'a HashMap<u32, f64>,
}

fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            let x = n.as_f64().unwrap_or(0.0);
            if x.fract() == 0.0 {
                format!("{}", x as i64)
            } else {
                format!("{x:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            }
        }
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(value_text).collect::<Vec<_>>().join(", "),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn draw(
    board: &GraphBoard,
    font: &mut BoardFont,
    graph: Option<&Graph>,
    compiled: Option<&Compiled>,
    view: &View,
) -> (Image, Image) {
    let (w, h) = (
        board.size[0].max(64) as usize,
        board.size[1].max(64) as usize,
    );
    let mut cv = Canvas::new(w, h);
    for p in cv.px.iter_mut() {
        *p = BG;
    }
    let mut labels: Vec<Label> = Vec::new();
    let head = (h as f32 * 0.07).max(18.0);
    let head_px = head * 0.5;
    cv.fill([0.0, 0.0, w as f32, head], 0.0, HEADER_BG, 0.08);
    let file = view.path.rsplit('/').next().unwrap_or(view.path);
    font.set_size(head_px);
    let adv = font.advance();
    labels.push(Label {
        px: head_px,
        x: head * 0.4,
        baseline: head * 0.68,
        text: file.to_string(),
        color: TITLE,
    });
    let (status, color) = match (view.error, compiled) {
        (Some(e), _) => (e.to_string(), ERROR),
        (None, Some(c)) => {
            let errors = c.errors().count();
            let warnings = c.problems.len() - errors;
            if errors > 0 {
                (
                    format!(
                        "{errors} mistake{} stop it loading",
                        if errors == 1 { "" } else { "s" }
                    ),
                    ERROR,
                )
            } else {
                let lines = c.lines.len();
                let mut s = format!("visual script, runs as {lines} lines of Luau");
                if warnings > 0 {
                    s.push_str(&format!(
                        " · {warnings} warning{}",
                        if warnings == 1 { "" } else { "s" }
                    ));
                }
                (s, DIM)
            }
        }
        (None, None) => ("reading...".to_string(), DIM),
    };
    let status_x = head * 0.4 + adv * (file.chars().count() as f32 + 2.0);
    let live_w = if view.running { adv * 6.0 } else { 0.0 };
    let room = ((w as f32 - status_x - live_w - head) / adv).max(4.0) as usize;
    labels.push(Label {
        px: head_px,
        x: status_x,
        baseline: head * 0.68,
        text: Canvas::fitted(&status, room),
        color,
    });
    if view.running {
        let x = w as f32 - head * 0.4 - adv * 4.0;
        cv.disc([x - head * 0.3, head * 0.5], head * 0.13, LIVE, 1.0);
        labels.push(Label {
            px: head_px,
            x,
            baseline: head * 0.68,
            text: "LIVE".into(),
            color: LIVE,
        });
    }
    let Some(graph) = graph else {
        finish(&mut cv, font, labels);
        return cv.into_images();
    };

    // Shapes, and the box around everything.
    let mut lib = graph::builtins();
    lib.extend(graph::own_specs(graph));
    let wired = |id: u32, pin: &str| graph.links.iter().any(|l| l.to.0 == id && l.to.1 == pin);
    let shapes: BTreeMap<u32, (graph::NodeSpec, look::Shape)> = graph
        .nodes
        .iter()
        .map(|n| {
            let spec = graph::spec_for(&lib, n, graph);
            let shape = look::shape(&spec, n, &|p| wired(n.id, p));
            (n.id, (spec, shape))
        })
        .collect();
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    let mut grow = |a: [f32; 2], b: [f32; 2]| {
        for k in 0..2 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(b[k]);
        }
    };
    for n in &graph.nodes {
        let size = shapes[&n.id].1.size;
        let at = [n.at[0] as f32, n.at[1] as f32];
        grow(at, [at[0] + size[0], at[1] + size[1]]);
    }
    for c in &graph.comments {
        let at = [c.at[0] as f32, c.at[1] as f32];
        grow(at, [at[0] + c.size[0] as f32, at[1] + c.size[1] as f32]);
    }
    if lo[0] > hi[0] {
        lo = [0.0, 0.0];
        hi = [100.0, 100.0];
    }
    let margin = head * 0.5;
    let area = [margin, head + margin, w as f32 - margin, h as f32 - margin];
    let s = ((area[2] - area[0]) / (hi[0] - lo[0]))
        .min((area[3] - area[1]) / (hi[1] - lo[1]))
        .min(2.5);
    let off = [
        area[0] + ((area[2] - area[0]) - (hi[0] - lo[0]) * s) / 2.0 - lo[0] * s,
        area[1] + ((area[3] - area[1]) - (hi[1] - lo[1]) * s) / 2.0 - lo[1] * s,
    ];
    let to = |p: [f32; 2]| [off[0] + p[0] * s, off[1] + p[1] * s];

    // The grid, as the editor's canvas has it.
    let step = 32.0 * s;
    if step >= 6.0 {
        let first = [
            ((area[0] - off[0]) / step).floor() as i32,
            ((area[1] - off[1]) / step).floor() as i32,
        ];
        let lines_x = ((w as f32 - area[0]) / step) as i32 + 2;
        let lines_y = ((h as f32 - area[1]) / step) as i32 + 2;
        for i in first[0]..first[0] + lines_x {
            let x = (off[0] + i as f32 * step).round() as i32;
            let c = if i % 8 == 0 { GRID_MAJOR } else { GRID };
            cv.rect(x, head as i32, x + 1, h as i32, c, 0.05);
        }
        for i in first[1]..first[1] + lines_y {
            let y = (off[1] + i as f32 * step).round() as i32;
            if y < head as i32 {
                continue;
            }
            let c = if i % 8 == 0 { GRID_MAJOR } else { GRID };
            cv.rect(0, y, w as i32, y + 1, c, 0.05);
        }
    }

    let text_px = (12.5 * s).max(6.0);
    font.set_size(text_px);
    let adv = font.advance();

    // Comments, behind everything.
    for c in &graph.comments {
        let a = to([c.at[0] as f32, c.at[1] as f32]);
        let b = to([(c.at[0] + c.size[0]) as f32, (c.at[1] + c.size[1]) as f32]);
        let color = srgb(c.color);
        cv.fill([a[0], a[1], b[0], b[1]], 8.0 * s, mix(BG, color, 0.22), 0.1);
        cv.fill(
            [a[0], a[1], b[0], a[1] + 30.0 * s],
            8.0 * s,
            mix(BG, color, 0.55),
            0.2,
        );
        labels.push(Label {
            px: text_px * 1.35,
            x: a[0] + 10.0 * s,
            baseline: a[1] + 21.0 * s,
            text: c.text.clone(),
            color: TEXT,
        });
    }

    let node_at = |id: u32| graph.node(id).map(|n| [n.at[0] as f32, n.at[1] as f32]);
    let socket = |id: u32, pin: &str, output: bool| -> Option<[f32; 2]> {
        let at = node_at(id)?;
        let p = shapes.get(&id)?.1.socket(pin, output)?;
        Some(to([at[0] + p[0], at[1] + p[1]]))
    };
    let lit = |id: u32| {
        view.ran
            .get(&id)
            .map(|t| (1.0 - t / LIT_SECS).max(0.0) as f32)
    };

    // Wires: data first, execution on top.
    let mut bubbles = Vec::new();
    let mut shown_outputs = std::collections::BTreeSet::new();
    for exec_pass in [false, true] {
        for (i, l) in graph.links.iter().enumerate() {
            let (Some((from_spec, _)), Some((to_spec, _))) =
                (shapes.get(&l.from.0), shapes.get(&l.to.0))
            else {
                continue;
            };
            let out_ty = from_spec
                .output_pin(&l.from.1)
                .map(|p| p.ty)
                .unwrap_or(PinType::Any);
            let in_ty = to_spec
                .input_pin(&l.to.1)
                .map(|p| p.ty)
                .unwrap_or(PinType::Any);
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
            let mut c = srgb(look::pin_color(ty));
            let mut glow = 0.35;
            let mut width = if exec { 3.4 * s } else { 2.4 * s };
            if exec && let Some(k) = lit(l.to.0).filter(|k| *k > 0.0) {
                c = mix(c, RAN, k);
                glow = 0.35 + 0.65 * k;
                width *= 1.0 + 0.5 * k;
            } else if exec && view.running {
                c = scaled(c, 0.55);
            }
            let pts = wire_points(a, b, s);
            for seg in pts.windows(2) {
                cv.segment(seg[0], seg[1], width.max(1.2), c, glow);
            }
            // One value per output, however many wires it feeds.
            if !exec
                && let Some(v) = view.live.values.get(&i)
                && shown_outputs.insert(l.from.clone())
            {
                bubbles.push((pts[pts.len() / 2], v.clone()));
            }
        }
    }

    // Nodes.
    let problems = |id: u32| {
        compiled
            .map(|c| {
                c.problems
                    .iter()
                    .filter(|p| p.node == Some(id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for n in &graph.nodes {
        let (spec, shape) = &shapes[&n.id];
        let a = to([n.at[0] as f32, n.at[1] as f32]);
        let b = [a[0] + shape.size[0] * s, a[1] + shape.size[1] * s];
        let r = 6.0 * s;
        let header = srgb(look::category_color(&spec.category));
        let k = lit(n.id).unwrap_or(0.0);
        cv.fill([a[0], a[1], b[0], b[1]], r, NODE_BG, 0.06);
        let (hx0, hx1) = (a[0], b[0]);
        let head_h = look::HEADER * s;
        cv.rounded(
            [a[0], a[1], b[0], a[1] + head_h + r],
            r,
            &|x, _| {
                let t = ((x - hx0) / (hx1 - hx0).max(1.0)).clamp(0.0, 1.0);
                mix(scaled(header, 1.0 + 0.6 * k), scaled(header, 0.45), t)
            },
            0.25 + 0.5 * k,
        );
        cv.fill(
            [a[0], a[1] + head_h, b[0], a[1] + head_h + r + 1.0],
            0.0,
            NODE_BG,
            0.06,
        );
        labels.push(Label {
            px: text_px,
            x: a[0] + look::PAD * s,
            baseline: a[1] + head_h * 0.68,
            text: Canvas::fitted(
                &spec.title,
                ((shape.size[0] - 2.0 * look::PAD) * s / adv) as usize,
            ),
            color: TEXT,
        });
        let ps = problems(n.id);
        if ps.iter().any(|p| p.error) {
            cv.outline([a[0], a[1], b[0], b[1]], r, 3.0 * s, ERROR, 0.9);
        } else if !ps.is_empty() {
            cv.outline([a[0], a[1], b[0], b[1]], r, 2.0 * s, WARN, 0.6);
        } else if k > 0.0 {
            cv.outline([a[0], a[1], b[0], b[1]], r, 2.5 * s, RAN, 0.4 + 0.6 * k);
        }
        // Pins.
        for row in &shape.inputs {
            let y = a[1] + row.y * s;
            let c = srgb(look::pin_color(row.pin.ty));
            let mut x = a[0] + 6.0 * s;
            if row.socket {
                let at = [a[0] + 8.0 * s, y];
                let on = wired(n.id, &row.pin.name);
                if row.pin.ty == PinType::Exec {
                    cv.arrow(at, 5.0 * s, c, on, NODE_BG);
                } else {
                    cv.disc(at, 4.5 * s, c, 0.5);
                    if !on {
                        cv.disc(at, 2.5 * s, NODE_BG, 0.05);
                    }
                }
                x = a[0] + look::SOCKET * s;
            }
            if !row.label.is_empty() {
                labels.push(Label {
                    px: text_px,
                    x,
                    baseline: y + text_px * 0.35,
                    text: row.label.clone(),
                    color: DIM,
                });
                x += (row.label.chars().count() as f32 * look::CHAR + 6.0) * s;
            }
            if row.field > 0.0 {
                let fw = row.field * s;
                let rect = [x, y - 8.0 * s, x + fw, y + 8.0 * s];
                cv.fill(rect, 3.0 * s, FIELD_BG, 0.03);
                let set = n.values.get(&row.pin.name);
                let unset = set.is_none() && look::patches(&n.kind) && row.pin.name != "entity";
                let v = set.cloned().unwrap_or_else(|| row.pin.default.clone());
                let text = match (row.pin.ty, &v) {
                    _ if unset => "-".to_string(),
                    (PinType::Entity, serde_json::Value::Null) => "self".to_string(),
                    _ => value_text(&v),
                };
                labels.push(Label {
                    px: text_px,
                    x: x + 4.0 * s,
                    baseline: y + text_px * 0.35,
                    text: Canvas::fitted(&text, ((fw - 8.0 * s) / adv).max(1.0) as usize),
                    color: if unset { DIM } else { TEXT },
                });
            }
        }
        for row in &shape.outputs {
            let y = a[1] + row.y * s;
            let c = srgb(look::pin_color(row.pin.ty));
            let at = [b[0] - 8.0 * s, y];
            let on = graph
                .links
                .iter()
                .any(|l| l.from.0 == n.id && l.from.1 == row.pin.name);
            if row.pin.ty == PinType::Exec {
                cv.arrow(at, 5.0 * s, c, on, NODE_BG);
            } else {
                cv.disc(at, 4.5 * s, c, 0.5);
                if !on {
                    cv.disc(at, 2.5 * s, NODE_BG, 0.05);
                }
            }
            if !row.label.is_empty() {
                let tw = row.label.chars().count() as f32 * adv;
                labels.push(Label {
                    px: text_px,
                    x: b[0] - look::SOCKET * s - tw,
                    baseline: y + text_px * 0.35,
                    text: row.label.clone(),
                    color: DIM,
                });
            }
        }
        // The first problem, under the node.
        if let Some(p) = ps.iter().find(|p| p.error).or(ps.first()) {
            let c = if p.error { ERROR } else { WARN };
            cv.disc([b[0] - 2.0 * s, a[1] + 2.0 * s], 9.0 * s, c, 0.9);
            labels.push(Label {
                px: text_px,
                x: b[0] - 2.0 * s - adv * 0.5,
                baseline: a[1] + 2.0 * s + text_px * 0.35,
                text: "!".into(),
                color: [0.05, 0.02, 0.02],
            });
            let room = ((shape.size[0] * 1.6 * s) / adv) as usize;
            labels.push(Label {
                px: text_px,
                x: a[0],
                baseline: b[1] + 16.0 * s,
                text: Canvas::fitted(&p.message, room.max(8)),
                color: c,
            });
        }
    }

    // Wire values, over everything.
    for (at, v) in bubbles {
        let text = Canvas::fitted(&v, 16);
        let tw = text.chars().count() as f32 * adv;
        let rect = [
            at[0] - tw / 2.0 - 5.0 * s,
            at[1] - 9.0 * s,
            at[0] + tw / 2.0 + 5.0 * s,
            at[1] + 9.0 * s,
        ];
        cv.fill(rect, 4.0 * s, VALUE_BG, 0.25);
        cv.outline(rect, 4.0 * s, 1.0, scaled(VALUE, 0.5), 0.4);
        labels.push(Label {
            px: text_px,
            x: rect[0] + 5.0 * s,
            baseline: at[1] + text_px * 0.35,
            text,
            color: VALUE,
        });
    }
    finish(&mut cv, font, labels);
    cv.into_images()
}

/// Draws the labels, a font size at a time.
fn finish(cv: &mut Canvas, font: &mut BoardFont, mut labels: Vec<Label>) {
    labels.sort_by(|a, b| a.px.total_cmp(&b.px));
    for l in labels {
        font.set_size(l.px);
        let c = l.color;
        cv.text(font, &l.text, l.x, l.baseline, &|_| c);
    }
}

// ----------------------------------------------------------------------------- the system

/// One board's state.
pub(crate) struct Drawn {
    text: String,
    graph: Option<Graph>,
    compiled: Option<Compiled>,
    error: Option<String>,
    image: Option<(Handle<Image>, Handle<Image>)>,
    checked_at: f64,
    drawn_at: f64,
    dirty: bool,
    /// The loaded version the logpoints were put on.
    watched: Option<u32>,
    live: Live,
    /// When each node last ran (real seconds).
    ran: HashMap<u32, f64>,
    running: bool,
    board: GraphBoard,
}

#[derive(Default)]
pub(crate) struct GraphBoards {
    drawn: HashMap<Entity, Drawn>,
    font: Option<BoardFont>,
}

/// Rereads graph boards' files a few times a second, watches their graphs while they run and
/// redraws them when something shows a change (`Update`, after the scripts ran).
pub(crate) fn draw_graph_boards(
    mut boards: Query<(Entity, &GraphBoard, &mut MeshRenderer)>,
    runtime: Option<Res<ScriptRuntime>>,
    server: Option<Res<AssetServer>>,
    settings: Option<Res<crate::ScriptSettings>>,
    time: Res<Time>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut state: Local<GraphBoards>,
) {
    let Some(images) = images.as_deref_mut() else {
        return;
    };
    let GraphBoards { drawn, font } = &mut *state;
    drawn.retain(|e, _| boards.contains(*e));
    let now = time.real_elapsed_secs_f64();
    for (e, board, mut renderer) in boards.iter_mut() {
        let path = normalize(&board.path);
        let d = drawn.entry(e).or_insert_with(|| Drawn {
            text: String::new(),
            graph: None,
            compiled: None,
            error: None,
            image: None,
            checked_at: f64::MIN,
            drawn_at: f64::MIN,
            dirty: true,
            watched: None,
            live: Live::default(),
            ran: HashMap::new(),
            running: false,
            board: board.clone(),
        });
        if d.board != *board {
            d.board = board.clone();
            d.dirty = true;
        }
        if now - d.checked_at >= 0.25 {
            d.checked_at = now;
            let root = settings
                .as_ref()
                .map(|s| s.folder.clone())
                .unwrap_or_else(|| "scripts".into());
            let full = server
                .as_ref()
                .map(|s| s.root())
                .unwrap_or_else(|| "assets".into())
                .join(root)
                .join(&path);
            let text =
                std::fs::read_to_string(&full).map_err(|e| format!("can't read {path}: {e}"));
            let changed = match &text {
                Ok(t) => *t != d.text || d.graph.is_none() && d.error.is_none(),
                Err(e) => d.error.as_ref() != Some(e),
            };
            if changed {
                d.dirty = true;
                d.watched = None;
                match text {
                    Ok(t) => {
                        match Graph::parse(&t) {
                            Ok(g) => {
                                d.compiled = Some(graph::compile(&g, &path));
                                d.graph = Some(g);
                                d.error = None;
                            }
                            Err(err) => {
                                d.error = Some(err);
                                d.compiled = None;
                            }
                        }
                        d.text = t;
                    }
                    Err(err) => {
                        d.error = Some(err);
                        d.graph = None;
                        d.compiled = None;
                    }
                }
            }
        }
        // Watch it while it runs: the logpoints go on the version the runtime loaded.
        let mut running = false;
        if board.live
            && let Some(rt) = runtime.as_deref()
            && let Some(c) = &d.compiled
            && let Some(version) = graph::loaded_in(rt, &path, c)
        {
            running = true;
            let lost = rt.debugger.line_hits(&path).is_empty() && !c.node_lines.is_empty();
            if d.watched != Some(version) || lost {
                graph::watch_in(rt, &path, c);
                d.watched = Some(version);
                d.live = Live::default();
            }
            if now - d.drawn_at >= 1.0 / board.fps.max(0.1) as f64 {
                let (_, values) = rt.debugger.inline_values(&path);
                let live = graph::live(c, &rt.debugger.line_hits(&path), &values);
                for (node, runs) in &live.runs {
                    if d.live.runs.get(node).is_some_and(|old| runs > old)
                        || !d.live.runs.contains_key(node) && *runs > 0
                    {
                        d.ran.insert(*node, now);
                    }
                }
                let fading = d.ran.values().any(|t| now - t < LIT_SECS + 0.4);
                if live != d.live || fading {
                    d.dirty = true;
                }
                d.live = live;
            }
        }
        if running != d.running {
            d.running = running;
            d.dirty = true;
        }
        if !d.dirty {
            continue;
        }
        d.dirty = false;
        d.drawn_at = now;
        let font = font.get_or_insert_with(|| BoardFont::load(server.as_ref().map(|s| s.root())));
        let ran: HashMap<u32, f64> = d.ran.iter().map(|(k, t)| (*k, now - t)).collect();
        let pictures = draw(
            board,
            font,
            d.graph.as_ref(),
            d.compiled.as_ref(),
            &View {
                path: &path,
                error: d.error.as_deref(),
                running: d.running,
                live: &d.live,
                ran: &ran,
            },
        );
        d.image = Some(show(
            images,
            d.image,
            pictures,
            &mut renderer,
            board.glow_cd_m2,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_graph_board_draws_nodes_wires_and_values() {
        let text = include_str!("../../../assets/scripts/showcase/crystal.graph");
        let g = Graph::parse(text).unwrap();
        let c = graph::compile(&g, "showcase/crystal.graph");
        assert!(c.errors().next().is_none(), "{:?}", c.problems);
        let mut font = BoardFont::load(None);
        let board = GraphBoard {
            size: [768, 480],
            ..Default::default()
        };
        let link = g
            .links
            .iter()
            .position(|l| l.to == (8, "position".to_string()))
            .unwrap();
        let live = Live {
            runs: BTreeMap::from([(5, 10)]),
            values: BTreeMap::from([(link, "vec3(1, 2, 3)".to_string())]),
        };
        let view = View {
            path: "showcase/crystal.graph",
            error: None,
            running: true,
            live: &live,
            ran: &HashMap::from([(5, 0.1)]),
        };
        let (img, glow) = draw(&board, &mut font, Some(&g), Some(&c), &view);
        assert_eq!((img.width, img.height), (768, 480));
        let count =
            |i: &Image, f: &dyn Fn(&[u8]) -> bool| i.data.chunks(4).filter(|p| f(p)).count();
        // Red event headers and the gold value bubble are there.
        assert!(count(&img, &|p| p[0] > 110 && p[1] < 60 && p[2] < 60) > 200);
        assert!(count(&img, &|p| p[0] > 200 && p[1] > 150 && p[2] < 110) > 20);
        // The glow is mostly dark.
        let lit = |i: &Image| {
            i.data
                .chunks(4)
                .map(|p| p[0] as u64 + p[1] as u64)
                .sum::<u64>()
        };
        assert!(lit(&glow) < lit(&img));
    }
}
