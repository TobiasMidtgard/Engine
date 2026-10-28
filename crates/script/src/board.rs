//! Code boards: a script's source drawn onto a mesh in the world, syntax-colored, redrawn the
//! moment the file reloads, with the lines that just changed lit up, logpoint values beside
//! their lines, the line an error came from marked, and each line's share of the script time
//! when the profiler has recorded. The showcase's scripted courtyard stands one beside what the
//! script drives, so one picture shows code and effect. [`ProfileBoard`] draws the profiler's
//! flame graph and hottest lines the same way.
//!
//! The text is rasterized on the CPU into an sRGB image (with mips) set as the material's
//! base color and emissive texture, so it reads by day and glows at night. The font is
//! JetBrains Mono when `fetched/fonts/JetBrainsMono-Regular.ttf` is there (assets/manifests/
//! script.toml), else the built-in 8×8 font.

use std::collections::{BTreeMap, HashMap, HashSet};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use bevy_ecs::prelude::*;
use engine_assets::{AssetServer, Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, MeshRenderer, StandardMaterial};

use crate::ScriptRuntime;
use crate::profiler::{FlameNode, Kind, Summary};
use crate::runtime::{Fault, error_location, normalize};

/// Draws a script's source on this entity's [`MeshRenderer`] (a plane or box face whose UVs
/// span the board). Changes to the file show within a moment.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeBoard {
    /// Script file, relative to the scripts folder.
    pub path: String,
    /// First line shown (1-based).
    pub first_line: u32,
    /// Lines shown at most; the text size follows from it.
    pub lines: u32,
    /// Texture size, pixels (width, height); its aspect should match the board's.
    pub size: [u32; 2],
    /// Glow, cd/m² for the brightest text (a phone screen is about 500).
    pub glow_cd_m2: f32,
    /// Seconds the lines a reload changed stay lit.
    pub changed_secs: f32,
}

impl Default for CodeBoard {
    fn default() -> Self {
        Self {
            path: String::new(),
            first_line: 1,
            lines: 34,
            size: [2048, 1280],
            glow_cd_m2: 60.0,
            changed_secs: 20.0,
        }
    }
}

impl CodeBoard {
    /// A board showing `path` from its first line.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }
}

/// One board's drawn state.
pub(crate) struct Drawn {
    text: String,
    version: u32,
    image: Handle<Image>,
    glow: Handle<Image>,
    changed: HashSet<usize>,
    changed_at: f64,
    error: Option<String>,
    faded: bool,
    /// The logpoint values drawn (`Debugger::inline_values`).
    inline_version: u64,
    /// The profile drawn (`profiler::line_heat`).
    heat_version: u64,
    fault: Option<Fault>,
    /// When it was drawn (real seconds).
    drawn_at: f64,
    /// The board's settings when drawn (scrolling it redraws it).
    board: CodeBoard,
}

/// Glyph coverage, cached per character.
struct Glyph {
    w: usize,
    h: usize,
    left: i32,
    top: i32,
    cov: Vec<u8>,
}

/// The font and its glyph cache.
pub(crate) struct BoardFont {
    font: Option<FontVec>,
    px: f32,
    glyphs: HashMap<char, Option<Glyph>>,
}

impl BoardFont {
    fn load(asset_root: Option<std::path::PathBuf>) -> Self {
        let font = asset_root
            .map(|r| r.join("fetched/fonts/JetBrainsMono-Regular.ttf"))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| FontVec::try_from_vec(bytes).ok());
        if font.is_none() {
            tracing::info!(
                target: "script",
                "code boards use the built-in 8×8 font (fetch assets for JetBrains Mono)"
            );
        }
        Self {
            font,
            px: 0.0,
            glyphs: HashMap::new(),
        }
    }

    fn set_size(&mut self, px: f32) {
        if (px - self.px).abs() > 0.01 {
            self.px = px;
            self.glyphs.clear();
        }
    }

    /// Horizontal advance of one character, pixels.
    fn advance(&self) -> f32 {
        match &self.font {
            Some(f) => f
                .as_scaled(PxScale::from(self.px))
                .h_advance(f.glyph_id('M')),
            None => self.px * 0.6,
        }
    }

    /// Distance from a line's top to its baseline, pixels.
    fn ascent(&self) -> f32 {
        match &self.font {
            Some(f) => f.as_scaled(PxScale::from(self.px)).ascent(),
            None => self.px * 0.8,
        }
    }

    fn glyph(&mut self, c: char) -> Option<&Glyph> {
        if !self.glyphs.contains_key(&c) {
            let g = self.rasterize(c);
            self.glyphs.insert(c, g);
        }
        self.glyphs.get(&c).and_then(|g| g.as_ref())
    }

    fn rasterize(&self, c: char) -> Option<Glyph> {
        match &self.font {
            Some(font) => {
                let glyph = font
                    .glyph_id(c)
                    .with_scale_and_position(PxScale::from(self.px), ab_glyph::point(0.0, 0.0));
                let outlined = font.outline_glyph(glyph)?;
                let b = outlined.px_bounds();
                let (w, h) = (
                    b.width().ceil() as usize + 1,
                    b.height().ceil() as usize + 1,
                );
                let mut cov = vec![0u8; w * h];
                outlined.draw(|x, y, v| {
                    let (x, y) = (x as usize, y as usize);
                    if x < w && y < h {
                        cov[y * w + x] = (v.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                });
                Some(Glyph {
                    w,
                    h,
                    left: b.min.x.floor() as i32,
                    top: b.min.y.floor() as i32,
                    cov,
                })
            }
            None => {
                // The 8×8 font, scaled with a box filter so its edges stay soft.
                let rows = *font8x8::legacy::BASIC_LEGACY.get(c as usize)?;
                let cell = (self.px * 0.6 / 8.0).max(1.0);
                let (w, h) = ((8.0 * cell) as usize, (8.0 * cell) as usize);
                let mut cov = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        let (gx, gy) = ((x as f32 / cell) as usize, (y as f32 / cell) as usize);
                        if gx < 8 && gy < 8 && rows[gy] & (1 << gx) != 0 {
                            cov[y * w + x] = 255;
                        }
                    }
                }
                Some(Glyph {
                    w,
                    h,
                    left: 0,
                    top: -(self.px * 0.7) as i32,
                    cov,
                })
            }
        }
    }
}

