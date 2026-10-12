//! Text: embedded fonts, shaping and line breaking (cosmic-text: HarfBuzz-style shaping, BiDi,
//! font fallback), a small rich-text markup, and the per-node layout cache.
//!
//! Text is shaped at its physical pixel size, so it is crisp at any UI scale. Glyphs are
//! rasterized once per size and subpixel offset into the glyph atlas (see `draw.rs`).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bevy_ecs::prelude::*;
use cosmic_text::{
    Attrs, Buffer, Family, FontFeatures, FontSystem, Metrics, Shaping, Style as FontStyle,
    SwashCache, Weight, Wrap, fontdb,
};

use crate::color::Color;
use crate::node::TextAlign;

/// The family name of the built-in UI font.
pub const UI_FONT: &str = "Inter";

const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.otf");
const FONT_ITALIC: &[u8] = include_bytes!("../assets/fonts/Inter-Italic.otf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.otf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.otf");

/// Fonts and the glyph rasterizer.
///
/// Built-in: Inter (regular, italic, semibold, bold; Latin, Greek, Cyrillic). Add fonts with
/// [`Fonts::add_font`] (for other scripts or a game's own typeface); set
/// `UiSettings::system_fonts` to also use the fonts installed on the machine as fallback.
#[derive(Resource)]
pub struct Fonts {
    pub system: FontSystem,
    pub swash: SwashCache,
    labels: HashMap<u64, Label>,
    frame: u64,
}

struct Label {
    buffer: Buffer,
    size: [f32; 2],
    last_used: u64,
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new(false)
    }
}

impl Fonts {
    /// The built-in fonts, plus the machine's fonts when `system_fonts`.
    pub fn new(system_fonts: bool) -> Self {
        let sources = [FONT_REGULAR, FONT_ITALIC, FONT_SEMIBOLD, FONT_BOLD]
            .into_iter()
            .map(|b| fontdb::Source::Binary(Arc::new(b)));
        let mut system = FontSystem::new_with_fonts(sources);
        if system_fonts {
            system.db_mut().load_system_fonts();
        }
        let db = system.db_mut();
        db.set_sans_serif_family(UI_FONT);
        Self {
            system,
            swash: SwashCache::new(),
            labels: HashMap::new(),
            frame: 0,
        }
    }

    /// Add a font file (TTF/OTF bytes). Use its family name in markup or [`TextSpec::family`].
    pub fn add_font(&mut self, bytes: Vec<u8>) {
        self.system.db_mut().load_font_data(bytes);
    }

    /// Shape a one-off label (map names, painter text) and cache it for a few frames.
    /// Returns the buffer and its size in physical px.
    pub fn label(&mut self, spec: &TextSpec) -> (&Buffer, [f32; 2]) {
        let key = spec.hash_key();
        let frame = self.frame;
        let system = &mut self.system;
        let label = self.labels.entry(key).or_insert_with(|| {
            let mut buffer = Buffer::new(system, spec.metrics());
            spec.apply(&mut buffer);
            buffer.set_size(None, None);
            buffer.shape_until_scroll(system, false);
            let size = buffer_size(&buffer);
            Label {
                buffer,
                size,
                last_used: frame,
            }
        });
        label.last_used = frame;
        (&label.buffer, label.size)
    }

    /// Like [`Fonts::label`], also handing out the rasterizer so the caller can draw the
    /// label's glyphs while holding the buffer.
    pub fn label_and_raster(
        &mut self,
        spec: &TextSpec,
    ) -> (&Buffer, [f32; 2], &mut FontSystem, &mut SwashCache) {
        let key = spec.hash_key();
        self.label(spec);
        let label = &self.labels[&key];
        (&label.buffer, label.size, &mut self.system, &mut self.swash)
    }

    /// Call once per frame: drops labels unused for a while.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        if self.labels.len() > 512 {
            self.labels.retain(|_, l| frame - l.last_used < 120);
        }
    }
}

/// Everything that decides how a piece of text is shaped (sizes in physical px).
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    pub text: String,
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
    pub color: Color,
    pub align: TextAlign,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub uppercase: bool,
    pub rich: bool,
    pub tabular: bool,
    pub wrap: bool,
    pub family: Option<String>,
}

impl Default for TextSpec {
    fn default() -> Self {
        Self {
            text: String::new(),
            size: 16.0,
            weight: 400,
            italic: false,
            color: Color::WHITE,
            align: TextAlign::Left,
            line_height: 1.3,
            letter_spacing: 0.0,
            uppercase: false,
            rich: true,
            tabular: false,
            wrap: true,
            family: None,
        }
    }
}

