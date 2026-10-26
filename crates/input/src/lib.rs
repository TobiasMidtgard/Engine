//! # engine_input
//!
//! Two layers:
//!
//! - **Devices**: [`ButtonInput<KeyCode>`], [`ButtonInput<MouseButton>`],
//!   [`ButtonInput<GamepadButton>`], [`Mouse`] (position, motion, wheel) and [`Gamepads`]
//!   (sticks/triggers). The platform layer feeds them through [`InputEvent`] messages, so
//!   tests, replays and AI agents can inject input the same way.
//! - **Actions**: games read named actions (`"jump"`, `"move"`, `"dash"`) from
//!   [`ActionState`], never raw keys. [`InputMap`] binds actions to keys, buttons and axes and
//!   is plain JSON, so players (or an AI) can rebind at runtime.
//!
//! Fixed-step gameplay: an action pressed for a single frame might fall between fixed steps
//! (or span several). Use [`ActionState::just_pressed`] in `Update` systems and
//! [`ActionState::pressed_since_tick`] in `FixedUpdate` systems; the latter stays true until
//! a fixed step has seen it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;

use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use serde_json::json;

/// Keyboard keys by physical position (US layout names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum KeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Escape,
    Tab,
    CapsLock,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    Space,
    Enter,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Minus,
    Equal,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Backquote,
    Comma,
    Period,
    Slash,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadEnter,
    NumpadDecimal,
    PrintScreen,
    Pause,
    /// Any other key, by platform scan code.
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Gamepad buttons (Xbox naming; South = A / Cross).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GamepadButton {
    South,
    East,
    North,
    West,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    Select,
    Start,
    Mode,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

/// Pressed / just pressed / just released state for a kind of button.
#[derive(Resource, Debug, Clone)]
pub struct ButtonInput<T: Copy + Eq + Hash + Send + Sync + 'static> {
    pressed: HashSet<T>,
    just_pressed: HashSet<T>,
    just_released: HashSet<T>,
}

impl<T: Copy + Eq + Hash + Send + Sync + 'static> Default for ButtonInput<T> {
    fn default() -> Self {
        Self {
            pressed: HashSet::new(),
            just_pressed: HashSet::new(),
            just_released: HashSet::new(),
        }
    }
}

impl<T: Copy + Eq + Hash + Send + Sync + 'static> ButtonInput<T> {
    pub fn press(&mut self, b: T) {
        if self.pressed.insert(b) {
            self.just_pressed.insert(b);
        }
    }

    pub fn release(&mut self, b: T) {
        if self.pressed.remove(&b) {
            self.just_released.insert(b);
        }
    }

    pub fn release_all(&mut self) {
        let all: Vec<T> = self.pressed.iter().copied().collect();
        for b in all {
            self.release(b);
        }
    }

    pub fn pressed(&self, b: T) -> bool {
        self.pressed.contains(&b)
    }

    pub fn just_pressed(&self, b: T) -> bool {
        self.just_pressed.contains(&b)
    }

    pub fn just_released(&self, b: T) -> bool {
        self.just_released.contains(&b)
    }

    pub fn any_pressed(&self, bs: impl IntoIterator<Item = T>) -> bool {
        bs.into_iter().any(|b| self.pressed(b))
    }

    pub fn get_pressed(&self) -> impl Iterator<Item = &T> {
        self.pressed.iter()
    }

    pub fn get_just_pressed(&self) -> impl Iterator<Item = &T> {
        self.just_pressed.iter()
    }

    /// Forget this frame's transitions (called at the start of each frame).
    pub fn clear_frame(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
    }
}

/// Mouse position and motion.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mouse {
    /// Cursor position in physical pixels from the window's top-left, if over the window.
    pub position: Option<Vec2>,
    /// Raw motion this frame (works while the cursor is locked), pixels.
    pub delta: Vec2,
    /// Wheel movement this frame in lines (y up = away from the user).
    pub scroll: Vec2,
}

/// Connected gamepads' analog state, by gamepad id.
#[derive(Resource, Debug, Clone, Default)]
pub struct Gamepads {
    pub axes: HashMap<(u32, GamepadAxis), f32>,
    pub connected: HashSet<u32>,
    /// Stick dead zone (radial).
    pub dead_zone: f32,
}

