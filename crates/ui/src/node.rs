//! UI components. A UI element is an entity with a [`Node`]; children use the ECS
//! `ChildOf`/`Children` hierarchy. Everything here is plain data (serde), so scenes, the AI
//! bridge and `entity.patch` can build and edit UI by name.

use bevy_ecs::prelude::*;
use engine_core::math::{DVec3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::style::{Style, StylePatch};

/// A UI element: theme classes plus an inline style, with per-state overrides.
///
/// The effective style is `Style::default()` ← each class in `classes` ← `style` ← the state
/// patches that apply (`hover`, `pressed`, `focused`, `selected`, `disabled`, from the classes
/// first, then inline).
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
#[require(ComputedNode, Interaction)]
pub struct Node {
    /// Theme classes, applied in order (`"panel"`, `"button primary"` splits on spaces).
    pub classes: Vec<String>,
    /// Inline style on top of the classes.
    pub style: StylePatch,
    /// Inline overrides while hovered / pressed / focused / selected / disabled.
    pub hover: StylePatch,
    pub pressed: StylePatch,
    pub focused: StylePatch,
    pub selected: StylePatch,
    pub disabled: StylePatch,
}

impl Node {
    /// A node with these classes (space separated) and no inline style.
    pub fn class(classes: &str) -> Self {
        Self {
            classes: classes.split_whitespace().map(String::from).collect(),
            ..Default::default()
        }
    }

    /// A node with just an inline style.
    pub fn styled(style: StylePatch) -> Self {
        Self {
            style,
            ..Default::default()
        }
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.classes.iter().any(|c| c == class)
    }
}

/// Interaction state. `hovered`, `pressed` and `focused` are set by the UI; games set
/// `selected` (current tab, chosen item) and `disabled` (can't afford it).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interaction {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub selected: bool,
    pub disabled: bool,
}

/// An axis-aligned rectangle in physical pixels (origin top-left, y down).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const EVERYTHING: Rect = Rect {
        x: -1.0e7,
        y: -1.0e7,
        w: 2.0e7,
        h: 2.0e7,
    };

    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn min(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn max(&self) -> Vec2 {
        Vec2::new(self.x + self.w, self.y + self.h)
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.w, self.h)
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.w && p.y < self.y + self.h
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Grow by `d` on every side.
    pub fn inflate(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    /// Scale about `pivot`.
    pub fn scaled_about(&self, pivot: Vec2, s: f32) -> Rect {
        Rect::new(
            pivot.x + (self.x - pivot.x) * s,
            pivot.y + (self.y - pivot.y) * s,
            self.w * s,
            self.h * s,
        )
    }

    pub fn offset(&self, d: Vec2) -> Rect {
        Rect::new(self.x + d.x, self.y + d.y, self.w, self.h)
    }
}

/// Where a node ended up this frame (filled by layout; read by drawing and input).
#[derive(Component, Debug, Clone, Default)]
pub struct ComputedNode {
    /// Final box on screen after layout and visual transforms, physical px.
    pub rect: Rect,
    /// Children and this node are clipped to this, physical px.
    pub clip: Rect,
    /// Effective opacity (own × ancestors).
    pub opacity: f32,
    /// Effective visual scale (own × ancestors); also scales borders and radii.
    pub scale: f32,
    /// Draw order, 0 first. Higher is on top (used for hit testing).
    pub order: u32,
    /// Laid out and on screen this frame.
    pub visible: bool,
    /// The style as currently drawn (mid-transition values for animated fields).
    pub style: Style,
    /// The style the node is transitioning to.
    pub target: Style,
    /// Effective (inherited) text/icon color, size (logical px) and weight.
    pub color: Color,
    pub font_size: f32,
    pub font_weight: u16,
    /// Size of the content (for scroll containers), physical px.
    pub content_size: Vec2,
    /// Layout box size before visual scale, physical px.
    pub layout_size: Vec2,
    /// Whether the style was resolved at least once.
    pub resolved: bool,
}

/// A stable name for a node: events carry it, `ui.click {"id": ...}` and `UiQuery` find it.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct UiId(pub String);

/// Text drawn in the node's content box. Size, weight and color come from the node's style
/// (inherited) unless set here.
///
/// Markup (when `rich`): `[b]bold[/b]`, `[i]italic[/i]`, `[color=#e8b65a]gold[/color]`
/// (or `[c=gold]`), `[size=24]big[/size]`. Bindings: `{player.gold}` is replaced with the value
/// at that path in [`UiData`](crate::data::UiData), and `{@Player.Health.current}` with a field
/// of a registered component on the entity named `Player`.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[require(Node)]
pub struct Text {
    pub text: String,
    /// Logical px; 0 = inherited from the style.
    pub size: f32,
    /// 0 = inherited (400 regular, 600 semibold, 700 bold).
    pub weight: u16,
    pub italic: bool,
    /// Overrides the style's color.
    pub color: Option<Color>,
    pub align: TextAlign,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Extra space between letters, em.
    pub letter_spacing: f32,
    pub uppercase: bool,
    /// Wrap at the node's width (otherwise one line per paragraph).
    pub wrap: bool,
    /// Parse `[b]`-style markup.
    pub rich: bool,
    /// Typewriter effect: how many characters are visible (None = all).
    pub reveal: Option<f32>,
    /// Soft dark shadow behind the glyphs (readability over the 3D world).
    pub shadow: bool,
    /// Use tabular (fixed-width) digits, for counters and prices.
    pub tabular: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            text: String::new(),
            size: 0.0,
            weight: 0,
            italic: false,
            color: None,
            align: TextAlign::Left,
            line_height: 1.3,
            letter_spacing: 0.0,
            uppercase: false,
            wrap: true,
            rich: true,
            reveal: None,
            shadow: false,
            tabular: false,
        }
    }
}

