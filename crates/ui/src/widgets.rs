//! Reusable widgets: value bars, cooldown sweeps, key hints, sliders, toggles, and `El`
//! builders for headers, tabs, currency and stat rows. Every widget is plain data
//! (registered components), so JSON layouts and AI agents use them the same way Rust does:
//!
//! ```json
//! {"class": "bar_track", "style": {"width": 240}, "paint": "bar",
//!  "components": {"Bar": {"bind": "player.health", "bind_max": "player.health_max",
//!                         "color": "#e0503c", "color2": "#ff8a5c"}}}
//! ```

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::math::Vec2;
use engine_core::prelude::Time;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data;
use crate::draw::{DrawCtx, Painters};
use crate::el::El;
use crate::input::{NavMode, UiEvent, UiInputState};
use crate::node::*;
use crate::style::{Corners, Fill, Pick, StylePatch, pct};
use crate::theme::Theme;

// ------------------------------------------------------------------------------------ Bar

/// A value bar (health, shield, stamina, progress) painted inside the node's box. Give the
/// node class `bar_track` for the groove and `paint: "bar"`.
///
/// The value comes from `value` (0..1), or from bindings: `bind` / `bind_max` are
/// [`UiData`] paths, `@Entity.Component.field` or `$Resource.field`. Losses leave a bright
/// trail that drains after a moment; low values pulse.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bar {
    /// Fill 0..1 (overwritten by the bindings when set).
    pub value: f32,
    pub bind: String,
    pub bind_max: String,
    /// Fill gradient, left to right.
    pub color: Color,
    pub color2: Color,
    /// Tick marks dividing the bar (e.g. 10 for 10 HP chunks), 0 = none.
    pub segments: u32,
    /// A tick at every whole unit of the bound max instead (step counters: 2 / 4 shows four
    /// chunks), when the max is 2 to 24.
    pub unit_ticks: bool,
    /// Pulse when at or below this fraction (0 = never).
    pub low: f32,
    /// Show "current / max" text on the bar.
    pub label: bool,
    /// Smoothed fill as drawn.
    #[serde(skip)]
    pub shown: f32,
    /// Lagging fill showing a recent loss.
    #[serde(skip)]
    pub trail: f32,
    #[serde(skip)]
    pub hold: f32,
    #[serde(skip)]
    pub current: f64,
    #[serde(skip)]
    pub max: f64,
    #[serde(skip)]
    pub started: bool,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            value: 1.0,
            bind: String::new(),
            bind_max: String::new(),
            color: Color::hex(0xe0503c),
            color2: Color::hex(0xff8a5c),
            segments: 0,
            unit_ticks: false,
            low: 0.0,
            label: false,
            shown: 1.0,
            trail: 1.0,
            hold: 0.0,
            current: 0.0,
            max: 0.0,
            started: false,
        }
    }
}

impl Bar {
    /// A bar bound to `bind` / `bind_max` with a two-color fill.
    pub fn bound(bind: &str, bind_max: &str, color: Color, color2: Color) -> Self {
        Self {
            bind: bind.into(),
            bind_max: bind_max.into(),
            color,
            color2,
            ..Default::default()
        }
    }
}

