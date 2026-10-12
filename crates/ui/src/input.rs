//! Pointer, keyboard and gamepad input for the UI.
//!
//! * **Pointer**: the topmost node under the cursor that catches it (interactive nodes and
//!   nodes with a background; see [`Pick`]) is hovered. Clicking a node with [`OnClick`] sends
//!   [`UiEvent::Action`]; right click sends it with `alt: true`. The wheel scrolls the
//!   `overflow: scroll` node under the cursor.
//! * **Keyboard and gamepad**: inside the topmost open modal screen, arrows / d-pad / left
//!   stick move focus to the nearest [`Focusable`] in that direction (with key repeat),
//!   Tab / Shift+Tab walk them in order, Enter / Space / A activate, gamepad X is the
//!   alternate action, Q / E and the bumpers switch tabs ([`UiEvent::Tab`]), Escape / B go
//!   back (closing a closable screen), and the right stick scrolls.
//! * **Drag and drop**: press on a [`Draggable`] and move; a ghost follows the cursor and
//!   [`DropTarget`]s that accept the payload light up. Releasing sends [`UiEvent::Dropped`]
//!   (or [`UiEvent::DragCancelled`]). With a gamepad, A picks a draggable up, navigate, A
//!   drops it, B cancels.
//! * **Tooltips** appear after hovering (or focusing) a node with [`Tooltip`] for
//!   `UiSettings::tooltip_delay` real seconds.
//! * [`UiInputCapture`] tells the game whether the UI owns the pointer and keyboard this
//!   frame. While a modal screen is open, game actions in `ActionState` are released (unless
//!   `UiSettings::block_game_input` is off), so the player doesn't walk while shopping.
//!
//! All timing is real time, so menus stay responsive in slow motion and while paused.

use std::collections::HashMap;

use bevy_ecs::message::Messages;
use bevy_ecs::prelude::*;
use engine_core::math::Vec2;
use engine_core::prelude::Time;
use engine_input::{
    ActionState, ButtonInput, GamepadAxis, GamepadButton, Gamepads, KeyCode, Mouse, MouseButton,
};
use serde::{Deserialize, Serialize};

use crate::el::{self, El};
use crate::layout::{UiDrawOrder, UiSettings, UiViewport, keep_inside};
use crate::node::*;
use crate::screens::{self, root_of};
use crate::style::{Overflow, Pick};

/// Something the player did in the UI. Read with `MessageReader<UiEvent>`; the last few are
/// also returned by `ui.events`.
#[derive(Message, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    /// A node with [`OnClick`] was activated (click, accept while focused, or `ui.click`).
    /// `alt` is a right click or the gamepad's X button (use, equip, sell one).
    Action {
        action: String,
        id: Option<String>,
        alt: bool,
        #[serde(skip)]
        entity: Option<Entity>,
    },
    /// The hovered or focused interactive node changed (detail panels follow this).
    Highlighted {
        id: Option<String>,
        #[serde(skip)]
        entity: Option<Entity>,
    },
    /// A drag began.
    DragStarted { payload: String },
    /// A drag ended over a [`DropTarget`] that accepts the payload.
    Dropped {
        payload: String,
        slot: String,
        #[serde(skip)]
        source: Option<Entity>,
        #[serde(skip)]
        target: Option<Entity>,
    },
    /// A drag ended anywhere else (or was cancelled with Escape / B).
    DragCancelled { payload: String },
    /// Switch tab: Q / E or the bumpers (`delta` -1 or 1) in the focused screen.
    Tab { screen: String, delta: i32 },
    /// Back (Escape / B) in a screen. Closable screens also close.
    Back { screen: String },
    /// Left/right on a focused [`Adjustable`] node (sliders, steppers): `delta` -1 or 1.
    Adjust {
        id: Option<String>,
        delta: i32,
        #[serde(skip)]
        entity: Option<Entity>,
    },
    ScreenOpened { screen: String },
    ScreenClosed { screen: String },
}

/// Left/right adjusts this node (sending [`UiEvent::Adjust`]) instead of moving focus.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Adjustable;

/// Whether the UI owns input this frame (read it before firing a weapon on click).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiInputCapture {
    /// The cursor is over UI that catches it, or a modal screen is open.
    pub pointer: bool,
    /// A modal screen is open: keyboard and gamepad drive the UI.
    pub keyboard: bool,
    /// Something is being dragged.
    pub dragging: bool,
}

/// How the player is driving the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavMode {
    #[default]
    Pointer,
    /// Keyboard or gamepad: focus rings are shown.
    Directional,
}

/// A drag in progress.
#[derive(Debug, Clone)]
pub struct Drag {
    pub source: Entity,
    pub payload: String,
    /// The ghost screen following the cursor (spawned once the drag starts).
    pub ghost: Option<Entity>,
    /// Pointer drags start after moving a few pixels.
    pub started: bool,
    /// Picked up with the gamepad or keyboard (follows focus instead of the cursor).
    pub directional: bool,
    /// Cursor offset from the source's top-left, physical px.
    pub grab: Vec2,
}

