//! [`Style`]: layout (flexbox/grid) and looks of one UI node, CSS-like and plain JSON.
//!
//! Lengths are logical pixels (scaled by [`UiScale`](crate::UiScale)) or percentages:
//! `{"width": 320, "height": "50%", "padding": [12, 16], "radius": 10}`.
//! [`StylePatch`] is the same set of fields, all optional: themes, classes and inline styles
//! are patches applied in order on top of [`Style::default`].

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::Color;

/// A length: `auto`, logical pixels or a percentage of the parent.
///
/// JSON: `12` (pixels), `"50%"`, `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Val {
    #[default]
    Auto,
    Px(f32),
    Percent(f32),
}

impl Val {
    fn to_dimension(self, scale: f32) -> taffy::Dimension {
        match self {
            Val::Auto => taffy::Dimension::auto(),
            Val::Px(v) => taffy::Dimension::length(v * scale),
            Val::Percent(p) => taffy::Dimension::percent(p / 100.0),
        }
    }

    fn to_lpa(self, scale: f32) -> taffy::LengthPercentageAuto {
        match self {
            Val::Auto => taffy::LengthPercentageAuto::auto(),
            Val::Px(v) => taffy::LengthPercentageAuto::length(v * scale),
            Val::Percent(p) => taffy::LengthPercentageAuto::percent(p / 100.0),
        }
    }

    fn to_lp(self, scale: f32) -> taffy::LengthPercentage {
        match self {
            Val::Auto => taffy::LengthPercentage::length(0.0),
            Val::Px(v) => taffy::LengthPercentage::length(v * scale),
            Val::Percent(p) => taffy::LengthPercentage::percent(p / 100.0),
        }
    }
}

impl From<f32> for Val {
    fn from(v: f32) -> Self {
        Val::Px(v)
    }
}

impl From<i32> for Val {
    fn from(v: i32) -> Self {
        Val::Px(v as f32)
    }
}

/// `pct(50.0)` = `Val::Percent(50.0)`.
pub fn pct(p: f32) -> Val {
    Val::Percent(p)
}

impl Serialize for Val {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Val::Auto => s.serialize_str("auto"),
            Val::Px(v) => s.serialize_f32(*v),
            Val::Percent(p) => s.serialize_str(&format!("{p}%")),
        }
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Val;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a length: pixels (12), a percentage (\"50%\") or \"auto\"")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Val, E> {
                Ok(Val::Px(v as f32))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Val, E> {
                Ok(Val::Px(v as f32))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Val, E> {
                Ok(Val::Px(v as f32))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Val, E> {
                let v = v.trim();
                if v == "auto" {
                    return Ok(Val::Auto);
                }
                if let Some(p) = v.strip_suffix('%') {
                    return p.trim().parse().map(Val::Percent).map_err(|_| {
                        E::custom(format!("\"{v}\" is not a percentage like \"50%\""))
                    });
                }
                v.trim_end_matches("px")
                    .trim()
                    .parse()
                    .map(Val::Px)
                    .map_err(|_| {
                        E::custom(format!(
                            "\"{v}\" is not a length: use 12, \"50%\" or \"auto\""
                        ))
                    })
            }
        }
        d.deserialize_any(V)
    }
}

/// Four sides (top, right, bottom, left), CSS order.
///
/// JSON: `8` (all), `[8, 16]` (vertical, horizontal), `[t, r, b, l]`, or
/// `{"top": 8, "left": 4}`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: Val,
    pub right: Val,
    pub bottom: Val,
    pub left: Val,
}

impl Edges {
    pub const ZERO: Edges = Edges {
        top: Val::Px(0.0),
        right: Val::Px(0.0),
        bottom: Val::Px(0.0),
        left: Val::Px(0.0),
    };

    pub fn all(v: impl Into<Val>) -> Self {
        let v = v.into();
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub fn xy(x: impl Into<Val>, y: impl Into<Val>) -> Self {
        let (x, y) = (x.into(), y.into());
        Self {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }

    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top: Val::Px(top),
            right: Val::Px(right),
            bottom: Val::Px(bottom),
            left: Val::Px(left),
        }
    }

    fn to_lp(self, scale: f32) -> taffy::Rect<taffy::LengthPercentage> {
        taffy::Rect {
            left: self.left.to_lp(scale),
            right: self.right.to_lp(scale),
            top: self.top.to_lp(scale),
            bottom: self.bottom.to_lp(scale),
        }
    }

    fn to_lpa(self, scale: f32) -> taffy::Rect<taffy::LengthPercentageAuto> {
        taffy::Rect {
            left: self.left.to_lpa(scale),
            right: self.right.to_lpa(scale),
            top: self.top.to_lpa(scale),
            bottom: self.bottom.to_lpa(scale),
        }
    }
}

impl From<f32> for Edges {
    fn from(v: f32) -> Self {
        Edges::all(v)
    }
}

impl From<[f32; 2]> for Edges {
    fn from([y, x]: [f32; 2]) -> Self {
        Edges::xy(x, y)
    }
}

impl From<[f32; 4]> for Edges {
    fn from([t, r, b, l]: [f32; 4]) -> Self {
        Edges::new(t, r, b, l)
    }
}

impl Serialize for Edges {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.top, self.right, self.bottom, self.left].serialize(s)
    }
}