/// Read bar bindings and animate fills and trails (real time).
pub fn bars_system(world: &mut World) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let mut q = world.query::<(Entity, &Bar)>();
    let reads: Vec<(Entity, Option<f64>, Option<f64>)> = q
        .iter(world)
        .map(|(e, b)| {
            let cur = (!b.bind.is_empty())
                .then(|| data::lookup_f64(world, &b.bind))
                .flatten();
            let max = (!b.bind_max.is_empty())
                .then(|| data::lookup_f64(world, &b.bind_max))
                .flatten();
            (e, cur, max)
        })
        .collect();
    for (e, cur, max) in reads {
        let Some(mut b) = world.get_mut::<Bar>(e) else {
            continue;
        };
        let mut target = b.value;
        if let Some(c) = cur {
            let m = max.unwrap_or(1.0).max(1e-9);
            target = (c / m) as f32;
            b.current = c;
            b.max = m;
        }
        let target = target.clamp(0.0, 1.0);
        if !b.started {
            b.started = true;
            b.shown = target;
            b.trail = target;
        }
        let k = 1.0 - (-dt * 14.0).exp();
        b.shown += (target - b.shown) * k;
        if (b.shown - target).abs() < 1e-4 {
            b.shown = target;
        }
        if target < b.trail - 1e-4 {
            if b.shown > target + 0.002 || b.hold <= 0.0 {
                // A fresh loss: hold the trail a moment before draining.
                if b.hold <= 0.0 && b.trail - target > 0.004 {
                    b.hold = 0.45;
                }
            }
            b.hold -= dt;
            if b.hold <= 0.0 {
                b.trail = (b.trail - dt * 0.9).max(target);
            }
        } else {
            b.trail = b.shown.max(target);
            b.hold = 0.0;
        }
    }
}

fn paint_bar(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(b) = world.get::<Bar>(e).cloned() else {
        return;
    };
    let c = world.get::<ComputedNode>(e).cloned().unwrap_or_default();
    let inner = content_rect(&c, ctx.px);
    let r = inner.h * 0.5;
    let radius = c
        .style
        .radius
        .0
        .map(|x| ((x - c.style.border_width - 1.0).max(0.0) * ctx.px).min(r));
    let w = inner.w;
    // Trail (recent loss), then the fill.
    if b.trail > b.shown + 0.001 {
        let tr = Rect::new(inner.x, inner.y, w * b.trail, inner.h);
        let radius = radius.map(|x| x.min(tr.w * 0.5));
        ctx.rect_fill(
            tr,
            Fill::Solid(Color::hexa(0xffe2c0d0)),
            radius,
            0.0,
            Color::TRANSPARENT,
        );
    }
    let mut pulse = 1.0;
    if b.low > 0.0 && b.shown <= b.low {
        pulse = 0.72 + 0.28 * (ctx.time * 7.0).sin().abs();
    }
    if b.shown > 0.0005 {
        let fr = Rect::new(inner.x, inner.y, (w * b.shown).max(1.0), inner.h);
        let radius = radius.map(|x| x.min(fr.w * 0.5));
        ctx.rect_fill(
            fr,
            Fill::Linear {
                from: b.color.scale(pulse),
                to: b.color2.scale(pulse),
                angle_deg: 0.0,
            },
            radius,
            0.0,
            Color::TRANSPARENT,
        );
        // Gloss on the upper half.
        let gloss = Rect::new(fr.x, fr.y, fr.w, fr.h * 0.48);
        ctx.rect_fill(
            gloss,
            Fill::Linear {
                from: Color::WHITE.with_alpha(0.28),
                to: Color::WHITE.with_alpha(0.04),
                angle_deg: 90.0,
            },
            [radius[0], radius[1], 0.0, 0.0],
            0.0,
            Color::TRANSPARENT,
        );
        // Bright leading edge.
        if b.shown < 0.999 {
            ctx.glow(
                Vec2::new(fr.x + fr.w, fr.y + fr.h * 0.5),
                inner.h * 1.4,
                b.color2.with_alpha(0.55),
                2.0,
            );
        }
    }
    let segments = if b.unit_ticks && (2.0..=24.0).contains(&b.max.round()) {
        b.max.round() as u32
    } else {
        b.segments
    };
    if segments > 1 {
        for i in 1..segments {
            let x = inner.x + w * i as f32 / segments as f32;
            ctx.line(
                Vec2::new(x, inner.y + 1.0),
                Vec2::new(x, inner.y + inner.h - 1.0),
                (1.0 * ctx.px).max(1.0),
                Color::BLACK.with_alpha(0.45),
                0.0,
            );
        }
    }
    if b.label && b.max > 0.0 {
        let text = format!("{} / {}", b.current.round() as i64, b.max.round() as i64);
        ctx.text(
            &text,
            (inner.h / ctx.px * 0.78).clamp(9.0, 15.0),
            600,
            Color::WHITE.with_alpha(0.92),
            inner.center(),
            Vec2::splat(0.5),
        );
    }
}

