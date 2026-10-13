//! The quick-select wheel: weapons, abilities and items around a ring, picked by pointing
//! (mouse or either stick) and confirmed with a click, Enter or A; number keys pick a slot at
//! once. The world slows down while it is open (a [`TimeDilation`] hold), so choosing a
//! weapon mid-fight reads like bullet time.
//!
//! ```ignore
//! ui::show(world, "wheel", &json!({"items": [{"id": "rifle", "icon": "rifle", "label": "Rail Rifle",
//!     "count": "4 / 20", "action": "weapon.equip:rifle"}], "current": "rifle", "slow": 0.2}))?;
//! ```
//!
//! Without `items` it shows `wheel.items` from [`UiData`] (and `wheel.current`), so a game
//! keeps its loadout there. Picking a slot sends its `action`, writes its `data` (UiData path
//! → value, e.g. the HUD's `weapon.*`) and `wheel.selected`, and closes the wheel.
//!
//! For "hold to open, release to pick", open it when the key goes down and pass that key as
//! `hold` (a key name like `"Tab"`) and/or `hold_pad` (`"LeftBumper"`): letting go picks the
//! slot the pointer is on, or closes without a change if it never moved.

use std::f32::consts::TAU;

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_core::time::TimeDilation;
use engine_input::{
    ButtonInput, GamepadAxis, GamepadButton, Gamepads, KeyCode, Mouse, MouseButton,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::draw::{DrawCtx, Painters};
use crate::el::El;
use crate::input::UiEvent;
use crate::items::ItemDatabase;
use crate::node::*;
use crate::screens;
use crate::style::{Corners, Fill, Pick, Position};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets;

/// The dilation key the wheel holds while open.
pub const SLOW_KEY: &str = "ui.wheel";

/// One slot on the wheel.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelItem {
    pub id: String,
    pub icon: String,
    pub label: String,
    /// The kind line in the middle ("Weapon · Legendary").
    pub kind: String,
    /// Short text under the icon ("12 / 36", "×4", "Ready").
    pub count: String,
    /// Lines in the middle under the name.
    pub details: Vec<String>,
    /// Slot color (rarity); the accent when unset.
    pub color: Option<Color>,
    /// Sent when picked ("weapon.equip:rifle").
    pub action: String,
    /// UiData values written when picked (path → value).
    pub data: Map<String, Value>,
    /// Shown dimmed and can't be picked (no ammo, cooling down).
    pub disabled: bool,
    /// Used at once instead of taken in hand (a medkit, an ability): picking it leaves the
    /// slot in use as it was.
    pub instant: bool,
}

/// The wheel canvas's state (on the node with `paint: "wheel"`).
#[derive(Component, Debug, Clone, Default)]
pub struct WheelView {
    pub items: Vec<WheelItem>,
    pub hovered: Option<usize>,
    /// The slot in use when it opened.
    pub current: Option<usize>,
    /// Where the pointer points (radians, 0 = up, clockwise).
    pub pointer: Option<f32>,
    /// Opening animation 0 → 1 and per-slot hover easing (real time).
    pub open: f32,
    pub pop: Vec<f32>,
}

/// The open wheel (on its root).
#[derive(Component, Debug, Clone)]
pub struct WheelScreen {
    pub canvas: Entity,
    /// Releasing this key (or pad button) picks the slot under the pointer.
    pub hold: Option<KeyCode>,
    pub hold_pad: Option<GamepadButton>,
    /// The mouse pointer relative to the middle; follows the cursor, or raw motion while the
    /// cursor is locked.
    pub aim: Vec2,
    pub last_cursor: Option<Vec2>,
}

// --------------------------------------------------------------------------------- paint

const R_OUT: f32 = 300.0;
const R_IN: f32 = 150.0;

fn dir(a: f32) -> Vec2 {
    Vec2::new(a.sin(), -a.cos())
}