/// Input bookkeeping (public so screens and painters can read what the pointer is on).
#[derive(Resource, Debug, Default)]
pub struct UiInputState {
    /// Topmost node under the cursor that catches it.
    pub hit: Option<Entity>,
    /// Interactive node under the cursor (hover target).
    pub hovered: Option<Entity>,
    /// Logical focus (shown only in directional mode).
    pub focused: Option<Entity>,
    /// [`UiId`] of the focused node: focus comes back to the node with this id when a screen
    /// rebuilds its content.
    pub focused_id: Option<String>,
    /// Node pressed with the pointer and whether it was the alternate button.
    pub pressed: Option<(Entity, bool)>,
    /// The node under the cursor when the button went down (maps drag on this).
    pub press_hit: Option<Entity>,
    pub press_pos: Vec2,
    pub drag: Option<Drag>,
    pub mode: NavMode,
    /// The screen keyboard/gamepad input goes to.
    pub scope: Option<Entity>,
    /// Last focus per screen, restored when returning to it.
    pub focus_memory: HashMap<Entity, Entity>,
    pub highlighted: Option<Entity>,
    /// The last directional input came from a gamepad (key hints show pad buttons).
    pub last_gamepad: bool,
    tooltip_target: Option<Entity>,
    tooltip_time: f32,
    tooltip: Option<Entity>,
    tooltip_content: (String, String),
    last_cursor: Option<Vec2>,
    repeat_dir: Option<NavDir>,
    repeat_timer: f32,
}

/// A focus move direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavDir {
    Up,
    Down,
    Left,
    Right,
}

impl NavDir {
    fn vector(self) -> Vec2 {
        match self {
            NavDir::Up => Vec2::new(0.0, -1.0),
            NavDir::Down => Vec2::new(0.0, 1.0),
            NavDir::Left => Vec2::new(-1.0, 0.0),
            NavDir::Right => Vec2::new(1.0, 0.0),
        }
    }
}

/// This frame's device state (all optional so the UI works without the input plugin).
#[derive(Default)]
struct Devices {
    cursor: Option<Vec2>,
    scroll: Vec2,
    left_down: bool,
    left_up: bool,
    right_down: bool,
    right_up: bool,
    keys_down: Vec<KeyCode>,
    keys_held: Vec<KeyCode>,
    pad_down: Vec<GamepadButton>,
    pad_held: Vec<GamepadButton>,
    left_stick: Vec2,
    right_stick: Vec2,
}

impl Devices {
    fn read(world: &World) -> Self {
        let mut d = Devices::default();
        if let Some(m) = world.get_resource::<Mouse>() {
            d.cursor = m.position;
            d.scroll = m.scroll;
        }
        if let Some(b) = world.get_resource::<ButtonInput<MouseButton>>() {
            d.left_down = b.just_pressed(MouseButton::Left);
            d.left_up = b.just_released(MouseButton::Left);
            d.right_down = b.just_pressed(MouseButton::Right);
            d.right_up = b.just_released(MouseButton::Right);
        }
        if let Some(k) = world.get_resource::<ButtonInput<KeyCode>>() {
            d.keys_down = k.get_just_pressed().copied().collect();
            d.keys_held = k.get_pressed().copied().collect();
        }
        if let Some(p) = world.get_resource::<ButtonInput<GamepadButton>>() {
            d.pad_down = p.get_just_pressed().copied().collect();
            d.pad_held = p.get_pressed().copied().collect();
        }
        if let Some(p) = world.get_resource::<Gamepads>() {
            d.left_stick = Vec2::new(
                p.any_axis(GamepadAxis::LeftStickX),
                p.any_axis(GamepadAxis::LeftStickY),
            );
            d.right_stick = Vec2::new(
                p.any_axis(GamepadAxis::RightStickX),
                p.any_axis(GamepadAxis::RightStickY),
            );
        }
        d
    }

    fn key(&self, k: KeyCode) -> bool {
        self.keys_down.contains(&k)
    }

    fn held(&self, k: KeyCode) -> bool {
        self.keys_held.contains(&k)
    }

    fn pad(&self, b: GamepadButton) -> bool {
        self.pad_down.contains(&b)
    }

    fn pad_held(&self, b: GamepadButton) -> bool {
        self.pad_held.contains(&b)
    }

    fn any_directional_input(&self) -> bool {
        !self.keys_down.is_empty()
            || !self.pad_down.is_empty()
            || self.left_stick.length() > 0.5
    }

    /// The direction held right now (keys, d-pad, stick).
    fn held_dir(&self) -> Option<NavDir> {
        use KeyCode::*;
        if self.held(ArrowUp) || self.pad_held(GamepadButton::DPadUp) {
            return Some(NavDir::Up);
        }
        if self.held(ArrowDown) || self.pad_held(GamepadButton::DPadDown) {
            return Some(NavDir::Down);
        }
        if self.held(ArrowLeft) || self.pad_held(GamepadButton::DPadLeft) {
            return Some(NavDir::Left);
        }
        if self.held(ArrowRight) || self.pad_held(GamepadButton::DPadRight) {
            return Some(NavDir::Right);
        }
        let s = self.left_stick;
        if s.length() > 0.6 {
            return Some(if s.x.abs() > s.y.abs() {
                if s.x > 0.0 { NavDir::Right } else { NavDir::Left }
            } else if s.y > 0.0 {
                NavDir::Up
            } else {
                NavDir::Down
            });
        }
        None
    }
}