impl TextSpec {
    pub fn new(text: impl Into<String>, size: f32) -> Self {
        Self {
            text: text.into(),
            size,
            ..Default::default()
        }
    }

    pub fn hash_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut h);
        self.size.to_bits().hash(&mut h);
        self.weight.hash(&mut h);
        self.italic.hash(&mut h);
        self.color.to_srgb8().hash(&mut h);
        (self.align as u8).hash(&mut h);
        self.line_height.to_bits().hash(&mut h);
        self.letter_spacing.to_bits().hash(&mut h);
        self.uppercase.hash(&mut h);
        self.rich.hash(&mut h);
        self.tabular.hash(&mut h);
        self.wrap.hash(&mut h);
        self.family.hash(&mut h);
        h.finish()
    }

    fn metrics(&self) -> Metrics {
        let size = self.size.max(1.0);
        Metrics::new(size, (size * self.line_height).ceil())
    }

    fn attrs(&self) -> Attrs<'_> {
        let family = match &self.family {
            Some(f) => Family::Name(f),
            None => Family::Name(UI_FONT),
        };
        let mut attrs = Attrs::new()
            .family(family)
            .weight(Weight(self.weight.max(100)))
            .color(to_cosmic(self.color));
        if self.italic {
            attrs = attrs.style(FontStyle::Italic);
        }
        if self.letter_spacing != 0.0 {
            attrs = attrs.letter_spacing(self.letter_spacing);
        }
        if self.tabular {
            let mut f = FontFeatures::new();
            f.enable(cosmic_text::FeatureTag::new(b"tnum"));
            attrs = attrs.font_features(f);
        }
        attrs
    }

    /// Set this text on `buffer` (metrics, wrap, spans). Shaping happens on the next
    /// `shape_until_scroll`.
    pub fn apply(&self, buffer: &mut Buffer) {
        buffer.set_metrics(self.metrics());
        buffer.set_wrap(
            if self.wrap {
                Wrap::WordOrGlyph
            } else {
                Wrap::None
            },
        );
        let align = Some(match self.align {
            TextAlign::Left => cosmic_text::Align::Left,
            TextAlign::Center => cosmic_text::Align::Center,
            TextAlign::Right => cosmic_text::Align::Right,
            TextAlign::Justify => cosmic_text::Align::Justified,
        });
        let base = self.attrs();
        let spans = if self.rich {
            parse_markup(&self.text)
        } else {
            vec![(self.text.clone(), SpanStyle::default())]
        };
        let spans: Vec<(String, Attrs)> = spans
            .into_iter()
            .map(|(mut s, st)| {
                if self.uppercase {
                    s = s.to_uppercase();
                }
                let mut a = base.clone();
                if let Some(w) = st.weight {
                    a = a.weight(Weight(w));
                }
                if st.italic {
                    a = a.style(FontStyle::Italic);
                }
                if let Some(c) = st.color {
                    a = a.color(to_cosmic(c));
                }
                if let Some(sz) = st.size {
                    let px = sz * self.size / 16.0;
                    a = a.metrics(Metrics::new(px, (px * self.line_height).ceil()));
                }
                (s, a)
            })
            .collect();
        buffer.set_rich_text(
            spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
            &base,
            Shaping::Advanced,
            align,
        );
    }
}

/// Linear color → cosmic-text's sRGB8 color.
pub fn to_cosmic(c: Color) -> cosmic_text::Color {
    let [r, g, b, a] = c.to_srgb8();
    cosmic_text::Color::rgba(r, g, b, a)
}

/// cosmic-text's sRGB8 color → linear.
pub fn from_cosmic(c: cosmic_text::Color) -> Color {
    Color::srgb8(c.r(), c.g(), c.b(), c.a())
}

/// Width (widest line) and height (all lines) of a shaped buffer, physical px.
pub fn buffer_size(buffer: &Buffer) -> [f32; 2] {
    let mut w = 0.0f32;
    let mut h = 0.0f32;
    for run in buffer.layout_runs() {
        w = w.max(run.line_w);
        h = h.max(run.line_top + run.line_height);
    }
    [w.ceil(), h.ceil()]
}

/// Style of one markup span (unset fields come from the text).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpanStyle {
    pub weight: Option<u16>,
    pub italic: bool,
    pub color: Option<Color>,
    /// Size relative to 16 (scaled with the text's own size).
    pub size: Option<f32>,
}

