//! The script debugger for an editor in the same process: breakpoints, the stop's call stack,
//! its variables and watches, and stepping, without a DAP client or a socket.
//!
//! A stop doesn't freeze the program (see [`crate::debugger`]): the script waits at its line,
//! the other scripts wait for it, the world's time stands still, and the editor keeps drawing
//! and answering. Its client and a DAP client (VS Code) can be attached at the same time; each
//! keeps its own breakpoints, and both see every stop.
//!
//! ```ignore
//! engine_script::debug::attach(world);
//! engine_script::debug::set_breakpoints(world, "showcase/courtyard.luau", &[31]);
//! // ... frames later
//! if let Some(stop) = engine_script::debug::stopped(world) {
//!     let locals = engine_script::debug::scopes(world, 0);
//!     engine_script::debug::go(world, Go::StepOver);
//! }
//! ```

use bevy_ecs::prelude::*;
use serde_json::{Value as Json, json};

pub use crate::debugger::{FrameView, Go, StopView};
use std::collections::BTreeMap;

use crate::{ScriptRuntime, ScriptsHeld, ScriptsRunning};

/// A variable, a table entry or a watch's value.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    /// As Luau would print it (strings quoted, tables summed up).
    pub value: String,
    /// Its type ("number", "vec3", "table", ...).
    pub kind: String,
    /// Non-zero for a table: its entries are [`children`] of this.
    pub reference: usize,
}

impl Variable {
    fn from_json(v: &Json) -> Self {
        Self {
            name: v["name"].as_str().unwrap_or_default().to_string(),
            value: v["value"]
                .as_str()
                .or(v["result"].as_str())
                .unwrap_or_default()
                .to_string(),
            kind: v["type"].as_str().unwrap_or_default().to_string(),
            reference: v["variablesReference"].as_u64().unwrap_or(0) as usize,
        }
    }
}

/// One of a file's breakpoints, as set in the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    /// The line it was set on (1-based).
    pub line: u32,
    /// Where it stops: the next line with code, once the file is loaded.
    pub at: Option<u32>,
    pub hits: u64,
}

/// The scripts' runtime, made if there is none yet (none while the editor holds the scripts,
/// or from inside a script).
fn runtime(world: &mut World) -> Option<Mut<'_, ScriptRuntime>> {
    if world.contains_resource::<ScriptsRunning>() || world.contains_resource::<ScriptsHeld>() {
        return None;
    }
    if !crate::ensure_runtime(world) {
        return None;
    }
    world.get_resource_mut::<ScriptRuntime>()
}

/// Attaches the editor's client: breakpoints set with [`set_breakpoints`] stop from now on.
pub fn attach(world: &mut World) {
    if let Some(rt) = runtime(world) {
        rt.debugger.attach_local();
    }
}

/// Detaches it: its breakpoints go, and a stop it alone was answering goes on.
pub fn detach(world: &mut World) {
    if let Some(rt) = runtime(world) {
        rt.debugger.detach_local(&rt.lua);
    }
}

pub fn attached(world: &World) -> bool {
    world
        .get_resource::<ScriptRuntime>()
        .is_some_and(|rt| rt.debugger.local())
}

/// Stops on errors nothing catches, where they were raised, with the stack and its locals.
pub fn set_break_on_errors(world: &mut World, on: bool) {
    if let Some(rt) = runtime(world) {
        rt.debugger.set_break_on_errors(on);
    }
}

/// Replaces `path`'s breakpoints (1-based lines). Returns where each one stops (the next line
/// with code), `None` while the file isn't loaded. Attaches the editor's client.
pub fn set_breakpoints(world: &mut World, path: &str, lines: &[u32]) -> Vec<Option<u32>> {
    let lines: Vec<(u32, Option<String>)> = lines.iter().map(|l| (*l, None)).collect();
    set_breakpoints_with(world, path, &lines)
}

/// As [`set_breakpoints`], each with a condition (a Luau expression in the line's scope).
pub fn set_breakpoints_with(
    world: &mut World,
    path: &str,
    lines: &[(u32, Option<String>)],
) -> Vec<Option<u32>> {
    let path = crate::normalize(path);
    let Some(rt) = runtime(world) else {
        return vec![None; lines.len()];
    };
    rt.debugger.attach_local();
    rt.debugger.set_editor_breakpoints(&rt.lua, &path, lines)
}