fn send(world: &mut World, ev: UiEvent) {
    if let Some(mut m) = world.get_resource_mut::<Messages<UiEvent>>() {
        m.write(ev);
    }
}

fn alive(world: &World, e: Option<Entity>) -> Option<Entity> {
    e.filter(|e| world.get_entity(*e).is_ok() && world.get::<Node>(*e).is_some())
}

/// Whether a node catches the pointer.
fn catches(world: &World, e: Entity, c: &ComputedNode) -> bool {
    match c.style.pick {
        Pick::Ignore => false,
        Pick::Block => true,
        Pick::Auto => {
            c.style.background.is_visible()
                || (c.style.border_width > 0.0 && c.style.border_color.a > 0.0)
                || world.get::<OnClick>(e).is_some()
                || world.get::<Draggable>(e).is_some()
                || world.get::<DropTarget>(e).is_some()
                || world.get::<Focusable>(e).is_some()
                || world.get::<Tooltip>(e).is_some()
                || c.style.overflow == Overflow::Scroll
        }
    }
}

fn screen_layer(world: &World, root: Entity) -> Option<(i32, bool)> {
    world.get::<Screen>(root).map(|s| (s.layer, s.closing))
}

/// The topmost node under `p` that catches the pointer. Nodes below the topmost modal
/// screen are blocked.
pub fn hit_test(world: &World, p: Vec2, modal_layer: Option<i32>) -> Option<Entity> {
    let order = world.get_resource::<UiDrawOrder>()?;
    for &e in order.nodes.iter().rev() {
        let Some(c) = world.get::<ComputedNode>(e) else {
            continue;
        };
        if !c.visible || c.opacity < 0.05 || !c.rect.contains(p) || !c.clip.contains(p) {
            continue;
        }
        if !catches(world, e, c) {
            continue;
        }
        let root = root_of(world, e);
        let (layer, closing) = screen_layer(world, root).unwrap_or((0, false));
        if closing {
            continue;
        }
        if let Some(m) = modal_layer
            && layer < m
        {
            return None;
        }
        return Some(e);
    }
    None
}

/// `e` or its nearest ancestor that has component `T`.
pub fn ancestor_with<T: Component>(world: &World, mut e: Entity) -> Option<Entity> {
    loop {
        if world.get::<Node>(e).is_none() {
            return None;
        }
        if world.get::<T>(e).is_some() {
            return Some(e);
        }
        e = world.get::<ChildOf>(e)?.parent();
    }
}

fn interactive(world: &World, mut e: Entity) -> Option<Entity> {
    loop {
        if world.get::<Node>(e).is_none() {
            return None;
        }
        if world.get::<OnClick>(e).is_some()
            || world.get::<Draggable>(e).is_some()
            || world.get::<Focusable>(e).is_some()
        {
            return Some(e);
        }
        e = world.get::<ChildOf>(e)?.parent();
    }
}

fn drop_target(world: &World, e: Entity, payload: &str) -> Option<Entity> {
    let mut cur = Some(e);
    while let Some(c) = cur {
        world.get::<Node>(c)?;
        if let Some(t) = world.get::<DropTarget>(c)
            && t.accepts(payload)
        {
            return Some(c);
        }
        cur = world.get::<ChildOf>(c).map(|p| p.parent());
    }
    None
}

fn set_interaction(world: &mut World, e: Entity, f: impl FnOnce(&mut Interaction)) {
    if let Some(mut it) = world.get_mut::<Interaction>(e) {
        let mut next = *it;
        f(&mut next);
        if next != *it {
            *it = next;
        }
    }
}

/// Focusable nodes inside `scope`, visible this frame, in draw order.
fn focusables(world: &World, scope: Entity) -> Vec<Entity> {
    let mut v: Vec<(u32, Entity)> = screens::subtree(world, scope)
        .into_iter()
        .filter(|e| world.get::<Focusable>(*e).is_some())
        .filter_map(|e| {
            let c = world.get::<ComputedNode>(e)?;
            (c.visible && c.opacity > 0.05).then_some((c.order, e))
        })
        .collect();
    v.sort_by_key(|(o, _)| *o);
    v.into_iter().map(|(_, e)| e).collect()
}

/// The focusable nearest to `from` in direction `dir` (rect edges, perpendicular offset
/// weighted twice).
fn nearest_in_direction(world: &World, from: Entity, dir: NavDir, candidates: &[Entity]) -> Option<Entity> {
    let a = world.get::<ComputedNode>(from)?.rect;
    let d = dir.vector();
    let mut best: Option<(f32, Entity)> = None;
    for &c in candidates {
        if c == from {
            continue;
        }
        let Some(b) = world.get::<ComputedNode>(c).map(|c| c.rect) else {
            continue;
        };
        // Distance along the direction between facing edges, and the perpendicular gap.
        let (along, perp) = match dir {
            NavDir::Right => (b.x - (a.x + a.w), gap(a.y, a.h, b.y, b.h)),
            NavDir::Left => (a.x - (b.x + b.w), gap(a.y, a.h, b.y, b.h)),
            NavDir::Down => (b.y - (a.y + a.h), gap(a.x, a.w, b.x, b.w)),
            NavDir::Up => (a.y - (b.y + b.h), gap(a.x, a.w, b.x, b.w)),
        };
        let centers = b.center() - a.center();
        if along < -0.5 * (a.size().dot(d.abs())).min(b.size().dot(d.abs())) || centers.dot(d) <= 1.0 {
            continue;
        }
        let center_perp = (centers - d * centers.dot(d)).length();
        let score = along.max(0.0) + perp * 2.0 + center_perp * 0.1;
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, c));
        }
    }
    best.map(|(_, e)| e)
}

