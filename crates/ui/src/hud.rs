//! The in-game HUD and world markers.
//!
//! `ui.show {"screen": "hud"}` opens a HUD bound to [`UiData`] paths, so a game only writes
//! numbers (`ui.set_data {"path": "player.health", "value": 40}`) and the HUD follows:
//!
//! | Part | Data |
//! |---|---|
//! | Vitals (top left) | `player.{name, level, health, health_max, shield, shield_max, stamina, stamina_max, xp, xp_max}` |
//! | Compass (top) | `player.heading_deg`, plus the [`MapData`] waypoint and discovered places |
//! | Minimap (top right) | the player's planet and position in [`MapData`] |
//! | Quest tracker (right) | `quest.{title, step, progress, progress_max}`, distance from the waypoint |
//! | Hotbar (bottom) | `abilities[i].{icon, key, cooldown, total}`, `player.{mana, mana_max}` |
//! | Prompt (center) | `prompt.{key, pad, verb, target}` (hidden while `verb` is empty) |
//! | Time readout (bottom left) | the engine's time scale (shown while time is dilated), `player.speed` |
//! | Weapon (bottom right) | `weapon.{name, icon, ammo, reserve, grenades}` |
//!
//! The HUD writes `nav.waypoint_distance`, `nav.waypoint_text` and `time.scale` for other
//! layouts to bind to.
//!
//! World markers ([`Marker`] + [`WorldAnchor`]) are nameplates and objective icons pinned to
//! world positions or entities: `hud.marker {"title": "Relay Tower", "icon": "marker_quest",
//! "position": [120, 30, -800], "clamp": true}`.

use bevy_ecs::prelude::*;
use engine_core::math::{DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::draw::{DrawCtx, Painters};
use crate::el::{self, El};
use crate::map::{self, MapData};
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Justify, Pick, Position, Shadow, pct};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets::{Bar, Cooldown, KeyHint};

// ------------------------------------------------------------------------------ components

/// A heading strip (`paint: "compass"`): ticks, cardinal letters, the waypoint and nearby
/// places by bearing.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compass {
    /// Data path of the heading in degrees (0 = north, clockwise).
    pub bind: String,
    /// Degrees of heading visible across the strip.
    pub fov_deg: f32,
}

impl Default for Compass {
    fn default() -> Self {
        Self {
            bind: "player.heading_deg".into(),
            fov_deg: 150.0,
        }
    }
}

/// A round minimap (`paint: "minimap"`) of the player's planet around their position, north
/// up, with discovered places and the waypoint.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Minimap {
    /// Degrees of latitude across the map.
    pub span_deg: f32,
    /// Data path of the player's heading (turns the player arrow).
    pub heading: String,
}

impl Default for Minimap {
    fn default() -> Self {
        Self {
            span_deg: 9.0,
            heading: "player.heading_deg".into(),
        }
    }
}

/// A crosshair (`paint: "crosshair"`); `spread` 0..1 (or `bind`) opens the gap.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Crosshair {
    pub spread: f32,
    pub bind: String,
}

/// A ring gauge of the engine's time scale (`paint: "time_gauge"`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct TimeGauge;

/// A world marker: icon, title and distance over a world point. Spawn it as a top-level node
/// with a [`WorldAnchor`] (see [`marker_el`] and `hud.marker`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Marker {
    pub title: String,
    pub subtitle: String,
    pub icon: String,
    pub color: Color,
    pub show_distance: bool,
    /// Show it even while the HUD screen is closed. By default markers belong to the HUD and
    /// fade in and out with it.
    pub always: bool,
    /// The marker stands for the map's waypoint: while the waypoint is on the player's planet
    /// it shows the distance to it rather than to its anchor, so a beacon placed in the
    /// direction of a far place reads the same as the compass and quest tracker.
    pub waypoint: bool,
}

impl Default for Marker {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: String::new(),
            icon: "marker_quest".into(),
            color: Color::hex(0xe8b65a),
            show_distance: true,
            always: false,
            waypoint: false,
        }
    }
}

/// The distance label inside a marker.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MarkerDistance;

/// The edge arrow inside a marker (shown while the marker is clamped to the screen edge).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MarkerArrow;

// --------------------------------------------------------------------------------- helpers

/// "45 m", "1.2 km", "1,240 km".
pub fn format_distance(m: f64) -> String {
    if !m.is_finite() {
        return "-".into();
    }
    if m < 1000.0 {
        format!("{:.0} m", m.max(0.0))
    } else if m < 10_000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!(
            "{} km",
            crate::data::group_thousands(format!("{:.0}", m / 1000.0))
        )
    }
}

fn data_f64(world: &World, path: &str) -> Option<f64> {
    if path.is_empty() {
        return None;
    }
    crate::data::lookup_f64(world, path)
}

#[allow(clippy::too_many_arguments)]
fn bar(
    bind: &str,
    max: &str,
    color: Color,
    color2: Color,
    height: f32,
    segments: u32,
    low: f32,
    label: bool,
) -> El {
    El::new()
        .class("bar_track")
        .paint("bar")
        .attach(
            "Bar",
            &Bar {
                segments,
                low,
                label,
                ..Bar::bound(bind, max, color, color2)
            },
        )
        .with_style(|s| {
            s.width(pct(100.0))
                .height(height)
                .radius(Corners::all(height * 0.5))
        })
}

fn hud_panel() -> El {
    El::new().class("hud_panel")
}

// ------------------------------------------------------------------------------------ HUD