impl Gamepads {
    pub fn axis(&self, pad: u32, axis: GamepadAxis) -> f32 {
        self.axes.get(&(pad, axis)).copied().unwrap_or(0.0)
    }

    /// Largest magnitude of `axis` across all pads.
    pub fn any_axis(&self, axis: GamepadAxis) -> f32 {
        self.axes
            .iter()
            .filter(|((_, a), _)| *a == axis)
            .map(|(_, v)| *v)
            .fold(0.0f32, |acc, v| if v.abs() > acc.abs() { v } else { acc })
    }
}

/// Raw input from the platform (or tests / AI).
#[derive(Message, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    Key {
        key: KeyCode,
        pressed: bool,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    CursorMoved {
        position: Vec2,
    },
    CursorLeft,
    MouseMotion {
        delta: Vec2,
    },
    MouseWheel {
        delta: Vec2,
    },
    GamepadButton {
        pad: u32,
        button: GamepadButton,
        pressed: bool,
    },
    GamepadAxis {
        pad: u32,
        axis: GamepadAxis,
        value: f32,
    },
    GamepadConnected {
        pad: u32,
        connected: bool,
    },
    /// The window lost focus: everything is released.
    FocusLost,
}

/// Typed text (for UI text fields), separate from key presses.
#[derive(Message, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextInput(pub String);

/// A file dragged onto the window from the OS (one message per file). The pointer position
/// comes from the usual cursor events, so a drop can land where the pointer is.
#[derive(Message, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDrop {
    /// A file is being dragged over the window.
    Hovered(std::path::PathBuf),
    /// A file was dropped on the window.
    Dropped(std::path::PathBuf),
    /// The drag left the window or was cancelled.
    Cancelled,
}

/// One input that can trigger an action.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    Gamepad(GamepadButton),
    /// An analog axis; `scale` flips or scales it (e.g. -1 for inverted look).
    Axis {
        axis: GamepadAxis,
        scale: f32,
    },
    /// Mouse motion on x or y (for look actions), pixels × scale.
    MouseMotionX {
        scale: f32,
    },
    MouseMotionY {
        scale: f32,
    },
    /// Mouse wheel y.
    Wheel {
        scale: f32,
    },
}

/// How an action maps to inputs. Buttons give 0/1; axes give -1..1; 2D actions combine four
/// directions (keys) and a stick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ActionBinding {
    /// Inputs that press the action (or push it toward +1 for 1D axes).
    pub buttons: Vec<Binding>,
    /// Inputs that push a 1D axis toward −1.
    pub negative: Vec<Binding>,
    /// 2D action: inputs for +y (up/forward), −y, −x, +x.
    pub up: Vec<Binding>,
    pub down: Vec<Binding>,
    pub left: Vec<Binding>,
    pub right: Vec<Binding>,
    /// 2D action: stick axes (x, y).
    pub stick: Option<(GamepadAxis, GamepadAxis)>,
    /// Ignored below this magnitude.
    pub dead_zone: f32,
}

/// Rebindable action map. Plain JSON: `resource.get InputMap`.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InputMap {
    pub actions: BTreeMap<String, ActionBinding>,
}

impl InputMap {
    pub fn bind(&mut self, action: &str, binding: Binding) -> &mut Self {
        self.actions
            .entry(action.into())
            .or_default()
            .buttons
            .push(binding);
        self
    }

    pub fn bind_axis(&mut self, action: &str, positive: Binding, negative: Binding) -> &mut Self {
        let a = self.actions.entry(action.into()).or_default();
        a.buttons.push(positive);
        a.negative.push(negative);
        self
    }

    /// WASD + left stick style 2D action.
    pub fn bind_2d(
        &mut self,
        action: &str,
        up: KeyCode,
        down: KeyCode,
        left: KeyCode,
        right: KeyCode,
        stick: Option<(GamepadAxis, GamepadAxis)>,
    ) -> &mut Self {
        let a = self.actions.entry(action.into()).or_default();
        a.up.push(Binding::Key(up));
        a.down.push(Binding::Key(down));
        a.left.push(Binding::Key(left));
        a.right.push(Binding::Key(right));
        a.stick = stick;
        a.dead_zone = 0.15;
        self
    }