/// Gap between two 1D spans (0 when they overlap).
fn gap(a0: f32, aw: f32, b0: f32, bw: f32) -> f32 {
    if b0 > a0 + aw {
        b0 - (a0 + aw)
    } else if a0 > b0 + bw {
        a0 - (b0 + bw)
    } else {
        0.0
    }
}

/// Scroll every scroll container above `e` so `e` is fully visible.
fn scroll_into_view(world: &mut World, e: Entity, margin: f32) {
    let Some(target) = world.get::<ComputedNode>(e).map(|c| c.rect) else {
        return;
    };
    let mut cur = world.get::<ChildOf>(e).map(|p| p.parent());
    while let Some(c) = cur {
        if world.get::<Node>(c).is_none() {
            break;
        }
        if let (Some(view), Some(mut s)) = (
            world.get::<ComputedNode>(c).map(|c| c.rect),
            world.get_mut::<ScrollState>(c),
        ) {
            let mut t = s.target;
            if target.y < view.y + margin {
                t.y -= view.y + margin - target.y;
            } else if target.y + target.h > view.y + view.h - margin {
                t.y += target.y + target.h - (view.y + view.h - margin);
            }
            if target.x < view.x + margin {
                t.x -= view.x + margin - target.x;
            } else if target.x + target.w > view.x + view.w - margin {
                t.x += target.x + target.w - (view.x + view.w - margin);
            }
            t = t.max(Vec2::ZERO);
            if t != s.target {
                s.target = t;
            }
        }
        cur = world.get::<ChildOf>(c).map(|p| p.parent());
    }
}

/// Activate a node as if clicked: sends [`UiEvent::Action`] when it has an [`OnClick`] and
/// isn't disabled. Returns whether an action was sent.
pub fn activate(world: &mut World, e: Entity, alt: bool) -> bool {
    let Some(action) = world.get::<OnClick>(e).map(|a| a.0.clone()) else {
        return false;
    };
    if world.get::<Interaction>(e).is_some_and(|i| i.disabled) {
        return false;
    }
    let id = world.get::<UiId>(e).map(|i| i.0.clone());
    builtin_action(world, Some(e), &action);
    send(
        world,
        UiEvent::Action {
            action,
            id,
            alt,
            entity: Some(e),
        },
    );
    true
}

/// Run an action string as if a button carrying it was clicked: built-in `ui.*` actions run
/// and a [`UiEvent::Action`] is sent (with no node). Used by dialogue choices and scripts.
pub fn dispatch(world: &mut World, action: &str) {
    builtin_action(world, None, action);
    send(
        world,
        UiEvent::Action {
            action: action.to_string(),
            id: None,
            alt: false,
            entity: None,
        },
    );
}

/// Actions the UI handles itself: `ui.close` (the enclosing screen, or the top screen when
/// there is no node), `ui.show:<screen>`, `ui.hide:<screen>`, `ui.toggle:<screen>`.
fn builtin_action(world: &mut World, e: Option<Entity>, action: &str) {
    if action == "ui.close" {
        let root = match e {
            Some(e) => Some(root_of(world, e)),
            None => screens::open_screens(world)
                .into_iter()
                .find(|(key, ..)| !matches!(key.as_str(), "hud" | "toasts" | "tooltip" | "drag"))
                .map(|(_, e, ..)| e),
        };
        if let Some(root) = root {
            screens::close_screen(world, root, false);
        }
    } else if let Some(key) = action.strip_prefix("ui.show:") {
        if let Err(err) = screens::show(world, key, &serde_json::Value::Null) {
            tracing::warn!("{err}");
        }
    } else if let Some(key) = action.strip_prefix("ui.hide:") {
        screens::hide(world, key, false);
    } else if let Some(key) = action.strip_prefix("ui.toggle:")
        && !screens::hide(world, key, false)
        && let Err(err) = screens::show(world, key, &serde_json::Value::Null)
    {
        tracing::warn!("{err}");
    }
}

/// Find a node by [`UiId`] (the most recently spawned one if several share it).
pub fn find_id(world: &mut World, id: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &UiId)>();
    q.iter(world)
        .filter(|(_, i)| i.0 == id)
        .map(|(e, _)| e)
        .max_by_key(|e| e.index())
}