impl Text {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn weight(mut self, weight: u16) -> Self {
        self.weight = weight;
        self
    }

    pub fn bold(self) -> Self {
        self.weight(700)
    }

    pub fn semibold(self) -> Self {
        self.weight(600)
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn align(mut self, a: TextAlign) -> Self {
        self.align = a;
        self
    }

    pub fn center(self) -> Self {
        self.align(TextAlign::Center)
    }

    pub fn caps(mut self, letter_spacing: f32) -> Self {
        self.uppercase = true;
        self.letter_spacing = letter_spacing;
        self
    }

    pub fn nowrap(mut self) -> Self {
        self.wrap = false;
        self
    }

    pub fn shadow(mut self) -> Self {
        self.shadow = true;
        self
    }

    pub fn tabular(mut self) -> Self {
        self.tabular = true;
        self
    }

    pub fn line_height(mut self, lh: f32) -> Self {
        self.line_height = lh;
        self
    }
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// An icon or picture drawn to fit the node's content box.
///
/// `name` is a built-in icon (`"sword"`, `"potion_health"`, `"ui_close"`; `ui.icons` lists
/// them), a portrait (`"portrait:pilot"`), or SVG source registered at runtime with
/// `ui.add_icon`.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[require(Node)]
pub struct Icon {
    pub name: String,
    /// Multiplies the icon's colors. White keeps them; for single-color `ui_*` glyphs it sets
    /// the color. None = the node's inherited color for `ui_*` glyphs, white otherwise.
    pub tint: Option<Color>,
    /// Mirror horizontally.
    pub flip_x: bool,
    /// Grayscale (locked or unaffordable items).
    pub desaturate: bool,
    /// Fill the node's box (cropping the sides, or the bottom first so faces stay in view)
    /// and clip to its rounded corners: round avatars from portraits.
    pub cover: bool,
}

impl Icon {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn tint(mut self, c: Color) -> Self {
        self.tint = Some(c);
        self
    }
}

/// Clicking (or pressing accept while focused) sends [`UiEvent::Action`](crate::UiEvent) with
/// this action string, e.g. `"shop.buy"` or `"dialogue.choose:2"`.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[require(Node, Focusable)]
pub struct OnClick(pub String);

/// Reachable with arrow keys, the d-pad and the left stick.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Focusable;

/// Rich text shown in a tooltip after hovering for a moment.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tooltip {
    pub text: String,
    /// Title line (bold), optional.
    pub title: String,
    /// Title color (e.g. item rarity).
    pub title_color: Option<Color>,
}

/// Can be dragged; `payload` travels with the drag (e.g. `"inventory:12"`).
#[derive(Component, Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[require(Node)]
pub struct Draggable {
    pub payload: String,
}

/// Accepts drops. `accepts` is a list of payload prefixes (`["inventory", "equip"]`);
/// empty accepts everything. `slot` names this target in [`UiEvent::Dropped`].
#[derive(Component, Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[require(Node)]
pub struct DropTarget {
    pub slot: String,
    pub accepts: Vec<String>,
}

impl DropTarget {
    pub fn accepts(&self, payload: &str) -> bool {
        self.accepts.is_empty() || self.accepts.iter().any(|p| payload.starts_with(p.as_str()))
    }
}

/// Scroll position of an `overflow: scroll` node, physical px (filled in automatically).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollState {
    pub offset: Vec2,
    /// Smoothed target the offset eases toward.
    pub target: Vec2,
}