    /// A sensible default map for action games (keyboard/mouse + gamepad).
    pub fn default_game() -> Self {
        use GamepadAxis::*;
        use KeyCode::*;
        let mut m = Self::default();
        m.bind_2d("move", W, S, A, D, Some((LeftStickX, LeftStickY)));
        let look = m.actions.entry("look".into()).or_default();
        look.right.push(Binding::MouseMotionX { scale: 1.0 });
        look.up.push(Binding::MouseMotionY { scale: -1.0 });
        look.stick = Some((RightStickX, RightStickY));
        look.dead_zone = 0.1;
        m.bind("jump", Binding::Key(Space))
            .bind("jump", Binding::Gamepad(GamepadButton::South));
        m.bind("sprint", Binding::Key(ShiftLeft))
            .bind("sprint", Binding::Gamepad(GamepadButton::LeftStick));
        m.bind("crouch", Binding::Key(ControlLeft))
            .bind("crouch", Binding::Gamepad(GamepadButton::East));
        m.bind("dash", Binding::Key(Q))
            .bind("dash", Binding::Gamepad(GamepadButton::RightBumper));
        m.bind("attack", Binding::Mouse(MouseButton::Left))
            .bind("attack", Binding::Gamepad(GamepadButton::RightTrigger));
        m.bind("block", Binding::Mouse(MouseButton::Right))
            .bind("block", Binding::Gamepad(GamepadButton::LeftTrigger));
        m.bind("interact", Binding::Key(E))
            .bind("interact", Binding::Gamepad(GamepadButton::West));
        m.bind("inventory", Binding::Key(Tab))
            .bind("inventory", Binding::Gamepad(GamepadButton::Select));
        m.bind("map", Binding::Key(M))
            .bind("map", Binding::Gamepad(GamepadButton::DPadUp));
        m.bind("pause", Binding::Key(Escape))
            .bind("pause", Binding::Gamepad(GamepadButton::Start));
        m.bind_axis(
            "zoom",
            Binding::Wheel { scale: 1.0 },
            Binding::Wheel { scale: -1.0 },
        );
        m.bind_axis("ascend", Binding::Key(Space), Binding::Key(ControlLeft));
        m.bind_axis("roll", Binding::Key(E), Binding::Key(Q));
        m
    }
}

/// Current value of one action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionValue {
    pub pressed: bool,
    pub just_pressed: bool,
    pub just_released: bool,
    /// Pressed at some point since the last fixed step ran.
    pub pressed_since_tick: bool,
    /// 1D value (−1..1, or unbounded for mouse motion).
    pub value: f32,
    /// 2D value (x right, y up/forward).
    pub axis: Vec2,
    /// Seconds held (real time).
    pub held_secs: f32,
}

/// Resolved actions for this frame. Read this in gameplay code.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionState {
    pub actions: BTreeMap<String, ActionValue>,
    /// Actions forced by tools/AI (`input.press`), applied on top of devices.
    #[serde(skip)]
    overrides: HashMap<String, ActionOverride>,
}

#[derive(Debug, Clone, Copy)]
struct ActionOverride {
    pressed: bool,
    value: f32,
    axis: Vec2,
    /// Remaining frames (None = until released).
    frames: Option<u32>,
}

impl ActionState {
    pub fn get(&self, action: &str) -> ActionValue {
        self.actions.get(action).copied().unwrap_or_default()
    }

    pub fn pressed(&self, action: &str) -> bool {
        self.get(action).pressed
    }

    pub fn just_pressed(&self, action: &str) -> bool {
        self.get(action).just_pressed
    }

    pub fn just_released(&self, action: &str) -> bool {
        self.get(action).just_released
    }

    /// For `FixedUpdate` systems: true if pressed since the previous fixed step.
    pub fn pressed_since_tick(&self, action: &str) -> bool {
        self.get(action).pressed_since_tick
    }

    pub fn value(&self, action: &str) -> f32 {
        self.get(action).value
    }

    pub fn axis(&self, action: &str) -> Vec2 {
        self.get(action).axis
    }

    /// Hold an action down (until [`ActionState::release`]) — for AI agents and tests.
    pub fn press(&mut self, action: &str) {
        self.overrides.insert(
            action.into(),
            ActionOverride {
                pressed: true,
                value: 1.0,
                axis: Vec2::ZERO,
                frames: None,
            },
        );
    }