fn vitals(p: &crate::theme::Palette, portrait: &str) -> El {
    let avatar = El::new()
        .with_style(|s| s.size(76.0, 76.0))
        .child(
            El::icon(portrait)
                .with_style(|s| {
                    s.size(76.0, 76.0)
                        .radius(Corners::all(38.0))
                        .background(Fill::Radial {
                            inner: p.info.fade(0.35),
                            outer: Color::hexa(0x0a1220ff),
                        })
                        .border(2.0, p.accent.fade(0.85))
                        .shadow(Shadow::glow(p.accent.fade(0.3), 10.0))
                })
                .component("Icon", json!({"name": portrait, "cover": true}))
                .pick_ignore(),
        )
        .child(
            El::text("{player.level}")
                .class("badge")
                .text_style(|t| t.tabular().nowrap())
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .right(-4.0)
                        .bottom(-2.0)
                        .min_width(26.0)
                        .height(22.0)
                        .radius(Corners::all(11.0))
                        .background(Fill::vertical(p.accent_light, p.accent_dark))
                        .border(1.5, Color::hex(0x0b1019))
                        .color(Color::hex(0x1a1206))
                        .font_size(12.0)
                })
                .pick_ignore(),
        );
    let bars = El::new()
        .class("col")
        .with_style(|s| s.gap(7.0).width(300.0))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.align_items(Align::End).gap(8.0))
                .child(
                    El::text("{player.name}")
                        .text_style(|t| t.bold().nowrap().shadow())
                        .with_style(|s| s.font_size(17.0).grow(1.0)),
                )
                .child(
                    El::text("[color=#8a94a8]SHIELD[/color] {player.shield:0}")
                        .text_style(|t| t.tabular().nowrap())
                        .with_style(|s| s.font_size(11.0).font_weight(700).color(p.info)),
                ),
        )
        .child(bar(
            "player.health",
            "player.health_max",
            Color::hex(0xd8433a),
            Color::hex(0xff8a5c),
            16.0,
            10,
            0.3,
            true,
        ))
        .child(bar(
            "player.shield",
            "player.shield_max",
            Color::hex(0x2f8fe0),
            Color::hex(0x7fe0ff),
            8.0,
            6,
            0.0,
            false,
        ))
        .child(bar(
            "player.stamina",
            "player.stamina_max",
            Color::hex(0x7fbf3f),
            Color::hex(0xd8e86a),
            5.0,
            0,
            0.0,
            false,
        ));
    hud_panel()
        .id("hud_vitals")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(32.0)
                .top(28.0)
                .row()
                .align_items(Align::Center)
                .gap(16.0)
                .padding([14.0, 18.0, 16.0, 14.0])
        })
        .child(avatar)
        .child(bars)
        .child(
            // Experience along the bottom edge.
            El::new()
                .class("bar_track")
                .paint("bar")
                .attach(
                    "Bar",
                    &Bar::bound("player.xp", "player.xp_max", p.accent_dark, p.accent_light),
                )
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(14.0)
                        .right(14.0)
                        .bottom(-1.0)
                        .height(3.0)
                        .border(0.0, Color::TRANSPARENT)
                        .background(Color::WHITE.fade(0.06))
                        .padding(0.0)
                })
                .tooltip("Experience", "{player.xp:0} / {player.xp_max:0}"),
        )
}

fn compass() -> El {
    El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(22.0)
                .row()
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .id("hud_compass")
                .paint("compass")
                .attach("Compass", &Compass::default())
                .with_style(|s| s.size(620.0, 62.0).pick(Pick::Ignore)),
        )
}

fn minimap(p: &crate::theme::Palette) -> El {
    El::new()
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(32.0)
                .top(26.0)
                .align_items(Align::Center)
                .gap(10.0)
        })
        .child(
            El::new()
                .id("hud_minimap")
                .paint("minimap")
                .attach("Minimap", &Minimap::default())
                .tooltip("Minimap", "[color=#8a94a8]M[/color] opens the map")
                .on_click("ui.show:map")
                .focusable(false)
                .with_style(|s| {
                    s.size(220.0, 220.0)
                        .radius(Corners::all(110.0))
                        .shadow(Shadow::new(Color::hexa(0x00000088), 18.0, 4.0))
                }),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.align_items(Align::Center).gap(1.0))
                .child(
                    El::text("{location.name}")
                        .text_style(|t| t.semibold().nowrap().shadow())
                        .with_style(|s| s.font_size(15.0)),
                )
                .child(
                    El::text("{location.region}  ·  {location.time}")
                        .text_style(|t| t.nowrap().shadow().tabular())
                        .with_style(|s| s.font_size(12.0).font_weight(600).color(p.text_dim)),
                ),
        )
}

fn quest_tracker(p: &crate::theme::Palette) -> El {
    El::new()
        .id("hud_quest")
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(32.0)
                .top(318.0)
                .width(300.0)
                .gap(7.0)
                .padding([14.0, 16.0])
                .radius(Corners::all(12.0))
                .background(Fill::Linear {
                    from: Color::hexa(0x080c1400),
                    to: Color::hexa(0x080c14b8),
                    angle_deg: 0.0,
                })
                .border(0.0, Color::TRANSPARENT)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0))
                .child(
                    El::icon("marker_quest")
                        .with_style(|s| s.size(16.0, 16.0))
                        .pick_ignore(),
                )
                .child(
                    El::text("QUEST")
                        .class("label_caps")
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| s.color(p.accent)),
                )
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    El::text("{nav.waypoint_text}")
                        .text_style(|t| t.tabular().nowrap().shadow())
                        .with_style(|s| s.font_size(13.0).font_weight(700).color(p.accent_light)),
                ),
        )
        .child(
            El::text("{quest.title}")
                .text_style(|t| t.bold().shadow())
                .with_style(|s| s.font_size(18.0).color(p.accent_light)),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Start))
                .child(El::new().with_style(|s| {
                    s.size(10.0, 10.0)
                        .margin([5.0, 0.0, 0.0, 2.0])
                        .radius(Corners::all(2.0))
                        .border(1.5, p.accent.fade(0.9))
                        .rotate_deg(45.0)
                }))
                .child(
                    El::text("{quest.step}")
                        .text_style(|t| t.shadow().line_height(1.35))
                        .with_style(|s| s.font_size(14.0).color(p.text).grow(1.0).shrink(1.0)),
                ),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).margin([2.0, 0.0, 0.0, 20.0]))
                .child(
                    El::new().with_style(|s| s.grow(1.0)).child(
                        El::new()
                            .class("bar_track")
                            .paint("bar")
                            .attach(
                                "Bar",
                                &Bar {
                                    unit_ticks: true,
                                    ..Bar::bound(
                                        "quest.progress",
                                        "quest.progress_max",
                                        p.accent_dark,
                                        p.accent,
                                    )
                                },
                            )
                            .with_style(|s| {
                                s.width(pct(100.0)).height(4.0).radius(Corners::all(2.0))
                            }),
                    ),
                )
                .child(
                    El::text("{quest.progress} / {quest.progress_max}")
                        .text_style(|t| t.tabular().nowrap())
                        .with_style(|s| s.font_size(12.0).font_weight(700).color(p.text_dim)),
                ),
        )
}

