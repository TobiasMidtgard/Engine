//! [`El`]: a UI tree as plain data. Screens are built from `El`s in Rust, and designers or AI
//! agents write the same tree as JSON (`ui.spawn {"layout": {...}}`).
//!
//! ```
//! # use engine_ui::prelude::*;
//! let card = El::new()
//!     .class("panel")
//!     .style(StylePatch::default().width(320.0))
//!     .child(El::text("Merchant").class("title"))
//!     .child(El::button("Trade", "shop.open").class("primary"));
//! let json = serde_json::to_string(&card).unwrap();
//! let back: El = serde_json::from_str(&json).unwrap();
//! assert_eq!(back.children.len(), 2);
//! ```

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use engine_core::registry::TypeRegistry;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::color::Color;
use crate::node::*;
use crate::style::StylePatch;

/// One element and its children.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct El {
    /// Name for events, `ui.click` and lookups.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Theme classes (`"button primary"` or `["button", "primary"]`).
    #[serde(deserialize_with = "classes", skip_serializing_if = "Vec::is_empty")]
    pub class: Vec<String>,
    #[serde(skip_serializing_if = "StylePatch::is_empty")]
    pub style: StylePatch,
    #[serde(skip_serializing_if = "StylePatch::is_empty")]
    pub hover: StylePatch,
    #[serde(skip_serializing_if = "StylePatch::is_empty")]
    pub pressed: StylePatch,
    #[serde(skip_serializing_if = "StylePatch::is_empty")]
    pub focused: StylePatch,
    #[serde(skip_serializing_if = "StylePatch::is_empty")]
    pub selected_style: StylePatch,
    #[serde(skip_serializing_if = "StylePatch::is_empty")]
    pub disabled_style: StylePatch,
    /// Text content (markup and `{bindings}` allowed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Text options (size, weight, align, …); `text` is filled from the field above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_style: Option<Text>,
    /// Icon name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_tint: Option<Color>,
    /// Mirror the icon horizontally.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub icon_flip: bool,
    /// Action sent when clicked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_click: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<Tooltip>,
    /// Keyboard/gamepad focusable (default: true when `on_click` is set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focusable: Option<bool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub selected: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Drag payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<DropTarget>,
    /// Custom painter kind drawn over the box (`"bar"`, `"cooldown"`, `"map"`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paint: Option<String>,
    /// Extra registered components by name: `{"Bar": {"value": 0.5}}`.
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub components: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<El>,
}

fn classes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Repr::deserialize(d)? {
        Repr::One(s) => s.split_whitespace().map(String::from).collect(),
        Repr::Many(v) => v
            .iter()
            .flat_map(|s| s.split_whitespace().map(String::from))
            .collect(),
    })
}

impl El {
    pub fn new() -> Self {
        Self::default()
    }