/// Content box (inside border and padding) of a computed node, physical px.
pub fn content_rect(c: &ComputedNode, px: f32) -> Rect {
    let s = &c.style;
    let v = |v: crate::style::Val| match v {
        crate::style::Val::Px(p) => p * px,
        crate::style::Val::Percent(p) => p / 100.0 * c.rect.w,
        crate::style::Val::Auto => 0.0,
    };
    let b = s.border_width * px;
    let (l, r, t, btm) = (
        v(s.padding.left) + b,
        v(s.padding.right) + b,
        v(s.padding.top) + b,
        v(s.padding.bottom) + b,
    );
    Rect::new(
        c.rect.x + l,
        c.rect.y + t,
        (c.rect.w - l - r).max(0.0),
        (c.rect.h - t - btm).max(0.0),
    )
}

// ------------------------------------------------------------------------------- Cooldown

/// A clockwise cooldown sweep over an ability or item slot (`paint: "cooldown"`). Set
/// `remaining`/`total` (seconds), or bind them. The slot flashes when it becomes ready.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cooldown {
    pub remaining: f32,
    pub total: f32,
    pub bind: String,
    pub bind_total: String,
    /// Show the seconds left.
    pub label: bool,
    #[serde(skip)]
    pub flash: f32,
}

impl Default for Cooldown {
    fn default() -> Self {
        Self {
            remaining: 0.0,
            total: 1.0,
            bind: String::new(),
            bind_total: String::new(),
            label: true,
            flash: 0.0,
        }
    }
}

/// Read cooldown bindings and fade the ready flash.
pub fn cooldowns_system(world: &mut World) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let mut q = world.query::<(Entity, &Cooldown)>();
    let reads: Vec<(Entity, Option<f64>, Option<f64>)> = q
        .iter(world)
        .map(|(e, c)| {
            (
                e,
                (!c.bind.is_empty())
                    .then(|| data::lookup_f64(world, &c.bind))
                    .flatten(),
                (!c.bind_total.is_empty())
                    .then(|| data::lookup_f64(world, &c.bind_total))
                    .flatten(),
            )
        })
        .collect();
    for (e, rem, total) in reads {
        let Some(mut c) = world.get_mut::<Cooldown>(e) else {
            continue;
        };
        let before = c.remaining;
        if let Some(r) = rem {
            c.remaining = r as f32;
        }
        if let Some(t) = total {
            c.total = t as f32;
        }
        if before > 0.0 && c.remaining <= 0.0 {
            c.flash = 1.0;
        }
        if c.flash > 0.0 {
            c.flash = (c.flash - dt * 2.5).max(0.0);
        }
    }
}