impl<'de> Deserialize<'de> for Edges {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            One(Val),
            Many(Vec<Val>),
            Named {
                #[serde(default)]
                top: Option<Val>,
                #[serde(default)]
                right: Option<Val>,
                #[serde(default)]
                bottom: Option<Val>,
                #[serde(default)]
                left: Option<Val>,
            },
        }
        let zero = Val::Px(0.0);
        Ok(match Repr::deserialize(d)? {
            Repr::One(v) => Edges::all(v),
            Repr::Many(v) => match v.as_slice() {
                [a] => Edges::all(*a),
                [y, x] => Edges::xy(*x, *y),
                [t, x, b] => Edges {
                    top: *t,
                    right: *x,
                    bottom: *b,
                    left: *x,
                },
                [t, r, b, l] => Edges {
                    top: *t,
                    right: *r,
                    bottom: *b,
                    left: *l,
                },
                _ => return Err(de::Error::custom("edges need 1, 2, 3 or 4 values")),
            },
            Repr::Named {
                top,
                right,
                bottom,
                left,
            } => Edges {
                top: top.unwrap_or(zero),
                right: right.unwrap_or(zero),
                bottom: bottom.unwrap_or(zero),
                left: left.unwrap_or(zero),
            },
        })
    }
}

/// Corner radii in logical pixels: top-left, top-right, bottom-right, bottom-left.
///
/// JSON: `10` or `[10, 10, 0, 0]`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Corners(pub [f32; 4]);

impl Corners {
    pub fn all(r: f32) -> Self {
        Self([r; 4])
    }

    pub fn top(r: f32) -> Self {
        Self([r, r, 0.0, 0.0])
    }

    pub fn bottom(r: f32) -> Self {
        Self([0.0, 0.0, r, r])
    }

    pub fn left(r: f32) -> Self {
        Self([r, 0.0, 0.0, r])
    }

    pub fn right(r: f32) -> Self {
        Self([0.0, r, r, 0.0])
    }
}

impl From<f32> for Corners {
    fn from(r: f32) -> Self {
        Corners::all(r)
    }
}

impl Serialize for Corners {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.iter().all(|r| *r == self.0[0]) {
            s.serialize_f32(self.0[0])
        } else {
            self.0.serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for Corners {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            One(f32),
            Four([f32; 4]),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::One(r) => Corners::all(r),
            Repr::Four(r) => Corners(r),
        })
    }
}

/// How a node takes part in layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Display {
    #[default]
    Flex,
    Grid,
    /// Not laid out, not drawn, not interactive.
    None,
}

/// `relative` (in the flow) or `absolute` (placed with left/top/right/bottom in the parent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    Relative,
    Absolute,
}

/// Main axis of a flex container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

/// Distribution along the main axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Alignment on the cross axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    /// The parent's `align_items` (for `align_self`), or stretch (for `align_items`).
    #[default]
    Auto,
    Start,
    End,
    Center,
    Stretch,
    Baseline,
}

/// What happens to children that don't fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overflow {
    #[default]
    Visible,
    /// Clip children to this node's box.
    Hidden,
    /// Clip and scroll with the mouse wheel, drag or gamepad.
    Scroll,
}

/// Whether a node takes pointer input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    /// Interactive nodes and nodes with a background catch the pointer; others let it through.
    #[default]
    Auto,
    /// Always catch the pointer (keeps clicks from reaching the game).
    Block,
    /// Never catch it (decorations, labels over buttons).
    Ignore,
}

/// A background fill.
///
/// JSON: `"#101826e0"`, `{"linear": {"from": "#1a2233", "to": "#0d121c", "angle_deg": 90}}`,
/// `{"radial": {"inner": "#e8b65a40", "outer": "transparent"}}`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    #[default]
    None,
    /// `angle_deg` 0 runs left → right, 90 top → bottom.
    Linear {
        from: Color,
        to: Color,
        #[serde(default)]
        angle_deg: f32,
    },
    /// From the center (`inner`) to the farthest corner (`outer`).
    Radial { inner: Color, outer: Color },
    /// One color (JSON: just the color).
    #[serde(untagged)]
    Solid(Color),
}

impl Fill {
    pub fn is_visible(&self) -> bool {
        match self {
            Fill::None => false,
            Fill::Solid(c) => c.a > 0.0,
            Fill::Linear { from, to, .. } => from.a > 0.0 || to.a > 0.0,
            Fill::Radial { inner, outer } => inner.a > 0.0 || outer.a > 0.0,
        }
    }

    pub fn vertical(top: Color, bottom: Color) -> Self {
        Fill::Linear {
            from: top,
            to: bottom,
            angle_deg: 90.0,
        }
    }

    /// The same look expressed as `like`'s kind where possible (a solid color as a gradient
    /// of one color, no fill as a transparent version of `like`).
    fn as_kind_of(&self, like: &Fill) -> Option<Fill> {
        use Fill::*;
        let solid = |c: Color| match like {
            Linear { angle_deg, .. } => Some(Linear {
                from: c,
                to: c,
                angle_deg: *angle_deg,
            }),
            Radial { .. } => Some(Radial { inner: c, outer: c }),
            Solid(_) => Some(Solid(c)),
            None => Option::None,
        };
        match self {
            Solid(c) => solid(*c),
            None => match like {
                Linear {
                    from,
                    to,
                    angle_deg,
                } => Some(Linear {
                    from: from.with_alpha(0.0),
                    to: to.with_alpha(0.0),
                    angle_deg: *angle_deg,
                }),
                Radial { inner, outer } => Some(Radial {
                    inner: inner.with_alpha(0.0),
                    outer: outer.with_alpha(0.0),
                }),
                Solid(c) => Some(Solid(c.with_alpha(0.0))),
                None => Some(None),
            },
            _ => Option::None,
        }
    }