/// The input pass. Runs in `PreUpdate` after the input plugin has read the devices, using
/// last frame's layout.
pub fn ui_input_system(world: &mut World) {
    let dt = world
        .get_resource::<Time>()
        .map(|t| t.real_delta_secs())
        .unwrap_or(1.0 / 60.0)
        .min(0.1);
    let mut st = world.remove_resource::<UiInputState>().unwrap_or_default();
    let dev = Devices::read(world);
    let vp = world.get_resource::<UiViewport>().copied().unwrap_or_default();
    let settings = world.get_resource::<UiSettings>().cloned().unwrap_or_default();

    // Forget entities that are gone (focus follows a rebuilt node with the same id).
    st.hovered = alive(world, st.hovered);
    if alive(world, st.focused).is_none() && st.focused.is_some() {
        st.focused = st.focused_id.clone().and_then(|id| {
            let scope = alive(world, st.scope)?;
            screens::find_in(world, scope, &id)
        });
    }
    st.highlighted = alive(world, st.highlighted);
    st.scope = alive(world, st.scope);
    st.tooltip = alive(world, st.tooltip);
    st.press_hit = alive(world, st.press_hit);
    if st.pressed.is_some_and(|(e, _)| alive(world, Some(e)).is_none()) {
        st.pressed = None;
    }
    if let Some(d) = &st.drag
        && alive(world, Some(d.source)).is_none()
    {
        if let Some(g) = d.ghost
            && let Ok(g) = world.get_entity_mut(g)
        {
            g.despawn();
        }
        st.drag = None;
    }
    st.focus_memory
        .retain(|k, v| world.get_entity(*k).is_ok() && world.get_entity(*v).is_ok());

    // Input scope: the topmost open modal screen.
    let open = screens::open_screens(world);
    let modal = open.iter().find(|(_, _, _, m)| *m).map(|(_, e, l, _)| (*e, *l));
    let scope = modal.map(|(e, _)| e);
    if scope != st.scope {
        if let (Some(old), Some(f)) = (st.scope, st.focused) {
            st.focus_memory.insert(old, f);
        }
        st.scope = scope;
        st.focused = scope.and_then(|s| st.focus_memory.get(&s).copied());
        st.repeat_dir = None;
    }

    // Mode switches: moving the mouse shows the pointer, keys/pad show focus.
    let moved = match (dev.cursor, st.last_cursor) {
        (Some(a), Some(b)) => (a - b).length() > 3.0,
        (Some(_), None) => true,
        _ => false,
    };
    if dev.cursor != st.last_cursor && (moved || dev.left_down || dev.right_down) {
        st.last_cursor = dev.cursor;
        if moved || dev.left_down {
            st.mode = NavMode::Pointer;
        }
    }
    if dev.any_directional_input() && !dev.left_down && !dev.right_down {
        st.mode = NavMode::Directional;
        st.last_gamepad = dev.keys_down.is_empty();
    }

    // ---------------------------------------------------------------- pointer
    let hit = dev.cursor.and_then(|p| hit_test(world, p, modal.map(|(_, l)| l)));
    st.hit = hit;
    let dragging_payload = st
        .drag
        .as_ref()
        .filter(|d| d.started)
        .map(|d| d.payload.clone());
    let hover_target = match (&dragging_payload, hit) {
        (Some(p), Some(h)) => drop_target(world, h, p),
        (None, Some(h)) => interactive(world, h),
        _ => None,
    };
    let hover_target = if st.mode == NavMode::Pointer || dragging_payload.is_some() {
        hover_target
    } else {
        None
    };
    if hover_target != st.hovered {
        if let Some(old) = st.hovered {
            set_interaction(world, old, |i| i.hovered = false);
        }
        if let Some(new) = hover_target {
            set_interaction(world, new, |i| i.hovered = true);
        }
        st.hovered = hover_target;
    }

    // Press.
    if (dev.left_down || dev.right_down)
        && let Some(h) = hit
    {
        let alt = !dev.left_down;
        st.press_hit = Some(h);
        st.press_pos = dev.cursor.unwrap_or_default();
        if let Some(target) = interactive(world, h) {
            st.pressed = Some((target, alt));
            set_interaction(world, target, |i| i.pressed = true);
            if world.get::<Focusable>(target).is_some() {
                st.focused = Some(target);
            }
            if !alt
                && st.drag.is_none()
                && let Some(src) = ancestor_with::<Draggable>(world, h)
            {
                let payload = world.get::<Draggable>(src).unwrap().payload.clone();
                let r = world.get::<ComputedNode>(src).map(|c| c.rect).unwrap_or_default();
                st.drag = Some(Drag {
                    source: src,
                    payload,
                    ghost: None,
                    started: false,
                    directional: false,
                    grab: st.press_pos - r.min(),
                });
            }
        }
    }

    // Start a pointer drag after moving a little.
    if let (Some(d), Some(p)) = (&mut st.drag, dev.cursor)
        && !d.started
        && !d.directional
        && (p - st.press_pos).length() > 6.0 * vp.scale
    {
        d.started = true;
        let payload = d.payload.clone();
        let source = d.source;
        d.ghost = spawn_ghost(world, source, &vp);
        if let Some((pressed, _)) = st.pressed.take() {
            set_interaction(world, pressed, |i| i.pressed = false);
        }
        send(world, UiEvent::DragStarted { payload });
    }

    // Release.
    if dev.left_up || dev.right_up {
        let alt = !dev.left_up;
        if let Some(d) = st.drag.take() {
            if d.started && !d.directional {
                finish_drag(world, &d, hit);
            } else if d.directional {
                st.drag = Some(d);
            }
        }
        if let Some((pressed, was_alt)) = st.pressed.take() {
            set_interaction(world, pressed, |i| i.pressed = false);
            let over = hit.and_then(|h| interactive(world, h));
            if over == Some(pressed) && was_alt == alt {
                activate(world, pressed, alt);
            }
        }
        st.press_hit = None;
    }

    // Wheel scrolls the scroll container under the cursor.
    if dev.scroll.y != 0.0
        && let Some(h) = hit
        && let Some(sc) = ancestor_with::<ScrollState>(world, h)
        && let Some(mut s) = world.get_mut::<ScrollState>(sc)
    {
        s.target.y = (s.target.y - dev.scroll.y * 64.0 * vp.scale).max(0.0);
    }

    // ---------------------------------------------------------------- keyboard / gamepad
    if let Some(scope) = scope {
        let screen_key = world
            .get::<Screen>(scope)
            .map(|s| s.key.clone())
            .unwrap_or_default();
        let list = focusables(world, scope);
        if st.focused.is_some_and(|f| !list.contains(&f)) {
            st.focused = None;
        }
        // Repeat directional moves while held.
        let dir = dev.held_dir();
        let mut step = None;
        if dir != st.repeat_dir {
            st.repeat_dir = dir;
            st.repeat_timer = 0.38;
            step = dir;
        } else if let Some(d) = dir {
            st.repeat_timer -= dt;
            if st.repeat_timer <= 0.0 {
                st.repeat_timer = 0.09;
                step = Some(d);
            }
        }
        let adjust = match (step, st.focused) {
            (Some(NavDir::Left | NavDir::Right), Some(f)) if world.get::<Adjustable>(f).is_some() => {
                Some(f)
            }
            _ => None,
        };
        if let (Some(f), Some(d)) = (adjust, step) {
            let id = world.get::<UiId>(f).map(|i| i.0.clone());
            send(
                world,
                UiEvent::Adjust {
                    id,
                    delta: if d == NavDir::Left { -1 } else { 1 },
                    entity: Some(f),
                },
            );
        } else if let Some(d) = step {
            st.mode = NavMode::Directional;
            let next = match st.focused {
                None => first_focus(world, &list),
                Some(f) => nearest_in_direction(world, f, d, &list).or(Some(f)),
            };
            if next != st.focused {
                st.focused = next;
                if let Some(n) = next {
                    scroll_into_view(world, n, 12.0 * vp.scale);
                }
            }
        }
        let shift = dev.held(KeyCode::ShiftLeft) || dev.held(KeyCode::ShiftRight);
        if dev.key(KeyCode::Tab) && !list.is_empty() {
            st.mode = NavMode::Directional;
            let i = st
                .focused
                .and_then(|f| list.iter().position(|e| *e == f))
                .map(|i| {
                    if shift {
                        (i + list.len() - 1) % list.len()
                    } else {
                        (i + 1) % list.len()
                    }
                })
                .unwrap_or(0);
            st.focused = Some(list[i]);
            scroll_into_view(world, list[i], 12.0 * vp.scale);
        }

        let accept = dev.key(KeyCode::Enter)
            || dev.key(KeyCode::NumpadEnter)
            || dev.key(KeyCode::Space)
            || dev.pad(GamepadButton::South);
        let alt = dev.pad(GamepadButton::West);
        let back = dev.key(KeyCode::Escape) || dev.pad(GamepadButton::East);
        if accept || alt {
            st.mode = NavMode::Directional;
            if st.focused.is_none() {
                st.focused = first_focus(world, &list);
            } else if let Some(f) = st.focused {
                let directional_drag = st.drag.as_ref().is_some_and(|d| d.directional);
                if directional_drag && accept {
                    let d = st.drag.take().unwrap();
                    finish_drag(world, &d, Some(f));
                } else if accept
                    && world.get::<OnClick>(f).is_none()
                    && let Some(payload) = world.get::<Draggable>(f).map(|d| d.payload.clone())
                {
                    st.drag = Some(Drag {
                        source: f,
                        payload: payload.clone(),
                        ghost: spawn_ghost(world, f, &vp),
                        started: true,
                        directional: true,
                        grab: Vec2::ZERO,
                    });
                    send(world, UiEvent::DragStarted { payload });
                } else {
                    activate(world, f, alt);
                }
            }
        }
        if back {
            if let Some(d) = st.drag.take() {
                cancel_drag(world, &d);
            } else {
                send(
                    world,
                    UiEvent::Back {
                        screen: screen_key.clone(),
                    },
                );
                if world.get::<Screen>(scope).is_some_and(|s| s.closable) {
                    screens::close_screen(world, scope, false);
                }
            }
        }
        let tab = if dev.key(KeyCode::Q) || dev.pad(GamepadButton::LeftBumper) {
            -1
        } else if dev.key(KeyCode::E) || dev.pad(GamepadButton::RightBumper) {
            1
        } else {
            0
        };
        if tab != 0 {
            send(
                world,
                UiEvent::Tab {
                    screen: screen_key,
                    delta: tab,
                },
            );
        }
        // Right stick scrolls the container around the focus.
        if dev.right_stick.y.abs() > 0.2
            && let Some(f) = st.focused.or(list.first().copied())
            && let Some(sc) = ancestor_with::<ScrollState>(world, f)
            && let Some(mut s) = world.get_mut::<ScrollState>(sc)
        {
            s.target.y = (s.target.y - dev.right_stick.y * 1100.0 * vp.scale * dt).max(0.0);
        }
    } else if st.drag.as_ref().is_some_and(|d| d.directional) {
        let d = st.drag.take().unwrap();
        cancel_drag(world, &d);
    }

    st.focused_id = st
        .focused
        .and_then(|f| world.get::<UiId>(f).map(|i| i.0.clone()));

    // Focus visuals: only while driving the UI with keys or a gamepad.
    let show_focus = if st.mode == NavMode::Directional {
        st.focused
    } else {
        None
    };
    {
        let mut q = world.query::<(Entity, &Interaction)>();
        let stale: Vec<Entity> = q
            .iter(world)
            .filter(|(e, i)| i.focused && Some(*e) != show_focus)
            .map(|(e, _)| e)
            .collect();
        for e in stale {
            set_interaction(world, e, |i| i.focused = false);
        }
    }
    if let Some(f) = show_focus {
        set_interaction(world, f, |i| i.focused = true);
    }

    // Highlight (detail panels follow the hovered or focused item).
    let highlighted = if st.mode == NavMode::Directional {
        st.focused
    } else {
        st.hovered.filter(|_| st.drag.is_none())
    };
    if highlighted != st.highlighted && (highlighted.is_some() || st.mode == NavMode::Directional) {
        st.highlighted = highlighted;
        let id = highlighted.and_then(|e| world.get::<UiId>(e).map(|i| i.0.clone()));
        send(
            world,
            UiEvent::Highlighted {
                id,
                entity: highlighted,
            },
        );
    }

    // Ghost follows the cursor (or the focus).
    if let Some(d) = &st.drag
        && let Some(g) = d.ghost
    {
        let size = world
            .get::<ComputedNode>(g)
            .map(|c| c.layout_size)
            .unwrap_or_default();
        let pos = if d.directional {
            st.focused
                .and_then(|f| world.get::<ComputedNode>(f))
                .map(|c| c.rect.min() + Vec2::new(c.rect.w * 0.35, -c.rect.h * 0.35))
        } else {
            dev.cursor.map(|p| p - d.grab)
        };
        if let Some(p) = pos {
            let p = keep_inside(Rect::new(p.x, p.y, size.x, size.y), &vp, 0.0) / vp.scale;
            place(world, g, p);
        }
    }

    update_tooltip(world, &mut st, &dev, &vp, &settings, dt);

    // Capture and game input blocking.
    let capture = UiInputCapture {
        pointer: hit.is_some() || modal.is_some() || st.drag.as_ref().is_some_and(|d| d.started),
        keyboard: modal.is_some(),
        dragging: st.drag.as_ref().is_some_and(|d| d.started),
    };
    if world.get_resource::<UiInputCapture>() != Some(&capture) {
        world.insert_resource(capture);
    }
    if capture.keyboard
        && settings.block_game_input
        && let Some(mut actions) = world.get_resource_mut::<ActionState>()
    {
        for v in actions.actions.values_mut() {
            let was = v.pressed;
            *v = Default::default();
            v.just_released = was;
        }
    }
    world.insert_resource(st);
}