fn hotbar(p: &crate::theme::Palette, world: &World) -> El {
    let n = world
        .get_resource::<UiData>()
        .and_then(|d| d.get("abilities"))
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0)
        .clamp(1, 10);
    let mut slots = hud_panel()
        .class("row")
        .with_style(|s| s.gap(8.0).padding(8.0).radius(Corners::all(14.0)));
    for i in 0..n {
        let icon = world
            .get_resource::<UiData>()
            .and_then(|d| d.get_str(&format!("abilities.{i}.icon")).map(String::from))
            .unwrap_or_default();
        let key = world
            .get_resource::<UiData>()
            .and_then(|d| d.get_str(&format!("abilities.{i}.key")).map(String::from))
            .unwrap_or_else(|| (i + 1).to_string());
        let pad = ["X", "Y", "B", "A", "LB", "RB", "LT", "RT", "L3", "R3"][i];
        slots = slots.child(
            El::new()
                .class("slot")
                .id(format!("hud_ability:{i}"))
                .with_style(|s| {
                    s.size(62.0, 62.0)
                        .background(Fill::Radial {
                            inner: p.info.fade(0.12),
                            outer: Color::hexa(0x060910d0),
                        })
                        .border(1.0, Color::WHITE.fade(0.14))
                })
                .child(
                    El::icon(icon)
                        .with_style(|s| s.size(44.0, 44.0))
                        .pick_ignore(),
                )
                .child(
                    // The sweep draws over the icon.
                    El::new()
                        .paint("cooldown")
                        .attach(
                            "Cooldown",
                            &Cooldown {
                                bind: format!("abilities.{i}.cooldown"),
                                bind_total: format!("abilities.{i}.total"),
                                ..Default::default()
                            },
                        )
                        .with_style(|s| {
                            s.position(Position::Absolute)
                                .left(0.0)
                                .top(0.0)
                                .right(0.0)
                                .bottom(0.0)
                                .radius(Corners::all(10.0))
                        })
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("kbd")
                        .attach(
                            "KeyHint",
                            &KeyHint {
                                key: key.clone(),
                                pad: pad.into(),
                            },
                        )
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|s| {
                            s.position(Position::Absolute)
                                .left(-5.0)
                                .top(-7.0)
                                .min_width(18.0)
                                .height(18.0)
                                .font_size(10.0)
                                .background(Color::hexa(0x0b1019f0))
                        })
                        .pick_ignore(),
                ),
        );
    }
    El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .bottom(26.0)
                .row()
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.align_items(Align::Center).gap(8.0))
                .child(El::new().with_style(|s| s.width(360.0)).child(bar(
                    "player.mana",
                    "player.mana_max",
                    Color::hex(0x6b5cff),
                    Color::hex(0xb98cff),
                    6.0,
                    8,
                    0.0,
                    false,
                )))
                .child(slots),
        )
}

fn time_and_speed(p: &crate::theme::Palette) -> El {
    El::new()
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(32.0)
                .bottom(30.0)
                .gap(10.0)
        })
        .child(
            hud_panel()
                .id("hud_time")
                .class("row")
                .with_style(|s| {
                    s.gap(12.0)
                        .padding([10.0, 16.0, 10.0, 10.0])
                        .border_color(p.info.fade(0.45))
                        .transition(0.3)
                })
                .child(
                    El::new()
                        .paint("time_gauge")
                        .attach("TimeGauge", &TimeGauge)
                        .with_style(|s| s.size(46.0, 46.0)),
                )
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0))
                        .child(
                            El::text("TIME")
                                .class("label_caps")
                                .text_style(|t| t.nowrap()),
                        )
                        .child(
                            El::text("{time.scale_text}")
                                .text_style(|t| t.bold().tabular().nowrap())
                                .with_style(|s| s.font_size(24.0).color(p.info)),
                        ),
                ),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::End).padding([0.0, 4.0]))
                .child(
                    El::text("{player.speed:1}")
                        .text_style(|t| t.bold().tabular().nowrap().shadow())
                        .with_style(|s| s.font_size(28.0)),
                )
                .child(
                    El::text("m/s")
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| {
                            s.font_size(13.0)
                                .font_weight(700)
                                .color(p.text_dim)
                                .margin([0.0, 0.0, 5.0, 0.0])
                        }),
                ),
        )
}

fn weapon(p: &crate::theme::Palette) -> El {
    hud_panel()
        .id("hud_weapon")
        .class("row")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(32.0)
                .bottom(30.0)
                .gap(16.0)
                .padding([12.0, 20.0, 12.0, 14.0])
                .align_items(Align::Center)
        })
        .child(
            El::new()
                .with_style(|s| {
                    s.size(64.0, 64.0)
                        .center()
                        .radius(Corners::all(12.0))
                        .background(Fill::Radial {
                            inner: p.accent.fade(0.2),
                            outer: p.accent.fade(0.0),
                        })
                })
                .child(
                    El::icon("pistol")
                        .id("hud_weapon_icon")
                        .with_style(|s| s.size(54.0, 54.0))
                        .pick_ignore(),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text("{weapon.name}")
                        .class("label_caps")
                        .text_style(|t| t.nowrap()),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(6.0).align_items(Align::End))
                        .child(
                            El::text("{weapon.ammo}")
                                .text_style(|t| t.bold().tabular().nowrap())
                                .with_style(|s| s.font_size(34.0)),
                        )
                        .child(
                            El::text("/ {weapon.reserve}")
                                .text_style(|t| t.tabular().nowrap())
                                .with_style(|s| {
                                    s.font_size(16.0)
                                        .font_weight(600)
                                        .color(p.text_dim)
                                        .margin([0.0, 0.0, 6.0, 0.0])
                                }),
                        ),
                ),
        )
        .child(El::new().with_style(|s| {
            s.width(1.0)
                .height(44.0)
                .background(Color::WHITE.fade(0.12))
        }))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.align_items(Align::Center).gap(2.0))
                .child(
                    El::icon("grenade")
                        .with_style(|s| s.size(28.0, 28.0))
                        .pick_ignore(),
                )
                .child(
                    El::text("×{weapon.grenades}")
                        .text_style(|t| t.bold().tabular().nowrap())
                        .with_style(|s| s.font_size(14.0)),
                ),
        )
}

fn prompt(p: &crate::theme::Palette) -> El {
    El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(pct(60.0))
                .row()
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .id("hud_prompt")
                .class("row")
                .with_style(|s| {
                    s.gap(10.0)
                        .padding([8.0, 16.0, 8.0, 10.0])
                        .radius(Corners::all(999.0))
                        .background(Color::hexa(0x070b12b0))
                        .border(1.0, Color::WHITE.fade(0.12))
                        .transition(0.2)
                })
                .child(
                    El::new()
                        .class("kbd")
                        .id("hud_prompt_key")
                        .attach(
                            "KeyHint",
                            &KeyHint {
                                key: "E".into(),
                                pad: "X".into(),
                            },
                        )
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|s| s.min_width(26.0).height(26.0).font_size(13.0)),
                )
                .child(
                    El::text("{prompt.verb}")
                        .text_style(|t| t.semibold().nowrap())
                        .with_style(|s| s.font_size(16.0)),
                )
                .child(
                    El::text("{prompt.target}")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(16.0).color(p.accent_light)),
                ),
        )
}

