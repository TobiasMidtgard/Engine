//! Menus: the pause menu, settings, and a confirm dialog.
//!
//! - `ui.show {"screen": "pause"}` pauses game time (the UI keeps animating) and offers
//!   Resume, Inventory, Star Map, Settings, Save and Quit. Menu items send `game.save`,
//!   `game.load` and `game.quit` actions for the game to handle.
//! - `ui.show {"screen": "settings"}`: Gameplay, Video, Audio, Controls and Interface tabs.
//!   Video rows edit the renderer's `RenderSettings`, Interface rows `UiSettings`, Controls
//!   lists and rebinds the input map's actions; the rest live in [`UiData`] under
//!   `settings.*` for the game to read.
//! - `ui.show {"screen": "confirm", "title": "Quit to title?", "text": "...", "action":
//!   "game.quit", "confirm": "Quit", "danger": true}` asks before doing something.

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_input::{Binding, ButtonInput, InputMap, KeyCode};
use serde_json::{Value, json};

use crate::color::Color;
use crate::data::UiData;
use crate::el::{self, El};
use crate::input::UiEvent;
use crate::node::*;
use crate::screens;
use crate::style::{Align, Corners, Fill, Justify, Pick, Position, pct};
use crate::theme::Theme;
use crate::toast::{self, ToastKind};
use crate::widgets::{self, Slider};

// ---------------------------------------------------------------------------------- pause

/// Game time scale to restore when the pause menu closes.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct PauseState {
    pub resume_scale: Option<f32>,
}

/// The pause screen's root.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PauseMenu;

fn menu_item(icon: &str, label: &str, action: &str, key: &str, id: &str) -> El {
    let mut row = El::new()
        .class("menu_item")
        .id(id)
        .on_click(action)
        .child(
            El::icon(icon)
                .with_style(|s| s.size(22.0, 22.0))
                .pick_ignore(),
        )
        .child(
            El::text(label)
                .text_style(|t| t.nowrap())
                .with_style(|s| s.grow(1.0))
                .pick_ignore(),
        );
    if !key.is_empty() {
        row = row.child(
            El::text(key)
                .class("kbd")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        );
    }
    row
}