fn paint_wheel(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(v) = world.get::<WheelView>(e).cloned() else {
        return;
    };
    let p = world.resource::<Theme>().palette.clone();
    let n = v.items.len();
    if n == 0 {
        return;
    }
    let px = ctx.px;
    let c = ctx.rect.center();
    let ease = 1.0 - (1.0 - v.open).powi(3);
    let k = (0.86 + 0.14 * ease) * px;
    let (r_out, r_in) = (R_OUT * k, R_IN * k);
    let sweep = TAU / n as f32;
    let gap = 0.018;
    ctx.faded(ease, |ctx| {
        // A dark well behind the ring.
        ctx.glow(c, r_out * 1.45, Color::hexa(0x000000b0), 1.2);
        for (i, it) in v.items.iter().enumerate() {
            let pop = v.pop.get(i).copied().unwrap_or(0.0);
            let col = it.color.unwrap_or(p.accent);
            let a = i as f32 * sweep;
            let start = a - sweep * 0.5 + gap;
            let len = sweep - gap * 2.0;
            let out = 12.0 * px * pop;
            let (ro, ri) = (r_out + out, r_in + out * 0.4);
            let mid = (ro + ri) * 0.5;
            let thick = ro - ri;
            let dim = if it.disabled { 0.45 } else { 1.0 };
            // The slot: dark glass, lit toward its color when hovered.
            ctx.ring(
                c,
                Vec2::splat(mid),
                thick,
                Color::hexa(0x0a1019d8),
                start,
                len,
                0.0,
            );
            ctx.ring(
                c,
                Vec2::splat(mid),
                thick,
                col.with_alpha(0.07 + 0.2 * pop),
                start,
                len,
                0.0,
            );
            // Light falling in from the outer edge.
            ctx.ring(
                c,
                Vec2::splat(ro - thick * 0.18),
                thick * 0.36,
                col.with_alpha(0.05 + 0.16 * pop),
                start,
                len,
                0.0,
            );
            // Edges: a hairline inside, the outer rim bright when hovered.
            ctx.ring(
                c,
                Vec2::splat(ri),
                1.2 * px,
                Color::WHITE.with_alpha(0.12),
                start,
                len,
                0.0,
            );
            ctx.ring(
                c,
                Vec2::splat(ro),
                (1.4 + 2.8 * pop) * px,
                col.lerp(Color::WHITE, 0.15 * pop)
                    .with_alpha((0.3 + 0.7 * pop) * dim),
                start,
                len,
                0.0,
            );
            if pop > 0.01 {
                for (w, al) in [(10.0, 0.10), (20.0, 0.05)] {
                    ctx.ring(
                        c,
                        Vec2::splat(ro + w * 0.5 * px),
                        w * px,
                        col.with_alpha(al * pop),
                        start,
                        len,
                        0.0,
                    );
                }
            }
            // The slot in use: a bar on its inner edge.
            if v.current == Some(i) {
                ctx.ring(
                    c,
                    Vec2::splat(ri + 5.0 * px),
                    3.0 * px,
                    p.accent_light.with_alpha(0.9),
                    a - sweep * 0.22,
                    sweep * 0.44,
                    0.0,
                );
            }
            // Icon and count.
            let q = c + dir(a) * (mid - 6.0 * px);
            let s = (60.0 + 12.0 * pop) * px;
            if pop > 0.01 {
                ctx.glow(q, s * 0.9, col.with_alpha(0.22 * pop), 1.6);
            }
            ctx.faded(dim, |ctx| {
                ctx.icon(
                    &it.icon,
                    Rect::new(q.x - s * 0.5, q.y - s * 0.5, s, s),
                    Color::WHITE,
                );
                if !it.count.is_empty() {
                    ctx.text(
                        &it.count,
                        13.0,
                        700,
                        if it.disabled {
                            p.bad
                        } else {
                            p.text_dim.lerp(p.text, pop)
                        },
                        q + Vec2::new(0.0, s * 0.5 + 4.0 * px),
                        Vec2::new(0.5, 0.0),
                    );
                }
            });
            // Slot number, small, near the inner edge.
            let nq = c + dir(a) * (ri + 16.0 * px);
            ctx.text(
                &(i + 1).to_string(),
                11.0,
                700,
                p.text_faint.with_alpha(0.8),
                nq,
                Vec2::splat(0.5),
            );
        }

        // The middle: a dark disc with the pointer on its rim and the hovered slot's name.
        let rd = r_in - 14.0 * px;
        ctx.rect_fill(
            Rect::new(c.x - rd, c.y - rd, rd * 2.0, rd * 2.0),
            Fill::Radial {
                inner: Color::hexa(0x141c2bf2),
                outer: Color::hexa(0x070a12f6),
            },
            [rd; 4],
            1.0 * px,
            Color::WHITE.with_alpha(0.1),
        );
        if let Some(a) = v.pointer {
            ctx.ring(c, Vec2::splat(rd), 3.0 * px, p.accent, a - 0.2, 0.4, 0.0);
            ctx.glow(c + dir(a) * rd, 18.0 * px, p.accent.with_alpha(0.5), 2.0);
        }
        let shown = v.hovered.or(v.current).and_then(|i| v.items.get(i));
        if let Some(it) = shown {
            let col = it.color.unwrap_or(p.accent);
            let mut y = c.y - 46.0 * px - it.details.len().min(3) as f32 * 9.0 * px;
            if !it.kind.is_empty() {
                ctx.text(
                    &it.kind.to_uppercase(),
                    11.0,
                    700,
                    col,
                    Vec2::new(c.x, y),
                    Vec2::new(0.5, 0.0),
                );
                y += 20.0 * px;
            }
            let r = ctx.text(
                &it.label,
                21.0,
                700,
                p.text,
                Vec2::new(c.x, y),
                Vec2::new(0.5, 0.0),
            );
            y = r.y + r.h + 10.0 * px;
            for line in it.details.iter().take(3) {
                ctx.text(
                    line,
                    13.0,
                    600,
                    p.text_dim,
                    Vec2::new(c.x, y),
                    Vec2::new(0.5, 0.0),
                );
                y += 19.0 * px;
            }
            let state = if it.disabled {
                Some(("Unavailable", p.bad))
            } else if v.current.is_some() && v.current == v.hovered.or(v.current) {
                Some(("Equipped", p.accent_light))
            } else {
                None
            };
            if let Some((t, color)) = state {
                ctx.text(
                    &t.to_uppercase(),
                    11.0,
                    700,
                    color,
                    Vec2::new(c.x, y + 6.0 * px),
                    Vec2::new(0.5, 0.0),
                );
            }
        }
    });
}

