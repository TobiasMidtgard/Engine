//! Renders every UI screen offscreen and saves a PNG per screen.
//!
//! `cargo run -p engine_ui --example gallery -- <out_dir> [width] [height] [shot ...]`
//!
//! Shots: hud, wheel, dialogue, dialogue_choices, shop, shop_sell, inventory, inventory_drag,
//! journal, journal_finished, skills, crafting, crafting_busy, map_galaxy, map_system, map_globe, map_surface, pause, confirm, settings, settings_video,
//! settings_controls, theme_cyan (all by default).
//! Needs a GPU or a software Vulkan driver (lavapipe).

use engine_core::math::{DVec3, Vec2};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_input::{InputEvent, MouseButton};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_ui::prelude::*;
use serde_json::json;

#[path = "common/backdrop.rs"]
mod backdrop;

fn scene(app: &mut App, w: u32, h: u32) {
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((RenderPlugin, engine_input::InputPlugin, UiPlugin));
    backdrop::spawn_backdrop(&mut app.world);
    engine_ui::demo::setup_data(&mut app.world);
    // A few bindings for the settings' Controls tab.
    use engine_input::{Binding, GamepadButton, InputMap, KeyCode};
    let mut map = app.world.resource_mut::<InputMap>();
    for (action, key, pad) in [
        ("jump", KeyCode::Space, GamepadButton::South),
        ("sprint", KeyCode::ShiftLeft, GamepadButton::LeftStick),
        ("crouch_slide", KeyCode::ControlLeft, GamepadButton::East),
        ("interact", KeyCode::E, GamepadButton::West),
        ("slow_time", KeyCode::Q, GamepadButton::LeftBumper),
        ("open_inventory", KeyCode::I, GamepadButton::Select),
        ("open_map", KeyCode::M, GamepadButton::North),
        ("pause", KeyCode::Escape, GamepadButton::Start),
    ] {
        if !map.actions.contains_key(action) {
            map.bind(action, Binding::Key(key))
                .bind(action, Binding::Gamepad(pad));
        }
    }
}

fn send(app: &mut App, e: InputEvent) {
    app.world.write_message(e);
}

/// Center of a node by id, physical px.
fn center_of(app: &mut App, id: &str) -> Option<Vec2> {
    let e = engine_ui::input::find_id(&mut app.world, id)?;
    app.world.get::<ComputedNode>(e).map(|c| c.rect.center())
}

fn hover(app: &mut App, id: &str) {
    if let Some(p) = center_of(app, id) {
        send(app, InputEvent::CursorMoved { position: p });
        app.update_n(40);
    } else {
        eprintln!("no node {id}");
    }
}

/// Steps until neither dialogue portrait is mid-blink.
fn eyes_open(app: &mut App) {
    for _ in 0..240 {
        let mut q = app.world.query::<&engine_ui::dialogue::DialogueScreen>();
        if q.iter(&app.world).all(|d| d.blink.iter().all(|b| *b > 0.2)) {
            return;
        }
        app.update();
    }
}

fn api(app: &mut App, name: &str, args: serde_json::Value) {
    if let Err(e) = call_api(&mut app.world, name, args) {
        eprintln!("{name}: {e}");
    }
}