fn first_focus(world: &World, list: &[Entity]) -> Option<Entity> {
    list.iter()
        .copied()
        .find(|e| world.get::<Interaction>(*e).is_some_and(|i| i.selected && !i.disabled))
        .or_else(|| {
            list.iter()
                .copied()
                .find(|e| world.get::<Interaction>(*e).is_some_and(|i| !i.disabled))
        })
        .or(list.first().copied())
}

/// Move an absolutely placed root to `p` (logical px).
fn place(world: &mut World, e: Entity, p: Vec2) {
    if let Some(mut n) = world.get_mut::<Node>(e) {
        let left = crate::style::Val::Px(p.x.round());
        let top = crate::style::Val::Px(p.y.round());
        if n.style.left != Some(left) || n.style.top != Some(top) {
            n.style.left = Some(left);
            n.style.top = Some(top);
        }
    }
}

fn spawn_ghost(world: &mut World, source: Entity, vp: &UiViewport) -> Option<Entity> {
    let c = world.get::<ComputedNode>(source)?.clone();
    let icon = screens::subtree(world, source)
        .into_iter()
        .find_map(|e| world.get::<Icon>(e).cloned());
    let size = c.layout_size / vp.scale.max(1e-3);
    let mut el = El::new().class("drag_ghost").with_style(|s| {
        s.at(c.rect.x / vp.scale, c.rect.y / vp.scale)
            .size(size.x, size.y)
            .pick(Pick::Ignore)
            .padding(size.x.min(size.y) * 0.14)
    });
    if let Some(icon) = icon {
        el = el.child(
            El::icon(icon.name)
                .with_style(|s| s.size(crate::style::pct(100.0), crate::style::pct(100.0)).pick(Pick::Ignore)),
        );
    }
    let spawned = el::spawn(world, None, &el).ok()?;
    let root = spawned.root?;
    world.entity_mut(root).insert(Screen {
        key: "drag".into(),
        modal: false,
        layer: 900,
        presence: 0.4,
        closing: false,
        closable: false,
    });
    set_interaction(world, source, |i| i.pressed = true);
    Some(root)
}