    /// Press for `frames` frames.
    pub fn tap(&mut self, action: &str, frames: u32) {
        self.overrides.insert(
            action.into(),
            ActionOverride {
                pressed: true,
                value: 1.0,
                axis: Vec2::ZERO,
                frames: Some(frames.max(1)),
            },
        );
    }

    /// Hold a 2D/1D value (e.g. `move` = (0, 1) to walk forward).
    pub fn set_axis(&mut self, action: &str, axis: Vec2, frames: Option<u32>) {
        self.overrides.insert(
            action.into(),
            ActionOverride {
                pressed: axis.length_squared() > 0.0,
                value: axis.x,
                axis,
                frames,
            },
        );
    }

    pub fn release(&mut self, action: &str) {
        self.overrides.remove(action);
    }

    pub fn release_all_overrides(&mut self) {
        self.overrides.clear();
    }
}

fn binding_value(
    b: &Binding,
    keys: &ButtonInput<KeyCode>,
    mouse_buttons: &ButtonInput<MouseButton>,
    pad_buttons: &ButtonInput<GamepadButton>,
    mouse: &Mouse,
    pads: &Gamepads,
) -> f32 {
    match *b {
        Binding::Key(k) => keys.pressed(k) as u8 as f32,
        Binding::Mouse(m) => mouse_buttons.pressed(m) as u8 as f32,
        Binding::Gamepad(g) => pad_buttons.pressed(g) as u8 as f32,
        Binding::Axis { axis, scale } => pads.any_axis(axis) * scale,
        Binding::MouseMotionX { scale } => mouse.delta.x * scale,
        Binding::MouseMotionY { scale } => mouse.delta.y * scale,
        Binding::Wheel { scale } => (mouse.scroll.y * scale).max(0.0),
    }
}

/// Start of frame: forget last frame's transitions and motion.
pub fn clear_input_system(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse_buttons: ResMut<ButtonInput<MouseButton>>,
    mut pad_buttons: ResMut<ButtonInput<GamepadButton>>,
    mut mouse: ResMut<Mouse>,
) {
    keys.clear_frame();
    mouse_buttons.clear_frame();
    pad_buttons.clear_frame();
    mouse.delta = Vec2::ZERO;
    mouse.scroll = Vec2::ZERO;
}

/// Apply this frame's [`InputEvent`]s to the device resources.
pub fn apply_input_events_system(
    mut events: MessageReader<InputEvent>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse_buttons: ResMut<ButtonInput<MouseButton>>,
    mut pad_buttons: ResMut<ButtonInput<GamepadButton>>,
    mut mouse: ResMut<Mouse>,
    mut pads: ResMut<Gamepads>,
) {
    for e in events.read() {
        match *e {
            InputEvent::Key { key, pressed } => {
                if pressed {
                    keys.press(key)
                } else {
                    keys.release(key)
                }
            }
            InputEvent::MouseButton { button, pressed } => {
                if pressed {
                    mouse_buttons.press(button)
                } else {
                    mouse_buttons.release(button)
                }
            }
            InputEvent::CursorMoved { position } => mouse.position = Some(position),
            InputEvent::CursorLeft => mouse.position = None,
            InputEvent::MouseMotion { delta } => mouse.delta += delta,
            InputEvent::MouseWheel { delta } => mouse.scroll += delta,
            InputEvent::GamepadButton {
                button, pressed, ..
            } => {
                if pressed {
                    pad_buttons.press(button)
                } else {
                    pad_buttons.release(button)
                }
            }
            InputEvent::GamepadAxis { pad, axis, value } => {
                pads.axes.insert((pad, axis), value);
            }
            InputEvent::GamepadConnected { pad, connected } => {
                if connected {
                    pads.connected.insert(pad);
                } else {
                    pads.connected.remove(&pad);
                    pads.axes.retain(|(p, _), _| *p != pad);
                }
            }
            InputEvent::FocusLost => {
                keys.release_all();
                mouse_buttons.release_all();
                pad_buttons.release_all();
            }
        }
    }
}