// ----------------------------------------------------------------------------- colors

type Rgb = [f32; 3];

const BG: Rgb = [0.055, 0.062, 0.078];
const GUTTER_BG: Rgb = [0.045, 0.05, 0.064];
const HEADER_BG: Rgb = [0.10, 0.11, 0.135];
const TEXT: Rgb = [0.80, 0.83, 0.88];
const LINE_NO: Rgb = [0.34, 0.38, 0.45];
const KEYWORD: Rgb = [0.80, 0.52, 0.95];
const STRING: Rgb = [0.62, 0.84, 0.45];
const NUMBER: Rgb = [0.95, 0.66, 0.38];
const COMMENT: Rgb = [0.45, 0.50, 0.58];
const SELF: Rgb = [0.95, 0.47, 0.50];
const CALL: Rgb = [0.40, 0.72, 1.00];
const ENGINE: Rgb = [0.35, 0.86, 0.85];
const TITLE: Rgb = [0.98, 0.80, 0.42];
const OK: Rgb = [0.45, 0.85, 0.50];
const ERROR: Rgb = [1.0, 0.40, 0.38];
const CHANGED_BG: Rgb = [0.15, 0.12, 0.035];
const ERROR_BG: Rgb = [0.16, 0.04, 0.045];
/// Logpoint values (`script.logpoint`) beside their lines.
const VALUE: Rgb = [1.0, 0.80, 0.40];
const VALUE_BG: Rgb = [0.13, 0.095, 0.03];
const VALUE_GLOW: f32 = 0.55;
const ERROR_GLOW: f32 = 0.6;
/// The error's message beside its line.
const FAULT_BG: Rgb = [0.30, 0.05, 0.05];
const FAULT_TEXT: Rgb = [1.0, 0.72, 0.68];
/// The profiler's heat, from a line that took little of the script time to the hottest.
const HEAT: [Rgb; 3] = [[0.30, 0.045, 0.03], [1.0, 0.36, 0.06], [1.0, 0.86, 0.42]];
const HEAT_GLOW: f32 = 0.7;

fn heat_color(t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0) * 2.0;
    let (a, b, k) = if t < 1.0 {
        (HEAT[0], HEAT[1], t)
    } else {
        (HEAT[1], HEAT[2], t - 1.0)
    };
    [
        a[0] + (b[0] - a[0]) * k,
        a[1] + (b[1] - a[1]) * k,
        a[2] + (b[2] - a[2]) * k,
    ]
}

/// How much the panels glow next to the text (which glows fully): enough to read the board's
/// shape in the dark, little enough that the text stands out at night.
const PANEL_GLOW: f32 = 0.06;
/// Lines a reload just changed glow more, so the edit shows from across the courtyard.
const CHANGED_GLOW: f32 = 0.8;

const KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "export", "false", "for",
    "function", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true",
    "type", "until", "while",
];

/// Globals the engine adds; colored apart from plain calls.
const ENGINE_GLOBALS: &[&str] = &[
    "vec3", "quat", "world", "task", "ease", "input", "api", "require", "print", "warn", "math",
];

/// Colors for each character of a line of Luau. `in_comment` carries a `--[[` block comment
/// over lines.
fn highlight(line: &str, in_comment: &mut bool) -> Vec<Rgb> {
    let chars: Vec<char> = line.chars().collect();
    let mut colors = vec![TEXT; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if *in_comment {
            let start = i;
            while i < chars.len() && !(chars[i] == ']' && chars.get(i + 1) == Some(&']')) {
                i += 1;
            }
            if i < chars.len() {
                i += 2;
                *in_comment = false;
            }
            colors[start..i.min(chars.len())].fill(COMMENT);
            continue;
        }
        let c = chars[i];
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            if chars.get(i + 2) == Some(&'[') && chars.get(i + 3) == Some(&'[') {
                *in_comment = true;
                colors[i..(i + 4).min(chars.len())].fill(COMMENT);
                i += 4;
                continue;
            }
            colors[i..].fill(COMMENT);
            break;
        }
        if c == '"' || c == '\'' || c == '`' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            colors[start..i].fill(STRING);
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            colors[start..i].fill(NUMBER);
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let prev = chars[..start].iter().rev().find(|c| !c.is_whitespace());
            let next = chars[i..].iter().find(|c| !c.is_whitespace());
            let color = if KEYWORDS.contains(&word.as_str()) {
                KEYWORD
            } else if word == "self" {
                SELF
            } else if prev != Some(&'.')
                && prev != Some(&':')
                && ENGINE_GLOBALS.contains(&word.as_str())
            {
                ENGINE
            } else if next == Some(&'(') || next == Some(&'{') || next == Some(&'"') {
                CALL
            } else {
                TEXT
            };
            colors[start..i].fill(color);
            continue;
        }
        i += 1;
    }
    colors
}