    /// Whether two fills look the same within `eps` per channel.
    pub fn approx_eq(&self, other: &Fill, eps: f32) -> bool {
        use Fill::*;
        let c = |a: &Color, b: &Color| {
            (a.r - b.r).abs() < eps
                && (a.g - b.g).abs() < eps
                && (a.b - b.b).abs() < eps
                && (a.a - b.a).abs() < eps
        };
        let a = self.as_kind_of(other).unwrap_or(*self);
        match (&a, other) {
            (None, None) => true,
            (Solid(x), Solid(y)) => c(x, y),
            (
                Linear {
                    from: a0, to: a1, ..
                },
                Linear {
                    from: b0, to: b1, ..
                },
            ) => c(a0, b0) && c(a1, b1),
            (
                Radial {
                    inner: a0,
                    outer: a1,
                },
                Radial {
                    inner: b0,
                    outer: b1,
                },
            ) => c(a0, b0) && c(a1, b1),
            _ => !self.is_visible() && !other.is_visible(),
        }
    }

    /// Interpolate two fills. Solid colors and no fill blend into gradients; a linear and a
    /// radial gradient switch at once.
    pub fn lerp(&self, other: &Fill, t: f32) -> Fill {
        use Fill::*;
        match (self, other) {
            (None, None) => None,
            (Solid(a), Solid(b)) => Solid(a.lerp(*b, t)),
            (
                Linear {
                    from: a0,
                    to: a1,
                    angle_deg: aa,
                },
                Linear {
                    from: b0,
                    to: b1,
                    angle_deg: ba,
                },
            ) => Linear {
                from: a0.lerp(*b0, t),
                to: a1.lerp(*b1, t),
                angle_deg: aa + (ba - aa) * t,
            },
            (
                Radial {
                    inner: a0,
                    outer: a1,
                },
                Radial {
                    inner: b0,
                    outer: b1,
                },
            ) => Radial {
                inner: a0.lerp(*b0, t),
                outer: a1.lerp(*b1, t),
            },
            (a, b) => {
                if let Some(a2) = a.as_kind_of(b) {
                    a2.lerp(b, t)
                } else if let Some(b2) = b.as_kind_of(a) {
                    // Fading out to nothing (or to a solid): blend in `a`'s kind, and land on
                    // the exact target at the end.
                    if t >= 0.999 { *b } else { a.lerp(&b2, t) }
                } else {
                    *b
                }
            }
        }
    }
}

impl From<Color> for Fill {
    fn from(c: Color) -> Self {
        Fill::Solid(c)
    }
}

/// A soft drop shadow (or glow, with zero offset) behind the node's rounded box.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    /// Transparent = no shadow.
    pub color: Color,
    /// Blur radius, logical px.
    pub blur: f32,
    /// Offset, logical px (y down).
    pub offset: [f32; 2],
    /// Grow the box before blurring, logical px.
    pub spread: f32,
}

impl Shadow {
    pub fn new(color: Color, blur: f32, offset_y: f32) -> Self {
        Self {
            color,
            blur,
            offset: [0.0, offset_y],
            spread: 0.0,
        }
    }

    /// A glow: no offset, slight spread.
    pub fn glow(color: Color, blur: f32) -> Self {
        Self {
            color,
            blur,
            offset: [0.0, 0.0],
            spread: 1.0,
        }
    }

    fn lerp(&self, o: &Shadow, t: f32) -> Shadow {
        let base = if self.color.a <= 0.0 {
            Shadow {
                color: o.color.with_alpha(0.0),
                ..*o
            }
        } else {
            *self
        };
        let to = if o.color.a <= 0.0 {
            Shadow {
                color: base.color.with_alpha(0.0),
                ..base
            }
        } else {
            *o
        };
        Shadow {
            color: base.color.lerp(to.color, t),
            blur: base.blur + (to.blur - base.blur) * t,
            offset: [
                base.offset[0] + (to.offset[0] - base.offset[0]) * t,
                base.offset[1] + (to.offset[1] - base.offset[1]) * t,
            ],
            spread: base.spread + (to.spread - base.spread) * t,
        }
    }
}

/// Corner geometry of a box: the vector shape every fill, border, shadow and bevel follows.
///
/// The corner size is the node's `radius`. JSON: `"round"`, `"chamfer"`, `"scoop"`, `"notch"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CornerShape {
    /// Rounded corners (a quarter circle).
    #[default]
    Round,
    /// Cut at 45° (sci-fi plates, Art Deco cartouches).
    Chamfer,
    /// A concave quarter circle bitten out of each corner (Victorian plaques, tickets).
    Scoop,
    /// A square step cut out of each corner (frames, engraved plates).
    Notch,
}