fn end_drag_visuals(world: &mut World, d: &Drag) {
    if let Some(g) = d.ghost
        && let Ok(g) = world.get_entity_mut(g)
    {
        g.despawn();
    }
    set_interaction(world, d.source, |i| i.pressed = false);
}

fn finish_drag(world: &mut World, d: &Drag, over: Option<Entity>) {
    end_drag_visuals(world, d);
    match over.and_then(|h| drop_target(world, h, &d.payload)) {
        Some(t) if t != d.source => {
            let slot = world.get::<DropTarget>(t).map(|t| t.slot.clone()).unwrap_or_default();
            set_interaction(world, t, |i| i.hovered = false);
            send(
                world,
                UiEvent::Dropped {
                    payload: d.payload.clone(),
                    slot,
                    source: Some(d.source),
                    target: Some(t),
                },
            );
        }
        _ => send(
            world,
            UiEvent::DragCancelled {
                payload: d.payload.clone(),
            },
        ),
    }
}

fn cancel_drag(world: &mut World, d: &Drag) {
    end_drag_visuals(world, d);
    send(
        world,
        UiEvent::DragCancelled {
            payload: d.payload.clone(),
        },
    );
}

fn update_tooltip(
    world: &mut World,
    st: &mut UiInputState,
    dev: &Devices,
    vp: &UiViewport,
    settings: &UiSettings,
    dt: f32,
) {
    let target = if st.drag.is_some() {
        None
    } else if st.mode == NavMode::Directional {
        st.focused.filter(|f| world.get::<Tooltip>(*f).is_some())
    } else {
        st.hit.and_then(|h| ancestor_with::<Tooltip>(world, h))
    };
    if target != st.tooltip_target {
        st.tooltip_target = target;
        st.tooltip_time = 0.0;
        if let Some(t) = st.tooltip.take()
            && let Ok(t) = world.get_entity_mut(t)
        {
            t.despawn();
        }
    }
    let Some(target) = target else {
        return;
    };
    st.tooltip_time += dt;
    if st.tooltip_time < settings.tooltip_delay {
        return;
    }
    let Some(tip) = world.get::<Tooltip>(target).cloned() else {
        return;
    };
    let content = (tip.title.clone(), tip.text.clone());
    if st.tooltip.is_some() && st.tooltip_content != content {
        if let Some(t) = st.tooltip.take()
            && let Ok(t) = world.get_entity_mut(t)
        {
            t.despawn();
        }
    }
    if st.tooltip.is_none() {
        let mut el = El::new()
            .class("tooltip")
            .with_style(|s| s.at(-1000.0, -1000.0).pick(Pick::Ignore));
        if !tip.title.is_empty() {
            let mut title = El::text(tip.title.clone()).class("heading").pick_ignore();
            if let Some(c) = tip.title_color {
                title = title.text_style(|t| t.color(c));
            }
            el = el.child(title);
        }
        if !tip.text.is_empty() {
            el = el.child(El::text(tip.text.clone()).class("body").pick_ignore());
        }
        if let Ok(sp) = el::spawn(world, None, &el)
            && let Some(root) = sp.root
        {
            world.entity_mut(root).insert(Screen {
                key: "tooltip".into(),
                modal: false,
                layer: 1000,
                presence: 0.0,
                closing: false,
                closable: false,
            });
            st.tooltip = Some(root);
            st.tooltip_content = content;
        }
    }
    let Some(t) = st.tooltip else {
        return;
    };
    let size = world
        .get::<ComputedNode>(t)
        .map(|c| c.layout_size)
        .unwrap_or_default();
    let anchor = if st.mode == NavMode::Directional {
        world
            .get::<ComputedNode>(target)
            .map(|c| Vec2::new(c.rect.x + c.rect.w + 10.0 * vp.scale, c.rect.y))
    } else {
        dev.cursor.map(|p| p + Vec2::new(18.0, 22.0) * vp.scale)
    };
    if let Some(mut p) = anchor {
        // Flip to the other side instead of covering the target.
        if p.x + size.x > vp.width - 8.0 * vp.scale {
            let left = world
                .get::<ComputedNode>(target)
                .map(|c| c.rect.x)
                .unwrap_or(p.x);
            p.x = if st.mode == NavMode::Directional {
                left - size.x - 10.0 * vp.scale
            } else {
                dev.cursor.map(|c| c.x).unwrap_or(p.x) - size.x - 12.0 * vp.scale
            };
        }
        let p = keep_inside(Rect::new(p.x, p.y, size.x, size.y), vp, 8.0 * vp.scale) / vp.scale;
        // Wait for the first layout before revealing the tooltip at its place.
        if size.x > 0.0 {
            place(world, t, p);
        }
    }
}

/// Record every UI event for `ui.events` (the last 64).
#[derive(Resource, Debug, Default)]
pub struct UiEventLog {
    pub events: std::collections::VecDeque<(u64, serde_json::Value)>,
    pub next: u64,
}

/// Copies this frame's [`UiEvent`]s into the [`UiEventLog`].
pub fn log_events_system(mut reader: MessageReader<UiEvent>, mut log: ResMut<UiEventLog>) {
    for ev in reader.read() {
        let n = log.next;
        log.next += 1;
        if let Ok(v) = serde_json::to_value(ev) {
            log.events.push_back((n, v));
        }
        while log.events.len() > 64 {
            log.events.pop_front();
        }
    }
}
