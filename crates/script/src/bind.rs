//! The engine's `module.verb` functions as Luau globals, plus `api`, `print` and the input
//! fast path.
//!
//! Every registered API function is reachable as `module.verb(args)` (`fire.ignite { at = p }`,
//! `physics.proving_ground.replay { station = "jenga" }`) and as `api("fire.ignite", args)` for
//! names built at run time. Arguments are converted to JSON (`convert`), the result comes back
//! as Lua values, and an error from the engine raises a Lua error with its message (catch it
//! with `pcall`). Functions are looked up on first use and cached; a misspelt name fails with
//! the registry's "did you mean" message.

use std::collections::BTreeSet;

use bevy_ecs::prelude::*;
use engine_core::registry::{ApiFn, ApiRegistry, unknown_name_error};
use engine_input::ActionState;
use mlua::{Function, Lua, MultiValue, Table, Value, Variadic};

use crate::access::{push_log, read_world_as, with_world_as};
use crate::convert::{to_json, to_lua};

/// Registry name of the hidden cache of resolved functions and sub-namespaces.
const CACHE: &str = "engine_script.api_cache";

/// The API's names, captured when the VM is made (modules register theirs while plugins build,
/// before the first frame).
#[derive(Clone, Default)]
struct Names(std::sync::Arc<BTreeSet<String>>);

fn call_entry(lua: &Lua, name: &str, func: &ApiFn, args: Variadic<Value>) -> mlua::Result<Value> {
    let json = match args.first() {
        None | Some(Value::Nil) => serde_json::json!({}),
        Some(v) => to_json(v)?,
    };
    let out = with_world_as(lua, name, |w| {
        func(w, json).map_err(|e| mlua::Error::runtime(format!("{name}: {e}")))
    })?;
    to_lua(lua, &out)
}

fn make_function(lua: &Lua, name: &str, func: ApiFn) -> mlua::Result<Function> {
    let name = name.to_string();
    lua.create_function(move |lua, args: Variadic<Value>| call_entry(lua, &name, &func, args))
}

/// A namespace table (`fire`, `physics.proving_ground`) whose missing keys resolve against the
/// registry.
fn namespace(lua: &Lua, prefix: &str, names: &Names, table: Option<Table>) -> mlua::Result<Table> {
    let t = match table {
        Some(t) => t,
        None => lua.create_table()?,
    };
    let meta = lua.create_table()?;
    let prefix_owned = prefix.to_string();
    let names = names.clone();
    meta.set(
        "__index",
        lua.create_function(move |lua, (_, key): (Table, String)| {
            let full = format!("{prefix_owned}.{key}");
            let cache: Table = lua.named_registry_value(CACHE)?;
            if let Value::Table(_) | Value::Function(_) = cache.raw_get::<Value>(full.as_str())? {
                return cache.raw_get::<Value>(full.as_str());
            }
            if names.0.contains(&full) {
                let func = read_world_as(lua, "api lookup", |w| {
                    w.resource::<ApiRegistry>()
                        .get(&full)
                        .map(|e| e.func.clone())
                        .ok_or_else(|| mlua::Error::runtime(format!("{full} is gone")))
                })?;
                let f = make_function(lua, &full, func)?;
                cache.raw_set(full.as_str(), f.clone())?;
                return Ok(Value::Function(f));
            }
            let sub = format!("{full}.");
            if names.0.iter().any(|n| n.starts_with(&sub)) {
                let t = namespace(lua, &full, &names, None)?;
                cache.raw_set(full.as_str(), t.clone())?;
                return Ok(Value::Table(t));
            }
            let siblings: Vec<&str> = names
                .0
                .iter()
                .filter(|n| n.starts_with(&format!("{prefix_owned}.")))
                .map(|s| s.as_str())
                .collect();
            Err(mlua::Error::runtime(unknown_name_error(
                "api function",
                &full,
                siblings.into_iter(),
            )))
        })?,
    )?;
    t.set_metatable(Some(meta))?;
    Ok(t)
}

