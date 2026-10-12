//! `debug.*` API functions: let AI agents, the console and scripts annotate the world.

use bevy_ecs::prelude::*;
use engine_core::App;
use engine_core::registry::{TypeRegistry, unknown_name_error};
use serde_json::{Value, json};

use crate::color::{self, Color};
use crate::draw::{DebugDraw, DepthMode};
use crate::shapes::{DebugShape, ShapeStyle};
use crate::views::DebugViews;

/// Duration used by `debug.draw` when a shape gives none, real seconds. Long enough for an
/// agent to take a screenshot after drawing.
pub const DEFAULT_API_DURATION: f32 = 10.0;

/// One example per shape kind, shown in error messages.
const EXAMPLES: [(&str, &str); 15] = [
    (
        "line",
        r#"{"line": {"a": [0, 1, 0], "b": [5, 1, 0], "color": "red"}}"#,
    ),
    (
        "ray",
        r#"{"ray": {"origin": [0, 1, 0], "dir": [0, 0, -10]}}"#,
    ),
    (
        "arrow",
        r#"{"arrow": {"from": [0, 0, 0], "to": [0, 3, 0], "head": 0.5}}"#,
    ),
    (
        "aabb",
        r#"{"aabb": {"min": [-1, 0, -1], "max": [1, 2, 1]}}"#,
    ),
    (
        "box",
        r#"{"box": {"center": [0, 1, 0], "half_extents": [1, 0.5, 2], "rotation": [0, 0, 0, 1]}}"#,
    ),
    (
        "sphere",
        r#"{"sphere": {"center": [0, 1, 0], "radius": 0.5}}"#,
    ),
    (
        "circle",
        r#"{"circle": {"center": [0, 0.1, 0], "normal": [0, 1, 0], "radius": 3}}"#,
    ),
    ("cross", r#"{"cross": {"at": [2, 0, 2], "size": 0.5}}"#),
    ("point", r#"{"point": {"at": [2, 0, 2]}}"#),
    (
        "axes",
        r#"{"axes": {"position": [0, 0, 0], "rotation": [0, 0, 0, 1], "size": 1}}"#,
    ),
    (
        "capsule",
        r#"{"capsule": {"a": [0, 0.4, 0], "b": [0, 1.4, 0], "radius": 0.4}}"#,
    ),
    (
        "cylinder",
        r#"{"cylinder": {"a": [0, 0, 0], "b": [0, 2, 0], "radius": 0.5}}"#,
    ),
    (
        "grid",
        r#"{"grid": {"center": [0, 0, 0], "normal": [0, 1, 0], "cell": 1, "cells": 20}}"#,
    ),
    // A camera-relative view-projection (16 numbers, column-major): this one is a reverse-Z
    // 60° perspective, 16:9, looking along -Z (`ViewData::view_proj` has this form).
    (
        "frustum",
        r#"{"frustum": {"view_proj": [0.974, 0, 0, 0, 0, 1.732, 0, 0, 0, 0, 0, -1, 0, 0, 0.1, 0], "origin": [0, 2, 0], "max_distance": 50}}"#,
    ),
    (
        "path",
        r#"{"path": {"points": [[0, 0, 0], [1, 0, 0], [1, 0, 1]], "closed": true}}"#,
    ),
];

const ALIASES: [(&str, &str); 4] = [
    ("obb", "box"),
    ("oriented_box", "box"),
    ("polyline", "path"),
    ("point", "cross"),
];

fn example(kind: &str) -> &'static str {
    let kind = ALIASES
        .iter()
        .find(|(a, _)| *a == kind)
        .map_or(kind, |(_, k)| *k);
    EXAMPLES
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(EXAMPLES[0].1, |(_, e)| *e)
}

/// Parse a color: `[r, g, b]` / `[r, g, b, a]` (linear 0..1), a name (`"red"`), or
/// `"#rrggbb"` / `"#rrggbbaa"` (sRGB).
pub fn parse_color(v: &Value) -> Result<Color, String> {
    const HELP: &str = "use [r, g, b] or [r, g, b, a] with linear values 0..1, a name such as \
                        \"red\", or an sRGB hex string such as \"#ff8800\"";
    match v {
        Value::Array(items) if items.len() == 3 || items.len() == 4 => {
            let mut c = [1.0f32; 4];
            for (i, item) in items.iter().enumerate() {
                c[i] = item
                    .as_f64()
                    .ok_or_else(|| format!("color component {i} is not a number; {HELP}"))?
                    as f32;
            }
            Ok(c)
        }
        Value::String(s) if s.starts_with('#') => {
            let hex = &s[1..];
            let valid =
                (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
            if !valid {
                return Err(format!("color \"{s}\" is not a valid hex color; {HELP}"));
            }
            let rgb = u32::from_str_radix(&hex[..6], 16).map_err(|e| e.to_string())?;
            let mut c = color::hex(rgb);
            if hex.len() == 8 {
                c[3] = u8::from_str_radix(&hex[6..], 16).map_err(|e| e.to_string())? as f32 / 255.0;
            }
            Ok(c)
        }
        Value::String(s) => {
            color::named(s).ok_or_else(|| unknown_name_error("color", s, color::NAMES.into_iter()))
        }
        _ => Err(format!("color {v} is not understood; {HELP}")),
    }
}

/// Parse the style fields of a shape body: `color`, `duration`, `depth` / `on_top`.
fn parse_style(body: &Value) -> Result<ShapeStyle, String> {
    let mut style = ShapeStyle {
        duration: DEFAULT_API_DURATION,
        ..Default::default()
    };
    if let Some(c) = body.get("color") {
        style.color = parse_color(c)?;
    }
    match body.get("duration") {
        None | Some(Value::Null) => {}
        Some(d) => {
            let secs = d.as_f64().ok_or(
                "'duration' must be a number of seconds (0 = one frame, -1 = until debug.clear)",
            )?;
            style.duration = if secs < 0.0 {
                DebugDraw::FOREVER
            } else {
                secs as f32
            };
        }
    }
    if let Some(d) = body.get("depth") {
        style.depth = serde_json::from_value::<DepthMode>(d.clone())
            .map_err(|_| format!("'depth' {d} is not understood; use \"tested\" (hidden behind objects) or \"on_top\""))?;
    }
    if body.get("on_top").and_then(Value::as_bool) == Some(true) {
        style.depth = DepthMode::OnTop;
    }
    Ok(style)
}

/// Parse one shape entry such as `{"sphere": {"center": [0, 1, 0], "radius": 1, "color": "red"}}`.
pub fn parse_shape(entry: &Value) -> Result<(DebugShape, ShapeStyle), String> {
    let obj = entry.as_object().filter(|o| o.len() == 1).ok_or_else(|| {
        format!(
            "each shape must be an object with one key naming its kind, for example {}",
            EXAMPLES[5].1
        )
    })?;
    let (kind, body) = obj.iter().next().expect("one key");
    let known =
        DebugShape::KINDS.iter().any(|k| k == kind) || ALIASES.iter().any(|(a, _)| a == kind);
    if !known {
        return Err(unknown_name_error(
            "shape",
            kind,
            DebugShape::KINDS.into_iter(),
        ));
    }
    if !body.is_object() {
        return Err(format!(
            "the body of '{kind}' must be an object, for example {}",
            example(kind)
        ));
    }
    let shape: DebugShape = serde_json::from_value(json!({ kind: body }))
        .map_err(|e| format!("bad '{kind}' shape: {e}. Example: {}", example(kind)))?;
    let style = parse_style(body).map_err(|e| format!("bad '{kind}' shape: {e}"))?;
    Ok((shape, style))
}

/// `debug.draw`: draw every shape in `args.shapes` (all or nothing).
pub fn draw_api(world: &mut World, args: Value) -> Result<Value, String> {
    let list: Vec<Value> = match args {
        Value::Array(items) => items,
        Value::Object(mut map) if map.contains_key("shapes") => match map.remove("shapes") {
            Some(Value::Array(items)) => items,
            _ => return Err("'shapes' must be an array of shapes".into()),
        },
        single @ Value::Object(_) => vec![single],
        _ => {
            return Err(format!(
                "expected {{\"shapes\": [...]}}, for example {{\"shapes\": [{}]}}",
                EXAMPLES[0].1
            ));
        }
    };
    let parsed = list
        .iter()
        .enumerate()
        .map(|(i, v)| parse_shape(v).map_err(|e| format!("shape {i}: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut draw = world.get_resource_or_init::<DebugDraw>();
    if !draw.enabled {
        return Err(
            "debug drawing is switched off; enable it with debug.views {\"enabled\": true}".into(),
        );
    }
    let before = draw.dropped();
    let mut lines = 0;
    for (shape, style) in &parsed {
        lines += draw.styled(shape, *style).line_count();
    }
    let dropped = draw.dropped() - before;
    Ok(json!({
        "shapes": parsed.len(),
        "lines": lines,
        "dropped_lines": dropped,
        "total_lines": draw.line_count(),
    }))
}

/// `debug.clear`: remove every shape, including timed and permanent ones.
pub fn clear_api(world: &mut World, _args: Value) -> Result<Value, String> {
    let mut draw = world.get_resource_or_init::<DebugDraw>();
    let n = draw.line_count();
    draw.clear();
    Ok(json!({ "cleared_lines": n }))
}

/// `debug.views`: return the view toggles, after applying any patch in `args`.
/// `{"all": true}` turns every built-in view on (`false` off).
pub fn views_api(world: &mut World, args: Value) -> Result<Value, String> {
    world.get_resource_or_init::<DebugViews>();
    let mut patch = match args {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map,
        other => {
            return Err(format!(
                "expected an object of DebugViews fields to change, got {other}"
            ));
        }
    };
    if let Some(all) = patch.remove("all") {
        let on = all.as_bool().ok_or("'all' must be true or false")?;
        world.resource_mut::<DebugViews>().set_all(on);
    }
    if !patch.is_empty() {
        let entry = world
            .resource::<TypeRegistry>()
            .resource_or_err("DebugViews")?
            .clone();
        (entry.patch)(world, Value::Object(patch))?;
    }
    // Apply the master switch now rather than at the next frame, so a `debug.draw` right
    // after this call already sees it.
    let enabled = world.resource::<DebugViews>().enabled;
    world.get_resource_or_init::<DebugDraw>().enabled = enabled;
    serde_json::to_value(world.resource::<DebugViews>()).map_err(|e| e.to_string())
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "debug.draw",
        "Draw debug shapes in the world (lines over the scene; positions in world meters). \
         Kinds: line, ray, arrow, aabb, box, sphere, circle, cross, point, axes, capsule, cylinder, \
         grid, frustum, path. Each shape also takes color ([r,g,b,a] linear 0..1, a name, or \"#rrggbb\"), \
         duration (real seconds, default 10; 0 = one frame; -1 = until debug.clear) and depth \
         (\"tested\" or \"on_top\").",
        r##"{"shapes": [{"line": {"a": [0, 1, 0], "b": [5, 1, 0], "color": "red", "duration": 10}}, {"sphere": {"center": [0, 1, 0], "radius": 0.5, "color": [0, 1, 1, 1], "depth": "on_top"}}, {"arrow": {"from": [0, 0, 0], "to": [0, 3, 0], "color": "#ff8800", "duration": -1}}]}"##,
        draw_api,
    )
    .register_api(
        "debug.clear",
        "Remove every debug shape, including timed and permanent ones.",
        "{}",
        clear_api,
    )
    .register_api(
        "debug.views",
        "Get or change the built-in debug views (wind arrows, ground wetness/snow, focus points, \
         gravity, world events) and the master switch. Send fields to change; {\"all\": true} \
         turns every view on. Returns the current settings.",
        r#"{"wind": {"enabled": true, "radius": 24}, "events": {"projectiles": true, "impulses": true}}"#,
        views_api,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::registry::call_api;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(crate::DebugPlugin);
        app
    }

    #[test]
    fn every_example_is_valid_and_draws() {
        let mut app = app();
        // The argument examples `engine.describe` shows to agents.
        let examples: Vec<(String, String)> = app
            .world
            .resource::<engine_core::registry::ApiRegistry>()
            .list()
            .filter(|e| e.name.starts_with("debug."))
            .map(|e| (e.name.clone(), e.args_example.clone()))
            .collect();
        assert_eq!(examples.len(), 3);
        for (name, args) in examples {
            let args: Value = serde_json::from_str(&args).unwrap_or_else(|e| panic!("{name}: {e}"));
            call_api(&mut app.world, &name, args).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // The per-shape examples quoted in error messages.
        for (kind, example) in EXAMPLES {
            let shape: Value =
                serde_json::from_str(example).unwrap_or_else(|e| panic!("{kind}: {e}"));
            let out = call_api(&mut app.world, "debug.draw", json!({ "shapes": [shape] }))
                .unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert!(out["lines"].as_u64().unwrap() > 0, "{kind}: {out}");
        }
    }

    #[test]
    fn colors_parse_in_every_form() {
        assert_eq!(
            parse_color(&json!([1, 0, 0])).unwrap(),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            parse_color(&json!([0, 1, 0, 0.5])).unwrap(),
            [0.0, 1.0, 0.0, 0.5]
        );
        assert_eq!(parse_color(&json!("red")).unwrap(), color::RED);
        let c = parse_color(&json!("#ff000080")).unwrap();
        assert_eq!(c[0], 1.0);
        assert!((c[3] - 0.502).abs() < 1e-3);
        assert!(parse_color(&json!("#ff00")).is_err());
        let err = parse_color(&json!("rde")).unwrap_err();
        assert!(err.contains("Did you mean 'red'"), "{err}");
        assert!(parse_color(&json!([1, 2])).is_err());
    }

    #[test]
    fn draw_api_draws_and_clear_removes() {
        let mut app = app();
        let out = call_api(
            &mut app.world,
            "debug.draw",
            json!({"shapes": [
                {"line": {"a": [0, 1, 0], "b": [5, 1, 0], "color": "red", "duration": 2}},
                {"sphere": {"center": [0, 1, 0], "radius": 0.5, "depth": "on_top"}},
                {"point": {"at": [1, 1, 1], "duration": 0}},
                {"path": {"points": [[0, 0, 0], [1, 0, 0], [1, 0, 1]], "closed": true, "duration": -1}}
            ]}),
        )
        .unwrap();
        assert_eq!(out["shapes"], 4);
        assert_eq!(out["lines"], 1 + 96 + 3 + 3);
        let draw = app.world.resource::<DebugDraw>();
        assert_eq!(draw.lines(DepthMode::OnTop).count(), 96);
        assert_eq!(draw.timed_line_count(), 1 + 96 + 3);

        // Frames advance real time; the 2 s line and 10 s sphere expire, the path stays.
        app.world
            .resource_mut::<engine_core::time::Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.25 });
        app.update_n(12);
        assert_eq!(app.world.resource::<DebugDraw>().line_count(), 96 + 3);
        app.update_n(40);
        assert_eq!(app.world.resource::<DebugDraw>().line_count(), 3);

        let out = call_api(&mut app.world, "debug.clear", json!({})).unwrap();
        assert_eq!(out["cleared_lines"], 3);
        assert!(app.world.resource::<DebugDraw>().is_empty());
    }

    #[test]
    fn draw_api_errors_are_helpful_and_atomic() {
        let mut app = app();
        let err = call_api(
            &mut app.world,
            "debug.draw",
            json!({"shapes": [{"line": {"a": [0, 0, 0], "b": [1, 0, 0]}}, {"sphre": {"center": [0, 0, 0]}}]}),
        )
        .unwrap_err();
        assert!(
            err.contains("shape 1") && err.contains("Did you mean 'sphere'"),
            "{err}"
        );
        assert!(
            app.world.resource::<DebugDraw>().is_empty(),
            "nothing drawn on error"
        );

        let err = call_api(
            &mut app.world,
            "debug.draw",
            json!({"shapes": [{"sphere": {"center": [0, 0, 0]}}]}),
        )
        .unwrap_err();
        assert!(err.contains("radius") && err.contains("Example"), "{err}");

        let err = call_api(
            &mut app.world,
            "debug.draw",
            json!([{"line": {"a": [0, 0, 0], "b": [1, 0, 0], "depth": "sideways"}}]),
        )
        .unwrap_err();
        assert!(err.contains("on_top"), "{err}");

        // A bare shape object also works.
        call_api(
            &mut app.world,
            "debug.draw",
            json!({"cross": {"at": [0, 0, 0]}}),
        )
        .unwrap();
        assert_eq!(app.world.resource::<DebugDraw>().line_count(), 3);
    }

    #[test]
    fn views_api_gets_and_patches() {
        let mut app = app();
        let v = call_api(&mut app.world, "debug.views", json!({})).unwrap();
        assert_eq!(v["wind"]["enabled"], false);
        let v = call_api(
            &mut app.world,
            "debug.views",
            json!({"wind": {"enabled": true, "radius": 8}}),
        )
        .unwrap();
        assert_eq!(v["wind"]["enabled"], true);
        assert_eq!(v["wind"]["radius"], 8.0);
        assert_eq!(v["wind"]["spacing"], 4.0, "other fields kept");
        let v = call_api(&mut app.world, "debug.views", json!({"all": true})).unwrap();
        assert_eq!(v["events"]["noise"], true);
        assert!(app.world.resource::<DebugViews>().any());
        let err = call_api(
            &mut app.world,
            "debug.views",
            json!({"wind": {"radius": "big"}}),
        )
        .unwrap_err();
        assert!(err.contains("DebugViews"), "{err}");

        // The master switch stops drawing right away (no frame in between)...
        call_api(&mut app.world, "debug.views", json!({"enabled": false})).unwrap();
        let err = call_api(
            &mut app.world,
            "debug.draw",
            json!({"point": {"at": [0, 0, 0]}}),
        )
        .unwrap_err();
        assert!(err.contains("enabled"), "{err}");
        assert!(app.world.resource::<DebugDraw>().is_empty());
        // ...and so does switching it back on.
        call_api(&mut app.world, "debug.views", json!({"enabled": true})).unwrap();
        call_api(
            &mut app.world,
            "debug.draw",
            json!({"point": {"at": [0, 0, 0]}}),
        )
        .unwrap();
        assert_eq!(app.world.resource::<DebugDraw>().line_count(), 3);
    }
}