/// Lines (0-based) of `new` that weren't in `old` at the same place or anywhere near it.
fn changed_lines(old: &str, new: &str) -> HashSet<usize> {
    let old: Vec<&str> = old.lines().collect();
    let mut out = HashSet::new();
    for (i, line) in new.lines().enumerate() {
        let lo = i.saturating_sub(8);
        let hi = (i + 8).min(old.len());
        let same = lo < hi && old[lo..hi].iter().any(|o| o == &line);
        if !same && !line.trim().is_empty() {
            out.insert(i);
        }
    }
    out
}

/// A board's picture: its colors (lit like paint) and its glow (the emissive texture).
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<Rgb>,
    glow: Vec<Rgb>,
}

fn scaled(c: Rgb, k: f32) -> Rgb {
    [c[0] * k, c[1] * k, c[2] * k]
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![BG; w * h],
            glow: vec![scaled(BG, PANEL_GLOW); w * h],
        }
    }

    /// Fills a panel of color `c` glowing at `glow` of its color.
    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb, glow: f32) {
        for y in y0.max(0)..y1.min(self.h as i32) {
            for x in x0.max(0)..x1.min(self.w as i32) {
                let i = y as usize * self.w + x as usize;
                self.px[i] = c;
                self.glow[i] = scaled(c, glow);
            }
        }
    }

    fn glyph(&mut self, g: &Glyph, x: i32, baseline: i32, c: Rgb) {
        for gy in 0..g.h {
            let y = baseline + g.top + gy as i32;
            if y < 0 || y >= self.h as i32 {
                continue;
            }
            for gx in 0..g.w {
                let a = g.cov[gy * g.w + gx] as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let x = x + g.left + gx as i32;
                if x < 0 || x >= self.w as i32 {
                    continue;
                }
                let i = y as usize * self.w + x as usize;
                for p in [&mut self.px[i], &mut self.glow[i]] {
                    for k in 0..3 {
                        p[k] += (c[k] - p[k]) * a;
                    }
                }
            }
        }
    }

    fn text(
        &mut self,
        font: &mut BoardFont,
        s: &str,
        x: f32,
        baseline: f32,
        colors: &dyn Fn(usize) -> Rgb,
    ) {
        let adv = font.advance();
        for (i, ch) in s.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let c = colors(i);
            if let Some(g) = font.glyph(ch) {
                self.glyph(g, (x + adv * i as f32) as i32, baseline as i32, c);
            }
        }
    }

    /// The color and glow images (sRGB).
    fn into_images(self) -> (Image, Image) {
        // Linear to sRGB through a table: a board has millions of channels to convert.
        const STEPS: usize = 4096;
        let table: Vec<u8> = (0..=STEPS)
            .map(|i| {
                let v = i as f32 / STEPS as f32;
                let s = if v <= 0.003_130_8 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
            .collect();
        let image = |px: Vec<Rgb>| {
            let mut data = Vec::with_capacity(self.w * self.h * 4);
            for p in px {
                for v in p {
                    data.push(table[(v.clamp(0.0, 1.0) * STEPS as f32 + 0.5) as usize]);
                }
                data.push(255);
            }
            Image::new(self.w as u32, self.h as u32, data, true)
        };
        (image(self.px), image(self.glow))
    }

    /// Text that fits in `max_chars` (cut with an ellipsis).
    fn fitted(s: &str, max_chars: usize) -> String {
        if s.chars().count() <= max_chars {
            return s.to_string();
        }
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// What a board marks beside its lines.
#[derive(Clone, Copy)]
struct Marks<'a> {
    /// Lines (0-based) the last reload changed, lit.
    changed: &'a HashSet<usize>,
    /// Logpoint values by line (1-based), drawn after the code.
    values: &'a BTreeMap<u32, String>,
    /// Each line's share of the recorded script time, percent (1-based lines).
    heat: &'a BTreeMap<u32, f32>,
    /// The last runtime error raised in this file.
    fault: Option<&'a Fault>,
}

/// Draws `source` (with `error`, if the last reload failed) onto a board's image.
fn draw(
    board: &CodeBoard,
    font: &mut BoardFont,
    title: &str,
    version: u32,
    source: &str,
    error: Option<&str>,
    marks: Marks<'_>,
) -> (Image, Image) {
    let Marks {
        changed,
        values,
        heat,
        fault,
    } = marks;
    // The line the error is on: the runtime error's, or the failed reload's.
    let error_line = fault.map(|f| f.line).or_else(|| {
        let (path, line) = error_location(error?)?;
        title.ends_with(&path).then_some(line)
    });
    let hottest = heat.values().fold(0.0f32, |a, &b| a.max(b));
    let (w, h) = (
        board.size[0].max(64) as usize,
        board.size[1].max(64) as usize,
    );
    let mut cv = Canvas::new(w, h);
    let margin = (h as f32 * 0.035).round();
    let header = (h as f32 * 0.085).round();
    let lines = board.lines.max(4) as f32;
    let line_h = ((h as f32 - header - margin * 2.0) / lines).floor();
    font.set_size(line_h * 0.78);

    // Header: file name, version, status.
    cv.rect(0, 0, w as i32, header as i32, HEADER_BG, PANEL_GLOW);
    let header_base = header * 0.5 + font.ascent() * 0.4;
    cv.text(font, title, margin, header_base, &|_| TITLE);
    let (status, status_color) = match (error, fault) {
        (Some(_), _) => ("● error: previous version running".to_string(), ERROR),
        (None, Some(f)) => (
            format!("● error on line {} · version {version}", f.line),
            ERROR,
        ),
        (None, None) => (format!("● running · version {version}"), OK),
    };
    let adv = font.advance();
    let sx = w as f32 - margin - adv * status.chars().count() as f32;
    cv.text(font, &status, sx, header_base, &|_| status_color);

    // Gutter with line numbers, then the code.
    let digits = 3.0;
    let gutter = margin + adv * (digits + 1.5);
    cv.rect(
        0,
        header as i32,
        gutter as i32,
        h as i32,
        GUTTER_BG,
        PANEL_GLOW,
    );
    let first = board.first_line.max(1) as usize - 1;
    let all: Vec<&str> = source.lines().collect();
    let mut in_comment = false;
    for line in all.iter().take(first) {
        highlight(line, &mut in_comment);
    }
    // A runtime error whose line is out of view goes in the footer, like a failed reload.
    let fault_hidden = fault.is_some_and(|f| {
        let l = f.line as usize;
        l <= first || l > (first + lines as usize).min(all.len())
    });
    let footer: Option<String> = match (error, fault) {
        (Some(e), _) => Some(e.to_string()),
        (None, Some(f)) if fault_hidden => {
            Some(format!("line {}: {} (×{})", f.line, f.message, f.count))
        }
        _ => None,
    };
    let error_lines = if footer.is_some() { 3 } else { 0 };
    let shown = (lines as usize).saturating_sub(error_lines);
    for (row, i) in (first..all.len()).take(shown).enumerate() {
        let top = header + margin + row as f32 * line_h;
        let line_no = i as u32 + 1;
        if error_line == Some(line_no) {
            cv.rect(
                0,
                top as i32,
                w as i32,
                (top + line_h) as i32,
                ERROR_BG,
                ERROR_GLOW,
            );
        } else if changed.contains(&i) {
            cv.rect(
                gutter as i32,
                top as i32,
                w as i32,
                (top + line_h) as i32,
                CHANGED_BG,
                CHANGED_GLOW,
            );
        }
        let base = top + line_h * 0.5 + font.ascent() * 0.4;
        let pct = heat.get(&line_no).copied().unwrap_or(0.0);
        if pct > 0.0 && hottest > 0.0 {
            // Heat: the line tinted by its share of the hottest line's time, a strip at the
            // board's edge in the full color, and the share at the right.
            let t = (pct / hottest).sqrt();
            let c = heat_color(t);
            if error_line != Some(line_no) && !changed.contains(&i) {
                cv.rect(
                    gutter as i32,
                    top as i32,
                    w as i32,
                    (top + line_h) as i32,
                    scaled(c, 0.07 + 0.2 * t),
                    0.45,
                );
            }
            cv.rect(
                0,
                top as i32,
                (margin * 0.55) as i32,
                (top + line_h) as i32,
                c,
                HEAT_GLOW,
            );
            if pct >= 0.5 {
                let label = format!("{pct:.1}%");
                let x = w as f32 - margin - adv * label.chars().count() as f32;
                cv.text(font, &label, x, base, &|_| heat_color(t.max(0.55)));
            }
        }
        let no = format!("{:>3}", i + 1);
        let no_color = if error_line == Some(line_no) {
            ERROR
        } else if pct > 0.0 {
            TEXT
        } else {
            LINE_NO
        };
        cv.text(font, &no, margin, base, &|_| no_color);
        let text = all[i].replace('\t', "    ");
        let colors = highlight(&text, &mut in_comment);
        cv.text(font, &text, gutter + adv, base, &|k| {
            colors.get(k).copied().unwrap_or(TEXT)
        });
        if let Some(value) = values.get(&(i as u32 + 1)) {
            // After the code, or over its end when the line is long.
            let label: String = format!("\u{25C0} {value}").chars().take(48).collect();
            let width = adv * (label.chars().count() as f32 + 1.0);
            let after = gutter + adv * (text.chars().count() as f32 + 3.0);
            let x = after.min(w as f32 - margin - width).max(gutter + adv);
            cv.rect(
                (x - adv * 0.5) as i32,
                (top + line_h * 0.06) as i32,
                (x + width) as i32,
                (top + line_h * 0.94) as i32,
                VALUE_BG,
                VALUE_GLOW,
            );
            cv.text(font, &label, x, base, &|_| VALUE);
        }
        if let Some(f) = fault.filter(|f| f.line == line_no) {
            let count = if f.count > 1 {
                format!(" (×{})", f.count)
            } else {
                String::new()
            };
            let label = Canvas::fitted(&format!("⚠ {}{count}", f.message), 60);
            let width = adv * (label.chars().count() as f32 + 1.0);
            let after = gutter + adv * (text.chars().count() as f32 + 3.0);
            let x = after.min(w as f32 - margin - width).max(gutter + adv);
            cv.rect(
                (x - adv * 0.5) as i32,
                (top + line_h * 0.06) as i32,
                (x + width) as i32,
                (top + line_h * 0.94) as i32,
                FAULT_BG,
                ERROR_GLOW,
            );
            cv.text(font, &label, x, base, &|_| FAULT_TEXT);
        }
    }
    if let Some(err) = footer.as_deref() {
        let top = h as f32 - margin - line_h * error_lines as f32;
        cv.rect(
            0,
            (top - margin * 0.5) as i32,
            w as i32,
            h as i32,
            ERROR_BG,
            0.5,
        );
        let max_chars = ((w as f32 - margin * 2.0) / adv) as usize;
        for (k, part) in err
            .lines()
            .flat_map(|l| {
                l.chars()
                    .collect::<Vec<_>>()
                    .chunks(max_chars.max(10))
                    .map(|c| c.iter().collect::<String>())
                    .collect::<Vec<_>>()
            })
            .take(error_lines)
            .enumerate()
        {
            let base = top + k as f32 * line_h + line_h * 0.5 + font.ascent() * 0.4;
            cv.text(font, &part, margin, base, &|_| ERROR);
        }
    }
    cv.into_images()
}

/// What [`draw_boards`] keeps between frames.
#[derive(Default)]
pub(crate) struct Boards {
    drawn: HashMap<Entity, Drawn>,
    font: Option<BoardFont>,
    /// When files no script has loaded are next read from disk (real seconds).
    next_check: f64,
}

/// Redraws boards whose script changed (`Update`, after the scripts ran).
pub(crate) fn draw_boards(
    mut boards: Query<(Entity, &CodeBoard, &mut MeshRenderer)>,
    runtime: Option<Res<ScriptRuntime>>,
    server: Option<Res<AssetServer>>,
    time: Res<Time>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut state: Local<Boards>,
    settings: Option<Res<crate::ScriptSettings>>,
) {
    let Some(images) = images.as_deref_mut() else {
        return;
    };
    let Boards {
        drawn,
        font,
        next_check,
    } = &mut *state;
    let now = time.real_elapsed_secs_f64();
    let checking = now >= *next_check;
    if checking {
        *next_check = now + 0.25;
    }
    drawn.retain(|e, _| boards.contains(*e));
    for (e, board, mut renderer) in boards.iter_mut() {
        let path = normalize(&board.path);
        let loaded = runtime.as_ref().and_then(|rt| {
            let files = rt.files();
            files.map.get(&path).map(|f| {
                (
                    f.source.clone(),
                    f.version,
                    f.error.clone(),
                    // Raised in whichever VM runs the script.
                    f.fault.clone().or_else(|| rt.fault_of(&path)),
                )
            })
        });
        let known = drawn.get(&e);
        let (inline_version, inline) = runtime
            .as_ref()
            .map(|rt| rt.debugger.inline_values(&path))
            .unwrap_or_default();
        let (heat_version, heat) = runtime
            .as_ref()
            .map(|rt| crate::profiler::line_heat(&rt.luas(), &path))
            .unwrap_or_default();
        // A file no script runs yet is read from disk (a few times a second).
        let (source, version, error, fault) = match loaded {
            Some(l) => l,
            None => {
                if !checking && known.is_some() {
                    continue;
                }
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
                match std::fs::read_to_string(&full) {
                    Ok(s) => (s, 0, None, None),
                    Err(err) => (
                        String::new(),
                        0,
                        Some(format!("can't read {path}: {err}")),
                        None,
                    ),
                }
            }
        };
        let fade = known.is_some_and(|d| {
            !d.faded && !d.changed.is_empty() && now - d.changed_at > board.changed_secs as f64
        });
        // An error that repeats every frame only updates its count every two seconds.
        let fault_key = |f: &Option<Fault>| f.as_ref().map(|f| (f.line, f.message.clone()));
        let same = known.is_some_and(|d| {
            d.version == version
                && d.text == source
                && d.error == error
                && fault_key(&d.fault) == fault_key(&fault)
                && d.board == *board
        });
        let recount = known.is_some_and(|d| d.fault != fault && now - d.drawn_at > 2.0);
        // New logpoint values redraw a few times a second at most; a new profile (once a
        // second at most) at once, before a live profile board starts the next one.
        let values = known.is_some_and(|d| {
            (checking && d.inline_version != inline_version) || d.heat_version != heat_version
        });
        if same && !fade && !values && !recount {
            continue;
        }
        let font = font.get_or_insert_with(|| BoardFont::load(server.as_ref().map(|s| s.root())));
        let (changed, changed_at, faded) = match known {
            Some(d) if fade && same => (HashSet::new(), d.changed_at, true),
            Some(d) if d.text != source => (changed_lines(&d.text, &source), now, false),
            Some(d) => (d.changed.clone(), d.changed_at, d.faded),
            None => (HashSet::new(), now, true),
        };
        let (image, glow) = draw(
            board,
            font,
            &path,
            version,
            &source,
            error.as_deref(),
            Marks {
                changed: &changed,
                values: &inline,
                heat: &heat,
                fault: fault.as_ref(),
            },
        );
        let (handle, glow_handle) = show(
            images,
            drawn.get(&e).map(|d| (d.image, d.glow)),
            (image, glow),
            &mut renderer,
            board.glow_cd_m2,
        );
        drawn.insert(
            e,
            Drawn {
                text: source,
                version,
                image: handle,
                glow: glow_handle,
                changed,
                changed_at,
                error,
                faded,
                inline_version,
                heat_version,
                fault,
                drawn_at: now,
                board: board.clone(),
            },
        );
    }
}

/// Puts a board's new images on its mesh: redrawn in place, so the material (and its key in the
/// material cache) stays the same. Returns the image handles.
fn show(
    images: &mut Assets<Image>,
    old: Option<(Handle<Image>, Handle<Image>)>,
    (image, glow): (Image, Image),
    renderer: &mut MeshRenderer,
    glow_cd_m2: f32,
) -> (Handle<Image>, Handle<Image>) {
    let mut put = |old: Option<Handle<Image>>, new: Image| match old {
        Some(h) if images.get(h).is_some() => {
            if let Some(slot) = images.get_mut(h) {
                *slot = new;
            }
            h
        }
        _ => images.add(new),
    };
    let handle = put(old.map(|o| o.0), image);
    let glow_handle = put(old.map(|o| o.1), glow);
    let mut material = match &renderer.material {
        MaterialRef::Inline(m) => *m,
        _ => StandardMaterial::default(),
    };
    material.base_color = [1.0, 1.0, 1.0, 1.0];
    material.base_color_texture = Some(handle);
    material.emissive_texture = Some(glow_handle);
    material.emissive = [1.0, 1.0, 1.0];
    material.emissive_strength = glow_cd_m2;
    material.perceptual_roughness = 0.35;
    material.metallic = 0.0;
    renderer.material = MaterialRef::Inline(material);
    (handle, glow_handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luau_is_colored_by_kind() {
        let mut c = false;
        let line = "local x = vec3(1, 2) -- note";
        let colors = highlight(line, &mut c);
        assert_eq!(colors[0], KEYWORD);
        assert_eq!(colors[10], ENGINE);
        assert_eq!(colors[15], NUMBER);
        assert_eq!(colors[line.len() - 1], COMMENT);
        let s = highlight("print(\"hi\")", &mut c);
        assert_eq!(s[7], STRING);
    }

    #[test]
    fn edited_lines_are_found() {
        let old = "a\nb\nc\nd";
        let new = "a\nb\nC changed\nd";
        let changed = changed_lines(old, new);
        assert_eq!(changed, HashSet::from([2]));
    }

    #[test]
    fn a_board_draws_without_a_font_file() {
        let mut font = BoardFont::load(None);
        let board = CodeBoard {
            size: [512, 320],
            lines: 12,
            ..Default::default()
        };
        let (img, glow) = draw(
            &board,
            &mut font,
            "demo.luau",
            3,
            "local S = {}\nfunction S:update(dt)\n  self.t += dt\nend\nreturn S",
            Some("demo.luau:3: attempt to add nil"),
            Marks {
                changed: &HashSet::from([2]),
                values: &BTreeMap::from([(3, "self.t = 0.25".to_string())]),
                heat: &BTreeMap::from([(2, 100.0), (3, 62.5)]),
                fault: Some(&Fault {
                    line: 4,
                    message: "attempt to add nil".into(),
                    count: 30,
                }),
            },
        );
        assert_eq!((img.width, img.height), (512, 320));
        // Some text pixels are much brighter than the background.
        let bright = img
            .data
            .chunks(4)
            .filter(|p| p[0] > 150 || p[1] > 150)
            .count();
        assert!(bright > 200, "{bright}");
        // The glow is the text: the panels hardly glow.
        let lit = |i: &Image| {
            i.data
                .chunks(4)
                .map(|p| p[0] as u32 + p[1] as u32)
                .sum::<u32>()
        };
        assert!(
            lit(&glow) < lit(&img),
            "the panels glow less than they are colored"
        );
    }

    #[test]
    fn a_profile_board_draws_a_flame_graph() {
        let node = |name: &str, kind, ns, children| FlameNode {
            name: name.into(),
            kind,
            at: (kind == Kind::Lua).then(|| ("demo.luau".to_string(), 3)),
            ns,
            children,
        };
        let root = node(
            "all scripts",
            Kind::Phase,
            1000,
            vec![node(
                "update",
                Kind::Phase,
                900,
                vec![node(
                    "update",
                    Kind::Lua,
                    900,
                    vec![node("e:set", Kind::Engine, 500, vec![])],
                )],
            )],
        );
        let summary = Summary {
            recording: false,
            hz: 1000,
            seconds: 5.0,
            samples: 1000,
            script_ms: 10.0,
            frames: 300,
            vms: 1,
        };
        let mut font = BoardFont::load(None);
        let board = ProfileBoard {
            size: [512, 320],
            ..Default::default()
        };
        let lines = vec![(
            "demo.luau".to_string(),
            4,
            50.0,
            "self.entity:set(\"PointLight\", {})".to_string(),
        )];
        let (img, _) = draw_profile(&board, &mut font, Some(&(summary, root)), &lines);
        assert_eq!((img.width, img.height), (512, 320));
        // The engine call's teal box is drawn.
        let teal = img
            .data
            .chunks(4)
            .filter(|p| p[1] > 120 && p[2] > 120 && p[0] < 110)
            .count();
        assert!(teal > 500, "{teal}");
        let (empty, _) = draw_profile(&board, &mut font, None, &[]);
        assert_eq!(empty.width, 512);
    }
}

// ----------------------------------------------------------------------------- profile boards

/// Draws the script profiler's results on this entity's [`MeshRenderer`]: the flame graph (each
/// box a function, as wide as its share of the script time, under the function that called it)
/// and the hottest lines with their code. Shows what `script.profile` recorded; with `live`, it
/// records on its own, a window at a time, whenever no one else has.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileBoard {
    /// Texture size, pixels (width, height); its aspect should match the board's.
    pub size: [u32; 2],
    /// Glow, cd/m² for the brightest parts.
    pub glow_cd_m2: f32,
    /// Records by itself while no `script.profile` recording is shown.
    pub live: bool,
    /// Seconds per live recording (the board shows the last finished one).
    pub window_secs: f32,
    /// Live recordings' samples per second.
    pub hz: u32,
}

impl Default for ProfileBoard {
    fn default() -> Self {
        Self {
            size: [2048, 1280],
            glow_cd_m2: 60.0,
            live: true,
            window_secs: 10.0,
            hz: 1000,
        }
    }
}

const PHASE_BOX: Rgb = [0.22, 0.27, 0.40];
const ENGINE_BOX: Rgb = [0.14, 0.50, 0.50];
const BOX_TEXT: Rgb = [0.02, 0.02, 0.025];
const BOX_GLOW: f32 = 0.35;

/// A Luau function's box color: warm, and the same for the same function every time.
fn lua_color(name: &str, at: &Option<(String, u32)>) -> Rgb {
    let mut h: u32 = 2166136261;
    for b in name
        .bytes()
        .chain(at.as_ref().map(|a| a.0.as_str()).unwrap_or("").bytes())
    {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let t = (h % 1000) as f32 / 1000.0;
    // From deep orange to amber.
    [0.85 + 0.15 * t, 0.30 + 0.38 * t, 0.06 + 0.10 * (1.0 - t)]
}

/// What one flame-graph row needs to draw a node.
struct FlameCtx<'a> {
    total: f64,
    x0: f32,
    width: f32,
    top: f32,
    row_h: f32,
    max_depth: usize,
    adv: f32,
    ascent: f32,
    ms_per_ns: &'a dyn Fn(u64) -> f64,
}

fn flame_node(
    cv: &mut Canvas,
    font: &mut BoardFont,
    node: &FlameNode,
    depth: usize,
    x: f32,
    cx: &FlameCtx<'_>,
) {
    if depth > cx.max_depth {
        return;
    }
    let w = (node.ns as f64 / cx.total) as f32 * cx.width;
    if w < 2.0 {
        return;
    }
    let top = cx.top + depth as f32 * cx.row_h;
    let color = match node.kind {
        Kind::Phase => PHASE_BOX,
        Kind::Engine => ENGINE_BOX,
        Kind::Lua => lua_color(&node.name, &node.at),
    };
    cv.rect(
        (cx.x0 + x) as i32,
        top as i32,
        (cx.x0 + x + w - 2.0) as i32,
        (top + cx.row_h - 2.0) as i32,
        color,
        BOX_GLOW,
    );
    let chars = ((w - cx.adv) / cx.adv).floor() as usize;
    if chars >= 3 {
        let pct = node.ns as f64 / cx.total * 100.0;
        let name = match node.kind {
            Kind::Engine => format!("⚙ {}", node.name),
            _ => node.name.clone(),
        };
        let full = if depth == 0 {
            format!("{name} · {:.2} ms", (cx.ms_per_ns)(node.ns))
        } else {
            format!("{name} {pct:.0}%")
        };
        let label = Canvas::fitted(
            if full.chars().count() <= chars {
                &full
            } else {
                &name
            },
            chars,
        );
        let text_color = if node.kind == Kind::Phase {
            TEXT
        } else {
            BOX_TEXT
        };
        let base = top + cx.row_h * 0.5 + cx.ascent * 0.4 - 1.0;
        cv.text(font, &label, cx.x0 + x + cx.adv * 0.5, base, &|_| {
            text_color
        });
    }
    let mut cx_x = x;
    for c in &node.children {
        flame_node(cv, font, c, depth + 1, cx_x, cx);
        cx_x += (c.ns as f64 / cx.total) as f32 * cx.width;
    }
}

/// Draws a profile board: header, flame graph, hottest lines.
fn draw_profile(
    board: &ProfileBoard,
    font: &mut BoardFont,
    profile: Option<&(Summary, FlameNode)>,
    lines: &[(String, u32, f64, String)],
) -> (Image, Image) {
    let (w, h) = (
        board.size[0].max(64) as usize,
        board.size[1].max(64) as usize,
    );
    let mut cv = Canvas::new(w, h);
    let margin = (h as f32 * 0.035).round();
    let header = (h as f32 * 0.085).round();
    font.set_size(header * 0.42);
    cv.rect(0, 0, w as i32, header as i32, HEADER_BG, PANEL_GLOW);
    let header_base = header * 0.5 + font.ascent() * 0.4;
    cv.text(font, "Script profile", margin, header_base, &|_| TITLE);
    let adv = font.advance();
    let Some((summary, root)) = profile.filter(|(s, _)| s.samples > 0) else {
        let note = match profile {
            Some((s, _)) if s.recording => "● recording…",
            _ => "no profile yet: script.profile {seconds = 5}",
        };
        let sx = w as f32 - margin - adv * note.chars().count() as f32;
        cv.text(font, note, sx, header_base, &|_| LINE_NO);
        return cv.into_images();
    };
    let khz = if summary.hz >= 1000 {
        format!("{:.0} kHz", summary.hz as f64 / 1000.0)
    } else {
        format!("{} Hz", summary.hz)
    };
    let (status, status_color) = if summary.recording {
        (
            format!("● recording · {:.1} s · {khz}", summary.seconds),
            VALUE,
        )
    } else if summary.vms > 1 {
        // Summed over VMs that ran side by side: CPU time, not frame time.
        (
            format!(
                "● {:.1} s · {} VMs · {:.2} ms of script CPU per frame",
                summary.seconds,
                summary.vms,
                summary.ms_per_frame()
            ),
            OK,
        )
    } else {
        (
            format!(
                "● {:.1} s · {} samples · {:.2} ms of script per frame",
                summary.seconds,
                summary.samples,
                summary.ms_per_frame()
            ),
            OK,
        )
    };
    let sx = w as f32 - margin - adv * status.chars().count() as f32;
    cv.text(font, &status, sx, header_base, &|_| status_color);

    // The flame graph: callers above, callees below (an icicle), widest first.
    let flame_top = header + margin * 0.8;
    let flame_bottom = h as f32 * 0.64;
    let max_depth = 9;
    let row_h = ((flame_bottom - flame_top) / (max_depth + 1) as f32).floor();
    font.set_size(row_h * 0.52);
    let frames = summary.frames.max(1) as f64;
    let per_frame = move |ns: u64| ns as f64 / 1e6 / frames;
    let cx = FlameCtx {
        total: root.ns.max(1) as f64,
        x0: margin,
        width: w as f32 - margin * 2.0,
        top: flame_top,
        row_h,
        max_depth,
        adv: font.advance(),
        ascent: font.ascent(),
        ms_per_ns: &per_frame,
    };
    flame_node(&mut cv, font, root, 0, 0.0, &cx);

    // The hottest lines: share, file:line, the code.
    let table_top = flame_bottom + margin * 0.6;
    let rows = 6usize;
    let line_h = ((h as f32 - table_top - margin) / (rows + 1) as f32).floor();
    font.set_size(line_h * 0.56);
    let adv = font.advance();
    let base0 = table_top + line_h * 0.5 + font.ascent() * 0.4;
    cv.text(
        font,
        "Hottest lines (time on the line, calls included)",
        margin,
        base0,
        &|_| TITLE,
    );
    let hottest = lines.first().map(|l| l.2).unwrap_or(1.0).max(1e-6);
    for (k, (path, line, pct, code)) in lines.iter().take(rows).enumerate() {
        let top = table_top + (k + 1) as f32 * line_h;
        let base = top + line_h * 0.5 + font.ascent() * 0.4;
        let t = ((*pct / hottest) as f32).sqrt();
        let bar = adv * 7.0;
        cv.rect(
            margin as i32,
            (top + line_h * 0.15) as i32,
            (margin + bar * t.max(0.05)) as i32,
            (top + line_h * 0.85) as i32,
            heat_color(t),
            HEAT_GLOW,
        );
        let share = format!("{pct:>5.1}%");
        cv.text(font, &share, margin + bar + adv, base, &|_| {
            heat_color(t.max(0.55))
        });
        let file = path.rsplit('/').next().unwrap_or(path);
        let at = format!("{file}:{line}");
        let at_x = margin + bar + adv * 8.5;
        cv.text(font, &at, at_x, base, &|_| LINE_NO);
        let code_x = at_x + adv * 22.0;
        let max = ((w as f32 - margin - code_x) / adv).max(4.0) as usize;
        let code = Canvas::fitted(code, max);
        let mut in_comment = false;
        let colors = highlight(&code, &mut in_comment);
        cv.text(font, &code, code_x, base, &|i| {
            colors.get(i).copied().unwrap_or(TEXT)
        });
    }
    cv.into_images()
}

/// What [`draw_profile_boards`] keeps between frames.
#[derive(Default)]
pub(crate) struct ProfileBoards {
    drawn: HashMap<Entity, (u64, Handle<Image>, Handle<Image>, ProfileBoard)>,
    font: Option<BoardFont>,
}

/// Redraws profile boards when a recording ends (or grows, for `script.profile`), and starts
/// live boards' recordings (`Update`, after the code boards drew the last one's heat).
pub(crate) fn draw_profile_boards(
    mut boards: Query<(Entity, &ProfileBoard, &mut MeshRenderer)>,
    runtime: Option<ResMut<ScriptRuntime>>,
    server: Option<Res<AssetServer>>,
    mut images: Option<ResMut<Assets<Image>>>,
    mut state: Local<ProfileBoards>,
) {
    let (Some(images), Some(mut runtime)) = (images.as_deref_mut(), runtime) else {
        return;
    };
    if boards.is_empty() {
        return;
    }
    let ProfileBoards { drawn, font } = &mut *state;
    drawn.retain(|e, _| boards.contains(*e));
    let shown = crate::profiler::shown(&runtime.luas());
    let mut profile = None;
    let mut lines = None;
    let mut live = None;
    for (e, board, mut renderer) in boards.iter_mut() {
        if board.live && live.is_none() {
            live = Some((board.hz, board.window_secs));
        }
        if drawn.get(&e).is_some_and(|d| d.0 == shown && d.3 == *board) {
            continue;
        }
        let rt = &*runtime;
        let profile = profile
            .get_or_insert_with(|| crate::profiler::flame(&rt.luas(), rt.profile_recording()));
        let lines = lines.get_or_insert_with(|| crate::profiler::hot_lines(&rt.luas(), 6));
        let font = font.get_or_insert_with(|| BoardFont::load(server.as_ref().map(|s| s.root())));
        let pictures = draw_profile(board, font, profile.as_ref(), lines);
        let (image, glow) = show(
            images,
            drawn.get(&e).map(|d| (d.1, d.2)),
            pictures,
            &mut renderer,
            board.glow_cd_m2,
        );
        drawn.insert(e, (shown, image, glow, board.clone()));
    }
    if let Some((hz, secs)) = live
        && runtime.profile_idle_for_boards()
    {
        runtime.profile_start(hz, secs.max(1.0) as f64, true);
    }
}