pub fn add_painters(p: &mut Painters) {
    p.add("wheel", paint_wheel);
}

// -------------------------------------------------------------------------------- screen

fn items_from(world: &World, args: &Value) -> Result<(Vec<WheelItem>, Option<String>), String> {
    let data = world.get_resource::<UiData>();
    let items = args
        .get("items")
        .or_else(|| data.and_then(|d| d.get("wheel.items")))
        .ok_or("Pass \"items\" (or fill wheel.items in the UI data).")?;
    let items: Vec<WheelItem> = serde_json::from_value(items.clone())
        .map_err(|e| format!("The wheel items don't match the slot format: {e}."))?;
    if items.is_empty() {
        return Err("The wheel needs at least one item.".into());
    }
    let current = args
        .get("current")
        .and_then(Value::as_str)
        .or_else(|| data.and_then(|d| d.get_str("wheel.current")))
        .map(str::to_string);
    Ok((items, current))
}

pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let (items, current) = items_from(world, args)?;
    let current = current.and_then(|id| items.iter().position(|i| i.id == id));
    let slow = args
        .get("slow")
        .and_then(Value::as_f64)
        .unwrap_or(0.2)
        .clamp(0.0, 1.0) as f32;
    let hold: Option<KeyCode> = match args.get("hold") {
        Some(k) => Some(
            serde_json::from_value(k.clone())
                .map_err(|_| format!("\"hold\" should be a key name like \"Tab\", not {k}."))?,
        ),
        None => None,
    };
    let hold_pad: Option<GamepadButton> = match args.get("hold_pad") {
        Some(b) => Some(serde_json::from_value(b.clone()).map_err(|_| {
            format!("\"hold_pad\" should be a pad button like \"LeftBumper\", not {b}.")
        })?),
        None => None,
    };
    let hints = El::new()
        .class("hud_panel row")
        .with_style(|s| {
            s.position(Position::Absolute)
                .bottom(48.0)
                .gap(22.0)
                .padding([10.0, 18.0])
                .radius(Corners::all(999.0))
        })
        .child(widgets::key_hint("Mouse", "LS", "Point"))
        .child(if hold.is_some() || hold_pad.is_some() {
            widgets::key_hint(
                &hold.map(crate::menus::key_label).unwrap_or_default(),
                &hold_pad.map(pad_label).unwrap_or_default(),
                "Release to equip",
            )
        } else {
            widgets::key_hint("LMB", "A", "Equip")
        })
        .child(widgets::key_hint(
            &format!("1-{}", items.len().min(9)),
            "",
            "Quick pick",
        ))
        .child(widgets::key_hint("Esc", "B", "Cancel"));
    let el = El::new()
        .class("screen")
        .with_style(|s| s.column().center())
        .child(El::new().with_style(|s| {
            s.fill_parent()
                .background(Fill::Radial {
                    inner: Color::hexa(0x05080e66),
                    outer: Color::hexa(0x05080eb0),
                })
                .pick(Pick::Block)
        }))
        .child(
            El::new()
                .id("wheel_canvas")
                .paint("wheel")
                .with_style(|s| s.fill_parent().pick(Pick::Ignore)),
        )
        .child(hints);
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 30,
            closable: true,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    let canvas = screens::find_in(world, root, "wheel_canvas").ok_or("no canvas")?;
    let n = items.len();
    world.entity_mut(canvas).insert(WheelView {
        items,
        hovered: current,
        current,
        pointer: None,
        open: 0.0,
        pop: vec![0.0; n],
    });
    let last_cursor = world.get_resource::<Mouse>().and_then(|m| m.position);
    world.entity_mut(root).insert(WheelScreen {
        canvas,
        hold,
        hold_pad,
        aim: Vec2::ZERO,
        last_cursor,
    });
    if slow < 0.999
        && let Some(mut d) = world.get_resource_mut::<TimeDilation>()
    {
        d.hold(SLOW_KEY, slow, 0.12);
    }
    Ok(root)
}