fn paint_cooldown(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(cd) = world.get::<Cooldown>(e).cloned() else {
        return;
    };
    let r = ctx.rect;
    let center = r.center();
    let frac = if cd.total > 0.0 {
        (cd.remaining / cd.total).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if frac > 0.0 {
        // Dark sweep: a thick ring wide enough to fill the slot, from the top clockwise.
        let radius = r.w.max(r.h) * 0.36;
        let start = (1.0 - frac) * std::f32::consts::TAU;
        ctx.with_clip(r.inflate(-1.0), |ctx| {
            ctx.ring(
                center,
                Vec2::splat(radius),
                radius * 2.0,
                Color::hexa(0x05080eb8),
                start,
                frac * std::f32::consts::TAU,
                0.0,
            );
        });
        // Bright edge of the sweep.
        ctx.ring(
            center,
            Vec2::splat(r.w.min(r.h) * 0.5 - 2.0 * ctx.px),
            2.0 * ctx.px,
            Color::hex(0x5fd0ff).with_alpha(0.85),
            start,
            frac * std::f32::consts::TAU,
            0.0,
        );
        if cd.label {
            let secs = if cd.remaining >= 10.0 {
                format!("{:.0}", cd.remaining.ceil())
            } else {
                format!("{:.1}", cd.remaining)
            };
            ctx.text(
                &secs,
                (r.h / ctx.px * 0.3).clamp(10.0, 22.0),
                700,
                Color::WHITE,
                center,
                Vec2::splat(0.5),
            );
        }
    }
    if cd.flash > 0.0 {
        ctx.glow(
            center,
            r.w * 0.75,
            Color::hex(0x9fe6ff).with_alpha(cd.flash * 0.8),
            1.5,
        );
    }
}

// -------------------------------------------------------------------------------- KeyHint

/// A key or button prompt that shows the keyboard key or the gamepad button, whichever the
/// player used last: `{"KeyHint": {"key": "E", "pad": "X"}}`.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyHint {
    pub key: String,
    /// Gamepad button: A, B, X, Y, LB, RB, LT, RT, Start, Select, Up, Down, Left, Right.
    pub pad: String,
}

fn pad_color(b: &str) -> Option<Color> {
    Some(match b {
        "A" => Color::hex(0x5fbf4a),
        "B" => Color::hex(0xd9504a),
        "X" => Color::hex(0x4a8fd9),
        "Y" => Color::hex(0xe0b33a),
        _ => return None,
    })
}

/// Swap key hints between keyboard and gamepad glyphs.
pub fn key_hints_system(world: &mut World) {
    let gamepad = world
        .get_resource::<UiInputState>()
        .is_some_and(|s| s.mode == NavMode::Directional && s.last_gamepad);
    let mut q = world.query::<(Entity, &KeyHint)>();
    let hints: Vec<(Entity, KeyHint)> = q.iter(world).map(|(e, h)| (e, h.clone())).collect();
    for (e, h) in hints {
        let use_pad = gamepad && !h.pad.is_empty();
        let label = if use_pad {
            h.pad.clone()
        } else {
            h.key.clone()
        };
        if let Some(mut t) = world.get_mut::<Text>(e) {
            if t.text != label {
                t.text = label.clone();
            }
        } else {
            world
                .entity_mut(e)
                .insert(Text::new(label.clone()).semibold().nowrap());
        }
        let want = if use_pad && pad_color(&h.pad).is_some() {
            "pad_button"
        } else {
            "kbd"
        };
        if let Some(mut n) = world.get_mut::<Node>(e) {
            let has = n.has_class(want);
            if !has {
                n.classes.retain(|c| c != "kbd" && c != "pad_button");
                n.classes.push(want.into());
            }
            let bg = if use_pad { pad_color(&h.pad) } else { None };
            let fill = bg.map(Fill::Solid);
            if n.style.background != fill && (fill.is_some() || n.style.background.is_some()) {
                n.style.background = fill;
            }
        }
    }
}

// --------------------------------------------------------------------------------- Slider

/// A horizontal slider (`paint: "slider"`): drag it, or focus it and press left/right.
/// Writes its value to `bind` (a [`UiData`] path or `$Resource.field`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slider {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub bind: String,
    /// Decimals shown in the value label (-1 = hide the label).
    pub decimals: i32,
    pub suffix: String,
}

impl Default for Slider {
    fn default() -> Self {
        Self {
            value: 0.5,
            min: 0.0,
            max: 1.0,
            step: 0.05,
            bind: String::new(),
            decimals: 2,
            suffix: String::new(),
        }
    }
}