/// `path`'s breakpoints set in the editor.
pub fn breakpoints(world: &World, path: &str) -> Vec<Breakpoint> {
    let path = crate::normalize(path);
    world
        .get_resource::<ScriptRuntime>()
        .map(|rt| {
            rt.debugger
                .editor_breakpoints(&path)
                .into_iter()
                .map(|(line, at, hits)| Breakpoint { line, at, hits })
                .collect()
        })
        .unwrap_or_default()
}

/// The stop, while the scripts are stopped.
pub fn stopped(world: &World) -> Option<StopView> {
    world.get_resource::<ScriptRuntime>()?.debugger.stop_view()
}

/// Frame `frame`'s scopes ("Locals", "Upvalues"), each with a reference to expand.
pub fn scopes(world: &World, frame: usize) -> Vec<Variable> {
    let Some(rt) = world.get_resource::<ScriptRuntime>() else {
        return Vec::new();
    };
    let Ok(scopes) = rt.debugger.stop_scopes(frame) else {
        return Vec::new();
    };
    scopes["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Variable {
            name: s["name"].as_str().unwrap_or_default().to_string(),
            value: String::new(),
            kind: "scope".into(),
            reference: s["variablesReference"].as_u64().unwrap_or(0) as usize,
        })
        .collect()
}

/// The variables of a scope, or a table's entries.
pub fn children(world: &World, reference: usize) -> Vec<Variable> {
    let Some(rt) = world.get_resource::<ScriptRuntime>() else {
        return Vec::new();
    };
    rt.debugger
        .stop_variables(reference)
        .map(|v| {
            v["variables"]
                .as_array()
                .into_iter()
                .flatten()
                .map(Variable::from_json)
                .collect()
        })
        .unwrap_or_default()
}

/// Runs a watch (an expression, or statements) in frame `frame` of the stop.
pub fn evaluate(world: &mut World, frame: usize, expr: &str) -> Result<Variable, String> {
    let rt = runtime(world).ok_or("the scripts aren't running")?;
    let mut v = rt.debugger.stop_evaluate(&rt.lua, frame, expr)?;
    v["name"] = json!(expr);
    Ok(Variable::from_json(&v))
}

/// Goes on from the stop (on the next frame). Returns false when there's no stop the editor
/// can answer.
pub fn go(world: &mut World, go: Go) -> bool {
    runtime(world).is_some_and(|rt| rt.debugger.go(go))
}

/// Lets the world's time run again when the scripts are dropped during a stop (the editor's
/// Stop): a stop holds it still while the scripts wait, and lets it go when they go on.
pub fn release_time(world: &mut World) {
    if let Some(mut d) = world.get_resource_mut::<engine_core::prelude::TimeDilation>() {
        d.release(crate::runtime::DEBUG_HOLD);
    }
}

/// Stops at the next line any script runs.
pub fn pause(world: &mut World) {
    if let Some(rt) = runtime(world) {
        rt.debugger.attach_local();
        rt.debugger.pause();
    }
}

/// The stop as it stands in one file: the innermost frame there, and its locals' values beside
/// the lines that declare them (as JetBrains IDEs and VS Code show inline values).
#[derive(Clone, Debug, PartialEq)]
pub struct FileStop {
    /// [`StopView::id`].
    pub id: u64,
    /// The frame's index in [`StopView::frames`].
    pub frame: usize,
    pub line: u32,
    /// The function stopped in.
    pub function: String,
    /// "breakpoint", "step", "pause" or "error".
    pub reason: String,
    /// The error's message, when it stopped on one.
    pub exception: Option<String>,
    /// `name = value` texts by the line (1-based) that declares them.
    pub values: BTreeMap<u32, String>,
}

/// The stop in `path` (whose text is `source`), if a frame of it is there: the innermost one.
pub fn stop_in(world: &World, path: &str, source: &str) -> Option<FileStop> {
    stop_at(world, path, source, None)
}

/// As [`stop_in`], showing frame `frame` when it is one of `path`'s (a caller picked in the
/// call stack).
pub fn stop_at(world: &World, path: &str, source: &str, frame: Option<usize>) -> Option<FileStop> {
    file_stop(
        world.get_resource::<ScriptRuntime>()?,
        &crate::normalize(path),
        source,
        frame,
    )
}