/// Split `[b]`, `[i]`, `[color=..]`/`[c=..]`, `[size=..]` markup into styled spans.
/// Unknown tags stay as written; `[[` is a literal `[`.
pub fn parse_markup(src: &str) -> Vec<(String, SpanStyle)> {
    let mut out: Vec<(String, SpanStyle)> = Vec::new();
    let mut stack: Vec<(String, SpanStyle)> = Vec::new();
    let mut cur = SpanStyle::default();
    let mut text = String::new();
    let mut rest = src;
    let flush = |text: &mut String, cur: SpanStyle, out: &mut Vec<(String, SpanStyle)>| {
        if !text.is_empty() {
            out.push((std::mem::take(text), cur));
        }
    };
    while let Some(i) = rest.find('[') {
        text.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with("[[") {
            text.push('[');
            rest = &rest[2..];
            continue;
        }
        let Some(end) = rest.find(']') else {
            break;
        };
        let tag = &rest[1..end];
        let lower = tag.to_ascii_lowercase();
        let (name, value) = match lower.split_once('=') {
            Some((n, _)) => (n.trim().to_string(), Some(tag[n.len() + 1..].trim().to_string())),
            None => (lower.trim().to_string(), None),
        };
        let mut handled = true;
        if let Some(closing) = name.strip_prefix('/') {
            let closing = match closing {
                "c" => "color",
                other => other,
            };
            if let Some(pos) = stack.iter().rposition(|(n, _)| n == closing) {
                flush(&mut text, cur, &mut out);
                cur = stack[pos].1;
                stack.truncate(pos);
            } else {
                handled = false;
            }
        } else {
            let mut next = cur;
            let key = match name.as_str() {
                "b" => {
                    next.weight = Some(700);
                    "b"
                }
                "sb" => {
                    next.weight = Some(600);
                    "sb"
                }
                "i" => {
                    next.italic = true;
                    "i"
                }
                "color" | "c" => match value.as_deref().map(Color::parse) {
                    Some(Ok(c)) => {
                        next.color = Some(c);
                        "color"
                    }
                    _ => "",
                },
                "size" => match value.as_deref().and_then(|v| v.parse::<f32>().ok()) {
                    Some(s) => {
                        next.size = Some(s);
                        "size"
                    }
                    None => "",
                },
                _ => "",
            };
            if key.is_empty() {
                handled = false;
            } else {
                flush(&mut text, cur, &mut out);
                stack.push((key.to_string(), cur));
                cur = next;
            }
        }
        if handled {
            rest = &rest[end + 1..];
        } else {
            text.push('[');
            rest = &rest[1..];
        }
    }
    text.push_str(rest);
    flush(&mut text, cur, &mut out);
    if out.is_empty() {
        out.push((String::new(), SpanStyle::default()));
    }
    out
}

/// Plain text with markup removed (for accessibility, the AI bridge and character counts).
pub fn strip_markup(src: &str) -> String {
    parse_markup(src).into_iter().map(|(s, _)| s).collect()
}

/// Shaped text of one node, re-shaped only when its spec or width changes.
pub struct TextEntry {
    pub spec_key: u64,
    pub buffer: Buffer,
    /// Width the buffer is currently shaped at (None = unbounded).
    pub width: Option<f32>,
    /// Measure results by available width (bits), to answer layout quickly.
    measured: Vec<(Option<u32>, [f32; 2])>,
    /// Characters in each line before it (for the typewriter effect).
    pub line_chars: Vec<usize>,
    pub total_chars: usize,
    pub wrap: bool,
}

impl TextEntry {
    fn new(system: &mut FontSystem, spec: &TextSpec) -> Self {
        let mut buffer = Buffer::new(system, spec.metrics());
        spec.apply(&mut buffer);
        let mut e = Self {
            spec_key: spec.hash_key(),
            buffer,
            width: Some(-1.0),
            measured: Vec::new(),
            line_chars: Vec::new(),
            total_chars: 0,
            wrap: spec.wrap,
        };
        e.count_chars(spec);
        e
    }

    fn count_chars(&mut self, spec: &TextSpec) {
        let plain = if spec.rich {
            strip_markup(&spec.text)
        } else {
            spec.text.clone()
        };
        self.line_chars.clear();
        let mut n = 0;
        for line in plain.split('\n') {
            self.line_chars.push(n);
            n += line.chars().count() + 1;
        }
        self.total_chars = plain.chars().count();
    }