impl CornerShape {
    pub fn gpu(self) -> u32 {
        match self {
            CornerShape::Round => 0,
            CornerShape::Chamfer => 1,
            CornerShape::Scoop => 2,
            CornerShape::Notch => 3,
        }
    }
}

/// A procedural surface laid over the background fill, so a panel reads as a material from
/// the game's world instead of a flat color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialKind {
    #[default]
    None,
    /// Parchment: cloudy mottling, fibres, flecks and darkened, aged edges.
    Paper,
    /// Planed wood: warped grain lines and pores along the length.
    Wood,
    /// Pebbled leather with a worn, darker rim.
    Leather,
    /// Brushed metal: fine streaks along the width and a broad soft sheen.
    Metal,
    /// Deep lacquer: very faint depth and a glossy band of reflected light near the top.
    Lacquer,
    /// Woven linen or canvas.
    Linen,
    /// Marble or slate: soft clouds and thin veins.
    Stone,
    /// Holographic smoked glass: fine scanlines, a faint grid, a cool glare across the top
    /// left and lit edges.
    Glass,
}

impl MaterialKind {
    pub fn gpu(self) -> f32 {
        match self {
            MaterialKind::None => 0.0,
            MaterialKind::Paper => 1.0,
            MaterialKind::Wood => 2.0,
            MaterialKind::Leather => 3.0,
            MaterialKind::Metal => 4.0,
            MaterialKind::Lacquer => 5.0,
            MaterialKind::Linen => 6.0,
            MaterialKind::Stone => 7.0,
            MaterialKind::Glass => 8.0,
        }
    }
}

/// [`MaterialKind`] with its strength and grain size.
///
/// JSON: `{"kind": "paper", "strength": 0.8, "scale": 1.2, "seed": 3}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub kind: MaterialKind,
    /// 0 = plain fill, 1 = the full look, more = exaggerated.
    pub strength: f32,
    /// Size of the grain, mottling and fibres (1 = designed size).
    pub scale: f32,
    /// Picks a different patch of the pattern, so neighbouring panels don't match.
    pub seed: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            kind: MaterialKind::None,
            strength: 1.0,
            scale: 1.0,
            seed: 0.0,
        }
    }
}

impl Material {
    pub fn new(kind: MaterialKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }

    pub fn strength(mut self, s: f32) -> Self {
        self.strength = s;
        self
    }

    pub fn scale(mut self, s: f32) -> Self {
        self.scale = s;
        self
    }

    pub fn seed(mut self, s: f32) -> Self {
        self.seed = s;
        self
    }
}

impl From<MaterialKind> for Material {
    fn from(k: MaterialKind) -> Self {
        Material::new(k)
    }
}

/// Raised edges: light catching the top-left rim and shade on the bottom-right, plus an
/// optional engraved hairline inside the edge (the gold inner line of a framed plate).
///
/// JSON: `{"width": 3, "light": "#ffffff40", "dark": "#00000060", "line": "#e8b65a80",
/// "line_width": 1, "line_inset": 5}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bevel {
    /// How far the rounded rim reaches in, logical px; 0 = no rim.
    pub width: f32,
    /// Color added where the rim faces the light (up and to the left).
    pub light: Color,
    /// Color laid over where it faces away.
    pub dark: Color,
    /// The inner hairline; transparent = none.
    pub line: Color,
    pub line_width: f32,
    /// Distance of the hairline from the outer edge, logical px.
    pub line_inset: f32,
}

impl Default for Bevel {
    fn default() -> Self {
        Self {
            width: 0.0,
            light: Color::TRANSPARENT,
            dark: Color::TRANSPARENT,
            line: Color::TRANSPARENT,
            line_width: 1.0,
            line_inset: 4.0,
        }
    }
}

impl Bevel {
    /// A raised rim `width` px wide.
    pub fn raised(width: f32, light: f32, dark: f32) -> Self {
        Self {
            width,
            light: Color::WHITE.with_alpha(light),
            dark: Color::BLACK.with_alpha(dark),
            ..Default::default()
        }
    }

    /// A sunken well (lit from inside: shade on top, light on the bottom).
    pub fn sunken(width: f32, light: f32, dark: f32) -> Self {
        Self {
            width: -width,
            light: Color::WHITE.with_alpha(light),
            dark: Color::BLACK.with_alpha(dark),
            ..Default::default()
        }
    }

    /// Add the engraved hairline.
    pub fn with_line(mut self, color: Color, width: f32, inset: f32) -> Self {
        self.line = color;
        self.line_width = width;
        self.line_inset = inset;
        self
    }

    pub fn is_visible(&self) -> bool {
        (self.width != 0.0 && (self.light.a > 0.0 || self.dark.a > 0.0))
            || (self.line.a > 0.0 && self.line_width > 0.0)
    }
}

/// A nine-slice frame drawn over the box: an icon (SVG) cut into corners that keep their
/// size, edges that stretch along the sides and a middle that stretches both ways (or is
/// left out). Ornate brass corners, carved wood, riveted iron.
///
/// JSON: `{"image": "frame_brass", "slice": 24, "width": 18, "outset": 4}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Frame {
    /// Icon name; empty = no frame.
    pub image: String,
    /// Size of the corner pieces in the image's own units (its SVG viewBox).
    pub slice: f32,
    /// Size the corner pieces are drawn at, logical px.
    pub width: f32,
    /// How far the frame reaches outside the box, logical px.
    pub outset: f32,
    /// Draw the stretched middle too.
    pub fill_center: bool,
    /// Multiplies the image's colors.
    pub tint: Color,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            image: String::new(),
            slice: 16.0,
            width: 16.0,
            outset: 0.0,
            fill_center: false,
            tint: Color::WHITE,
        }
    }
}