pub(crate) fn file_stop(
    rt: &ScriptRuntime,
    path: &str,
    source: &str,
    prefer: Option<usize>,
) -> Option<FileStop> {
    let stop = rt.debugger.stop_view()?;
    let (index, frame) = prefer
        .and_then(|i| stop.frames.get(i).map(|f| (i, f)))
        .filter(|(_, f)| f.path == path)
        .or_else(|| stop.frames.iter().enumerate().find(|(_, f)| f.path == path))?;
    let locals: Vec<(String, String)> = rt
        .debugger
        .stop_scopes(index)
        .ok()
        .and_then(|s| s["scopes"][0]["variablesReference"].as_u64())
        .and_then(|r| rt.debugger.stop_variables(r as usize).ok())
        .and_then(|v| v["variables"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|v| {
            let value = v["value"].as_str().unwrap_or_default();
            let value = if v["type"] == "number" || v["type"] == "integer" {
                crate::graph::short_value(value)
            } else if value.chars().count() > 26 {
                format!("{}…", value.chars().take(25).collect::<String>())
            } else {
                value.to_string()
            };
            (v["name"].as_str().unwrap_or_default().to_string(), value)
        })
        .collect();
    let lines: Vec<&str> = source.lines().collect();
    let mut values: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (name, value) in locals {
        let first = frame.line.saturating_sub(80).max(1);
        if let Some(at) = (first..=frame.line).rev().find(|l| {
            lines
                .get(*l as usize - 1)
                .is_some_and(|t| declares(t, &name))
        }) {
            values
                .entry(at)
                .or_default()
                .push(format!("{name} = {value}"));
        }
    }
    Some(FileStop {
        id: stop.id,
        frame: index,
        line: frame.line,
        function: frame.name.clone(),
        reason: match stop.exception {
            Some(_) => "error".into(),
            None => stop.reason.clone(),
        },
        exception: stop.exception.clone(),
        values: values
            .into_iter()
            .map(|(l, v)| (l, v.join(" · ")))
            .collect(),
    })
}

/// Whether a line of Luau declares local `name`: `local a, b = ...`, `for i, v in ...`,
/// `function f(a, b)` (and `self` in `function T:m()`).
pub fn declares(text: &str, name: &str) -> bool {
    let t = text.trim_start();
    let words = |part: &str| {
        part.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| w == name)
    };
    if let Some(rest) = t.strip_prefix("local ") {
        let names = rest.split('=').next().unwrap_or(rest);
        // Leave out the types: `local a: number, b: T = ...`.
        return names
            .split(',')
            .any(|n| n.split(':').next().is_some_and(words));
    }
    if let Some(rest) = t.strip_prefix("for ") {
        let names = rest.split(" in ").next().unwrap_or(rest);
        return words(names.split('=').next().unwrap_or(names));
    }
    if let Some(at) = t.find("function") {
        let head = &t[at..];
        if name == "self" && head.split('(').next().is_some_and(|h| h.contains(':')) {
            return true;
        }
        if let (Some(open), Some(close)) = (head.find('('), head.find(')')) {
            return head[open + 1..close.max(open + 1)]
                .split(',')
                .any(|p| p.split(':').next().is_some_and(words));
        }
    }
    false
}

/// `script.break`'s answer: the stop (where, why, the call stack, the top frame's locals).
pub(crate) fn stop_json(rt: &ScriptRuntime) -> Json {
    let Some(stop) = rt.debugger.stop_view() else {
        return json!({"attached": rt.debugger.local(), "stopped": null});
    };
    let locals: Vec<Json> = rt
        .debugger
        .stop_scopes(0)
        .ok()
        .and_then(|s| s["scopes"][0]["variablesReference"].as_u64())
        .and_then(|r| rt.debugger.stop_variables(r as usize).ok())
        .and_then(|v| v["variables"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .map(|v| json!({"name": v["name"], "value": v["value"]}))
        .collect();
    json!({
        "attached": rt.debugger.local(),
        "stopped": {
            "reason": stop.reason,
            "exception": stop.exception,
            "live": stop.live,
            "frames": stop.frames.iter().map(|f| json!({"name": f.name, "path": f.path, "line": f.line})).collect::<Vec<_>>(),
            "locals": locals,
        },
    })
}