/// Drag and keyboard adjustment of sliders; keeps them in sync with their bindings.
pub fn sliders_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let (pressed, cursor) = {
        let st = world.resource::<UiInputState>();
        (
            st.pressed.map(|(e, _)| e),
            world
                .get_resource::<engine_input::Mouse>()
                .and_then(|m| m.position),
        )
    };
    let adjusts: Vec<(Entity, i32)> = crate::input::read_events(world, &mut seen)
        .iter()
        .filter_map(|e| match e {
            UiEvent::Adjust {
                entity: Some(en),
                delta,
                ..
            } => Some((*en, *delta)),
            _ => None,
        })
        .collect();
    let mut q = world.query::<(Entity, &Slider)>();
    let sliders: Vec<(Entity, Slider)> = q.iter(world).map(|(e, s)| (e, s.clone())).collect();
    for (e, s) in sliders {
        let mut value = s.value;
        if !s.bind.is_empty()
            && let Some(v) = data::lookup_f64(world, &s.bind)
        {
            value = v as f32;
        }
        let mut changed = false;
        if pressed == Some(e)
            && let Some(p) = cursor
            && let Some(c) = world.get::<ComputedNode>(e)
        {
            let px = world.resource::<crate::layout::UiViewport>().scale * c.scale;
            let track = content_rect(c, px);
            let t = ((p.x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
            let mut v = s.min + t * (s.max - s.min);
            if s.step > 0.0 {
                v = (v / s.step).round() * s.step;
            }
            if (v - value).abs() > 1e-6 {
                value = v;
                changed = true;
            }
        }
        for (target, delta) in &adjusts {
            if *target == e {
                let step = if s.step > 0.0 {
                    s.step
                } else {
                    (s.max - s.min) / 20.0
                };
                value = (value + step * *delta as f32).clamp(s.min, s.max);
                changed = true;
            }
        }
        let value = value.clamp(s.min, s.max);
        if changed && !s.bind.is_empty() {
            let v = (value as f64 * 1e4).round() / 1e4;
            if let Err(err) = data::write(world, &s.bind, json!(v)) {
                tracing::warn!("slider: {err}");
            }
        }
        if let Some(mut sl) = world.get_mut::<Slider>(e)
            && (sl.value - value).abs() > 1e-6
        {
            sl.value = value;
        }
    }
}

fn paint_slider(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(s) = world.get::<Slider>(e).cloned() else {
        return;
    };
    let it = world.get::<Interaction>(e).copied().unwrap_or_default();
    let accent = world.resource::<Theme>().palette.accent;
    let c = world.get::<ComputedNode>(e).cloned().unwrap_or_default();
    let inner = content_rect(&c, ctx.px);
    let label_w = if s.decimals >= 0 { 56.0 * ctx.px } else { 0.0 };
    let track_w = (inner.w - label_w).max(10.0);
    let y = inner.center().y;
    let h = 6.0 * ctx.px;
    let t = if s.max > s.min {
        ((s.value - s.min) / (s.max - s.min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let track = Rect::new(inner.x, y - h * 0.5, track_w, h);
    ctx.rect(
        track,
        Color::hexa(0x0a0f18e0),
        h * 0.5,
        1.0,
        Color::hexa(0xffffff1a),
    );
    ctx.rect_fill(
        Rect::new(track.x, track.y, (track.w * t).max(h), h),
        Fill::Linear {
            from: accent.scale(0.7),
            to: accent,
            angle_deg: 0.0,
        },
        [h * 0.5; 4],
        0.0,
        Color::TRANSPARENT,
    );
    let knob = Vec2::new(track.x + track.w * t, y);
    let kr = if it.pressed || it.focused { 10.0 } else { 8.5 } * ctx.px;
    if it.focused || it.hovered {
        ctx.glow(knob, kr * 2.6, accent.with_alpha(0.5), 1.6);
    }
    ctx.shadow(
        Rect::new(knob.x - kr, knob.y - kr, kr * 2.0, kr * 2.0),
        kr,
        6.0 * ctx.px,
        Color::hexa(0x00000090),
    );
    ctx.rect(
        Rect::new(knob.x - kr, knob.y - kr, kr * 2.0, kr * 2.0),
        Color::hex(0xf4f1ea),
        kr,
        2.0 * ctx.px,
        accent,
    );
    if s.decimals >= 0 {
        let text = format!("{:.*}{}", s.decimals as usize, s.value, s.suffix);
        ctx.text(
            &text,
            14.0,
            600,
            Color::hex(0xdfe6f2),
            Vec2::new(inner.x + inner.w, y),
            Vec2::new(1.0, 0.5),
        );
    }
}

// --------------------------------------------------------------------------------- Toggle

/// An on/off switch (`paint: "toggle"`, with `on_click`): clicking flips `value` and writes
/// it to `bind`.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Toggle {
    pub value: bool,
    pub bind: String,
    #[serde(skip)]
    pub anim: f32,
}

/// Flip toggles that were clicked; sync them with their bindings and animate the knob.
pub fn toggles_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let clicked: Vec<Entity> = crate::input::read_events(world, &mut seen)
        .iter()
        .filter_map(|e| match e {
            UiEvent::Action {
                entity: Some(en), ..
            } => Some(*en),
            _ => None,
        })
        .collect();
    let mut q = world.query::<(Entity, &Toggle)>();
    let toggles: Vec<(Entity, Toggle)> = q.iter(world).map(|(e, t)| (e, t.clone())).collect();
    for (e, t) in toggles {
        let mut value = t.value;
        if !t.bind.is_empty()
            && let Some(v) = data::lookup_f64(world, &t.bind)
        {
            value = v != 0.0;
        }
        if clicked.contains(&e) {
            value = !value;
            if !t.bind.is_empty()
                && let Err(err) = data::write(world, &t.bind, json!(value))
            {
                tracing::warn!("toggle: {err}");
            }
        }
        let target = if value { 1.0 } else { 0.0 };
        if let Some(mut tg) = world.get_mut::<Toggle>(e) {
            if tg.value != value {
                tg.value = value;
            }
            let k = 1.0 - (-dt * 16.0).exp();
            let a = tg.anim + (target - tg.anim) * k;
            if (a - tg.anim).abs() > 1e-4 {
                tg.anim = if (a - target).abs() < 1e-3 { target } else { a };
            }
        }
    }
}

fn paint_toggle(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(t) = world.get::<Toggle>(e).cloned() else {
        return;
    };
    let it = world.get::<Interaction>(e).copied().unwrap_or_default();
    let accent = world.resource::<Theme>().palette.accent;
    let c = world.get::<ComputedNode>(e).cloned().unwrap_or_default();
    let inner = content_rect(&c, ctx.px);
    let (w, h) = (44.0 * ctx.px, 24.0 * ctx.px);
    let r = Rect::new(inner.x + inner.w - w, inner.center().y - h * 0.5, w, h);
    let off = Color::hexa(0x1a2232ff);
    let fill = off.lerp(accent.scale(0.85), t.anim);
    if it.focused || it.hovered {
        ctx.glow(r.center(), w, accent.with_alpha(0.25), 1.5);
    }
    ctx.rect(r, fill, h * 0.5, 1.0 * ctx.px, Color::hexa(0xffffff26));
    let kr = h * 0.5 - 3.0 * ctx.px;
    let kx = r.x + h * 0.5 + (w - h) * t.anim;
    let knob = Rect::new(kx - kr, r.center().y - kr, kr * 2.0, kr * 2.0);
    ctx.shadow(knob, kr, 4.0 * ctx.px, Color::hexa(0x00000080));
    ctx.rect(knob, Color::hex(0xf7f4ee), kr, 0.0, Color::TRANSPARENT);
}

/// Register the widget painters.
pub fn add_painters(p: &mut Painters) {
    p.add("bar", paint_bar);
    p.add("cooldown", paint_cooldown);
    p.add("slider", paint_slider);
    p.add("toggle", paint_toggle);
}

// ------------------------------------------------------------------------------- builders

/// A screen title row: optional icon, title, subtitle, and a close button.
pub fn header(icon: Option<&str>, title: &str, subtitle: &str, closable: bool) -> El {
    let mut row = El::new()
        .class("row")
        .with_style(|s| s.gap(14.0).padding([0.0, 0.0, 0.0, 0.0]));
    if let Some(i) = icon {
        row = row.child(
            El::new()
                .class("header_icon")
                .child(El::icon(i).with_style(|s| s.size(26.0, 26.0).pick(Pick::Ignore))),
        );
    }
    let mut titles = El::new().class("col").with_style(|s| s.gap(2.0).grow(1.0));
    titles = titles.child(El::text(title).class("title"));
    if !subtitle.is_empty() {
        titles = titles.child(El::text(subtitle).class("caption"));
    }
    row = row.child(titles);
    if closable {
        row = row.child(close_button());
    }
    row
}

/// A round × button that closes the enclosing screen.
pub fn close_button() -> El {
    El::new()
        .class("icon_button")
        .on_click("ui.close")
        .tooltip("", "Close [color=#8a94a8](Esc)[/color]")
        .child(El::icon("ui_close").with_style(|s| s.size(16.0, 16.0).pick(Pick::Ignore)))
}

/// A row of tabs; `selected` is the active index. Each tab sends `"{action}:{index}"`.
pub fn tabs(labels: &[&str], selected: usize, action: &str) -> El {
    El::new()
        .class("tab_bar")
        .children(labels.iter().enumerate().map(|(i, l)| {
            El::new()
                .class("tab")
                .id(format!("{action}:{i}"))
                .on_click(format!("{action}:{i}"))
                .selected(i == selected)
                .child(El::text(*l).pick_ignore())
        }))
}

/// A key prompt with a label: `[E] Talk`.
pub fn key_hint(key: &str, pad: &str, label: &str) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(8.0))
        .child(
            El::new()
                .class("kbd")
                .attach(
                    "KeyHint",
                    &KeyHint {
                        key: key.into(),
                        pad: pad.into(),
                    },
                )
                .text_style(|t| t.semibold().nowrap()),
        )
        .child(El::text(label).class("caption").text_style(|t| t.nowrap()))
}