/// Pick slot `i`: send its action, write its data, close the wheel.
pub fn pick(world: &mut World, root: Entity, i: usize) -> Result<String, String> {
    let canvas = world
        .get::<WheelScreen>(root)
        .map(|w| w.canvas)
        .ok_or("The wheel isn't open.")?;
    let it = world
        .get::<WheelView>(canvas)
        .and_then(|v| v.items.get(i).cloned())
        .ok_or_else(|| format!("The wheel has no slot {i}."))?;
    if it.disabled {
        return Err(format!("{} isn't available right now.", it.label));
    }
    if let Some(mut data) = world.get_resource_mut::<UiData>() {
        for (path, v) in &it.data {
            data.set(path, v.clone());
        }
        data.set("wheel.selected", it.id.clone());
        if !it.instant {
            data.set("wheel.current", it.id.clone());
        }
    }
    if !it.action.is_empty() {
        crate::input::dispatch(world, &it.action);
    }
    toast::notify(
        world,
        ToastKind::Info,
        &it.icon,
        if it.instant { "Used" } else { "Equipped" },
        &format!("[b]{}[/b]", it.label),
        1.8,
    );
    screens::close_screen(world, root, false);
    Ok(it.id)
}

fn pad_label(b: GamepadButton) -> String {
    match b {
        GamepadButton::South => "A",
        GamepadButton::East => "B",
        GamepadButton::North => "Y",
        GamepadButton::West => "X",
        GamepadButton::LeftBumper => "LB",
        GamepadButton::RightBumper => "RB",
        GamepadButton::LeftTrigger => "LT",
        GamepadButton::RightTrigger => "RT",
        _ => "",
    }
    .into()
}

