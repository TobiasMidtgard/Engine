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
    if !path.ends_with(".luau") && !path.ends_with(".lua") && !path.ends_with(".graph") {
        return Err(format!("'{raw}' is not a .luau or .graph file"));
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
        "script.test",
        "Run the script tests (*.test.luau: describe / it / expect) in the live game, one at a \
         time as tasks. \"files\" (default: every test file), \"file\" with \"line\" (the test \
         or describe on that line), \"filter\" (titles containing it); \"stop\": true stops a \
         run. Poll script.tests for the results.",
        r#"{"files": ["examples/lamp.test.luau"], "filter": "goes out"}"#,
        |world, args| {
            let stop = args["stop"].as_bool() == Some(true);
            let mut files: Vec<String> = match &args["files"] {
                Value::Array(list) => list
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(safe_path)
                    .collect::<Result<_, _>>()?,
                Value::String(one) => vec![safe_path(one)?],
                _ => Vec::new(),
            };
            let file = match args["file"].as_str() {
                Some(f) => Some(safe_path(f)?),
                None => None,
            };
            if let Some(f) = &file
                && files.is_empty()
            {
                files.push(f.clone());
            }
            let line = args["line"].as_u64().map(|l| l as u32);
            let filter = args["filter"]
                .as_str()
                .filter(|f| !f.trim().is_empty())
                .map(str::to_string);
            with_runtime(world, |world, rt, settings| {
                if stop {
                    rt.stop_tests();
                    return Ok(rt.tests());
                }
                rt.start_tests(world, files, (file, line, filter), settings)
            })?
        },
    )
    .register_api(
        "script.tests",
        "The current or last script test run: state (idle, running, done), passed, failed, \
         skipped, and each test's file, title, line, status, message and the line it failed on.",
        "{}",
        |world, _| with_runtime(world, |_, rt, _| rt.tests()),
    )
    .register_api(
        "script.states",
        "The state machine of the script on an entity (its class's states table): its states \
         and transitions with their lines, the active states (outermost first), seconds in the \
         current one, and the last changes with what caused each.",
        r#"{"entity": {"name": "Gate"}}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").unwrap_or(&args))?;
            crate::states::describe(world, e)
        },
    )
    .register_api(
        "script.state_breaks",
        "Breakpoints on a script's states: the game's editor pauses when a machine of that file \
         enters (or, with exit, leaves) the state. {path, state, exit?, on?} sets or clears one; \
         {} lists them and the ones reached since the last call (taking them).",
        r#"{"path": "examples/gate.luau", "state": "closing", "on": true}"#,
        |world, args| {
            let mut b = world.get_resource_or_insert_with(crate::states::StateBreakpoints::default);
            if let (Some(path), Some(state)) = (str_arg(&args, "path"), str_arg(&args, "state")) {
                let exit = args["exit"].as_bool().unwrap_or(false);
                let on = args["on"].as_bool().unwrap_or(true);
                b.set(&normalize(path), state, exit, on);
            }
            let hits: Vec<Value> = std::mem::take(&mut b.hits)
                .into_iter()
                .map(|h| {
                    json!({"entity": h.entity.to_bits(), "path": h.path, "kind": h.kind,
                           "state": h.state, "why": h.why, "t": h.t})
                })
                .collect();
            let breaks: serde_json::Map<String, Value> = b
                .by_path
                .iter()
                .map(|(p, br)| (p.clone(), json!({"enter": br.enter, "exit": br.exit})))
                .collect();
            Ok(json!({"breaks": breaks, "hits": hits}))
        },
    )
    .register_api(
        "script.gizmos",
        "Script gizmos: each script's draw_gizmos runs while show is on, draw_gizmos_selected on \
         the selected entities (names or ids), also in an editor's edit mode. {} reads them.",
        r#"{"show": true, "selected": ["Turret"]}"#,
        |world, args| {
            let picked = match args["selected"].as_array() {
                Some(list) => Some(
                    list.iter()
                        .map(|v| resolve_entity(world, v))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                None => None,
            };
            let mut g = world.get_resource_or_insert_with(crate::ScriptGizmos::default);
            if let Some(show) = args["show"].as_bool() {
                g.show = show;
            }
            if let Some(picked) = picked {
                g.selected = picked;
            }
            let selected: Vec<u64> = g.selected.iter().map(|e| e.to_bits()).collect();
            Ok(json!({"show": g.show, "selected": selected}))
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
        "script.break",
        "Debug the scripts without a client: \"path\" and \"lines\" set a file's breakpoints \
         (an empty list clears them). The next one a script reaches stops it there: the \
         scripts wait and the world's time stands still, while everything else runs on. \
         \"go\": \"continue\", \"over\", \"in\" or \"out\" goes on; \"pause\": true \
         stops at the next line any script runs; \"detach\": true ends it; no arguments \
         just answer. Returns the stop: why, the call stack and the top frame's locals.",
        r#"{"path": "showcase/courtyard.luau", "lines": [31]}"#,
        |world, args| {
            with_runtime(world, |_, rt, _| {
                if args["detach"].as_bool() == Some(true) {
                    rt.debugger.detach_local(&rt.lua);
                    return Ok(crate::debug::stop_json(rt));
                }
                if args.get("path").is_some() || args["pause"].as_bool() == Some(true) {
                    rt.debugger.attach_local();
                }
                if let Some(path) = str_arg(&args, "path") {
                    let path = safe_path(path)?;
                    let lines: Vec<(u32, Option<String>)> = args["lines"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l.as_u64())
                        .map(|l| (l as u32, None))
                        .collect();
                    rt.debugger.set_editor_breakpoints(&rt.lua, &path, &lines);
                }
                if args["pause"].as_bool() == Some(true) {
                    rt.debugger.pause();
                }
                if let Some(go) = args["go"].as_str() {
                    let go = match go {
                        "continue" => crate::debug::Go::Continue,
                        "over" => crate::debug::Go::StepOver,
                        "in" => crate::debug::Go::StepIn,
                        "out" => crate::debug::Go::StepOut,
                        other => {
                            return Err(format!(
                                "go: {other}? one of continue, over, in, out"
                            ));
                        }
                    };
                    rt.debugger.go(go);
                }
                Ok(crate::debug::stop_json(rt))
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
        "script.graph",
        "Compile a visual script (a .graph file, \"path\", or a graph given as \"graph\") to \
         Luau without running it: the code, the node each line came from, the problems found \
         (on which node and pin; errors stop it loading) and a Luau expression per data wire.",
        r#"{"path": "showcase/glow_crystal.graph"}"#,
        |world, args| {
            let (graph, path) = if let Some(g) = args.get("graph").filter(|g| g.is_object()) {
                let graph: crate::graph::Graph = serde_json::from_value(g.clone())
                    .map_err(|e| format!("not a graph: {e}"))?;
                (graph, str_arg(&args, "path").unwrap_or("graph.graph").to_string())
            } else {
                let path = safe_path(str_arg(&args, "path").ok_or("missing 'path' or 'graph'")?)?;
                let full = scripts_root(world).join(&path);
                let text = std::fs::read_to_string(&full)
                    .map_err(|e| format!("can't read {}: {e}", full.display()))?;
                (crate::graph::Graph::parse(&text)?, path)
            };
            let c = crate::graph::compile(&graph, &path);
            Ok(json!({
                "path": path,
                "ok": c.errors().next().is_none(),
                "luau": c.luau,
                "node_lines": c.node_lines,
                "problems": c.problems,
                "watches": c.watches,
            }))
        },
    )
    .register_api(
        "script.graph_nodes",
        "Every node a visual script can use in this game: events, flow, math, one per engine \
         function and a get and a set per component, with their pins and docs. \"category\" \
         narrows it (\"Flow\", \"Engine: fire\").",
        r#"{"category": "Flow"}"#,
        |world, args| {
            let category = str_arg(&args, "category").map(str::to_lowercase);
            let nodes: Vec<_> = crate::graph::library(world, None)
                .into_iter()
                .filter(|s| {
                    category
                        .as_ref()
                        .is_none_or(|c| s.category.to_lowercase().starts_with(c.as_str()))
                })
                .collect();
            Ok(json!(nodes))
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
    #[cfg(feature = "analysis")]
    app.register_api(
        "script.typecheck",
        "Type-check scripts with Luau's own checker before running them: each file's type \
         errors and lint warnings, with lines (1-based). \"path\" checks one file, else every \
         script in the folder; \"text\" checks that text as the file.",
        r#"{"path": "examples/lamp.luau"}"#,
        |world, args| {
            let mut checker = crate::analysis::Analyzer::for_world(world)?;
            let paths = match str_arg(&args, "path") {
                Some(p) => vec![crate::runtime::normalize(p)],
                None => crate::props::files(world),
            };
            if let (Some(text), [one]) = (str_arg(&args, "text"), paths.as_slice()) {
                checker.set_text(one, Some(text.to_string()));
            }
            let files: Vec<Value> = paths
                .iter()
                .map(|p| {
                    let r = checker.check(p);
                    let problems: Vec<Value> = r
                        .problems
                        .iter()
                        .map(|p| {
                            json!({
                                "line": p.span.line + 1,
                                "column": p.span.col + 1,
                                "message": p.message,
                                "error": p.error,
                                "code": p.code,
                            })
                        })
                        .collect();
                    json!({"path": r.path, "errors": r.errors(), "problems": problems})
                })
                .collect();
            Ok(json!({"files": files}))
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
