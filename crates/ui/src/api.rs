//! The `ui.*` API: everything a script, the console or an AI agent needs to open screens,
//! read what is on screen, press buttons and feed data to bindings. JSON in, JSON out.

use bevy_ecs::prelude::*;
use engine_core::prelude::App;
use serde_json::{Value, json};

use crate::data::UiData;
use crate::el::{self, El};
use crate::icons::IconLibrary;
use crate::input::{self, UiEventLog, UiInputState};
use crate::layout::{UiDrawOrder, UiViewport};
use crate::node::*;
use crate::screens::{self, ScreenRegistry};
use crate::theme::{Palette, Theme};

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn need_str<'a>(args: &'a Value, key: &str, example: &str) -> Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("Pass \"{key}\", e.g. {example}."))
}

fn node_by_id(world: &mut World, id: &str) -> Result<Entity, String> {
    input::find_id(world, id).ok_or_else(|| {
        format!("No UI node has the id \"{id}\". `ui.tree` lists the nodes on screen and their ids.")
    })
}

/// A JSON description of a node and its children (what an AI agent "sees").
pub fn describe_node(world: &World, e: Entity, depth: usize) -> Value {
    let vp = world.get_resource::<UiViewport>().copied().unwrap_or_default();
    let mut v = serde_json::Map::new();
    if let Some(id) = world.get::<UiId>(e) {
        v.insert("id".into(), json!(id.0));
    }
    if let Some(n) = world.get::<Node>(e)
        && !n.classes.is_empty()
    {
        v.insert("class".into(), json!(n.classes.join(" ")));
    }
    if let Some(s) = world.get::<Screen>(e) {
        v.insert("screen".into(), json!(s.key));
    }
    if let Some(t) = world.get::<Text>(e) {
        let data = world.get_resource::<UiData>();
        let text = match data {
            Some(d) if crate::data::has_bindings(&t.text) => {
                crate::data::resolve(&t.text, d, Some(world))
            }
            _ => t.text.clone(),
        };
        v.insert("text".into(), json!(crate::text::strip_markup(&text)));
    }
    if let Some(i) = world.get::<Icon>(e) {
        v.insert("icon".into(), json!(i.name));
    }
    if let Some(a) = world.get::<OnClick>(e) {
        v.insert("on_click".into(), json!(a.0));
    }
    if let Some(d) = world.get::<Draggable>(e) {
        v.insert("drag".into(), json!(d.payload));
    }
    if let Some(d) = world.get::<DropTarget>(e) {
        v.insert("drop_slot".into(), json!(d.slot));
    }
    if let Some(t) = world.get::<Tooltip>(e) {
        v.insert("tooltip".into(), json!(format!("{} {}", t.title, t.text).trim()));
    }
    if let Some(i) = world.get::<Interaction>(e) {
        for (k, on) in [
            ("hovered", i.hovered),
            ("focused", i.focused),
            ("selected", i.selected),
            ("disabled", i.disabled),
        ] {
            if on {
                v.insert(k.into(), json!(true));
            }
        }
    }
    if let Some(c) = world.get::<ComputedNode>(e) {
        let s = vp.scale.max(1e-3);
        v.insert(
            "rect".into(),
            json!([
                (c.rect.x / s).round(),
                (c.rect.y / s).round(),
                (c.rect.w / s).round(),
                (c.rect.h / s).round()
            ]),
        );
        if !c.visible {
            v.insert("visible".into(), json!(false));
        }
    }
    let children: Vec<Entity> = world
        .get::<Children>(e)
        .map(|c| c.iter().filter(|c| world.get::<Node>(*c).is_some()).collect())
        .unwrap_or_default();
    if !children.is_empty() {
        if depth == 0 {
            v.insert("children".into(), json!(children.len()));
        } else {
            v.insert(
                "children".into(),
                Value::Array(
                    children
                        .into_iter()
                        .map(|c| describe_node(world, c, depth - 1))
                        .collect(),
                ),
            );
        }
    }
    Value::Object(v)
}