/// A top-level UI screen (dialogue, shop, HUD, a custom layout). `ui.show` opens screens by
/// key; [`ScreenRegistry`](crate::screens::ScreenRegistry) knows how to build each one.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[require(Node)]
pub struct Screen {
    pub key: String,
    /// Takes keyboard/gamepad focus and blocks game input while open.
    pub modal: bool,
    /// Draw order between screens (HUD 0, menus 10, dialogue 20, tooltips 100).
    pub layer: i32,
    /// 0 → 1 while opening, 1 → 0 while closing.
    pub presence: f32,
    pub closing: bool,
    /// Close with the "back" input (Escape / B).
    pub closable: bool,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            key: String::new(),
            modal: true,
            layer: 10,
            presence: 0.0,
            closing: false,
            closable: true,
        }
    }
}

/// Pins a top-level node to a point in the 3D world (nameplates, quest markers, damage
/// numbers). The node's center goes to the projected point.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[require(Node)]
pub struct WorldAnchor {
    /// World position (f64), or offset from `entity` when set.
    pub position: DVec3,
    /// Follow this entity's `GlobalTransform`.
    #[serde(skip)]
    pub entity: Option<Entity>,
    /// World-space offset added to the followed entity, meters.
    pub offset: Vec3,
    /// Off-screen or behind the camera: stick to the screen edge (quest markers) instead of
    /// hiding.
    pub clamp_to_edges: bool,
    /// Fade out between these distances, meters (`[0, 0]` = never).
    pub fade_distance: [f32; 2],
    /// Filled in: distance from the camera, meters.
    pub distance: f32,
    /// Filled in: on screen and in front of the camera.
    pub on_screen: bool,
    /// Filled in: direction toward the target in screen space when clamped (unit, y down).
    pub edge_direction: Vec2,
}

impl Default for WorldAnchor {
    fn default() -> Self {
        Self {
            position: DVec3::ZERO,
            entity: None,
            offset: Vec3::ZERO,
            clamp_to_edges: false,
            fade_distance: [0.0, 0.0],
            distance: 0.0,
            on_screen: false,
            edge_direction: Vec2::ZERO,
        }
    }
}

impl WorldAnchor {
    pub fn at(position: DVec3) -> Self {
        Self {
            position,
            ..Default::default()
        }
    }

    pub fn follow(entity: Entity, offset: Vec3) -> Self {
        Self {
            entity: Some(entity),
            offset,
            ..Default::default()
        }
    }
}

/// Drawn by a custom painter instead of (on top of) the normal box: maps, portraits, charts.
/// The painter is looked up by `kind` in [`Painters`](crate::draw::Painters).
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[require(Node)]
pub struct CustomPaint {
    pub kind: String,
}