/// A prompt with two keys (`[Q] [E] Category`).
pub fn key_hint2(keys: [&str; 2], pads: [&str; 2], label: &str) -> El {
    let k = |key: &str, pad: &str| {
        El::new()
            .class("kbd")
            .attach(
                "KeyHint",
                &KeyHint {
                    key: key.into(),
                    pad: pad.into(),
                },
            )
            .text_style(|t| t.semibold().nowrap())
    };
    El::new()
        .class("row")
        .with_style(|s| s.gap(4.0))
        .child(k(keys[0], pads[0]))
        .child(k(keys[1], pads[1]))
        .child(
            El::text(label)
                .class("caption")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.margin([0.0, 0.0, 0.0, 4.0])),
        )
}

/// A coin icon and an amount (a binding like `"{player.gold}"`, or a literal).
pub fn currency(amount: &str, size: f32) -> El {
    currency_el(amount, size, None)
}

/// [`currency`] with the amount in `color` (a price the player can't pay, in red).
pub fn currency_tinted(amount: &str, size: f32, color: Color) -> El {
    currency_el(amount, size, Some(color))
}

fn currency_el(amount: &str, size: f32, color: Option<Color>) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(6.0))
        .child(El::icon("coin").with_style(|s| s.size(size, size).pick(Pick::Ignore)))
        .child(
            El::text(amount)
                .class("currency")
                .text_style(|t| {
                    let t = t.tabular().nowrap();
                    match color {
                        Some(c) => t.color(c),
                        None => t,
                    }
                })
                .with_style(|s| s.font_size(size * 0.9)),
        )
}