fn build(world: &World, args: &Value) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let portrait = args
        .get("portrait")
        .and_then(Value::as_str)
        .unwrap_or("portrait:pilot");
    El::new()
        .class("screen")
        .with_style(|s| s.pick(Pick::Ignore))
        .child(El::new().id("hud_vignette").with_style(|s| {
            s.fill_parent()
                .background(Fill::Radial {
                    inner: Color::hexa(0xff202000),
                    outer: Color::hexa(0xc0101070),
                })
                .opacity(0.0)
                .transition(0.4)
                .pick(Pick::Ignore)
        }))
        .child(vitals(p, portrait))
        .child(compass())
        .child(minimap(p))
        .child(quest_tracker(p))
        .child(
            El::new()
                .with_style(|s| s.fill_parent().center().pick(Pick::Ignore))
                .child(
                    El::new()
                        .paint("crosshair")
                        .attach("Crosshair", &Crosshair::default())
                        .with_style(|s| s.size(56.0, 56.0).pick(Pick::Ignore)),
                ),
        )
        .child(prompt(p))
        .child(hotbar(p, world))
        .child(time_and_speed(p))
        .child(weapon(p))
}

/// Builder for `ui.show {"screen": "hud"}`; `portrait` names the player's portrait icon.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let el = build(world, args);
    let sp = screens::spawn_screen(
        world,
        Screen {
            key: "hud".into(),
            modal: false,
            layer: 5,
            closable: false,
            ..Default::default()
        },
        &el,
    )?;
    sp.root.ok_or_else(|| "The HUD is empty.".into())
}

// ------------------------------------------------------------------------------- systems

/// Keeps derived HUD data (`nav.*`, `time.*`) fresh, shows or hides the prompt, the time
/// readout and the low-health vignette, and updates world markers.
pub fn hud_system(world: &mut World) {
    // Derived data.
    let scale = world
        .get_resource::<Time>()
        .map(|t| t.scale())
        .unwrap_or(1.0);
    let nav = world
        .get_resource::<MapData>()
        .and_then(|m| m.waypoint_nav());
    if let Some(mut data) = world.get_resource_mut::<UiData>() {
        let s = (scale as f64 * 100.0).round() / 100.0;
        if data.get_f64("time.scale") != Some(s) {
            data.set("time.scale", s);
            data.set("time.scale_text", format!("×{s:.2}"));
        }
        match nav {
            Some((dist_m, bearing)) => {
                let d = (dist_m / 10.0).round() * 10.0;
                if data.get_f64("nav.waypoint_distance") != Some(d) {
                    data.set("nav.waypoint_distance", d);
                    data.set("nav.waypoint_text", format_distance(d));
                }
                let b = bearing.round();
                if data.get_f64("nav.waypoint_bearing") != Some(b) {
                    data.set("nav.waypoint_bearing", b);
                }
            }
            None => {
                if data.get_str("nav.waypoint_text") != Some("") {
                    data.set("nav.waypoint_text", "");
                }
            }
        }
    }

    if let Some(root) = screens::find_screen(world, "hud") {
        // The interaction prompt sits where the wheel's middle is.
        let wheel_open = screens::find_screen(world, "wheel").is_some();
        let prompt_on = !wheel_open
            && world
                .get_resource::<UiData>()
                .and_then(|d| d.get_str("prompt.verb"))
                .is_some_and(|v| !v.is_empty());
        let health = match (
            data_f64(world, "player.health"),
            data_f64(world, "player.health_max"),
        ) {
            (Some(h), Some(m)) if m > 0.0 => (h / m) as f32,
            _ => 1.0,
        };
        let dilated = (scale - 1.0).abs() > 0.02;
        let key = world
            .get_resource::<UiData>()
            .and_then(|d| d.get_str("prompt.key").map(String::from));
        let pad = world
            .get_resource::<UiData>()
            .and_then(|d| d.get_str("prompt.pad").map(String::from));
        let quest_on = world
            .get_resource::<UiData>()
            .and_then(|d| d.get_str("quest.title"))
            .is_some_and(|t| !t.is_empty());
        set_opacity(world, root, "hud_prompt", if prompt_on { 1.0 } else { 0.0 });
        set_opacity(world, root, "hud_quest", if quest_on { 1.0 } else { 0.0 });
        set_opacity(world, root, "hud_time", if dilated { 1.0 } else { 0.0 });
        set_opacity(
            world,
            root,
            "hud_vignette",
            ((0.35 - health) / 0.35).clamp(0.0, 1.0),
        );
        let weapon_icon = world
            .get_resource::<UiData>()
            .and_then(|d| d.get_str("weapon.icon"))
            .filter(|i| !i.is_empty())
            .map(String::from);
        if let Some(name) = weapon_icon
            && let Some(e) = screens::find_in(world, root, "hud_weapon_icon")
            && let Some(mut icon) = world.get_mut::<Icon>(e)
            && icon.name != name
        {
            icon.name = name;
        }
        if let Some(e) = screens::find_in(world, root, "hud_prompt_key")
            && let Some(mut k) = world.get_mut::<KeyHint>(e)
        {
            if let Some(key) = key
                && k.key != key
            {
                k.key = key;
            }
            if let Some(pad) = pad
                && k.pad != pad
            {
                k.pad = pad;
            }
        }
    }

    step_aside_for_menus(world);
    markers_update(world);
}

fn set_opacity(world: &mut World, root: Entity, id: &str, opacity: f32) {
    if let Some(e) = screens::find_in(world, root, id)
        && let Some(mut n) = world.get_mut::<Node>(e)
        && n.style.opacity != Some(opacity)
    {
        n.style.opacity = Some(opacity);
    }
}

/// The HUD fades out while a menu (any other modal screen: dialogue, shop, inventory, map,
/// settings) is open, and back in when it closes. It stays up under the quick-select wheel,
/// so the ammo and the slow-motion gauge read while choosing; only the interaction prompt,
/// which would sit in the wheel's middle, hides (in `hud_system`).
fn step_aside_for_menus(world: &mut World) {
    let Some(root) = screens::find_screen(world, "hud") else {
        return;
    };
    let menu_open = screens::open_screens(world)
        .iter()
        .any(|(key, _, _, modal)| *modal && !matches!(key.as_str(), "hud" | "wheel"));
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let Some(mut n) = world.get_mut::<Node>(root) else {
        return;
    };
    let current = n.style.opacity.unwrap_or(1.0);
    let target = if menu_open { 0.0 } else { 1.0 };
    let next = if (target - current).abs() < 0.01 {
        target
    } else {
        current + (target - current) * (dt * 10.0).min(1.0)
    };
    if n.style.opacity != Some(next) {
        n.style.opacity = Some(next);
    }
}