fn open_wheel(world: &mut World) -> Option<(Entity, WheelScreen)> {
    let mut q = world.query::<(Entity, &WheelScreen, &Screen)>();
    q.iter(world)
        .find(|(_, _, s)| !s.closing)
        .map(|(e, w, _)| (e, w.clone()))
}

/// Pointing, picking and the slow-motion hold.
pub fn wheel_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let Some((root, mut ws)) = open_wheel(world) else {
        // Closed (picked, cancelled or hidden): let time run again.
        if let Some(mut d) = world.get_resource_mut::<TimeDilation>()
            && d.is_active(SLOW_KEY)
        {
            d.release(SLOW_KEY);
        }
        return;
    };
    let canvas = ws.canvas;
    let Some(mut v) = world.get::<WheelView>(canvas).cloned() else {
        return;
    };
    let n = v.items.len().max(1);
    let sweep = TAU / n as f32;
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let center = world
        .get::<ComputedNode>(canvas)
        .map(|c| c.rect.center())
        .unwrap_or_default();
    let scale = world
        .get_resource::<crate::layout::UiViewport>()
        .map(|vp| vp.scale)
        .unwrap_or(1.0);
    let slot_at = |a: f32| ((a.rem_euclid(TAU) + sweep * 0.5) / sweep) as usize % n;
    let mut confirm: Option<usize> = None;
    let mut cancel = false;

    // Mouse: the direction from the middle once it leaves the dead zone. A moving cursor
    // points where it is; raw motion (a locked cursor) pushes the aim around.
    let mouse = world.get_resource::<Mouse>().cloned().unwrap_or_default();
    let cursor_moved = mouse.position.is_some() && mouse.position != ws.last_cursor;
    let mut aimed = false;
    if cursor_moved && let Some(pos) = mouse.position {
        ws.aim = pos - center;
        aimed = true;
    } else if mouse.delta.length() > 0.0 {
        ws.aim = (ws.aim + mouse.delta).clamp_length_max(R_OUT * scale);
        aimed = true;
    }
    ws.last_cursor = mouse.position;
    if aimed && ws.aim.length() > R_IN * 0.3 * scale {
        let a = ws.aim.x.atan2(-ws.aim.y);
        v.pointer = Some(a);
        v.hovered = Some(slot_at(a));
    }
    // Either stick.
    if let Some(pads) = world.get_resource::<Gamepads>() {
        for (x, y) in [
            (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY),
            (GamepadAxis::RightStickX, GamepadAxis::RightStickY),
        ] {
            let s = Vec2::new(pads.any_axis(x), pads.any_axis(y));
            if s.length() > 0.5 {
                let a = s.x.atan2(s.y);
                v.pointer = Some(a);
                v.hovered = Some(slot_at(a));
            }
        }
    }
    let on_top = screens::open_screens(world)
        .into_iter()
        .find(|(k, ..)| !matches!(k.as_str(), "toasts" | "tooltip" | "drag"))
        .is_some_and(|(_, e, ..)| e == root);
    if on_top {
        if let Some(k) = world.get_resource::<ButtonInput<KeyCode>>() {
            use KeyCode::*;
            let digits = [
                Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
            ];
            if let Some(i) = digits.iter().position(|d| k.just_pressed(*d))
                && i < n
            {
                v.hovered = Some(i);
                confirm = Some(i);
            }
            let step = k.just_pressed(ArrowRight) as i32 - k.just_pressed(ArrowLeft) as i32;
            if step != 0 {
                let h = v.hovered.or(v.current).unwrap_or(0) as i32;
                v.hovered = Some((h + step).rem_euclid(n as i32) as usize);
                v.pointer = v.hovered.map(|i| i as f32 * sweep);
            }
            if k.just_pressed(Enter) || k.just_pressed(NumpadEnter) {
                confirm = confirm.or(v.hovered);
            }
        }
        if world
            .get_resource::<ButtonInput<MouseButton>>()
            .is_some_and(|b| b.just_pressed(MouseButton::Left))
            || world
                .get_resource::<ButtonInput<GamepadButton>>()
                .is_some_and(|b| b.just_pressed(GamepadButton::South))
        {
            confirm = confirm.or(v.hovered);
        }
        // Hold to open, release to pick.
        let released = ws.hold.is_some_and(|key| {
            world
                .get_resource::<ButtonInput<KeyCode>>()
                .is_some_and(|k| k.just_released(key))
        }) || ws.hold_pad.is_some_and(|b| {
            world
                .get_resource::<ButtonInput<GamepadButton>>()
                .is_some_and(|p| p.just_released(b))
        });
        if released && confirm.is_none() {
            match v.hovered {
                Some(i) if v.pointer.is_some() && v.hovered != v.current => confirm = Some(i),
                _ => cancel = true,
            }
        }
    }
    for ev in &events {
        if let UiEvent::Action { action, .. } = ev {
            if let Some(i) = action
                .strip_prefix("wheel.pick:")
                .and_then(|i| i.parse::<usize>().ok())
            {
                confirm = Some(i);
            } else if action == "wheel.confirm" {
                confirm = confirm.or(v.hovered);
            }
        }
    }

    // Animation.
    v.open = (v.open + dt / 0.18).min(1.0);
    let k = 1.0 - (-dt * 16.0).exp();
    v.pop.resize(v.items.len(), 0.0);
    for (i, p) in v.pop.iter_mut().enumerate() {
        let target = if v.hovered == Some(i) { 1.0 } else { 0.0 };
        *p += (target - *p) * k;
    }
    if world.get::<WheelView>(canvas) != Some(&v)
        && let Some(mut w) = world.get_mut::<WheelView>(canvas)
    {
        *w = v;
    }
    if let Some(mut w) = world.get_mut::<WheelScreen>(root) {
        w.aim = ws.aim;
        w.last_cursor = ws.last_cursor;
    }
    if cancel {
        screens::close_screen(world, root, false);
        return;
    }
    if let Some(i) = confirm
        && let Err(e) = pick(world, root, i)
    {
        toast::notify(world, ToastKind::Bad, "", "", &e, 2.0);
    }
}