/// A labeled value bar bound to UI data.
pub fn labeled_bar(
    label: &str,
    bind: &str,
    bind_max: &str,
    color: Color,
    color2: Color,
    width: f32,
) -> El {
    El::new()
        .class("col")
        .with_style(|s| s.gap(4.0).width(width))
        .child(
            El::new()
                .class("row")
                .child(El::text(label).class("caption").with_style(|s| s.grow(1.0)))
                .child(
                    El::text(format!("{{{bind}:0}} / {{{bind_max}:0}}"))
                        .class("caption")
                        .text_style(|t| t.tabular().nowrap()),
                ),
        )
        .child(
            El::new()
                .class("bar_track")
                .with_style(|s| s.width(pct(100.0)))
                .paint("bar")
                .attach("Bar", &Bar::bound(bind, bind_max, color, color2)),
        )
}

/// A "label ...... value" row, with an optional comparison (green up / red down).
pub fn stat_row(label: &str, value: &str, delta: Option<f32>) -> El {
    let mut row = El::new()
        .class("stat_row")
        .child(El::text(label).class("dim").with_style(|s| s.grow(1.0)))
        .child(
            El::text(value)
                .class("stat_value")
                .text_style(|t| t.tabular().nowrap()),
        );
    if let Some(d) = delta
        && d.abs() > 1e-4
    {
        let (icon, color) = if d > 0.0 {
            ("ui_arrow_up", Color::hex(0x6fdc8c))
        } else {
            ("ui_arrow_down", Color::hex(0xff6b6b))
        };
        row = row.child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(2.0).width(64.0).justify(crate::style::Justify::End))
                .child(
                    El::icon(icon)
                        .icon_tint(color)
                        .with_style(|s| s.size(12.0, 12.0)),
                )
                .child(
                    El::text(format!("{:+}", (d * 10.0).round() / 10.0))
                        .text_style(|t| t.color(color).tabular().nowrap())
                        .with_style(|s| s.font_size(13.0)),
                ),
        );
    } else {
        row = row.child(El::new().with_style(|s| s.width(64.0)));
    }
    row
}