/// Register the core `ui.*` functions.
pub fn register(app: &mut App) {
    app.register_api(
        "ui.show",
        "Open a screen (dialogue, shop, inventory, map, hud, pause, layout, ...). `ui.screens` lists them with their arguments. `instant` skips the animation.",
        r#"{"screen": "shop", "vendor": "merchant", "instant": false}"#,
        |world, args| {
            let kind = need_str(&args, "screen", r#"{"screen": "inventory"}"#)?.to_string();
            let e = screens::show(world, &kind, &args)?;
            let key = world.get::<Screen>(e).map(|s| s.key.clone()).unwrap_or(kind);
            Ok(json!({"ok": true, "key": key}))
        },
    );
    app.register_api(
        "ui.hide",
        "Close an open screen by key (it fades out; `instant` removes it now).",
        r#"{"screen": "shop", "instant": false}"#,
        |world, args| {
            let key = need_str(&args, "screen", r#"{"screen": "shop"}"#)?;
            let instant = args.get("instant").and_then(Value::as_bool).unwrap_or(false);
            Ok(json!({"closed": screens::hide(world, key, instant)}))
        },
    );
    app.register_api(
        "ui.hide_all",
        "Close every screen except the HUD and the ones in `keep`.",
        r#"{"keep": [], "instant": true}"#,
        |world, args| {
            let keep: Vec<String> = args
                .get("keep")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
            let instant = args.get("instant").and_then(Value::as_bool).unwrap_or(false);
            Ok(json!({"closed": screens::hide_all(world, &keep, instant)}))
        },
    );
    app.register_api(
        "ui.screens",
        "Open screens (top first) and every screen kind ui.show can open, with example arguments.",
        "{}",
        |world, _| {
            let open: Vec<Value> = screens::open_screens(world)
                .into_iter()
                .filter(|(k, ..)| k != "tooltip" && k != "drag")
                .map(|(key, _, layer, modal)| json!({"key": key, "layer": layer, "modal": modal}))
                .collect();
            let kinds: Vec<Value> = world
                .resource::<ScreenRegistry>()
                .kinds
                .iter()
                .map(|(k, v)| {
                    let example: Value =
                        serde_json::from_str(&v.args_example).unwrap_or(Value::Null);
                    json!({"screen": k, "doc": v.doc, "args": example})
                })
                .collect();
            Ok(json!({"open": open, "available": kinds}))
        },
    );
    app.register_api(
        "ui.tree",
        "What is on screen: nodes with id, class, text, icon, on_click, state and rect (logical px). Pass `screen` or `id` to start there, `depth` to limit.",
        r#"{"screen": "shop", "depth": 6}"#,
        |world, args| {
            let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(8) as usize;
            let start = if let Some(id) = str_arg(&args, "id") {
                vec![node_by_id(world, id)?]
            } else if let Some(key) = str_arg(&args, "screen") {
                vec![screens::find_screen(world, key).ok_or_else(|| {
                    format!("The screen \"{key}\" isn't open. `ui.screens` lists the open ones.")
                })?]
            } else {
                world.resource::<UiDrawOrder>().roots.clone()
            };
            let vp = *world.resource::<UiViewport>();
            let nodes: Vec<Value> = start
                .into_iter()
                .map(|e| describe_node(world, e, depth))
                .collect();
            Ok(json!({
                "viewport": [ (vp.width / vp.scale).round(), (vp.height / vp.scale).round() ],
                "nodes": nodes
            }))
        },
    );
    app.register_api(
        "ui.click",
        "Press a button by id, as the player would (sends its action). `alt` is the right-click / X action.",
        r#"{"id": "buy", "alt": false}"#,
        |world, args| {
            let id = need_str(&args, "id", r#"{"id": "buy"}"#)?;
            let e = node_by_id(world, id)?;
            let alt = args.get("alt").and_then(Value::as_bool).unwrap_or(false);
            if world.get::<OnClick>(e).is_none() {
                return Err(format!("The node \"{id}\" isn't clickable (it has no on_click)."));
            }
            if !input::activate(world, e, alt) {
                return Err(format!("The node \"{id}\" is disabled right now."));
            }
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "ui.focus",
        "Move keyboard/gamepad focus to a node by id.",
        r#"{"id": "choice_0"}"#,
        |world, args| {
            let id = need_str(&args, "id", r#"{"id": "choice_0"}"#)?;
            let e = node_by_id(world, id)?;
            let mut st = world.resource_mut::<UiInputState>();
            st.focused = Some(e);
            st.mode = crate::input::NavMode::Directional;
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "ui.set_data",
        "Set values that UI text binds to with {path}: one `path` + `value`, or several in `values`.",
        r#"{"path": "player.gold", "value": 1250}"#,
        |world, args| {
            let mut data = world.resource_mut::<UiData>();
            if let Some(values) = args.get("values").and_then(Value::as_object) {
                for (k, v) in values {
                    data.set(k, v.clone());
                }
                return Ok(json!({"ok": true, "count": values.len()}));
            }
            let path = need_str(&args, "path", r#"{"path": "player.gold", "value": 10}"#)?;
            let value = args.get("value").cloned().unwrap_or(Value::Null);
            data.set(path, value);
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "ui.get_data",
        "Read UI data at a dotted path (all of it without `path`).",
        r#"{"path": "player"}"#,
        |world, args| {
            let data = world.resource::<UiData>();
            Ok(match str_arg(&args, "path") {
                Some(p) => data.get(p).cloned().unwrap_or(Value::Null),
                None => data.root().clone(),
            })
        },
    );
    app.register_api(
        "ui.spawn",
        "Spawn a UI tree from JSON (see El: class, style, text, icon, on_click, tooltip, drag, drop, children). Without `parent` it is a top-level node; ids are returned.",
        r#"{"layout": {"id": "banner", "class": "panel", "style": {"width": 300}, "children": [{"text": "Hello", "class": "title"}]}, "parent": null}"#,
        |world, args| {
            let layout = args.get("layout").ok_or("Pass the tree in \"layout\".")?;
            let el: El = serde_json::from_value(layout.clone())
                .map_err(|e| format!("The layout doesn't match the UI element format: {e}."))?;
            let parent = match str_arg(&args, "parent") {
                Some(p) => Some(node_by_id(world, p)?),
                None => None,
            };
            let spawned = el::spawn(world, parent, &el)?;
            let ids: Vec<&String> = spawned.ids.keys().collect();
            Ok(json!({"ok": true, "ids": ids, "count": el.count()}))
        },
    );
    app.register_api(
        "ui.despawn",
        "Remove a UI node (and its children) by id.",
        r#"{"id": "banner"}"#,
        |world, args| {
            let id = need_str(&args, "id", r#"{"id": "banner"}"#)?;
            let e = node_by_id(world, id)?;
            world.entity_mut(e).despawn();
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "ui.icons",
        "Names of every icon and portrait (use them in `icon` fields).",
        "{}",
        |world, _| Ok(json!(world.resource::<IconLibrary>().names())),
    );
    app.register_api(
        "ui.add_icon",
        "Add or replace an icon from SVG source; single-color icons named ui_* take the text color.",
        r#"{"name": "relic", "svg": "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><circle cx='12' cy='12' r='9' fill='#e8b65a'/></svg>"}"#,
        |world, args| {
            let name = need_str(&args, "name", r#"{"name": "relic", "svg": "<svg ...>"}"#)?;
            let svg = need_str(&args, "svg", r#"{"name": "relic", "svg": "<svg ...>"}"#)?;
            world.resource_mut::<IconLibrary>().add(name, svg)?;
            Ok(json!({"ok": true}))
        },
    );
    app.register_api(
        "ui.theme",
        "Read the theme palette, or change colors: {\"palette\": {\"accent\": \"#5fd0ff\"}} rebuilds every class from it.",
        r##"{"palette": {"accent": "#e8b65a"}}"##,
        |world, args| {
            if let Some(changes) = args.get("palette") {
                let current = serde_json::to_value(&world.resource::<Theme>().palette)
                    .map_err(|e| e.to_string())?;
                let mut merged = current;
                if let (Some(m), Some(c)) = (merged.as_object_mut(), changes.as_object()) {
                    for (k, v) in c {
                        m.insert(k.clone(), v.clone());
                    }
                }
                let palette: Palette = serde_json::from_value(merged)
                    .map_err(|e| format!("The palette doesn't parse: {e}."))?;
                let font_size = world.resource::<Theme>().font_size;
                let mut theme = Theme::from_palette(palette);
                theme.font_size = font_size;
                world.insert_resource(theme);
            }
            serde_json::to_value(&world.resource::<Theme>().palette).map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "ui.events",
        "Recent UI events (clicks, drops, tabs, screens opened/closed) after sequence number `since`.",
        r#"{"since": 0}"#,
        |world, args| {
            let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
            let log = world.resource::<UiEventLog>();
            let events: Vec<Value> = log
                .events
                .iter()
                .filter(|(n, _)| *n >= since)
                .map(|(n, v)| {
                    let mut v = v.clone();
                    v["seq"] = json!(n);
                    v
                })
                .collect();
            Ok(json!({"events": events, "next": log.next}))
        },
    );
}