fn show(app: &mut App, screen: &str, extra: serde_json::Value) {
    api(app, "ui.hide_all", json!({"instant": true, "keep": []}));
    app.update_n(2);
    let mut args = json!({"screen": screen});
    if let (Some(a), Some(b)) = (args.as_object_mut(), extra.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
    api(app, "ui.show", args);
    app.update_n(45);
}

fn shot(app: &mut App, dir: &std::path::Path, name: &str) {
    let path = dir.join(format!("{name}.png"));
    match capture_now(&mut app.world, &path) {
        Ok(_) => println!("wrote {}", path.display()),
        Err(e) => eprintln!("{name}: {e}"),
    }
}

fn main() {
    engine_core::log::init("warn");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "screenshots/ui".into()),
    );
    let w: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1920);
    let h: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1080);
    let only: Vec<String> = args.iter().skip(3).cloned().collect();
    let want = |s: &str| only.is_empty() || only.iter().any(|o| o == s);
    std::fs::create_dir_all(&dir).expect("output dir");

    let mut app = App::new();
    scene(&mut app, w, h);
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    send(
        &mut app,
        InputEvent::CursorMoved {
            position: Vec2::new(-100.0, -100.0),
        },
    );

    if want("hud") {
        engine_ui::hud::spawn_world_markers(
            &mut app.world,
            DVec3::new(0.0, 0.0, -2.0),
            DVec3::new(6.0, 3.0, -4.0),
        );
        api(&mut app, "time.set_speed", json!({"scale": 0.35}));
        show(&mut app, "hud", json!({}));
        api(
            &mut app,
            "ui.notify",
            json!({"kind": "loot", "icon": "crystal_glow", "title": "Glowing crystal", "text": "×2 added to your bag"}),
        );
        app.update_n(30);
        shot(&mut app, &dir, "hud");
        api(&mut app, "time.set_speed", json!({"scale": 1.0}));
    }
    if want("wheel") {
        // The quick-select wheel over the HUD, pointing at the rail rifle.
        show(&mut app, "hud", json!({}));
        api(&mut app, "ui.show", json!({"screen": "wheel"}));
        app.update_n(30);
        if let Some(c) = center_of(&mut app, "wheel_canvas") {
            let a = std::f32::consts::TAU / 8.0;
            let p = c + Vec2::new(a.sin(), -a.cos()) * 240.0;
            send(&mut app, InputEvent::CursorMoved { position: p });
        }
        app.update_n(40);
        shot(&mut app, &dir, "wheel");
    }
    if want("dialogue") {
        show(&mut app, "dialogue", json!({"tree": "merchant_intro"}));
        app.update_n(200);
        eyes_open(&mut app);
        shot(&mut app, &dir, "dialogue");
    }
    if want("dialogue_choices") {
        show(
            &mut app,
            "dialogue",
            json!({"tree": "merchant_intro", "node": "offer"}),
        );
        app.update_n(240);
        hover(&mut app, "choice:1");
        eyes_open(&mut app);
        shot(&mut app, &dir, "dialogue_choices");
    }
    if want("shop") {
        show(&mut app, "shop", json!({"vendor": "Merchant"}));
        hover(&mut app, "shop_item:0");
        shot(&mut app, &dir, "shop");
    }
    if want("shop_sell") {
        show(
            &mut app,
            "shop",
            json!({"vendor": "Merchant", "mode": "sell"}),
        );
        hover(&mut app, "shop_item:1");
        shot(&mut app, &dir, "shop_sell");
    }
    if want("inventory") {
        show(&mut app, "inventory", json!({}));
        hover(&mut app, "inv_slot:0");
        app.update_n(30);
        shot(&mut app, &dir, "inventory");
    }
    if want("inventory_drag") {
        show(&mut app, "inventory", json!({}));
        if let (Some(a), Some(b)) = (
            center_of(&mut app, "inv_slot:4"),
            center_of(&mut app, "inv_slot:30"),
        ) {
            send(&mut app, InputEvent::CursorMoved { position: a });
            app.update_n(2);
            send(
                &mut app,
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    pressed: true,
                },
            );
            app.update_n(2);
            for i in 1..=20 {
                let t = i as f32 / 20.0;
                send(
                    &mut app,
                    InputEvent::CursorMoved {
                        position: a.lerp(b, t),
                    },
                );
                app.update_n(1);
            }
            app.update_n(20);
            shot(&mut app, &dir, "inventory_drag");
            send(
                &mut app,
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    pressed: false,
                },
            );
            app.update_n(5);
        }
    }
    if want("journal") {
        show(&mut app, "journal", json!({"select": "silent_relay"}));
        app.update_n(30);
        shot(&mut app, &dir, "journal");
    }
    if want("journal_finished") {
        show(
            &mut app,
            "journal",
            json!({"tab": "finished", "select": "landfall"}),
        );
        app.update_n(30);
        shot(&mut app, &dir, "journal_finished");
    }
    if want("skills") {
        show(&mut app, "skills", json!({"select": "c_cleave"}));
        app.update_n(30);
        shot(&mut app, &dir, "skills");
    }
    if want("crafting") {
        show(&mut app, "crafting", json!({"select": "r_elixir"}));
        app.update_n(30);
        shot(&mut app, &dir, "crafting");
    }
    if want("crafting_busy") {
        // Halfway through three Glowcap brews at the alchemy bench.
        show(&mut app, "crafting", json!({"select": "r_potion_mana"}));
        app.update_n(5);
        for _ in 0..2 {
            let _ = call_api(&mut app.world, "ui.click", json!({"id": "craft_more"}));
            app.update_n(2);
        }
        let _ = call_api(&mut app.world, "ui.click", json!({"id": "craft_go"}));
        app.update_n(80);
        shot(&mut app, &dir, "crafting_busy");
    }
    for (name, zoom, select) in [
        (
            "map_galaxy",
            3.6,
            json!({"type": "system", "id": "veyra_system"}),
        ),
        ("map_system", 2.72, json!({"type": "planet", "id": "veyra"})),
        (
            "map_globe",
            1.4,
            json!({"type": "poi", "id": "relay_tower"}),
        ),
        (
            "map_surface",
            0.5,
            json!({"type": "poi", "id": "hollow_reach"}),
        ),
    ] {
        if want(name) {
            show(&mut app, "map", json!({"zoom": zoom, "select": select}));
            app.update_n(60);
            shot(&mut app, &dir, name);
        }
    }
    if want("pause") {
        show(&mut app, "pause", json!({}));
        hover(&mut app, "pause_map");
        shot(&mut app, &dir, "pause");
        api(
            &mut app,
            "ui.hide_all",
            json!({"instant": true, "keep": []}),
        );
        app.update_n(2);
    }
    if want("confirm") {
        show(&mut app, "pause", json!({}));
        api(
            &mut app,
            "ui.show",
            json!({"screen": "confirm", "title": "Quit to title?", "text": "Progress since your last save will be lost.", "confirm": "Quit", "action": "game.quit", "danger": true}),
        );
        app.update_n(40);
        shot(&mut app, &dir, "confirm");
        api(
            &mut app,
            "ui.hide_all",
            json!({"instant": true, "keep": []}),
        );
        app.update_n(2);
    }
    for (name, tab, hover_id) in [
        ("settings", "interface", "set_scale"),
        ("settings_video", "video", "set_bloom"),
        ("settings_controls", "controls", "bind:interact"),
    ] {
        if want(name) {
            show(&mut app, "settings", json!({"tab": tab}));
            hover(&mut app, hover_id);
            shot(&mut app, &dir, name);
        }
    }
    if want("theme_cyan") {
        api(
            &mut app,
            "ui.theme",
            json!({"palette": {"accent": "#5fd0ff", "accent_light": "#a8e8ff", "accent_dark": "#2a7aa0"}}),
        );
        show(&mut app, "inventory", json!({}));
        hover(&mut app, "inv_slot:2");
        shot(&mut app, &dir, "theme_cyan");
    }
}
