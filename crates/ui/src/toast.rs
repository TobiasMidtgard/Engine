//! Toast notifications: short messages that slide in at the bottom right (above the weapon
//! card) and fade away
//! (`ui.notify {"text": "Bought Iron Sword", "kind": "good", "icon": "coin"}`).

use bevy_ecs::prelude::*;
use engine_core::prelude::Time;
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::el::{self, El};
use crate::node::*;
use crate::screens;
use crate::style::{Fill, Pick, Position};
use crate::theme::Theme;

/// What a toast is about (sets its accent color).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastKind {
    #[default]
    Info,
    Good,
    Bad,
    Warn,
    /// Rare or legendary loot: gold.
    Loot,
}

/// A live toast (on the toast node).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Toast {
    /// Seconds it stays (real time).
    pub duration: f32,
    pub age: f32,
    pub leaving: bool,
}

impl Default for Toast {
    fn default() -> Self {
        Self {
            duration: 3.5,
            age: 0.0,
            leaving: false,
        }
    }
}

const KEY: &str = "toasts";

fn container(world: &mut World) -> Option<Entity> {
    if let Some(root) = screens::find_screen(world, KEY) {
        return world
            .get::<Children>(root)
            .and_then(|c| c.first().copied())
            .or(Some(root));
    }
    let el = El::new().class("screen").child(El::new().with_style(|s| {
        s.position(Position::Absolute)
            .bottom(176.0)
            .right(32.0)
            .column()
            .gap(10.0)
            .align_items(crate::style::Align::End)
            .pick(Pick::Ignore)
    }));
    let sp = screens::spawn_screen(
        world,
        Screen {
            key: KEY.into(),
            modal: false,
            layer: 60,
            presence: 1.0,
            closing: false,
            closable: false,
        },
        &el,
    )
    .ok()?;
    let root = sp.root?;
    world.get::<Children>(root).and_then(|c| c.first().copied())
}

/// Show a toast. `title` may be empty; text takes markup.
pub fn notify(
    world: &mut World,
    kind: ToastKind,
    icon: &str,
    title: &str,
    text: &str,
    duration: f32,
) {
    let Some(parent) = container(world) else {
        return;
    };
    let p = world.resource::<Theme>().palette.clone();
    let color = match kind {
        ToastKind::Info => p.info,
        ToastKind::Good => p.good,
        ToastKind::Bad => p.bad,
        ToastKind::Warn => p.warn,
        ToastKind::Loot => p.accent,
    };
    let icon = if icon.is_empty() {
        match kind {
            ToastKind::Info => "ui_info",
            ToastKind::Good => "ui_check",
            ToastKind::Bad => "ui_close",
            ToastKind::Warn => "marker_danger",
            ToastKind::Loot => "star",
        }
    } else {
        icon
    };
    let glyph = icon.starts_with("ui_");
    let mut col = El::new()
        .class("col")
        .with_style(|s| s.gap(2.0).grow(1.0).shrink(1.0));
    if !title.is_empty() {
        col = col.child(
            El::text(title)
                .text_style(|t| t.semibold().color(color.lerp(Color::WHITE, 0.35)))
                .with_style(|s| s.font_size(15.0)),
        );
    }
    if !text.is_empty() {
        col = col.child(El::text(text).with_style(|s| s.font_size(14.0).color(p.text_dim)));
    }
    let el = El::new()
        .class("toast")
        .with_style(|s| {
            s.border_color(color.fade(0.45))
                .opacity(0.0)
                .translate([48.0, 0.0])
                .transition(0.22)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .with_style(|s| {
                    s.width(4.0)
                        .height(crate::style::pct(100.0))
                        .position(Position::Absolute)
                        .left(0.0)
                        .top(0.0)
                        .radius(crate::style::Corners::left(12.0))
                        .bg(color)
                })
                .pick_ignore(),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.size(34.0, 34.0)
                        .center()
                        .radius(crate::style::Corners::all(17.0))
                        .background(Fill::Radial {
                            inner: color.fade(0.32),
                            outer: color.fade(0.08),
                        })
                        .border(1.0, color.fade(0.5))
                        .color(color.lerp(Color::WHITE, 0.2))
                })
                .child(
                    El::icon(icon)
                        .with_style(|s| {
                            s.size(
                                if glyph { 16.0 } else { 24.0 },
                                if glyph { 16.0 } else { 24.0 },
                            )
                        })
                        .pick_ignore(),
                ),
        )
        .child(col);
    if let Ok(sp) = el::spawn(world, Some(parent), &el)
        && let Some(e) = sp.root
    {
        world.entity_mut(e).insert(Toast {
            duration,
            ..Default::default()
        });
    }
}

/// Age toasts: slide in, wait, fade out, despawn. Keeps at most five.
pub fn toasts_system(world: &mut World) {
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let mut q = world.query::<(Entity, &mut Toast, &mut Node)>();
    let mut done = Vec::new();
    let mut live: Vec<(Entity, u32)> = Vec::new();
    for (e, mut t, mut n) in q.iter_mut(world) {
        let first = t.age == 0.0;
        t.age += dt;
        if first {
            continue;
        }
        if n.style.opacity == Some(0.0) && !t.leaving {
            n.style.opacity = None;
            n.style.translate = None;
        }
        if t.age > t.duration && !t.leaving {
            t.leaving = true;
            n.style.opacity = Some(0.0);
            n.style.translate = Some([24.0, 0.0]);
        }
        if t.leaving && t.age > t.duration + 0.45 {
            done.push(e);
        } else if !t.leaving {
            live.push((e, (t.age * 1000.0) as u32));
        }
    }
    // Too many: retire the oldest.
    if live.len() > 5 {
        live.sort_by_key(|(_, age)| std::cmp::Reverse(*age));
        for (e, _) in live.iter().take(live.len() - 5) {
            if let Some(mut t) = world.get_mut::<Toast>(*e) {
                t.duration = t.duration.min(t.age);
            }
        }
    }
    for e in done {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
}