fn pause_el(world: &World) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let data = world.get_resource::<UiData>();
    let s = |path: &str| data.and_then(|d| d.get_str(path)).unwrap_or("").to_string();
    let chapter = s("quest.title");
    let menu = El::new()
        .class("col")
        .id("pause_menu")
        .with_style(|s| s.gap(4.0).width(420.0))
        .child(menu_item(
            "ui_arrow_right",
            "Resume",
            "ui.close",
            "Esc",
            "pause_resume",
        ))
        .child(menu_item(
            "backpack",
            "Inventory",
            "pause.open:inventory",
            "I",
            "pause_inventory",
        ))
        .child(menu_item(
            "marker_quest",
            "Journal",
            "pause.open:journal",
            "J",
            "pause_journal",
        ))
        .child(menu_item(
            "ui_galaxy",
            "Star Map",
            "pause.open:map",
            "M",
            "pause_map",
        ))
        .child(menu_item(
            "ui_settings",
            "Settings",
            "pause.open:settings",
            "",
            "pause_settings",
        ))
        .child(
            El::new()
                .class("divider")
                .with_style(|s| s.margin([8.0, 0.0])),
        )
        .child(menu_item(
            "scroll",
            "Save Game",
            "pause.save",
            "F5",
            "pause_save",
        ))
        .child(menu_item(
            "map_scroll",
            "Load Game",
            "game.load",
            "F9",
            "pause_load",
        ))
        .child(menu_item(
            "ui_close",
            "Quit to Title",
            "pause.quit",
            "",
            "pause_quit",
        ));
    let left = El::new()
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(120.0)
                .top(0.0)
                .bottom(0.0)
                .justify(Justify::Center)
                .gap(28.0)
        })
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(6.0))
                .child(
                    El::text("PAUSED")
                        .class("label_caps")
                        .with_style(|s| s.font_size(13.0).color(p.accent)),
                )
                .child(
                    El::text("THE VEIL")
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|s| s.font_size(64.0).color(p.text)),
                )
                .child(
                    El::text(if chapter.is_empty() {
                        "Chapter II".to_string()
                    } else {
                        format!("Chapter II  ·  {chapter}")
                    })
                    .text_style(|t| t.nowrap())
                    .with_style(|s| s.font_size(17.0).font_weight(600).color(p.text_dim)),
                ),
        )
        .child(menu);
    // The character card.
    let stat_bar = |label: &str, bind: &str, max: &str, c1: u32, c2: u32| {
        El::new()
            .class("col")
            .with_style(|s| s.gap(4.0))
            .child(
                El::new()
                    .class("row")
                    .child(El::text(label).class("caption").with_style(|s| s.grow(1.0)))
                    .child(
                        El::text(format!("{{{bind}:0}} / {{{max}:0}}"))
                            .class("caption")
                            .text_style(|t| t.tabular().nowrap()),
                    ),
            )
            .child(
                El::new()
                    .class("bar_track")
                    .paint("bar")
                    .attach(
                        "Bar",
                        &widgets::Bar::bound(bind, max, Color::hex(c1), Color::hex(c2)),
                    )
                    .with_style(|s| s.width(pct(100.0)).height(8.0)),
            )
    };
    let card = El::new()
        .class("panel")
        .id("pause_card")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(120.0)
                .top(pct(50.0))
                .translate([0.0, -300.0])
                .width(400.0)
                .gap(16.0)
                .padding(24.0)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0))
                .child(
                    El::icon("portrait:pilot")
                        .with_style(|s| {
                            s.size(96.0, 96.0)
                                .radius(Corners::all(48.0))
                                .background(Fill::Radial {
                                    inner: p.info.fade(0.35),
                                    outer: Color::hexa(0x0a1220ff),
                                })
                                .border(2.0, p.accent.fade(0.85))
                        })
                        .component("Icon", json!({"name": "portrait:pilot", "cover": true}))
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(3.0).grow(1.0))
                        .child(
                            El::text("{player.name}")
                                .class("heading")
                                .with_style(|s| s.font_size(22.0)),
                        )
                        .child(El::text("Courier  ·  Level {player.level}").class("caption"))
                        .child(
                            widgets::currency("{player.gold}", 18.0)
                                .with_style(|s| s.margin([6.0, 0.0, 0.0, 0.0])),
                        ),
                ),
        )
        .child(stat_bar(
            "Health",
            "player.health",
            "player.health_max",
            0xd8433a,
            0xff8a5c,
        ))
        .child(stat_bar(
            "Shield",
            "player.shield",
            "player.shield_max",
            0x2f8fe0,
            0x7fe0ff,
        ))
        .child(stat_bar(
            "Experience",
            "player.xp",
            "player.xp_max",
            0x9c7330,
            0xf7d999,
        ))
        .child(widgets::divider())
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(6.0))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(8.0))
                        .child(El::icon("marker_quest").with_style(|s| s.size(16.0, 16.0)))
                        .child(El::text("CURRENT OBJECTIVE").class("label_caps")),
                )
                .child(
                    El::text("{quest.title}")
                        .with_style(|s| s.font_size(16.0).font_weight(700).color(p.accent_light)),
                )
                .child(
                    El::text("{quest.step}")
                        .class("body")
                        .text_style(|t| t.line_height(1.4))
                        .with_style(|s| s.font_size(14.0)),
                ),
        )
        .child(widgets::divider())
        .child(widgets::stat_row("Location", "{location.name}", None))
        .child(widgets::stat_row("Play time", "12 h 41 m", None));
    El::new()
        .class("screen")
        .child(El::new().with_style(|s| {
            s.fill_parent()
                .backdrop_blur(14.0)
                .background(Fill::Linear {
                    from: Color::hexa(0x04060cf0),
                    to: Color::hexa(0x04060c90),
                    angle_deg: 0.0,
                })
                .pick(Pick::Block)
        }))
        .child(left)
        .child(card)
        .child(
            El::text("Engine UI showcase  ·  build 0.1")
                .class("caption")
                .with_style(|s| s.position(Position::Absolute).left(120.0).bottom(40.0)),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .right(120.0)
                        .bottom(36.0)
                        .gap(24.0)
                })
                .child(widgets::key_hint("Enter", "A", "Select"))
                .child(widgets::key_hint("Esc", "B", "Resume")),
        )
}

