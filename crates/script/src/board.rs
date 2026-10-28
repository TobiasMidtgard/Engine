//! Code boards: a script's source drawn onto a mesh in the world, syntax-colored, redrawn the
//! moment the file reloads, with the lines that just changed lit up. The showcase's scripted
//! courtyard stands one beside what the script drives, so one picture shows code and effect.
//!
//! The text is rasterized on the CPU into an sRGB image (with mips) set as the material's
//! base color and emissive texture, so it reads by day and glows at night. The font is
//! JetBrains Mono when `fetched/fonts/JetBrainsMono-Regular.ttf` is there (assets/manifests/
//! script.toml), else the built-in 8×8 font.

use std::collections::{HashMap, HashSet};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use bevy_ecs::prelude::*;
use engine_assets::{AssetServer, Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, MeshRenderer, StandardMaterial};

use crate::ScriptRuntime;
use crate::runtime::normalize;

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
        let image = |px: Vec<Rgb>| {
            let mut data = Vec::with_capacity(self.w * self.h * 4);
            for p in px {
                for v in p {
                    // Linear to sRGB.
                    let s = if v <= 0.003_130_8 {
                        v * 12.92
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    };
                    data.push((s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                }
                data.push(255);
            }
            Image::new(self.w as u32, self.h as u32, data, true)
        };
        (image(self.px), image(self.glow))
    }
}

/// Draws `source` (with `error`, if the last reload failed) onto a board's image.
fn draw(
    board: &CodeBoard,
    font: &mut BoardFont,
    title: &str,
    version: u32,
    source: &str,
    error: Option<&str>,
    changed: &HashSet<usize>,
) -> (Image, Image) {
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
    let (status, status_color) = match error {
        Some(_) => ("● error: previous version running".to_string(), ERROR),
        None => (format!("● running · version {version}"), OK),
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
    let error_lines = if error.is_some() { 3 } else { 0 };
    let shown = (lines as usize).saturating_sub(error_lines);
    for (row, i) in (first..all.len()).take(shown).enumerate() {
        let top = header + margin + row as f32 * line_h;
        if changed.contains(&i) {
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
        let no = format!("{:>3}", i + 1);
        cv.text(font, &no, margin, base, &|_| LINE_NO);
        let text = all[i].replace('\t', "    ");
        let colors = highlight(&text, &mut in_comment);
        cv.text(font, &text, gutter + adv, base, &|k| {
            colors.get(k).copied().unwrap_or(TEXT)
        });
    }
    if let Some(err) = error {
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
            files
                .map
                .get(&path)
                .map(|f| (f.source.clone(), f.version, f.error.clone()))
        });
        let known = drawn.get(&e);
        // A file no script runs yet is read from disk (a few times a second).
        let (source, version, error) = match loaded {
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
                    Ok(s) => (s, 0, None),
                    Err(err) => (String::new(), 0, Some(format!("can't read {path}: {err}"))),
                }
            }
        };
        let fade = known.is_some_and(|d| {
            !d.faded && !d.changed.is_empty() && now - d.changed_at > board.changed_secs as f64
        });
        let same =
            known.is_some_and(|d| d.version == version && d.text == source && d.error == error);
        if same && !fade {
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
            &changed,
        );
        // Redrawn in place, so the material (and its key in the material cache) stays the same.
        let mut put = |old: Option<Handle<Image>>, new: Image| match old {
            Some(h) if images.get(h).is_some() => {
                if let Some(slot) = images.get_mut(h) {
                    *slot = new;
                }
                h
            }
            _ => images.add(new),
        };
        let handle = put(drawn.get(&e).map(|d| d.image), image);
        let glow_handle = put(drawn.get(&e).map(|d| d.glow), glow);
        let mut material = match &renderer.material {
            MaterialRef::Inline(m) => *m,
            _ => StandardMaterial::default(),
        };
        material.base_color = [1.0, 1.0, 1.0, 1.0];
        material.base_color_texture = Some(handle);
        material.emissive_texture = Some(glow_handle);
        material.emissive = [1.0, 1.0, 1.0];
        material.emissive_strength = board.glow_cd_m2;
        material.perceptual_roughness = 0.35;
        material.metallic = 0.0;
        renderer.material = MaterialRef::Inline(material);
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
            },
        );
    }
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
            &HashSet::from([2]),
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
}