impl PartialEq for WheelView {
    fn eq(&self, o: &Self) -> bool {
        self.items == o.items
            && self.hovered == o.hovered
            && self.current == o.current
            && self.pointer == o.pointer
            && self.open == o.open
            && self.pop == o.pop
    }
}

// ---------------------------------------------------------------------------------- demo

/// The demo loadout: four weapons, two items and two abilities, the pistol in hand.
pub fn demo_items(db: &ItemDatabase, theme: &Theme) -> Vec<WheelItem> {
    let rarity = ["Common", "Uncommon", "Rare", "Epic", "Legendary"];
    let weapon = |id: &str, count: &str, ammo: Option<(u32, u32)>| {
        let d = db.get(id);
        let name = d.map(|d| d.name.clone()).unwrap_or_else(|| id.into());
        let r = d.map(|d| d.rarity as usize).unwrap_or(0).min(4);
        let mut details: Vec<String> = d
            .map(|d| {
                d.stats
                    .iter()
                    .take(2)
                    .map(|(k, v)| {
                        let unit = if k == "range" { " m" } else { "" };
                        format!("{} {}{unit}", capitalize(k), trim(*v))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        details.truncate(2);
        let mut data = Map::new();
        data.insert("weapon.name".into(), json!(name));
        data.insert("weapon.icon".into(), json!(id));
        let (ammo, reserve) = match ammo {
            Some((a, r)) => (json!(a), json!(r)),
            None => (json!("—"), json!("—")),
        };
        data.insert("weapon.ammo".into(), ammo);
        data.insert("weapon.reserve".into(), reserve);
        WheelItem {
            id: id.into(),
            icon: id.into(),
            label: name,
            kind: format!("Weapon · {}", rarity[r]),
            count: count.into(),
            details: vec![details.join("   ·   ")],
            color: Some(theme.rarity(r)),
            action: format!("weapon.equip:{id}"),
            data,
            disabled: false,
            instant: false,
        }
    };
    let item = |id: &str, count: &str, what: &str| {
        let d = db.get(id);
        WheelItem {
            id: id.into(),
            icon: id.into(),
            label: d.map(|d| d.name.clone()).unwrap_or_else(|| id.into()),
            kind: "Item".into(),
            count: count.into(),
            details: vec![what.into()],
            color: Some(theme.rarity(d.map(|d| d.rarity as usize).unwrap_or(0))),
            action: format!("item.use:{id}"),
            instant: true,
            ..Default::default()
        }
    };
    let ability = |id: &str, label: &str, count: &str, what: &str, ready: bool| WheelItem {
        id: id.into(),
        icon: id.into(),
        label: label.into(),
        kind: "Ability".into(),
        count: count.into(),
        details: vec![what.into()],
        color: Some(theme.palette.info),
        action: format!("ability.cast:{id}"),
        disabled: !ready,
        instant: true,
        ..Default::default()
    };
    vec![
        weapon("pistol", "12 / 36", Some((12, 36))),
        weapon("rifle", "4 / 20", Some((4, 20))),
        weapon("greatsword", "Melee", None),
        weapon("bow", "18 arrows", Some((1, 18))),
        item("grenade", "×4", "Throw: stuns everything in 6 m"),
        item("medkit", "×2", "Restores 60 health over 4 s"),
        ability(
            "ability_time_slow",
            "Time Slow",
            "12 s",
            "Slows the world to a fifth for 6 s",
            false,
        ),
        ability(
            "ability_shield_bubble",
            "Shield Bubble",
            "Ready",
            "Blocks shots from outside for 5 s",
            true,
        ),
    ]
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn trim(v: f32) -> String {
    if (v - v.round()).abs() < 1e-3 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Put the demo loadout in `wheel.items` (unless a game already did).
pub fn add_demo_items(world: &mut World) {
    let db = world
        .get_resource::<ItemDatabase>()
        .cloned()
        .unwrap_or_default();
    let theme = world.get_resource::<Theme>().cloned().unwrap_or_default();
    if let Some(mut data) = world.get_resource_mut::<UiData>()
        && data.get("wheel.items").is_none()
    {
        let items = serde_json::to_value(demo_items(&db, &theme)).unwrap_or(Value::Null);
        data.set("wheel.items", items);
        data.set("wheel.current", "pistol");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_round_trip_and_point_where_the_pointer_is() {
        let items = demo_items(&ItemDatabase::default(), &Theme::default());
        assert_eq!(items.len(), 8);
        let v = serde_json::to_value(&items).unwrap();
        let back: Vec<WheelItem> = serde_json::from_value(v).unwrap();
        assert_eq!(back, items);
        // Up is slot 0, then clockwise.
        let sweep = TAU / 8.0;
        let slot_at = |a: f32| ((a.rem_euclid(TAU) + sweep * 0.5) / sweep) as usize % 8;
        assert_eq!(slot_at(0.0), 0);
        assert_eq!(slot_at(-0.2), 0);
        assert_eq!(slot_at(TAU / 4.0), 2);
        assert_eq!(slot_at(-TAU / 4.0), 6);
    }
}