/// Builder for `ui.show {"screen": "pause"}`.
pub fn build_pause(world: &mut World, _args: &Value) -> Result<Entity, String> {
    let el = pause_el(world);
    let sp = screens::spawn_screen(
        world,
        Screen {
            key: "pause".into(),
            modal: true,
            layer: 50,
            closable: true,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("The pause menu is empty.")?;
    world.entity_mut(root).insert(PauseMenu);
    // Stop game time while paused.
    let mut state = world
        .get_resource::<PauseState>()
        .copied()
        .unwrap_or_default();
    if state.resume_scale.is_none()
        && let Some(mut t) = world.get_resource_mut::<Time>()
    {
        state.resume_scale = Some(t.base_scale());
        t.set_base_scale(0.0);
    }
    world.insert_resource(state);
    Ok(root)
}

// ------------------------------------------------------------------------------- confirm

/// Builder for `ui.show {"screen": "confirm"}`: title, text, confirm (button label), action
/// (sent on confirm), cancel (label), danger (red button).
pub fn build_confirm(world: &mut World, args: &Value) -> Result<Entity, String> {
    let s = |k: &str, d: &str| args.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
    let action = s("action", "");
    if action.is_empty() {
        return Err("Give the confirm dialog an \"action\" to send when confirmed.".into());
    }
    let danger = args.get("danger").and_then(Value::as_bool).unwrap_or(false);
    let el = El::new()
        .class("screen center")
        .child(El::new().with_style(|_| widgets::scrim(0.7)))
        .child(
            El::new()
                .class("panel")
                .with_style(|s| s.width(520.0).gap(18.0).padding(28.0))
                .child(
                    El::text(s("title", "Are you sure?"))
                        .class("title")
                        .with_style(|s| s.font_size(24.0)),
                )
                .child(
                    El::text(s("text", ""))
                        .class("body")
                        .text_style(|t| t.line_height(1.45)),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| {
                            s.gap(10.0)
                                .justify(Justify::End)
                                .margin([8.0, 0.0, 0.0, 0.0])
                        })
                        .child(
                            El::button(s("cancel", "Cancel"), "ui.close")
                                .id("confirm_cancel")
                                .class("ghost"),
                        )
                        .child(
                            El::button(s("confirm", "Confirm"), format!("confirm:{action}"))
                                .id("confirm_ok")
                                .class(if danger { "danger" } else { "primary" }),
                        ),
                ),
        );
    let sp = screens::spawn_screen(
        world,
        Screen {
            key: "confirm".into(),
            modal: true,
            layer: 70,
            closable: true,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("The dialog is empty.")?;
    Ok(root)
}

// ------------------------------------------------------------------------------ settings

/// Settings tabs.
pub const SETTINGS_TABS: [&str; 5] = ["Gameplay", "Video", "Audio", "Controls", "Interface"];

/// The settings screen's state (on its root).
#[derive(Component, Debug, Clone, Default)]
pub struct SettingsScreen {
    pub tab: usize,
    /// Waiting for a key to bind to this action.
    pub rebinding: Option<String>,
}

fn default_settings() -> Value {
    json!({
        "fov": 80.0, "sensitivity": 1.0, "invert_y": false, "camera_shake": true, "subtitles": true,
        "aim_assist": true,
        "audio": {"master": 0.9, "music": 0.7, "effects": 0.85, "voice": 1.0, "mute_unfocused": true},
        "ui": {"minimap": true, "damage_numbers": true, "reduce_motion": false}
    })
}

#[allow(clippy::too_many_arguments)]
fn slider(
    label: &str,
    bind: &str,
    min: f32,
    max: f32,
    step: f32,
    decimals: i32,
    suffix: &str,
    id: &str,
) -> El {
    widgets::slider_row(
        label,
        Slider {
            min,
            max,
            step,
            bind: bind.into(),
            decimals,
            suffix: suffix.into(),
            ..Default::default()
        },
    )
    .id(id)
}

fn section(title: &str) -> El {
    El::text(title.to_uppercase())
        .class("label_caps")
        .with_style(|s| s.margin([14.0, 4.0, 4.0, 4.0]))
}

/// "Digit1" → "1", "ShiftLeft" → "Shift", ...
pub fn key_label(k: KeyCode) -> String {
    let s = format!("{k:?}");
    if let Some(d) = s.strip_prefix("Digit") {
        return d.into();
    }
    match s.as_str() {
        "ShiftLeft" | "ShiftRight" => "Shift".into(),
        "ControlLeft" | "ControlRight" => "Ctrl".into(),
        "AltLeft" | "AltRight" => "Alt".into(),
        "ArrowUp" => "↑".into(),
        "ArrowDown" => "↓".into(),
        "ArrowLeft" => "←".into(),
        "ArrowRight" => "→".into(),
        "Escape" => "Esc".into(),
        "Backspace" => "⌫".into(),
        other => other
            .strip_prefix("Numpad")
            .map(|n| format!("Num {n}"))
            .unwrap_or_else(|| other.into()),
    }
}

fn binding_label(b: &Binding) -> Option<(String, bool)> {
    match b {
        Binding::Key(k) => Some((key_label(*k), false)),
        Binding::Mouse(m) => Some((format!("Mouse {m:?}"), false)),
        Binding::Gamepad(g) => Some((
            match format!("{g:?}").as_str() {
                "South" => "A".into(),
                "East" => "B".into(),
                "West" => "X".into(),
                "North" => "Y".into(),
                "LeftBumper" => "LB".into(),
                "RightBumper" => "RB".into(),
                "LeftTrigger" => "LT".into(),
                "RightTrigger" => "RT".into(),
                "LeftStick" => "L3".into(),
                "RightStick" => "R3".into(),
                "Start" => "Menu".into(),
                "Select" => "View".into(),
                other => other.into(),
            },
            true,
        )),
        _ => None,
    }
}

/// A key cap (square corners) or gamepad button (pill) holding a binding label.
fn binding_chip(label: String, pad: bool) -> El {
    El::new()
        .class("kbd")
        .with_style(move |s| {
            let s = s.min_width(30.0).height(26.0).padding([0.0, 8.0]);
            if pad { s.radius(Corners::all(13.0)) } else { s }
        })
        .pick_ignore()
        .child(
            El::text(label)
                .text_style(|t| t.nowrap())
                .with_style(|s| s.font_size(13.0))
                .pick_ignore(),
        )
}

fn pretty_action(a: &str) -> String {
    let mut s = a.replace(['_', '.'], " ");
    if let Some(f) = s.get(0..1) {
        let up = f.to_uppercase();
        s.replace_range(0..1, &up);
    }
    s
}

fn controls_rows(world: &World, rebinding: Option<&str>) -> Vec<El> {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let mut rows = vec![section("Key bindings")];
    let Some(map) = world.get_resource::<InputMap>() else {
        rows.push(El::text("No input map.").class("body"));
        return rows;
    };
    if map.actions.is_empty() {
        rows.push(El::text("The game hasn't bound any actions yet.").class("body"));
        return rows;
    }
    for (name, b) in &map.actions {
        let waiting = rebinding == Some(name.as_str());
        let mut row = El::new()
            .class("setting_row")
            .id(format!("bind:{name}"))
            .on_click(format!("settings.rebind:{name}"))
            .tooltip("", "Click, then press a key")
            .child(
                El::text(pretty_action(name))
                    .with_style(|s| s.grow(1.0))
                    .pick_ignore(),
            );
        if waiting {
            row = row.child(
                El::text("Press a key…  [color=#8a94a8](Esc cancels)[/color]")
                    .text_style(|t| t.nowrap())
                    .with_style(|s| s.font_size(14.0).font_weight(600).color(p.accent_light))
                    .pick_ignore(),
            );
        } else {
            let mut chips = El::new()
                .class("row")
                .with_style(|s| s.gap(6.0))
                .pick_ignore();
            for (label, pad) in b.buttons.iter().filter_map(binding_label) {
                chips = chips.child(binding_chip(label, pad));
            }
            row = row.child(chips);
        }
        rows.push(row);
    }
    rows
}

fn settings_rows(world: &World, tab: usize, rebinding: Option<&str>) -> Vec<El> {
    let has = |name: &str| {
        world
            .get_resource::<engine_core::registry::TypeRegistry>()
            .is_some_and(|r| r.resource(name).is_some())
    };
    match tab {
        0 => vec![
            section("Camera"),
            slider(
                "Field of view",
                "settings.fov",
                60.0,
                110.0,
                1.0,
                0,
                "°",
                "set_fov",
            ),
            slider(
                "Look sensitivity",
                "settings.sensitivity",
                0.1,
                3.0,
                0.05,
                2,
                "×",
                "set_sens",
            ),
            widgets::toggle_row("Invert look", "settings.invert_y", "set_invert"),
            widgets::toggle_row("Camera shake", "settings.camera_shake", "set_shake"),
            section("Assists"),
            widgets::toggle_row("Aim assist", "settings.aim_assist", "set_aim"),
            widgets::toggle_row("Subtitles", "settings.subtitles", "set_subs"),
        ],
        1 => {
            if !has("RenderSettings") {
                return vec![El::text("The renderer isn't running.").class("body")];
            }
            vec![
                section("Image"),
                slider(
                    "Bloom",
                    "$RenderSettings.bloom_intensity",
                    0.0,
                    0.3,
                    0.01,
                    2,
                    "",
                    "set_bloom",
                ),
                slider(
                    "Saturation",
                    "$RenderSettings.saturation",
                    0.5,
                    1.5,
                    0.05,
                    2,
                    "",
                    "set_sat",
                ),
                slider(
                    "Contrast",
                    "$RenderSettings.contrast",
                    0.5,
                    1.5,
                    0.05,
                    2,
                    "",
                    "set_contrast",
                ),
                slider(
                    "Vignette",
                    "$RenderSettings.vignette",
                    0.0,
                    1.0,
                    0.05,
                    2,
                    "",
                    "set_vignette",
                ),
                slider(
                    "Film grain",
                    "$RenderSettings.film_grain",
                    0.0,
                    0.2,
                    0.01,
                    2,
                    "",
                    "set_grain",
                ),
                section("Performance"),
                slider(
                    "Shadow distance",
                    "$RenderSettings.shadow_distance",
                    20.0,
                    600.0,
                    10.0,
                    0,
                    " m",
                    "set_shadow",
                ),
                widgets::toggle_row("Vertical sync", "$RenderSettings.vsync", "set_vsync"),
            ]
        }
        2 => vec![
            section("Volume"),
            slider(
                "Master",
                "settings.audio.master",
                0.0,
                1.0,
                0.05,
                2,
                "",
                "set_master",
            ),
            slider(
                "Music",
                "settings.audio.music",
                0.0,
                1.0,
                0.05,
                2,
                "",
                "set_music",
            ),
            slider(
                "Effects",
                "settings.audio.effects",
                0.0,
                1.0,
                0.05,
                2,
                "",
                "set_fx",
            ),
            slider(
                "Voice",
                "settings.audio.voice",
                0.0,
                1.0,
                0.05,
                2,
                "",
                "set_voice",
            ),
            widgets::toggle_row(
                "Mute in background",
                "settings.audio.mute_unfocused",
                "set_mute",
            ),
        ],
        3 => controls_rows(world, rebinding),
        _ => vec![
            section("Layout"),
            slider(
                "Interface scale",
                "$UiSettings.scale",
                0.75,
                1.5,
                0.05,
                2,
                "×",
                "set_scale",
            ),
            slider(
                "Tooltip delay",
                "$UiSettings.tooltip_delay",
                0.0,
                1.5,
                0.05,
                2,
                " s",
                "set_tooltip",
            ),
            widgets::toggle_row("Show minimap", "settings.ui.minimap", "set_minimap"),
            widgets::toggle_row("Damage numbers", "settings.ui.damage_numbers", "set_dmg"),
            widgets::toggle_row("Reduce motion", "settings.ui.reduce_motion", "set_motion"),
            section("Accent color"),
            El::new()
                .class("row")
                .id("set_accent")
                .with_style(|s| s.gap(12.0).padding([6.0, 16.0]))
                .children(ACCENTS.iter().enumerate().map(|(i, (name, hex, _, _))| {
                    El::new()
                        .class("swatch")
                        .id(format!("accent:{i}"))
                        .on_click(format!("settings.accent:{i}"))
                        .tooltip(*name, "")
                        .with_style(|s| {
                            s.background(Fill::vertical(
                                Color::hex(*hex).scale(1.15),
                                Color::hex(*hex).scale(0.8),
                            ))
                        })
                })),
        ],
    }
}

/// Accent presets: name, accent, light, dark.
const ACCENTS: [(&str, u32, u32, u32); 5] = [
    ("Gold", 0xe8b65a, 0xf7d999, 0x9c7330),
    ("Cyan", 0x5fd0ff, 0xa8e8ff, 0x2a7aa0),
    ("Violet", 0xb08cff, 0xd8c8ff, 0x6a4ab0),
    ("Jade", 0x5fd8a0, 0xa8f0cc, 0x2a8a60),
    ("Ember", 0xff8a5c, 0xffc0a0, 0xb04a28),
];

fn settings_el(world: &World, st: &SettingsScreen) -> El {
    let rows = settings_rows(world, st.tab, st.rebinding.as_deref());
    El::new()
        .class("screen center")
        .child(El::new().with_style(|_| widgets::scrim(0.8)))
        .child(
            El::new()
                .class("panel")
                .id("settings_panel")
                .with_style(|s| {
                    s.width(1040.0)
                        .height(780.0)
                        .max_height(pct(92.0))
                        .gap(18.0)
                        .padding(28.0)
                })
                .child(widgets::header(
                    Some("ui_settings"),
                    "Settings",
                    "Changes apply right away",
                    true,
                ))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(12.0))
                        .child(widgets::tabs(&SETTINGS_TABS, st.tab, "settings.tab"))
                        .child(El::new().with_style(|s| s.grow(1.0)))
                        .child(widgets::key_hint2(["Q", "E"], ["LB", "RB"], "Tab")),
                )
                .child(
                    El::new()
                        .class("scroll")
                        .id("settings_rows")
                        .with_style(|s| {
                            s.grow(1.0)
                                .shrink(1.0)
                                .basis(0.0)
                                .gap(6.0)
                                .padding([0.0, 8.0, 0.0, 0.0])
                        })
                        .children(rows),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(10.0).align_items(Align::Center))
                        .child(widgets::key_hint("←→", "◀▶", "Adjust"))
                        .child(El::new().with_style(|s| s.grow(1.0)))
                        .child(
                            El::button("Reset to defaults", "settings.reset")
                                .id("settings_reset")
                                .class("ghost"),
                        )
                        .child(
                            El::button("Done", "ui.close")
                                .id("settings_done")
                                .class("primary")
                                .with_style(|s| s.min_width(140.0)),
                        ),
                ),
        )
}