fn markers_update(world: &mut World) {
    let hud = screens::find_screen(world, "hud").map_or(0.0, |e| {
        let presence = world.get::<Screen>(e).map_or(0.0, |s| s.presence);
        let shown = world
            .get::<Node>(e)
            .and_then(|n| n.style.opacity)
            .unwrap_or(1.0);
        presence * shown
    });
    // Markers step aside while the quick-select wheel covers the middle of the screen.
    let wheel = screens::find_screen(world, "wheel").map_or(1.0, |e| {
        1.0 - world.get::<Screen>(e).map_or(1.0, |s| s.presence)
    });
    let hud = hud * wheel;
    let nav = world
        .get_resource::<MapData>()
        .and_then(|m| m.waypoint_nav())
        .map(|(d, _)| d);
    let mut q = world.query::<(Entity, &Marker, &WorldAnchor)>();
    let items: Vec<(Entity, bool, f32, bool, Vec2, f32)> = q
        .iter(world)
        .map(|(e, m, a)| {
            (
                e,
                m.show_distance,
                match nav {
                    Some(d) if m.waypoint => d as f32,
                    _ => a.distance,
                },
                !a.on_screen && a.clamp_to_edges,
                a.edge_direction,
                if m.always { 1.0 } else { hud },
            )
        })
        .collect();
    for (e, show, distance, clamped, dir, opacity) in items {
        if let Some(mut n) = world.get_mut::<Node>(e)
            && n.style.opacity != Some(opacity)
        {
            n.style.opacity = Some(opacity);
        }
        for c in screens::subtree(world, e) {
            if world.get::<MarkerDistance>(c).is_some() {
                let text = if show {
                    format_distance(distance as f64)
                } else {
                    String::new()
                };
                if let Some(mut t) = world.get_mut::<Text>(c)
                    && t.text != text
                {
                    t.text = text;
                }
            } else if world.get::<MarkerArrow>(c).is_some()
                && let Some(mut n) = world.get_mut::<Node>(c)
            {
                let (op, rot) = if clamped {
                    (1.0, dir.x.atan2(-dir.y).to_degrees())
                } else {
                    (0.0, 0.0)
                };
                if n.style.opacity != Some(op) {
                    n.style.opacity = Some(op);
                }
                if n.style.rotate_deg != Some(rot) && clamped {
                    n.style.rotate_deg = Some(rot);
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------ painters

fn paint_compass(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(c) = world.get::<Compass>(e).cloned() else {
        return;
    };
    let heading = data_f64(world, &c.bind).unwrap_or(0.0) as f32;
    let p = world.resource::<Theme>().palette.clone();
    let r = ctx.rect;
    let px = ctx.px;
    let cx = r.x + r.w * 0.5;
    let strip_h = 30.0 * px;
    let top = r.y + 2.0 * px;
    let fov = c.fov_deg.max(20.0);
    let x_of = |deg: f32| {
        let mut d = (deg - heading) % 360.0;
        if d > 180.0 {
            d -= 360.0;
        }
        if d < -180.0 {
            d += 360.0;
        }
        (cx + d / fov * r.w, d)
    };
    // Backing: dark in the middle, fading out to the sides.
    let back = Rect::new(r.x, top, r.w, strip_h);
    ctx.rect_fill(
        Rect::new(r.x, top, r.w * 0.5, strip_h),
        Fill::Linear {
            from: Color::hexa(0x070b1200),
            to: Color::hexa(0x070b12a8),
            angle_deg: 0.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    ctx.rect_fill(
        Rect::new(cx, top, r.w * 0.5, strip_h),
        Fill::Linear {
            from: Color::hexa(0x070b12a8),
            to: Color::hexa(0x070b1200),
            angle_deg: 0.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    let fade = |x: f32| (1.0 - ((x - cx).abs() / (r.w * 0.5)).powf(2.2)).clamp(0.0, 1.0);
    ctx.with_clip(back, |ctx| {
        let start = ((heading - fov * 0.5) / 5.0).floor() as i32 * 5;
        let end = ((heading + fov * 0.5) / 5.0).ceil() as i32 * 5;
        for deg in (start..=end).step_by(5) {
            let d = deg.rem_euclid(360);
            let (x, _) = x_of(deg as f32);
            let a = fade(x);
            if a <= 0.0 {
                continue;
            }
            let (label, major) = match d {
                0 => (Some("N"), 2),
                90 => (Some("E"), 2),
                180 => (Some("S"), 2),
                270 => (Some("W"), 2),
                45 => (Some("NE"), 1),
                135 => (Some("SE"), 1),
                225 => (Some("SW"), 1),
                315 => (Some("NW"), 1),
                _ if d % 15 == 0 => (None, 1),
                _ => (None, 0),
            };
            let len = [5.0, 8.0, 10.0][major] * px;
            let col = Color::WHITE.with_alpha(0.25 + 0.35 * major as f32);
            ctx.faded(a, |ctx| {
                ctx.line(
                    Vec2::new(x, top + 1.0 * px),
                    Vec2::new(x, top + 1.0 * px + len),
                    if major == 2 { 2.0 * px } else { 1.2 * px },
                    col,
                    0.0,
                );
                let y = top + 20.0 * px;
                if let Some(l) = label {
                    let cardinal = l.len() == 1;
                    ctx.text(
                        l,
                        if cardinal { 15.0 } else { 11.0 },
                        if cardinal { 800 } else { 700 },
                        if l == "N" {
                            p.accent_light
                        } else if cardinal {
                            Color::WHITE
                        } else {
                            Color::WHITE.with_alpha(0.7)
                        },
                        Vec2::new(x, y),
                        Vec2::splat(0.5),
                    );
                } else if major == 1 {
                    ctx.text(
                        &d.to_string(),
                        10.0,
                        600,
                        Color::WHITE.with_alpha(0.5),
                        Vec2::new(x, y),
                        Vec2::splat(0.5),
                    );
                }
            });
        }
    });
    // Places and the waypoint by bearing.
    if let Some(map) = world.get_resource::<MapData>() {
        let mut marks: Vec<(f32, String, Color, bool)> = Vec::new();
        if let Some(pl) = map.player_planet() {
            // The waypoint's own place shows as the waypoint.
            for poi in pl
                .pois
                .iter()
                .filter(|q| q.discovered && !map.is_waypoint(&pl.id, q))
            {
                let (dist, bearing) =
                    map::great_circle(map.player.lon, map.player.lat, poi.lon, poi.lat);
                if dist.to_degrees() < 12.0 && dist > 1e-6 {
                    marks.push((
                        bearing as f32,
                        poi.kind.icon().to_string(),
                        poi.kind.color(),
                        false,
                    ));
                }
            }
        }
        if let Some((_, bearing)) = map.waypoint_nav() {
            marks.push((bearing as f32, "marker_waypoint".into(), p.accent, true));
        }
        for (bearing, icon, color, quest) in marks {
            let (mut x, d) = x_of(bearing);
            let off = d.abs() > fov * 0.5;
            if off && !quest {
                continue;
            }
            x = x.clamp(r.x + 12.0 * px, r.x + r.w - 12.0 * px);
            let size = if quest { 20.0 } else { 15.0 } * px;
            let y = top + strip_h * 0.5 - 2.0 * px;
            if quest {
                ctx.glow(Vec2::new(x, y), 16.0 * px, color.with_alpha(0.45), 1.5);
            }
            ctx.faded(if quest { 1.0 } else { 0.85 * fade(x).max(0.3) }, |ctx| {
                ctx.icon(
                    &icon,
                    Rect::new(x - size * 0.5, y - size * 0.5, size, size),
                    Color::WHITE,
                );
            });
        }
    }
    // Center notch and the heading readout.
    let notch_y = top + strip_h;
    ctx.line(
        Vec2::new(cx, top - 1.0 * px),
        Vec2::new(cx, top + strip_h),
        1.5 * px,
        p.accent.with_alpha(0.9),
        0.0,
    );
    let label = format!("{:03.0}°", heading.rem_euclid(360.0).round() % 360.0);
    let box_w = 50.0 * px;
    let box_h = 20.0 * px;
    let b = Rect::new(cx - box_w * 0.5, notch_y + 6.0 * px, box_w, box_h);
    ctx.rect(
        b,
        Color::hexa(0x0b1019e0),
        box_h * 0.5,
        1.0 * px,
        p.accent.fade(0.6),
    );
    ctx.text(
        &label,
        12.0,
        700,
        p.accent_light,
        b.center(),
        Vec2::splat(0.5),
    );
    // Little triangle on top of the readout.
    ctx.line(
        Vec2::new(cx - 4.0 * px, notch_y + 6.0 * px),
        Vec2::new(cx, notch_y + 1.0 * px),
        1.5 * px,
        p.accent,
        0.0,
    );
    ctx.line(
        Vec2::new(cx + 4.0 * px, notch_y + 6.0 * px),
        Vec2::new(cx, notch_y + 1.0 * px),
        1.5 * px,
        p.accent,
        0.0,
    );
}

fn paint_minimap(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(mm) = world.get::<Minimap>(e).cloned() else {
        return;
    };
    let heading = data_f64(world, &mm.heading).unwrap_or(0.0) as f32;
    let p = world.resource::<Theme>().palette.clone();
    let Some(map) = world.get_resource::<MapData>().cloned() else {
        return;
    };
    let r = ctx.rect;
    let px = ctx.px;
    let c = r.center();
    let rad = r.w.min(r.h) * 0.5;
    // Backdrop and the terrain window around the player (north up).
    ctx.rect(r, Color::hex(0x081018), rad, 0.0, Color::TRANSPARENT);
    let Some(planet) = map.player_planet() else {
        ctx.text("No signal", 13.0, 700, p.text_faint, c, Vec2::splat(0.5));
        return;
    };
    let span = mm.span_deg.max(0.1) as f64;
    let (lon, lat) = (map.player.lon, map.player.lat);
    let lon_span = span / lat.to_radians().cos().max(0.2);
    ctx.planet_flat(
        r,
        planet.look(),
        [
            (lon - lon_span * 0.5).to_radians() as f32,
            (lon + lon_span * 0.5).to_radians() as f32,
        ],
        [
            (lat + span * 0.5).to_radians() as f32,
            (lat - span * 0.5).to_radians() as f32,
        ],
        0.0,
        0.6,
        rad,
    );
    // Darken toward the rim.
    ctx.ring(
        c,
        Vec2::splat(rad - 14.0 * px),
        28.0 * px,
        Color::hexa(0x05080e70),
        0.0,
        std::f32::consts::TAU,
        0.0,
    );
    // Range rings.
    ctx.ring(
        c,
        Vec2::splat(rad * 0.5),
        1.0 * px,
        Color::WHITE.with_alpha(0.12),
        0.0,
        std::f32::consts::TAU,
        24.0,
    );
    let to_px = |plon: f64, plat: f64| {
        let mut dl = plon - lon;
        if dl > 180.0 {
            dl -= 360.0;
        }
        if dl < -180.0 {
            dl += 360.0;
        }
        Vec2::new(
            c.x + (dl / lon_span) as f32 * r.w,
            c.y - ((plat - lat) / span) as f32 * r.h,
        )
    };
    for poi in planet
        .pois
        .iter()
        .filter(|q| q.discovered && !map.is_waypoint(&planet.id, q))
    {
        let q = to_px(poi.lon, poi.lat);
        if (q - c).length() > rad - 16.0 * px {
            continue;
        }
        let s = 20.0 * px;
        ctx.glow(q, 12.0 * px, poi.kind.color().with_alpha(0.35), 1.6);
        ctx.icon(
            poi.kind.icon(),
            Rect::new(q.x - s * 0.5, q.y - s * 0.5, s, s),
            Color::WHITE,
        );
    }
    if let Some(w) = &map.waypoint
        && w.planet == planet.id
    {
        let mut q = to_px(w.lon, w.lat);
        let d = q - c;
        let edge = rad - 14.0 * px;
        let outside = d.length() > edge;
        if outside {
            q = c + d.normalize_or_zero() * edge;
        }
        // The pin stands on the spot; off the map, its head sits on the rim the way to go.
        let s = 24.0 * px;
        let tip = if outside { q + Vec2::Y * (s * 0.6) } else { q };
        map::waypoint_pin(ctx, tip, s, p.accent);
    }
    // The player: a view cone and the arrow.
    let cone = 22.0 * px;
    let hr = heading.to_radians();
    for k in [-1.0f32, 1.0] {
        let a = hr + k * 0.45;
        ctx.line(
            c,
            c + Vec2::new(a.sin(), -a.cos()) * cone * 2.2,
            1.0 * px,
            Color::WHITE.with_alpha(0.25),
            0.0,
        );
    }
    ctx.glow(c, 18.0 * px, p.info.with_alpha(0.35), 1.5);
    let s = 26.0 * px;
    ctx.icon_rotated(
        "marker_player",
        Rect::new(c.x - s * 0.5, c.y - s * 0.5, s, s),
        Color::WHITE,
        hr,
    );
    // Frame, north mark.
    ctx.ring(
        c,
        Vec2::splat(rad - 1.0 * px),
        2.0 * px,
        p.accent.with_alpha(0.75),
        0.0,
        std::f32::consts::TAU,
        0.0,
    );
    ctx.ring(
        c,
        Vec2::splat(rad - 5.0 * px),
        1.0 * px,
        Color::WHITE.with_alpha(0.15),
        0.0,
        std::f32::consts::TAU,
        72.0,
    );
    let n = Vec2::new(c.x, r.y + 1.0 * px);
    ctx.rect(
        Rect::new(n.x - 11.0 * px, n.y - 11.0 * px, 22.0 * px, 22.0 * px),
        Color::hex(0x0b1019),
        11.0 * px,
        1.5 * px,
        p.accent,
    );
    ctx.text("N", 11.0, 800, p.accent_light, n, Vec2::splat(0.5));
    // Scale: the inner ring's radius.
    let km = span / 4.0 * planet.radius_km * std::f64::consts::PI / 180.0;
    ctx.text(
        &format_distance(km * 1000.0),
        10.0,
        700,
        Color::WHITE.with_alpha(0.6),
        Vec2::new(c.x, c.y + rad * 0.5 + 9.0 * px),
        Vec2::splat(0.5),
    );
}

fn paint_crosshair(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(ch) = world.get::<Crosshair>(e).cloned() else {
        return;
    };
    let spread = data_f64(world, &ch.bind)
        .map(|v| v as f32)
        .unwrap_or(ch.spread)
        .clamp(0.0, 1.0);
    let c = ctx.rect.center();
    let px = ctx.px;
    let gap = (5.0 + spread * 16.0) * px;
    let len = 7.0 * px;
    let dark = Color::hexa(0x00000080);
    for d in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
        let a = c + d * gap;
        let b = c + d * (gap + len);
        ctx.line(a, b, 3.5 * px, dark, 0.0);
        ctx.line(a, b, 1.6 * px, Color::WHITE.with_alpha(0.92), 0.0);
    }
    ctx.rect(
        Rect::new(c.x - 2.0 * px, c.y - 2.0 * px, 4.0 * px, 4.0 * px),
        Color::WHITE,
        2.0 * px,
        1.0 * px,
        dark,
    );
}

fn paint_time_gauge(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let scale = world
        .get_resource::<Time>()
        .map(|t| t.scale())
        .unwrap_or(1.0);
    let p = world.resource::<Theme>().palette.clone();
    let r = ctx.rect;
    let c = r.center();
    let rad = r.w.min(r.h) * 0.5 - 3.0 * ctx.px;
    let tau = std::f32::consts::TAU;
    ctx.ring(
        c,
        Vec2::splat(rad),
        4.0 * ctx.px,
        Color::WHITE.with_alpha(0.12),
        0.0,
        tau,
        0.0,
    );
    let frac = scale.clamp(0.0, 1.0);
    ctx.ring(
        c,
        Vec2::splat(rad),
        4.0 * ctx.px,
        p.info,
        0.0,
        frac * tau,
        0.0,
    );
    ctx.ring(
        c,
        Vec2::splat(rad - 6.0 * ctx.px),
        1.0 * ctx.px,
        Color::WHITE.with_alpha(0.25),
        0.0,
        tau,
        12.0,
    );
    ctx.glow(c, rad, p.info.with_alpha(0.18 * (1.0 - frac)), 1.2);
    let s = rad * 1.05;
    ctx.icon(
        "ability_time_slow",
        Rect::new(c.x - s * 0.5, c.y - s * 0.5, s, s),
        Color::WHITE,
    );
}

/// Register the HUD painters.
pub fn add_painters(p: &mut Painters) {
    p.add("compass", paint_compass);
    p.add("minimap", paint_minimap);
    p.add("crosshair", paint_crosshair);
    p.add("time_gauge", paint_time_gauge);
}

// ------------------------------------------------------------------------------- markers

/// A marker node: edge arrow, icon disc, title, subtitle and distance. Insert a
/// [`WorldAnchor`] on the spawned root to place it.
pub fn marker_el(m: &Marker) -> El {
    let c = m.color;
    let mut col = El::new()
        .class("col")
        .attach("Marker", m)
        .with_style(|s| s.align_items(Align::Center).gap(3.0).pick(Pick::Ignore))
        .child(
            El::icon("ui_arrow_up")
                .attach("MarkerArrow", &MarkerArrow)
                .with_style(|s| s.size(16.0, 16.0).color(c).opacity(0.0).transition(0.15))
                .pick_ignore(),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.size(34.0, 34.0)
                        .center()
                        .radius(Corners::all(17.0))
                        .background(Fill::Radial {
                            inner: c.fade(0.4),
                            outer: Color::hexa(0x070b12d8),
                        })
                        .border(1.5, c.fade(0.9))
                        .shadow(Shadow::glow(c.fade(0.45), 12.0))
                })
                .child(
                    El::icon(m.icon.clone())
                        .with_style(|s| s.size(22.0, 22.0))
                        .pick_ignore(),
                ),
        );
    if !m.title.is_empty() {
        col = col.child(
            El::text(m.title.clone())
                .text_style(|t| t.semibold().nowrap().shadow())
                .with_style(|s| s.font_size(15.0)),
        );
    }
    if !m.subtitle.is_empty() {
        col = col.child(
            El::text(m.subtitle.clone())
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| {
                    s.font_size(12.0)
                        .font_weight(600)
                        .color(c.lerp(Color::WHITE, 0.4))
                }),
        );
    }
    col.child(
        El::text("")
            .attach("MarkerDistance", &MarkerDistance)
            .text_style(|t| t.tabular().nowrap().shadow())
            .with_style(|s| {
                s.font_size(12.0)
                    .font_weight(700)
                    .color(Color::WHITE.with_alpha(0.8))
            }),
    )
}

/// Spawn a world marker at `anchor`; returns its root.
pub fn spawn_marker(world: &mut World, m: &Marker, anchor: WorldAnchor) -> Result<Entity, String> {
    let sp = el::spawn(world, None, &marker_el(m))?;
    let root = sp.root.ok_or("empty marker")?;
    world
        .entity_mut(root)
        .insert((anchor, UiId(format!("marker:{}", m.title))));
    Ok(root)
}

/// The UI test area's markers: the trader over `trader` (relative to `origin`), a quest
/// beacon off in the distance (clamped to the screen edge) and a lookout.
pub fn spawn_world_markers(world: &mut World, origin: DVec3, trader: DVec3) {
    let p = world.resource::<Theme>().palette.clone();
    let items = [
        (
            Marker {
                title: "Old Brannoc".into(),
                subtitle: "Trader".into(),
                icon: "marker_vendor".into(),
                color: p.accent,
                show_distance: false,
                always: false,
                waypoint: false,
            },
            trader,
            false,
            [8.0, 45.0],
        ),
        (
            Marker {
                title: "Relay Tower".into(),
                subtitle: "The Silent Relay".into(),
                icon: "marker_quest".into(),
                color: p.accent_light,
                show_distance: true,
                always: false,
                waypoint: true,
            },
            DVec3::new(-140.0, 40.0, -900.0),
            true,
            [0.0, 0.0],
        ),
        (
            Marker {
                title: "Ridge Lookout".into(),
                subtitle: String::new(),
                icon: "marker_outpost".into(),
                color: p.info,
                show_distance: true,
                always: false,
                waypoint: false,
            },
            DVec3::new(60.0, 12.0, -180.0),
            false,
            [20.0, 400.0],
        ),
    ];
    for (m, offset, clamp, fade) in items {
        let anchor = WorldAnchor {
            position: origin + offset,
            clamp_to_edges: clamp,
            fade_distance: fade,
            ..Default::default()
        };
        if let Err(e) = spawn_marker(world, &m, anchor) {
            tracing::warn!("marker: {e}");
        }
    }
}

// ----------------------------------------------------------------------------------- API

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// `ui.notify`, `hud.marker`, `hud.markers`, `hud.remove_marker`.
pub fn register_api(app: &mut App) {
    app.register_api(
        "ui.notify",
        "Show a toast: text (markup), title, kind (info, good, bad, warn, loot), icon, duration (s).",
        r#"{"title": "Quest updated", "text": "Reach the relay tower", "kind": "info"}"#,
        |world, args| {
            let kind = match str_arg(&args, "kind").unwrap_or("info") {
                "good" => ToastKind::Good,
                "bad" => ToastKind::Bad,
                "warn" => ToastKind::Warn,
                "loot" => ToastKind::Loot,
                "info" => ToastKind::Info,
                other => {
                    return Err(format!(
                        "There is no toast kind \"{other}\". Use info, good, bad, warn or loot."
                    ));
                }
            };
            let text = str_arg(&args, "text").unwrap_or("");
            let title = str_arg(&args, "title").unwrap_or("");
            if text.is_empty() && title.is_empty() {
                return Err("Give the toast a \"text\" or a \"title\".".into());
            }
            let duration = args.get("duration").and_then(Value::as_f64).unwrap_or(3.5) as f32;
            toast::notify(world, kind, str_arg(&args, "icon").unwrap_or(""), title, text, duration);
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "hud.marker",
        "Pin a marker to the world: title, subtitle, icon, color, and position [x, y, z] or entity (a name) with offset [x, y, z]; clamp keeps it on the screen edge; fade [near, far] m; always shows it without the HUD; waypoint shows the distance to the map waypoint.",
        r#"{"title": "Relay Tower", "icon": "marker_quest", "position": [120, 30, -800], "clamp": true}"#,
        |world, args| {
            let mut m = Marker {
                title: str_arg(&args, "title").unwrap_or("").into(),
                subtitle: str_arg(&args, "subtitle").unwrap_or("").into(),
                ..Default::default()
            };
            if let Some(i) = str_arg(&args, "icon") {
                m.icon = i.into();
            }
            if let Some(c) = str_arg(&args, "color") {
                m.color = Color::parse(c)?;
            }
            if let Some(b) = args.get("distance").and_then(Value::as_bool) {
                m.show_distance = b;
            }
            m.always = args.get("always").and_then(Value::as_bool).unwrap_or(false);
            m.waypoint = args.get("waypoint").and_then(Value::as_bool).unwrap_or(false);
            let v3 = |v: Option<&Value>| -> Option<[f64; 3]> {
                let a = v?.as_array()?;
                Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
            };
            let mut anchor = WorldAnchor {
                clamp_to_edges: args.get("clamp").and_then(Value::as_bool).unwrap_or(false),
                ..Default::default()
            };
            if let Some(f) = args.get("fade").and_then(Value::as_array)
                && f.len() == 2
            {
                anchor.fade_distance = [
                    f[0].as_f64().unwrap_or(0.0) as f32,
                    f[1].as_f64().unwrap_or(0.0) as f32,
                ];
            }
            if let Some(name) = str_arg(&args, "entity") {
                anchor.entity = Some(engine_core::registry::find_by_name(world, name)?);
                if let Some(o) = v3(args.get("offset")) {
                    anchor.offset = Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32);
                }
            } else if let Some(pos) = v3(args.get("position")) {
                anchor.position = DVec3::from_array(pos);
            } else {
                return Err("Give the marker a \"position\" [x, y, z] or an \"entity\" name.".into());
            }
            let e = spawn_marker(world, &m, anchor)?;
            Ok(json!({"id": format!("marker:{}", m.title), "entity": e.to_bits()}))
        },
    );
    app.register_api(
        "hud.markers",
        "World markers: id, title, distance (m), on screen.",
        "{}",
        |world, _| {
            let mut q = world.query::<(&UiId, &Marker, &WorldAnchor)>();
            let v: Vec<Value> = q
                .iter(world)
                .map(|(id, m, a)| json!({"id": id.0, "title": m.title, "distance": a.distance, "on_screen": a.on_screen}))
                .collect();
            Ok(json!(v))
        },
    );
    app.register_api(
        "hud.remove_marker",
        "Remove a world marker by id (from hud.markers).",
        r#"{"id": "marker:Relay Tower"}"#,
        |world, args| {
            let id = str_arg(&args, "id").ok_or("Pass the marker's \"id\".")?;
            let mut q = world.query_filtered::<(Entity, &UiId), With<Marker>>();
            let found: Vec<Entity> = q
                .iter(world)
                .filter(|(_, i)| i.0 == id)
                .map(|(e, _)| e)
                .collect();
            if found.is_empty() {
                return Err(format!(
                    "There is no marker \"{id}\". hud.markers lists them."
                ));
            }
            for e in &found {
                for c in screens::subtree(world, *e).into_iter().rev() {
                    if let Ok(ent) = world.get_entity_mut(c) {
                        ent.despawn();
                    }
                }
            }
            Ok(json!({"removed": found.len()}))
        },
    );
}