    /// Shape for `width` (None = unbounded) if not already.
    pub fn shape(&mut self, system: &mut FontSystem, width: Option<f32>) {
        let width = if self.wrap { width } else { None };
        if self.width == width {
            return;
        }
        self.width = width;
        self.buffer.set_size(width, None);
        self.buffer.shape_until_scroll(system, false);
    }

    /// Size for an available width, physical px.
    pub fn measure(&mut self, system: &mut FontSystem, width: Option<f32>) -> [f32; 2] {
        let width = if self.wrap { width.map(|w| w.max(1.0)) } else { None };
        let key = width.map(f32::to_bits);
        if let Some((_, s)) = self.measured.iter().find(|(k, _)| *k == key) {
            return *s;
        }
        self.shape(system, width);
        let size = buffer_size(&self.buffer);
        if self.measured.len() > 8 {
            self.measured.remove(0);
        }
        self.measured.push((key, size));
        size
    }
}

/// Shaped text per entity.
#[derive(Resource, Default)]
pub struct TextLayouts {
    pub entries: HashMap<Entity, TextEntry>,
}

impl TextLayouts {
    /// Make sure `entity`'s buffer matches `spec`. Returns true when the text changed (layout
    /// must be recomputed).
    pub fn sync(&mut self, system: &mut FontSystem, entity: Entity, spec: &TextSpec) -> bool {
        let key = spec.hash_key();
        match self.entries.get_mut(&entity) {
            Some(e) if e.spec_key == key => false,
            Some(e) => {
                spec.apply(&mut e.buffer);
                e.spec_key = key;
                e.width = Some(-1.0);
                e.measured.clear();
                e.wrap = spec.wrap;
                e.count_chars(spec);
                true
            }
            None => {
                self.entries.insert(entity, TextEntry::new(system, spec));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_spans() {
        let spans = parse_markup("Buy [b]Iron[/b] for [color=#e8b65a]12 gold[/color]!");
        let texts: Vec<_> = spans.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, ["Buy ", "Iron", " for ", "12 gold", "!"]);
        assert_eq!(spans[1].1.weight, Some(700));
        assert!(spans[3].1.color.is_some());
        assert_eq!(spans[4].1, SpanStyle::default());
    }

    #[test]
    fn markup_nesting_and_unknown_tags() {
        let spans = parse_markup("[i]a [b]b[/b] c[/i] [x] [[lit] [color=nope]z");
        assert_eq!(strip_markup("[i]a [b]b[/b] c[/i]"), "a b c");
        assert!(spans[0].1.italic);
        assert!(spans[1].1.italic && spans[1].1.weight == Some(700));
        let tail: String = spans.iter().skip(3).map(|(t, _)| t.as_str()).collect();
        assert_eq!(tail, " [x] [lit] [color=nope]z");
    }

    #[test]
    fn shapes_and_wraps_with_embedded_font() {
        let mut fonts = Fonts::new(false);
        let mut layouts = TextLayouts::default();
        let e = Entity::from_raw_u32(1).unwrap();
        let spec = TextSpec::new("The quick brown fox jumps over the lazy dog", 20.0);
        assert!(layouts.sync(&mut fonts.system, e, &spec));
        assert!(!layouts.sync(&mut fonts.system, e, &spec));
        let entry = layouts.entries.get_mut(&e).unwrap();
        let [w, h] = entry.measure(&mut fonts.system, None);
        assert!(w > 300.0 && w < 500.0, "one line is {w} px wide");
        assert!(h > 20.0 && h < 30.0, "one line is {h} px tall");
        let [w2, h2] = entry.measure(&mut fonts.system, Some(150.0));
        assert!(w2 <= 150.0 && h2 > h * 2.5, "wrapped to {w2}x{h2}");
        // Glyphs come from the embedded font.
        let glyphs: usize = entry.buffer.layout_runs().map(|r| r.glyphs.len()).sum();
        assert!(glyphs >= 35);
    }

    #[test]
    fn bold_is_wider_than_regular() {
        let mut fonts = Fonts::new(false);
        let regular = TextSpec::new("Inventory", 24.0);
        let bold = TextSpec {
            weight: 700,
            ..regular.clone()
        };
        let (_, a) = fonts.label(&regular);
        let a = a[0];
        let (_, b) = fonts.label(&bold);
        assert!(b[0] > a, "bold {} vs regular {a}", b[0]);
    }
}