/// A thin horizontal rule.
pub fn divider() -> El {
    El::new().class("divider")
}

/// A settings row: label on the left, a slider on the right.
pub fn slider_row(label: &str, slider: Slider) -> El {
    El::new()
        .class("setting_row")
        .child(El::text(label).with_style(|s| s.grow(1.0)))
        .child(
            El::new()
                .class("slider")
                .focusable(true)
                .paint("slider")
                .attach("Slider", &slider)
                .attach("Adjustable", &crate::input::Adjustable)
                .with_style(|s| s.width(240.0).height(28.0)),
        )
}

/// A settings row with an on/off switch.
pub fn toggle_row(label: &str, bind: &str, id: &str) -> El {
    El::new()
        .class("setting_row")
        .id(id)
        .on_click(format!("toggle:{id}"))
        .paint("toggle")
        .attach(
            "Toggle",
            &Toggle {
                bind: bind.into(),
                ..Default::default()
            },
        )
        .child(El::text(label).pick_ignore())
}

/// A soft full-screen dim behind modal screens.
pub fn scrim(alpha: f32) -> StylePatch {
    StylePatch::default()
        .fill_parent()
        .background(Fill::Radial {
            inner: Color::hexa(0x05080e00).with_alpha(alpha * 0.55),
            outer: Color::hexa(0x05080e00).with_alpha(alpha),
        })
        .pick(Pick::Block)
}

/// Corner radii helper re-export for builders.
pub fn radius(r: f32) -> Corners {
    Corners::all(r)
}

/// JSON for an `El` attaching `Bar` (for JSON layouts written in Rust tests and docs).
pub fn bar_json(bind: &str, bind_max: &str) -> Value {
    json!({"class": "bar_track", "paint": "bar", "components": {"Bar": {"bind": bind, "bind_max": bind_max}}})
}