/// Builder for `ui.show {"screen": "settings"}`; `tab` picks the first tab (0-4 or a name).
pub fn build_settings(world: &mut World, args: &Value) -> Result<Entity, String> {
    if let Some(mut d) = world.get_resource_mut::<UiData>()
        && d.get("settings").is_none()
    {
        d.set("settings", default_settings());
    }
    let tab = match args.get("tab") {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as usize,
        Some(Value::String(s)) => SETTINGS_TABS
            .iter()
            .position(|t| t.eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("There is no settings tab \"{s}\". Tabs: {SETTINGS_TABS:?}."))?,
        _ => 0,
    }
    .min(SETTINGS_TABS.len() - 1);
    let st = SettingsScreen {
        tab,
        rebinding: None,
    };
    let el = settings_el(world, &st);
    let sp = screens::spawn_screen(
        world,
        Screen {
            key: "settings".into(),
            modal: true,
            layer: 60,
            closable: true,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("The settings screen is empty.")?;
    world.entity_mut(root).insert(st);
    Ok(root)
}

fn refresh_settings(world: &mut World, root: Entity, st: &SettingsScreen) {
    let rows = settings_rows(world, st.tab, st.rebinding.as_deref());
    if let Some(list) = screens::find_in(world, root, "settings_rows") {
        if let Err(e) = el::replace_children(world, list, &rows) {
            tracing::warn!("settings: {e}");
        }
        if let Some(mut s) = world.get_mut::<ScrollState>(list) {
            s.target = engine_core::math::Vec2::ZERO;
        }
    }
    for i in 0..SETTINGS_TABS.len() {
        if let Some(e) = screens::find_in(world, root, &format!("settings.tab:{i}"))
            && let Some(mut it) = world.get_mut::<Interaction>(e)
        {
            it.selected = i == st.tab;
        }
    }
    world.entity_mut(root).insert(st.clone());
}

// -------------------------------------------------------------------------------- system

/// Pause-menu actions, confirm dialogs, settings tabs, resets, accents and key rebinding;
/// restores game time when the pause menu closes.
pub fn menus_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    for ev in &events {
        match ev {
            UiEvent::Action { action, entity, .. } => {
                if let Some(screen) = action.strip_prefix("pause.open:") {
                    screens::hide(world, "pause", true);
                    resume_time(world);
                    if let Err(e) = screens::show(world, screen, &Value::Null) {
                        tracing::warn!("{e}");
                    }
                } else if action == "pause.save" {
                    crate::input::dispatch(world, "game.save");
                    toast::notify(
                        world,
                        ToastKind::Good,
                        "scroll",
                        "Game saved",
                        "Hollow Reach  ·  autosave slot 1",
                        2.5,
                    );
                } else if action == "pause.quit" {
                    let args = json!({"title": "Quit to title?", "text": "Progress since your last save will be lost.", "confirm": "Quit", "action": "game.quit", "danger": true});
                    if let Err(e) = screens::show(world, "confirm", &args) {
                        tracing::warn!("{e}");
                    }
                } else if let Some(a) = action.strip_prefix("confirm:") {
                    if let Some(e) = entity {
                        let root = screens::root_of(world, *e);
                        screens::close_screen(world, root, false);
                    }
                    crate::input::dispatch(world, a);
                } else if let Some(t) = action
                    .strip_prefix("settings.tab:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    with_settings(world, |st| {
                        st.tab = t.min(SETTINGS_TABS.len() - 1);
                        st.rebinding = None;
                    });
                } else if action == "settings.reset" {
                    if let Some(mut d) = world.get_resource_mut::<UiData>() {
                        d.set("settings", default_settings());
                    }
                    let _ = crate::data::write(world, "$UiSettings.scale", json!(1.0));
                    let _ = crate::data::write(world, "$UiSettings.tooltip_delay", json!(0.45));
                    if world.contains_resource::<engine_render::RenderSettings>() {
                        world.insert_resource(engine_render::RenderSettings::default());
                    }
                    toast::notify(
                        world,
                        ToastKind::Info,
                        "ui_settings",
                        "Settings reset",
                        "",
                        2.0,
                    );
                    with_settings(world, |_| {});
                } else if let Some(i) = action
                    .strip_prefix("settings.accent:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    if let Some((_, a, l, d)) = ACCENTS.get(i) {
                        let palette = json!({"palette": {"accent": format!("#{a:06x}"), "accent_light": format!("#{l:06x}"), "accent_dark": format!("#{d:06x}")}});
                        if let Err(e) = engine_core::registry::call_api(world, "ui.theme", palette)
                        {
                            tracing::warn!("{e}");
                        }
                    }
                } else if let Some(a) = action.strip_prefix("settings.rebind:") {
                    let a = a.to_string();
                    with_settings(world, |st| st.rebinding = Some(a.clone()));
                }
            }
            UiEvent::Tab { screen, delta } if screen == "settings" => {
                let d = *delta;
                with_settings(world, |st| {
                    st.tab = (st.tab as i32 + d).rem_euclid(SETTINGS_TABS.len() as i32) as usize;
                    st.rebinding = None;
                });
            }
            UiEvent::ScreenClosed { screen } if screen == "pause" => resume_time(world),
            _ => {}
        }
    }
    // Waiting for a key to rebind.
    let waiting = {
        let mut q = world.query::<&SettingsScreen>();
        q.iter(world).find_map(|s| s.rebinding.clone())
    };
    if let Some(action) = waiting {
        let key = world
            .get_resource::<ButtonInput<KeyCode>>()
            .and_then(|k| k.get_just_pressed().next().copied());
        if let Some(key) = key {
            if key != KeyCode::Escape
                && let Some(mut map) = world.get_resource_mut::<InputMap>()
                && let Some(b) = map.actions.get_mut(&action)
            {
                b.buttons.retain(|x| !matches!(x, Binding::Key(_)));
                b.buttons.insert(0, Binding::Key(key));
                toast::notify(
                    world,
                    ToastKind::Good,
                    "ui_check",
                    "",
                    &format!("{} → [b]{}[/b]", pretty_action(&action), key_label(key)),
                    2.0,
                );
            }
            with_settings(world, |st| st.rebinding = None);
        }
    }
}

fn with_settings(world: &mut World, f: impl FnOnce(&mut SettingsScreen)) {
    let mut q = world.query::<(Entity, &SettingsScreen, &Screen)>();
    let Some((root, mut st)) = q
        .iter(world)
        .filter(|(_, _, s)| !s.closing)
        .map(|(e, st, _)| (e, st.clone()))
        .next()
    else {
        return;
    };
    f(&mut st);
    refresh_settings(world, root, &st);
}

fn resume_time(world: &mut World) {
    let scale = world
        .get_resource_mut::<PauseState>()
        .and_then(|mut s| s.resume_scale.take());
    if let Some(scale) = scale
        && let Some(mut t) = world.get_resource_mut::<Time>()
    {
        t.set_base_scale(scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_labels_are_short() {
        assert_eq!(key_label(KeyCode::Digit3), "3");
        assert_eq!(key_label(KeyCode::ShiftLeft), "Shift");
        assert_eq!(key_label(KeyCode::E), "E");
        assert_eq!(pretty_action("open_inventory"), "Open inventory");
    }
}
