//! The `script.*` API functions (AI bridge, console, editor, tour stops).

use std::path::Path;

use bevy_ecs::prelude::*;
use engine_core::App;
use engine_core::registry::resolve_entity;
use serde_json::{Value, json};

use crate::access::{Outgoing, shared};
use crate::runtime::normalize;
use crate::{Script, scripts_root, with_runtime};

/// A path inside the scripts folder, refusing ones that climb out of it.
fn safe_path(raw: &str) -> Result<String, String> {
    let path = normalize(raw);
    if path.is_empty() || path.split('/').any(|p| p == ".." || p.is_empty()) {
        return Err(format!(
            "'{raw}' is not a path inside the scripts folder (no '..', e.g. \"showcase/lamp.luau\")"
        ));
    }
    if !path.ends_with(".luau") && !path.ends_with(".lua") {
        return Err(format!("'{raw}' is not a .luau file"));
    }
    Ok(path)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .or_else(|| args.as_str())
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "script.list",
        "Loaded script files (version, error, users) and running script instances (entity, \
         path, ms per frame, errors, tasks).",
        "{}",
        |world, _| with_runtime(world, |world, rt, _| rt.describe(world)),
    )
    .register_api(
        "script.errors",
        "Recent script errors with file, line and traceback, oldest first. \"clear\": true \
         empties the list.",
        r#"{"clear": false}"#,
        |world, args| {
            with_runtime(world, |_, rt, _| {
                let errors = rt.errors();
                if args["clear"].as_bool() == Some(true) {
                    rt.clear_errors();
                }
                json!(errors)
            })
        },
    )
    .register_api(
        "script.log",
        "Recent print and warn output of the scripts, oldest first.",
        "{}",
        |world, _| with_runtime(world, |_, rt, _| json!(rt.log())),
    )
    .register_api(
        "script.eval",
        "Run Luau in the live game, as the console: every module.verb, world.find, vec3 and the \
         task library are there. Returns the value (a bare expression returns its value).",
        r#"{"code": "return world.find('Courtyard director').position"}"#,
        |world, args| {
            let code = str_arg(&args, "code")
                .ok_or("missing 'code': Luau source, e.g. \"return 1 + 1\"")?
                .to_string();
            with_runtime(world, |world, rt, settings| rt.eval(world, &code, settings))?
        },
    )
    .register_api(
        "script.reload",
        "Reload a script file now (\"path\"), or every loaded file when no path is given.",
        r#"{"path": "showcase/courtyard.luau"}"#,
        |world, args| {
            with_runtime(world, |world, rt, settings| {
                let paths: Vec<String> = match str_arg(&args, "path") {
                    Some(p) => vec![normalize(p)],
                    None => Vec::new(),
                };
                json!({"reloaded": rt.reload_all(world, paths, settings)})
            })
        },
    )
    .register_api(
        "script.restart",
        "Restart the script on an entity from scratch: a fresh self, start() again.",
        r#"{"entity": {"name": "Courtyard director"}}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").unwrap_or(&args))?;
            if world.get::<Script>(e).is_none() {
                return Err(format!("entity {e} has no Script"));
            }
            with_runtime(world, |world, rt, settings| rt.restart(world, e, settings))?;
            Ok(json!({"restarted": e.to_bits()}))
        },
    )
    .register_api(
        "script.send",
        "Send a message to an entity's script: it calls on_message(event, data), or the \
         script's method named after the event. Delivered on the next frame.",
        r#"{"entity": {"name": "Courtyard director"}, "event": "ritual", "data": {}}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").unwrap_or(&Value::Null))?;
            let event = args["event"]
                .as_str()
                .ok_or("missing 'event': the message name, e.g. \"open\"")?
                .to_string();
            let data = args.get("data").cloned().unwrap_or(Value::Null);
            with_runtime(world, |_, rt, _| {
                shared(&rt.lua).outbox.push(Outgoing { to: e, event, data });
            })?;
            Ok(json!({"queued": true}))
        },
    )
    .register_api(
        "script.source",
        "A script file's text (\"path\"), from disk, and the version the runtime has loaded.",
        r#"{"path": "showcase/courtyard.luau"}"#,
        |world, args| {
            let path = safe_path(str_arg(&args, "path").ok_or("missing 'path'")?)?;
            let full = scripts_root(world).join(&path);
            let source = std::fs::read_to_string(&full)
                .map_err(|e| format!("can't read {}: {e}", full.display()))?;
            let version = world
                .get_resource::<crate::ScriptRuntime>()
                .and_then(|rt| rt.file_source(&path))
                .map(|(_, v)| v);
            Ok(json!({"path": path, "source": source, "loaded_version": version}))
        },
    )
    .register_api(
        "script.write",
        "Write a script file under the scripts folder (creating folders). A loaded script \
         reloads within a moment, like an edit in a text editor.",
        r#"{"path": "showcase/hello.luau", "source": "local S = {}\nfunction S:start() print('hi') end\nreturn S"}"#,
        |world, args| {
            let path = safe_path(str_arg(&args, "path").ok_or("missing 'path'")?)?;
            let source = args["source"]
                .as_str()
                .ok_or("missing 'source': the file's new text")?;
            let full = scripts_root(world).join(&path);
            write_file(&full, source)?;
            Ok(json!({"path": path, "bytes": source.len(), "file": full.display().to_string()}))
        },
    )
    .register_api(
        "script.edit",
        "Edit a script file in place: replace the text \"find\" with \"replace\" (the first \
         match, or every one with \"all\": true). A loaded script reloads within a moment. \
         \"missing_ok\": true leaves a file without \"find\" as it is instead of failing.",
        r#"{"path": "showcase/courtyard.luau", "find": "lanterns = 12", "replace": "lanterns = 16", "all": false}"#,
        |world, args| {
            let path = safe_path(str_arg(&args, "path").ok_or("missing 'path'")?)?;
            let find = args["find"]
                .as_str()
                .filter(|f| !f.is_empty())
                .ok_or("missing 'find': the text to replace")?;
            let replace = args["replace"]
                .as_str()
                .ok_or("missing 'replace': the new text")?;
            let full = scripts_root(world).join(&path);
            let text = std::fs::read_to_string(&full)
                .map_err(|e| format!("can't read {}: {e}", full.display()))?;
            let count = text.matches(find).count();
            if count == 0 {
                if args["missing_ok"].as_bool() == Some(true) {
                    return Ok(json!({"path": path, "replaced": 0}));
                }
                return Err(format!("{path} has no \"{find}\""));
            }
            let all = args["all"].as_bool() == Some(true);
            let edited = if all {
                text.replace(find, replace)
            } else {
                text.replacen(find, replace, 1)
            };
            write_file(&full, &edited)?;
            Ok(json!({"path": path, "replaced": if all { count } else { 1 }}))
        },
    )
    .register_api(
        "script.debug",
        "Start the script debugger on a localhost port (\"port\", default 4711; 0 picks a \
         free one) for VS Code or another DAP client to attach to. Returns the port, whether a \
         client is attached, and every breakpoint and logpoint.",
        r#"{"port": 4711}"#,
        |world, args| {
            with_runtime(world, |_, rt, _| {
                if args.is_object() || args.is_u64() {
                    let port = args["port"].as_u64().or(args.as_u64()).unwrap_or(4711);
                    let port = u16::try_from(port).map_err(|_| format!("bad port {port}"))?;
                    rt.debugger
                        .listen(port)
                        .map_err(|e| format!("can't listen on port {port}: {e}"))?;
                }
                Ok(rt.debugger.status())
            })?
        },
    )
    .register_api(
        "script.logpoint",
        "Print values from a script line each time it runs, without stopping: \"message\" \
         fills in {expressions} with the line's locals (\"glow = {glow}\"). Code boards show \
         the newest beside the line; \"log\": true also writes script.log. \"clear\": true \
         removes the logpoints (of \"path\", or all).",
        r#"{"path": "showcase/courtyard.luau", "line": 40, "message": "glow = {glow}", "log": false}"#,
        |world, args| {
            with_runtime(world, |_, rt, _| {
                if args["clear"].as_bool() == Some(true) {
                    let path = str_arg(&args, "path").map(normalize);
                    let removed = rt.debugger.clear_logpoints(&rt.lua, path.as_deref());
                    return Ok(json!({"removed": removed}));
                }
                let path = safe_path(str_arg(&args, "path").ok_or("missing 'path'")?)?;
                let line = args["line"]
                    .as_u64()
                    .filter(|l| *l > 0)
                    .ok_or("missing 'line': the 1-based line number")?;
                let message = args["message"]
                    .as_str()
                    .ok_or("missing 'message', e.g. \"glow = {glow}\"")?
                    .to_string();
                let log = args["log"].as_bool() == Some(true);
                Ok(rt
                    .debugger
                    .add_logpoint(&rt.lua, &path, line as u32, message, log))
            })?
        },
    )
    .register_api(
        "script.profile",
        "Profile the scripts while the game runs: \"seconds\" starts a recording (0: until \
         stopped; \"hz\": samples a second, default 1000), \"stop\": true ends it, \"reset\": \
         true drops it. Returns the script time per function (self and total; engine calls \
         like e:set are their own entries) and per line, hottest first (\"top\" of each); \
         \"folded\": true adds every stack in the folded format flame graph tools read.",
        r#"{"seconds": 5, "hz": 1000}"#,
        |world, args| {
            with_runtime(world, |_, rt, _| {
                let flag = |key: &str| args[key].as_bool() == Some(true);
                if flag("reset") {
                    rt.profile_reset();
                    return Ok(json!({"reset": true}));
                }
                if flag("stop") {
                    rt.profile_stop();
                } else if let Some(seconds) = args["seconds"].as_f64() {
                    let hz = args["hz"].as_u64().unwrap_or(1000).min(20_000) as u32;
                    rt.profile_start(hz, seconds.max(0.0), false);
                }
                let top = args["top"].as_u64().unwrap_or(15).clamp(1, 200) as usize;
                Ok(crate::profiler::report(
                    &rt.luas(),
                    rt.profile_recording(),
                    top,
                    flag("folded"),
                ))
            })?
        },
    )
    .register_api(
        "script.marks",
        "What a code view shows beside a file's lines (\"path\"): whether it is loaded and its \
         version, instances and ms, why its newest text didn't load, its last runtime error, \
         logpoint values and profiler heat (percent) by line.",
        r#"{"path": "showcase/courtyard.luau"}"#,
        |world, args| {
            let path = safe_path(str_arg(&args, "path").ok_or("missing 'path'")?)?;
            serde_json::to_value(crate::editing::file_marks(world, &path))
                .map_err(|e| e.to_string())
        },
    )
    .register_api(
        "script.check",
        "Compile Luau text (\"source\") without running it: ok, or the line and message of \
         its first syntax error.",
        r#"{"source": "local S = {}\nreturn S"}"#,
        |_, args| {
            let source = str_arg(&args, "source").ok_or("missing 'source'")?;
            Ok(match crate::editing::check(source) {
                None => json!({"ok": true}),
                Some((line, message)) => json!({"ok": false, "line": line, "message": message}),
            })
        },
    )
    .register_api(
        "script.types",
        "Write engine.d.luau (Luau type definitions of every engine function, vec3, quat, \
         entities and tasks) into the scripts folder for luau-lsp autocompletion; returns its \
         path. \"path\" overrides where.",
        r#"{"path": "engine.d.luau"}"#,
        |world, args| {
            let text = crate::types::definitions(world);
            let rel = str_arg(&args, "path").unwrap_or("engine.d.luau");
            if rel.split('/').any(|p| p == "..") {
                return Err("the path must stay inside the scripts folder".into());
            }
            let full = scripts_root(world).join(rel);
            write_file(&full, &text)?;
            Ok(json!({"file": full.display().to_string(), "bytes": text.len()}))
        },
    );
}

fn write_file(full: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    // Written beside and renamed over, so the poller never reads half a file.
    let tmp = full.with_extension("luau.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, full).map_err(|e| format!("can't write {}: {e}", full.display()))
}