impl Frame {
    pub fn new(image: &str, slice: f32, width: f32) -> Self {
        Self {
            image: image.into(),
            slice,
            width,
            ..Default::default()
        }
    }

    pub fn outset(mut self, o: f32) -> Self {
        self.outset = o;
        self
    }

    pub fn tint(mut self, c: Color) -> Self {
        self.tint = c;
        self
    }
}

/// How a transition moves from one state to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// Exponential approach (fast start, long soft landing); `transition` is the time
    /// constant. The engine's original behaviour.
    #[default]
    Auto,
    Linear,
    /// Cubic ease-out: quick start, gentle stop. Most UI motion.
    Out,
    /// Cubic ease-in-out: for things that travel across the screen.
    InOut,
    /// Ease-out that overshoots a little and settles (buttons that pop, cards that land).
    Back,
    /// A damped spring that wobbles once or twice (rewards, notifications).
    Spring,
}

impl Ease {
    /// Eased progress for `t` in 0..1.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Auto | Ease::Out => 1.0 - (1.0 - t).powi(3),
            Ease::Linear => t,
            Ease::InOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) * 0.5
                }
            }
            Ease::Back => {
                let (c1, c3) = (1.70158, 2.70158);
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
            Ease::Spring => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - (-6.5 * t).exp() * (t * 11.0).cos()
                }
            }
        }
    }
}

/// How a node arrives when it is spawned: it starts faded, moved and scaled by these
/// amounts and eases into place after `delay` seconds. Give a row of children growing
/// delays for a staggered entrance.
///
/// JSON: `{"duration": 0.35, "delay": 0.05, "opacity": 0, "translate": [0, 24], "scale": 0.96,
/// "ease": "back"}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Enter {
    /// Seconds; 0 = appear at once.
    pub duration: f32,
    pub delay: f32,
    /// Opacity at the start (multiplies the node's own).
    pub opacity: f32,
    /// Offset at the start, logical px.
    pub translate: [f32; 2],
    /// Scale at the start (multiplies the node's own).
    pub scale: f32,
    pub ease: Ease,
}

impl Default for Enter {
    fn default() -> Self {
        Self {
            duration: 0.0,
            delay: 0.0,
            opacity: 0.0,
            translate: [0.0, 0.0],
            scale: 1.0,
            ease: Ease::Out,
        }
    }
}

impl Enter {
    /// Fade in while rising `dy` px (negative: falling into place from above).
    pub fn rise(duration: f32, dy: f32) -> Self {
        Self {
            duration,
            translate: [0.0, dy],
            ..Default::default()
        }
    }

    /// Fade in while sliding `dx` px sideways.
    pub fn slide(duration: f32, dx: f32) -> Self {
        Self {
            duration,
            translate: [dx, 0.0],
            ..Default::default()
        }
    }

    /// Fade in while growing from `scale`, with a little overshoot.
    pub fn pop(duration: f32, scale: f32) -> Self {
        Self {
            duration,
            scale,
            ease: Ease::Back,
            ..Default::default()
        }
    }

    pub fn delay(mut self, d: f32) -> Self {
        self.delay = d;
        self
    }

    pub fn ease(mut self, e: Ease) -> Self {
        self.ease = e;
        self
    }

    /// The style as the entrance starts.
    pub fn start_of(&self, s: &Style) -> Style {
        Style {
            opacity: s.opacity * self.opacity,
            translate: [
                s.translate[0] + self.translate[0],
                s.translate[1] + self.translate[1],
            ],
            scale: s.scale * self.scale,
            ..s.clone()
        }
    }
}

/// Conversions accepted by [`StylePatch`]'s builder methods, so `.width(64.0)`,
/// `.padding([8.0, 12.0])`, `.font_weight(600)` and `.background(color)` all work with plain
/// literals.
pub trait StyleValue<T> {
    fn into_style(self) -> T;
}

impl<T> StyleValue<T> for T {
    fn into_style(self) -> T {
        self
    }
}

macro_rules! style_value {
    ($($from:ty => $to:ty : |$v:ident| $e:expr;)*) => {
        $(impl StyleValue<$to> for $from {
            fn into_style(self) -> $to {
                let $v = self;
                $e
            }
        })*
    };
}