/// Adds a global per API namespace, `api(name, args)`, `print`/`warn` into the script log, and
/// `input.pressed`/`just_pressed`/`just_released`/`value`/`stick`.
pub fn install(lua: &Lua, api: &ApiRegistry) -> mlua::Result<()> {
    lua.set_named_registry_value(CACHE, lua.create_table()?)?;
    let names = Names(std::sync::Arc::new(
        api.list().map(|e| e.name.clone()).collect(),
    ));
    let g = lua.globals();

    // Fast functions that live beside a namespace's API functions.
    let input = lua.create_table()?;
    input.set(
        "pressed",
        lua.create_function(|lua, action: String| {
            read_world_as(lua, "input.pressed", |w| {
                Ok(actions(w).is_some_and(|a| a.pressed(&action)))
            })
        })?,
    )?;
    input.set(
        "just_pressed",
        lua.create_function(|lua, action: String| {
            read_world_as(lua, "input.just_pressed", |w| {
                Ok(actions(w).is_some_and(|a| a.just_pressed(&action)))
            })
        })?,
    )?;
    input.set(
        "just_released",
        lua.create_function(|lua, action: String| {
            read_world_as(lua, "input.just_released", |w| {
                Ok(actions(w).is_some_and(|a| a.just_released(&action)))
            })
        })?,
    )?;
    input.set(
        "value",
        lua.create_function(|lua, action: String| {
            read_world_as(lua, "input.value", |w| {
                Ok(actions(w).map(|a| a.value(&action)).unwrap_or(0.0))
            })
        })?,
    )?;
    input.set(
        "stick",
        lua.create_function(|lua, action: String| {
            read_world_as(lua, "input.stick", |w| {
                let v = actions(w).map(|a| a.axis(&action)).unwrap_or_default();
                Ok((v.x, v.y))
            })
        })?,
    )?;
    let mut extra = vec![("input".to_string(), input)];

    // Namespaces: the first part of every name. A Luau library with the same name (`debug`)
    // keeps its functions and gains the engine's.
    let tops: BTreeSet<String> = names
        .0
        .iter()
        .filter_map(|n| n.split_once('.').map(|(top, _)| top.to_string()))
        .collect();
    for top in tops {
        let base = match extra.iter().position(|(n, _)| *n == top) {
            Some(i) => Some(extra.remove(i).1),
            None => match g.raw_get::<Value>(top.as_str())? {
                Value::Nil => None,
                Value::Table(lib) => {
                    let copy = lua.create_table()?;
                    for pair in lib.pairs::<Value, Value>() {
                        let (k, v) = pair?;
                        copy.raw_set(k, v)?;
                    }
                    Some(copy)
                }
                // A global that isn't a table: its functions stay reachable through `api`.
                _ => continue,
            },
        };
        let t = namespace(lua, &top, &names, base)?;
        g.raw_set(top.as_str(), t)?;
    }
    for (name, t) in extra {
        g.raw_set(name.as_str(), t)?;
    }

    g.raw_set(
        "api",
        lua.create_function(|lua, (name, args): (String, Variadic<Value>)| {
            let func = read_world_as(lua, "api lookup", |w| {
                let reg = w.resource::<ApiRegistry>();
                reg.get(&name).map(|e| e.func.clone()).ok_or_else(|| {
                    mlua::Error::runtime(unknown_name_error(
                        "api function",
                        &name,
                        reg.list().map(|e| e.name.as_str()),
                    ))
                })
            })?;
            call_entry(lua, &name, &func, args)
        })?,
    )?;

    let print = |prefix: &'static str| {
        move |lua: &Lua, args: MultiValue| {
            let mut line = String::from(prefix);
            for (i, v) in args.iter().enumerate() {
                if i > 0 {
                    line.push('\t');
                }
                line.push_str(&display(v));
            }
            if let Some(at) = user_caller(lua) {
                line = format!("{at}: {line}");
            }
            push_log(lua, line);
            Ok(())
        }
    };
    g.raw_set("print", lua.create_function(print(""))?)?;
    g.raw_set("warn", lua.create_function(print("warning: "))?)?;
    Ok(())
}

fn actions(world: &World) -> Option<&ActionState> {
    world.get_resource::<ActionState>()
}

/// `file:line` of the Luau code that called the running Rust function.
pub(crate) fn caller(lua: &Lua) -> Option<String> {
    lua.inspect_stack(1, |d| {
        let src = d.source();
        let line = d.current_line()?;
        let file = src.short_src?;
        Some(format!("{file}:{line}"))
    })
    .flatten()
}

/// Where the nearest script that isn't the engine's own Luau called from: a conversation's
/// line prints at its `.yarn` line rather than inside `engine/story.luau`.
fn user_caller(lua: &Lua) -> Option<String> {
    let mut first = None;
    for level in 1..16 {
        let Some((file, line)) = lua
            .inspect_stack(level, |d| {
                Some((d.source().short_src?.to_string(), d.current_line()?))
            })
            .flatten()
        else {
            break;
        };
        let at = format!("{file}:{line}");
        if !file.starts_with("engine/") {
            return Some(at);
        }
        first.get_or_insert(at);
    }
    first
}

/// A value as `print` shows it: tables as JSON where they convert, else their type.
pub(crate) fn display(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", *n as i64)
            } else {
                format!("{n}")
            }
        }
        Value::String(s) => s.to_string_lossy(),
        Value::Table(t) if crate::math::kind(t).is_some() => {
            v.to_string().unwrap_or_else(|_| "table".into())
        }
        Value::Table(_) => match to_json(v) {
            Ok(j) => j.to_string(),
            Err(_) => "table".into(),
        },
        Value::UserData(_) => v.to_string().unwrap_or_else(|_| "userdata".into()),
        other => other.type_name().to_string(),
    }
}