/// Resolve [`InputMap`] bindings into [`ActionState`].
#[allow(clippy::too_many_arguments)]
pub fn update_actions_system(
    map: Res<InputMap>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    pad_buttons: Res<ButtonInput<GamepadButton>>,
    mouse: Res<Mouse>,
    pads: Res<Gamepads>,
    time: Res<Time>,
    mut state: ResMut<ActionState>,
) {
    let dt = time.real_delta_secs();
    let v = |b: &Binding| binding_value(b, &keys, &mouse_buttons, &pad_buttons, &mouse, &pads);
    let sum = |bs: &[Binding]| bs.iter().map(v).sum::<f32>();
    let mut names: Vec<String> = map.actions.keys().cloned().collect();
    for k in state.overrides.keys() {
        if !map.actions.contains_key(k) {
            names.push(k.clone());
        }
    }
    for name in names {
        let (mut value, mut axis) = (0.0f32, Vec2::ZERO);
        if let Some(b) = map.actions.get(&name) {
            value = sum(&b.buttons) - sum(&b.negative);
            axis = Vec2::new(sum(&b.right) - sum(&b.left), sum(&b.up) - sum(&b.down));
            if let Some((x, y)) = b.stick {
                let stick = Vec2::new(pads.any_axis(x), pads.any_axis(y));
                if stick.length() > b.dead_zone {
                    axis += stick;
                }
            }
            // Keyboard diagonals shouldn't be faster than straight lines.
            let is_digital =
                b.right.iter().chain(&b.up).all(|b| {
                    matches!(b, Binding::Key(_) | Binding::Mouse(_) | Binding::Gamepad(_))
                });
            if is_digital && axis.length() > 1.0 {
                axis = axis.normalize();
            }
            if value.abs() < b.dead_zone {
                value = 0.0;
            }
        }
        let over = state.overrides.get(&name).copied();
        let mut pressed = value.abs() > 0.5 || axis.length() > 0.5;
        if let Some(o) = over {
            pressed |= o.pressed;
            if o.value != 0.0 {
                value = o.value;
            }
            if o.axis != Vec2::ZERO {
                axis = o.axis;
            }
        }
        let prev = state.actions.get(&name).copied().unwrap_or_default();
        let entry = ActionValue {
            pressed,
            just_pressed: pressed && !prev.pressed,
            just_released: !pressed && prev.pressed,
            pressed_since_tick: prev.pressed_since_tick || (pressed && !prev.pressed),
            value,
            axis,
            held_secs: if pressed { prev.held_secs + dt } else { 0.0 },
        };
        state.actions.insert(name, entry);
    }
    // Count down timed overrides.
    state.overrides.retain(|_, o| match &mut o.frames {
        Some(f) => {
            *f -= 1;
            *f > 0
        }
        None => true,
    });
}

/// After each fixed step, latch-cleared presses are consumed.
pub fn consume_fixed_presses_system(mut state: ResMut<ActionState>) {
    for v in state.actions.values_mut() {
        v.pressed_since_tick = false;
    }
}