    /// A text element.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Default::default()
        }
    }

    /// A text element with options.
    pub fn rich(text: Text) -> Self {
        Self {
            text: Some(text.text.clone()),
            text_style: Some(text),
            ..Default::default()
        }
    }

    /// An icon element (size it with the style).
    pub fn icon(name: impl Into<String>) -> Self {
        Self {
            icon: Some(name.into()),
            ..Default::default()
        }
    }

    /// A `button` with a text label.
    pub fn button(label: impl Into<String>, action: impl Into<String>) -> Self {
        Self::new()
            .class("button")
            .on_click(action)
            .child(Self::text(label).pick_ignore())
    }

    /// Add classes (space separated).
    pub fn class(mut self, classes: &str) -> Self {
        self.class
            .extend(classes.split_whitespace().map(String::from));
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Merge an inline style.
    pub fn style(mut self, style: StylePatch) -> Self {
        self.style.merge(&style);
        self
    }

    /// Edit the inline style with a closure.
    pub fn with_style(mut self, f: impl FnOnce(StylePatch) -> StylePatch) -> Self {
        self.style = f(std::mem::take(&mut self.style));
        self
    }

    pub fn hover(mut self, style: StylePatch) -> Self {
        self.hover.merge(&style);
        self
    }

    pub fn selected_style(mut self, style: StylePatch) -> Self {
        self.selected_style.merge(&style);
        self
    }

    pub fn text_style(mut self, f: impl FnOnce(Text) -> Text) -> Self {
        let t = self.text_style.take().unwrap_or_default();
        self.text_style = Some(f(t));
        self
    }

    pub fn icon_tint(mut self, c: Color) -> Self {
        self.icon_tint = Some(c);
        self
    }

    /// Mirror the icon horizontally (a portrait facing the other way).
    pub fn icon_flip(mut self, flip: bool) -> Self {
        self.icon_flip = flip;
        self
    }

    pub fn on_click(mut self, action: impl Into<String>) -> Self {
        self.on_click = Some(action.into());
        self
    }

    pub fn tooltip(mut self, title: impl Into<String>, text: impl Into<String>) -> Self {
        self.tooltip = Some(Tooltip {
            title: title.into(),
            text: text.into(),
            title_color: None,
        });
        self
    }

    pub fn tooltip_full(mut self, tooltip: Tooltip) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn focusable(mut self, f: bool) -> Self {
        self.focusable = Some(f);
        self
    }

    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn drag(mut self, payload: impl Into<String>) -> Self {
        self.drag = Some(payload.into());
        self
    }

    pub fn drop_target(mut self, slot: impl Into<String>, accepts: &[&str]) -> Self {
        self.drop = Some(DropTarget {
            slot: slot.into(),
            accepts: accepts.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Draw this node with a custom painter.
    pub fn paint(mut self, kind: impl Into<String>) -> Self {
        self.paint = Some(kind.into());
        self
    }

    /// Attach a registered component by name with a JSON value.
    pub fn component(mut self, name: &str, value: Value) -> Self {
        self.components.insert(name.to_string(), value);
        self
    }

    /// Attach a registered component from its Rust value (stored as JSON, so the tree stays
    /// plain data).
    pub fn attach<T: Serialize>(self, name: &str, component: &T) -> Self {
        let v = serde_json::to_value(component).unwrap_or(Value::Null);
        self.component(name, v)
    }

    /// Don't take pointer input (labels inside buttons).
    pub fn pick_ignore(self) -> Self {
        self.with_style(|s| s.pick(crate::style::Pick::Ignore))
    }

    pub fn child(mut self, child: El) -> Self {
        self.children.push(child);
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = El>) -> Self {
        self.children.extend(children);
        self
    }

    /// Add a child only when `Some`.
    pub fn child_opt(mut self, child: Option<El>) -> Self {
        if let Some(c) = child {
            self.children.push(c);
        }
        self
    }

    /// Count of elements in this tree.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(El::count).sum::<usize>()
    }
}

/// What [`spawn`] created: the root and every element that had an `id`.
#[derive(Debug, Clone, Default)]
pub struct Spawned {
    pub root: Option<Entity>,
    pub ids: HashMap<String, Entity>,
}

impl Spawned {
    pub fn get(&self, id: &str) -> Option<Entity> {
        self.ids.get(id).copied()
    }
}

/// Spawn `el` (and its children) under `parent`. Unknown component names in
/// `El::components` are an error (the tree is spawned up to that point).
pub fn spawn(world: &mut World, parent: Option<Entity>, el: &El) -> Result<Spawned, String> {
    let mut out = Spawned::default();
    let root = spawn_one(world, parent, el, &mut out)?;
    out.root = Some(root);
    Ok(out)
}

fn spawn_one(
    world: &mut World,
    parent: Option<Entity>,
    el: &El,
    out: &mut Spawned,
) -> Result<Entity, String> {
    let node = Node {
        classes: el.class.clone(),
        style: el.style.clone(),
        hover: el.hover.clone(),
        pressed: el.pressed.clone(),
        focused: el.focused.clone(),
        selected: el.selected_style.clone(),
        disabled: el.disabled_style.clone(),
    };
    let mut e = world.spawn((
        node,
        Interaction {
            selected: el.selected,
            disabled: el.disabled,
            ..Default::default()
        },
    ));
    if let Some(p) = parent {
        e.insert(ChildOf(p));
    }
    if let Some(id) = &el.id {
        e.insert(UiId(id.clone()));
        out.ids.insert(id.clone(), e.id());
    }
    if let Some(text) = &el.text {
        let mut t = el.text_style.clone().unwrap_or_default();
        t.text = text.clone();
        e.insert(t);
    }
    if let Some(icon) = &el.icon {
        e.insert(Icon {
            name: icon.clone(),
            tint: el.icon_tint,
            flip_x: el.icon_flip,
            ..Default::default()
        });
    }
    if let Some(action) = &el.on_click {
        e.insert(OnClick(action.clone()));
    }
    match el.focusable {
        Some(true) => {
            e.insert(Focusable);
        }
        Some(false) => {
            e.remove::<Focusable>();
        }
        None => {}
    }
    if let Some(t) = &el.tooltip {
        e.insert(t.clone());
    }
    if let Some(payload) = &el.drag {
        e.insert(Draggable {
            payload: payload.clone(),
        });
    }
    if let Some(d) = &el.drop {
        e.insert(d.clone());
    }
    if let Some(kind) = &el.paint {
        e.insert(CustomPaint { kind: kind.clone() });
    }
    let id = e.id();
    if !el.components.is_empty() {
        let registry = world.resource::<TypeRegistry>().clone();
        for (name, value) in &el.components {
            let entry = registry.component_or_err(name)?;
            (entry.insert)(world, id, value.clone())?;
        }
    }
    for child in &el.children {
        spawn_one(world, Some(id), child, out)?;
    }
    Ok(id)
}

/// Despawn every child of `parent` (keeps `parent`).
pub fn clear_children(world: &mut World, parent: Entity) {
    let children: Vec<Entity> = world
        .get::<Children>(parent)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for c in children {
        if let Ok(e) = world.get_entity_mut(c) {
            e.despawn();
        }
    }
}

/// Replace `parent`'s children with `els`.
pub fn replace_children(world: &mut World, parent: Entity, els: &[El]) -> Result<Spawned, String> {
    clear_children(world, parent);
    let mut out = Spawned::default();
    for el in els {
        spawn_one(world, Some(parent), el, &mut out)?;
    }
    Ok(out)
}