style_value! {
    f64 => f32: |v| v as f32;
    i32 => f32: |v| v as f32;
    i32 => u16: |v| v.clamp(0, u16::MAX as i32) as u16;
    f32 => Val: |v| Val::Px(v);
    f64 => Val: |v| Val::Px(v as f32);
    i32 => Val: |v| Val::Px(v as f32);
    f32 => Edges: |v| Edges::all(v);
    f64 => Edges: |v| Edges::all(v as f32);
    i32 => Edges: |v| Edges::all(v as f32);
    Val => Edges: |v| Edges::all(v);
    [f64; 2] => Edges: |v| Edges::xy(v[1] as f32, v[0] as f32);
    [f32; 2] => Edges: |v| Edges::xy(v[1], v[0]);
    [f64; 4] => Edges: |v| Edges::new(v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32);
    [f32; 4] => Edges: |v| Edges::new(v[0], v[1], v[2], v[3]);
    f32 => Corners: |v| Corners::all(v);
    f64 => Corners: |v| Corners::all(v as f32);
    [f64; 4] => Corners: |v| Corners([v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
    [f64; 2] => [f32; 2]: |v| [v[0] as f32, v[1] as f32];
    Color => Fill: |v| Fill::Solid(v);
    MaterialKind => Material: |v| Material::new(v);
    &str => String: |v| v.to_string();
}

macro_rules! define_style {
    ($( $(#[$doc:meta])* $field:ident : $ty:ty = $default:expr ),* $(,)?) => {
        /// Layout and looks of a UI node. See the module docs for units and JSON forms.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct Style {
            $( $(#[$doc])* pub $field: $ty, )*
        }

        impl Default for Style {
            fn default() -> Self {
                Self { $( $field: $default, )* }
            }
        }

        /// Any subset of [`Style`]'s fields. Applied in order: theme class, then inline.
        #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
        #[serde(default)]
        pub struct StylePatch {
            $( $(#[$doc])* #[serde(skip_serializing_if = "Option::is_none")] pub $field: Option<$ty>, )*
        }

        impl StylePatch {
            /// Write the fields this patch sets into `style`.
            pub fn apply(&self, style: &mut Style) {
                $( if let Some(v) = &self.$field { style.$field = v.clone(); } )*
            }

            /// Take every field `other` sets.
            pub fn merge(&mut self, other: &StylePatch) {
                $( if other.$field.is_some() { self.$field = other.$field.clone(); } )*
            }

            /// `self` with `other` on top.
            pub fn merged(mut self, other: &StylePatch) -> Self {
                self.merge(other);
                self
            }

            pub fn is_empty(&self) -> bool {
                true $( && self.$field.is_none() )*
            }

            $(
                $(#[$doc])*
                pub fn $field(mut self, v: impl StyleValue<$ty>) -> Self {
                    self.$field = Some(v.into_style());
                    self
                }
            )*
        }
    };
}

define_style! {
    /// `flex` (default), `grid` or `none` (hidden).
    display: Display = Display::Flex,
    /// `relative` or `absolute`.
    position: Position = Position::Relative,
    /// Flex main axis.
    direction: Direction = Direction::Row,
    /// Wrap children onto new lines.
    wrap: bool = false,
    /// Main-axis distribution.
    justify: Justify = Justify::Start,
    /// Cross-axis alignment of children.
    align_items: Align = Align::Auto,
    /// Cross-axis alignment of this node (overrides the parent's `align_items`).
    align_self: Align = Align::Auto,
    /// Alignment of wrapped lines.
    align_content: Align = Align::Auto,
    /// Share of free space this node takes.
    grow: f32 = 0.0,
    /// Share of overflow this node gives up.
    shrink: f32 = 1.0,
    /// Initial main size before growing/shrinking.
    basis: Val = Val::Auto,
    width: Val = Val::Auto,
    height: Val = Val::Auto,
    min_width: Val = Val::Auto,
    min_height: Val = Val::Auto,
    max_width: Val = Val::Auto,
    max_height: Val = Val::Auto,
    /// Offsets for `absolute` (and relative nudges).
    left: Val = Val::Auto,
    right: Val = Val::Auto,
    top: Val = Val::Auto,
    bottom: Val = Val::Auto,
    margin: Edges = Edges::ZERO,
    padding: Edges = Edges::ZERO,
    /// Space between children, logical px.
    gap: f32 = 0.0,
    /// Width / height; 0 = free.
    aspect_ratio: f32 = 0.0,
    overflow: Overflow = Overflow::Visible,
    /// Grid: number of equal columns (0 = from content).
    grid_columns: u16 = 0,
    /// Background fill.
    background: Fill = Fill::None,
    /// Border thickness, logical px (drawn inside the box).
    border_width: f32 = 0.0,
    border_color: Color = Color::TRANSPARENT,
    /// Corner radii, logical px.
    radius: Corners = Corners([0.0; 4]),
    /// Drop shadow / glow.
    shadow: Shadow = Shadow { color: Color::TRANSPARENT, blur: 0.0, offset: [0.0, 0.0], spread: 0.0 },
    /// Blur whatever is behind the node (frosted glass), logical px; 0 = off.
    backdrop_blur: f32 = 0.0,
    /// 0..1, multiplies this node and its children.
    opacity: f32 = 1.0,
    /// Visual offset after layout, logical px (animations); children move with it.
    translate: [f32; 2] = [0.0, 0.0],
    /// Visual scale about the node's center; children scale with it.
    scale: f32 = 1.0,
    /// Visual rotation about the center, degrees (this node only).
    rotate_deg: f32 = 0.0,
    /// Draw above siblings (and everything else in the screen) when > 0.
    z_index: i32 = 0,
    /// Pointer handling.
    pick: Pick = Pick::Auto,
    /// Seconds to animate visual changes (colors, opacity, translate, scale) between states.
    transition: f32 = 0.0,
    /// Text and icon color for this node and its descendants; transparent = inherit.
    color: Color = Color::TRANSPARENT,
    /// Font size, logical px, for this node and its descendants; 0 = inherit.
    font_size: f32 = 0.0,
    /// Font weight (400 regular, 600 semibold, 700 bold); 0 = inherit.
    font_weight: u16 = 0,
    /// Font family for this node and its descendants (`"Harbour Serif"`); empty = inherit.
    font_family: String = String::new(),
    /// Shape of the corners (`radius` is their size): round, chamfer, scoop or notch.
    corner_shape: CornerShape = CornerShape::Round,
    /// Procedural surface over the background (paper, wood, leather, metal, ...).
    material: Material = Material::default(),
    /// Raised or sunken rim and an engraved inner hairline.
    bevel: Bevel = Bevel::default(),
    /// Shadow inside the box (a recessed well, a pressed button).
    inset_shadow: Shadow = Shadow { color: Color::TRANSPARENT, blur: 0.0, offset: [0.0, 0.0], spread: 0.0 },
    /// A second drop shadow under the first: a tight contact shadow plus a wide soft one
    /// reads as real depth.
    shadow2: Shadow = Shadow { color: Color::TRANSPARENT, blur: 0.0, offset: [0.0, 0.0], spread: 0.0 },
    /// Nine-slice image frame drawn over the box.
    frame: Frame = Frame::default(),
    /// A band of light that sweeps across the box every few seconds, 0..1 (selected or rare
    /// things).
    sheen: f32 = 0.0,
    /// Curve of `transition`s between states.
    ease: Ease = Ease::Auto,
    /// Entrance animation when the node is spawned.
    enter: Enter = Enter::default(),
}

impl Style {
    /// The taffy layout style, with pixel lengths multiplied by `scale` (physical pixels).
    pub fn to_taffy(&self, scale: f32) -> taffy::Style {
        use taffy::style as ts;
        let align = |a: Align| -> Option<ts::AlignItems> {
            match a {
                Align::Auto => None,
                Align::Start => Some(ts::AlignItems::FLEX_START),
                Align::End => Some(ts::AlignItems::FLEX_END),
                Align::Center => Some(ts::AlignItems::CENTER),
                Align::Stretch => Some(ts::AlignItems::STRETCH),
                Align::Baseline => Some(ts::AlignItems::BASELINE),
            }
        };
        let align_content = match self.align_content {
            Align::Auto | Align::Stretch => None,
            Align::Start | Align::Baseline => Some(ts::AlignContent::FLEX_START),
            Align::End => Some(ts::AlignContent::FLEX_END),
            Align::Center => Some(ts::AlignContent::CENTER),
        };
        let justify = match self.justify {
            Justify::Start => ts::JustifyContent::FLEX_START,
            Justify::End => ts::JustifyContent::FLEX_END,
            Justify::Center => ts::JustifyContent::CENTER,
            Justify::SpaceBetween => ts::JustifyContent::SPACE_BETWEEN,
            Justify::SpaceAround => ts::JustifyContent::SPACE_AROUND,
            Justify::SpaceEvenly => ts::JustifyContent::SPACE_EVENLY,
        };
        let overflow = match self.overflow {
            Overflow::Visible => ts::Overflow::Visible,
            Overflow::Hidden => ts::Overflow::Hidden,
            Overflow::Scroll => ts::Overflow::Scroll,
        };
        let gap = taffy::LengthPercentage::length(self.gap * scale);
        let mut style = taffy::Style {
            display: match self.display {
                Display::Flex => ts::Display::Flex,
                Display::Grid => ts::Display::Grid,
                Display::None => ts::Display::None,
            },
            position: match self.position {
                Position::Relative => ts::Position::Relative,
                Position::Absolute => ts::Position::Absolute,
            },
            flex_direction: match self.direction {
                Direction::Row => ts::FlexDirection::Row,
                Direction::Column => ts::FlexDirection::Column,
                Direction::RowReverse => ts::FlexDirection::RowReverse,
                Direction::ColumnReverse => ts::FlexDirection::ColumnReverse,
            },
            flex_wrap: if self.wrap {
                ts::FlexWrap::Wrap
            } else {
                ts::FlexWrap::NoWrap
            },
            justify_content: Some(justify),
            align_items: align(self.align_items),
            align_self: align(self.align_self),
            align_content,
            flex_grow: self.grow,
            flex_shrink: self.shrink,
            flex_basis: self.basis.to_dimension(scale),
            size: taffy::Size {
                width: self.width.to_dimension(scale),
                height: self.height.to_dimension(scale),
            },
            min_size: taffy::Size {
                width: self.min_width.to_lpa(scale),
                height: self.min_height.to_lpa(scale),
            },
            max_size: taffy::Size {
                width: self.max_width.to_lpa(scale),
                height: self.max_height.to_lpa(scale),
            },
            inset: taffy::Rect {
                left: self.left.to_lpa(scale),
                right: self.right.to_lpa(scale),
                top: self.top.to_lpa(scale),
                bottom: self.bottom.to_lpa(scale),
            },
            margin: self.margin.to_lpa(scale),
            padding: self.padding.to_lp(scale),
            border: taffy::Rect::length(self.border_width * scale),
            gap: taffy::Size {
                width: gap,
                height: gap,
            },
            aspect_ratio: (self.aspect_ratio > 0.0).then_some(self.aspect_ratio),
            overflow: taffy::Point {
                x: overflow,
                y: overflow,
            },
            scrollbar_width: 0.0,
            ..Default::default()
        };
        if self.grid_columns > 0 {
            style.grid_template_columns = vec![taffy::style_helpers::repeat(
                self.grid_columns,
                vec![taffy::style_helpers::fr(1.0)],
            )];
        }
        style
    }

    /// Interpolate the animatable (visual) fields toward `target`; layout fields come from
    /// `target`.
    pub fn lerp_visual(&self, target: &Style, t: f32) -> Style {
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        Style {
            background: self.background.lerp(&target.background, t),
            border_color: self.border_color.lerp(target.border_color, t),
            border_width: lerp(self.border_width, target.border_width),
            shadow: self.shadow.lerp(&target.shadow, t),
            shadow2: self.shadow2.lerp(&target.shadow2, t),
            inset_shadow: self.inset_shadow.lerp(&target.inset_shadow, t),
            sheen: lerp(self.sheen, target.sheen),
            opacity: lerp(self.opacity, target.opacity),
            translate: [
                lerp(self.translate[0], target.translate[0]),
                lerp(self.translate[1], target.translate[1]),
            ],
            scale: lerp(self.scale, target.scale),
            rotate_deg: lerp(self.rotate_deg, target.rotate_deg),
            color: if self.color.a > 0.0 && target.color.a > 0.0 {
                self.color.lerp(target.color, t)
            } else {
                target.color
            },
            ..target.clone()
        }
    }

    /// Whether the visual fields match (no transition needed).
    pub fn visual_eq(&self, o: &Style) -> bool {
        self.background == o.background
            && self.border_color == o.border_color
            && self.border_width == o.border_width
            && self.shadow == o.shadow
            && self.shadow2 == o.shadow2
            && self.inset_shadow == o.inset_shadow
            && self.sheen == o.sheen
            && self.opacity == o.opacity
            && self.translate == o.translate
            && self.scale == o.scale
            && self.rotate_deg == o.rotate_deg
            && self.color == o.color
    }
}

impl StylePatch {
    /// Width and height in logical px.
    pub fn size(self, w: impl StyleValue<Val>, h: impl StyleValue<Val>) -> Self {
        self.width(w).height(h)
    }

    /// Column flex container.
    pub fn column(self) -> Self {
        self.direction(Direction::Column)
    }

    /// Row flex container.
    pub fn row(self) -> Self {
        self.direction(Direction::Row)
    }

    /// Center children on both axes.
    pub fn center(self) -> Self {
        self.justify(Justify::Center).align_items(Align::Center)
    }

    /// Absolute position from the parent's top-left, logical px.
    pub fn at(self, left: impl StyleValue<f32>, top: impl StyleValue<f32>) -> Self {
        let (left, top): (f32, f32) = (left.into_style(), top.into_style());
        self.position(Position::Absolute).left(left).top(top)
    }

    /// Absolute, filling the parent.
    pub fn fill_parent(self) -> Self {
        self.position(Position::Absolute)
            .left(0.0)
            .top(0.0)
            .right(0.0)
            .bottom(0.0)
    }

    /// Solid background color.
    pub fn bg(self, c: Color) -> Self {
        self.background(Fill::Solid(c))
    }

    /// Border width and color.
    pub fn border(self, width: impl StyleValue<f32>, color: Color) -> Self {
        self.border_width(width).border_color(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_forms() {
        let p: StylePatch = serde_json::from_str(
            r##"{"width": 320, "height": "50%", "padding": [12, 16], "radius": 10,
                "background": "#101826e0", "margin": {"top": 4}, "direction": "column"}"##,
        )
        .unwrap();
        assert_eq!(p.width, Some(Val::Px(320.0)));
        assert_eq!(p.height, Some(Val::Percent(50.0)));
        assert_eq!(p.padding, Some(Edges::xy(16.0, 12.0)));
        assert_eq!(p.radius, Some(Corners::all(10.0)));
        assert!(matches!(p.background, Some(Fill::Solid(_))));
        assert_eq!(p.margin.unwrap().top, Val::Px(4.0));
        assert_eq!(p.direction, Some(Direction::Column));

        let f: Fill = serde_json::from_str(
            r##"{"linear": {"from": "#000", "to": "#fff", "angle_deg": 90}}"##,
        )
        .unwrap();
        assert!(matches!(f, Fill::Linear { angle_deg, .. } if angle_deg == 90.0));
        let back: StylePatch = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn patches_apply_in_order() {
        let mut s = Style::default();
        StylePatch::default().width(10.0).gap(4.0).apply(&mut s);
        StylePatch::default().width(20.0).apply(&mut s);
        assert_eq!(s.width, Val::Px(20.0));
        assert_eq!(s.gap, 4.0);
        assert!(StylePatch::default().is_empty());
    }

    #[test]
    fn visual_lerp_keeps_layout_of_target() {
        let a = Style {
            opacity: 0.0,
            width: Val::Px(10.0),
            ..Default::default()
        };
        let b = Style {
            opacity: 1.0,
            width: Val::Px(20.0),
            ..Default::default()
        };
        let m = a.lerp_visual(&b, 0.5);
        assert_eq!(m.opacity, 0.5);
        assert_eq!(m.width, Val::Px(20.0));
    }
}