/// Input plugin: devices, actions and the `input.*` API.
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<GamepadButton>>()
            .init_resource::<Mouse>()
            .insert_resource(Gamepads {
                dead_zone: 0.15,
                ..Default::default()
            })
            .insert_resource(InputMap::default_game())
            .init_resource::<ActionState>()
            .add_message::<InputEvent>()
            .add_message::<TextInput>()
            .add_message::<FileDrop>()
            .register_resource::<InputMap>(
                "Action bindings: {actions: {name: {buttons: [{key: \"Space\"}], up/down/left/right: [...], stick: [\"LeftStickX\",\"LeftStickY\"]}}}",
            )
            .register_resource::<Mouse>("Mouse position (physical px), motion delta and wheel this frame.")
            .add_systems(
                PreUpdate,
                (clear_input_system, apply_input_events_system, update_actions_system).chain(),
            )
            .add_systems(FixedPostUpdate, consume_fixed_presses_system);

        app.register_api(
            "input.state",
            "Current value of every action (pressed, value, axis).",
            "{}",
            |world, _| {
                let s = world.resource::<ActionState>();
                serde_json::to_value(&s.actions).map_err(|e| e.to_string())
            },
        );
        app.register_api(
            "input.press",
            "Hold an action down until input.release (or for `frames` frames).",
            r#"{"action": "jump", "frames": 1}"#,
            |world, args| {
                let action = args["action"].as_str().ok_or("missing \"action\"")?;
                let mut s = world.resource_mut::<ActionState>();
                match args.get("frames").and_then(|f| f.as_u64()) {
                    Some(f) => s.tap(action, f as u32),
                    None => s.press(action),
                }
                Ok(json!({"ok": true}))
            },
        );
        app.register_api(
            "input.axis",
            "Hold a 2D action value, e.g. move forward: {action: move, x: 0, y: 1}.",
            r#"{"action": "move", "x": 0.0, "y": 1.0, "frames": 30}"#,
            |world, args| {
                let action = args["action"].as_str().ok_or("missing \"action\"")?;
                let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let frames = args
                    .get("frames")
                    .and_then(|f| f.as_u64())
                    .map(|f| f as u32);
                world
                    .resource_mut::<ActionState>()
                    .set_axis(action, Vec2::new(x, y), frames);
                Ok(json!({"ok": true}))
            },
        );
        app.register_api(
            "input.release",
            "Release an action held with input.press/input.axis (or all with {\"all\": true}).",
            r#"{"action": "jump"}"#,
            |world, args| {
                let mut s = world.resource_mut::<ActionState>();
                if args.get("all").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s.release_all_overrides();
                } else {
                    s.release(args["action"].as_str().ok_or("missing \"action\"")?);
                }
                Ok(json!({"ok": true}))
            },
        );
        app.register_api(
            "input.send",
            "Inject a raw device event, e.g. {\"Key\": {\"key\": \"W\", \"pressed\": true}}.",
            r#"{"Key": {"key": "W", "pressed": true}}"#,
            |world, args| {
                let e: InputEvent = serde_json::from_value(args).map_err(|e| e.to_string())?;
                world.write_message(e);
                Ok(json!({"ok": true}))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(InputPlugin);
        app
    }

    #[test]
    fn key_press_drives_actions() {
        let mut app = app();
        app.world.write_message(InputEvent::Key {
            key: KeyCode::Space,
            pressed: true,
        });
        app.update();
        let s = app.world.resource::<ActionState>();
        assert!(s.pressed("jump") && s.just_pressed("jump"));
        app.update();
        let s = app.world.resource::<ActionState>();
        assert!(s.pressed("jump") && !s.just_pressed("jump"));
        app.world.write_message(InputEvent::Key {
            key: KeyCode::Space,
            pressed: false,
        });
        app.update();
        assert!(app.world.resource::<ActionState>().just_released("jump"));
    }

    #[test]
    fn file_drops_reach_readers() {
        let mut app = app();
        let path = std::path::PathBuf::from("models/crate.glb");
        app.world.write_message(FileDrop::Hovered(path.clone()));
        app.world.write_message(FileDrop::Dropped(path.clone()));
        let messages = app.world.resource::<Messages<FileDrop>>();
        let drops: Vec<_> = messages.get_cursor().read(messages).cloned().collect();
        assert_eq!(
            drops,
            [FileDrop::Hovered(path.clone()), FileDrop::Dropped(path)]
        );
        let json = serde_json::to_string(&FileDrop::Cancelled).unwrap();
        assert_eq!(json, "\"cancelled\"");
    }

    #[test]
    fn wasd_diagonal_is_normalized() {
        let mut app = app();
        for key in [KeyCode::W, KeyCode::D] {
            app.world
                .write_message(InputEvent::Key { key, pressed: true });
        }
        app.update();
        let a = app.world.resource::<ActionState>().axis("move");
        assert!((a.length() - 1.0).abs() < 1e-5, "{a}");
        assert!(a.x > 0.0 && a.y > 0.0);
    }

    #[test]
    fn ai_can_tap_actions() {
        let mut app = app();
        engine_core::registry::call_api(
            &mut app.world,
            "input.press",
            json!({"action": "dash", "frames": 1}),
        )
        .unwrap();
        app.update();
        assert!(app.world.resource::<ActionState>().just_pressed("dash"));
        app.update();
        assert!(!app.world.resource::<ActionState>().pressed("dash"));
    }

    #[test]
    fn fixed_step_sees_short_press() {
        let mut app = app();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 240.0,
            });
        app.world.resource_mut::<ActionState>().tap("jump", 1);
        // 240 fps with 60 Hz fixed steps: the press frame might have no fixed step.
        app.update();
        let latched = app
            .world
            .resource::<ActionState>()
            .get("jump")
            .pressed_since_tick;
        let steps = app.world.resource::<FixedTime>().steps_this_frame();
        assert!(latched || steps > 0);
    }
}
