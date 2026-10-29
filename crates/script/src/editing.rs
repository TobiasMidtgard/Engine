//! What a script editor needs from the runtime (the editor's Scripts tab, the AI bridge):
//! [`file_marks`] (a file's load error, its last runtime error, logpoint values and profiler
//! heat, by line), [`check`] (compile unsaved text without running it), output since a cursor
//! ([`ScriptRuntime::output_since`]) and [`ScriptsHeld`] to keep scripts still while a scene is
//! edited.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use serde::Serialize;

use crate::access::shared;
use crate::runtime::{ScriptRuntime, error_gist, error_location, normalize};

/// While this resource exists no script runs: no hooks, tasks or hot reloads (an editor's edit
/// mode: Roblox Studio and Godot keep scripts still until Play). The `script.*` API still
/// works. Removing the [`ScriptRuntime`] too makes the next run start from fresh VMs.
#[derive(Resource, Debug, Default)]
pub struct ScriptsHeld;

/// An error on a line of a file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineMark {
    /// 1-based; None when the message names no line.
    pub line: Option<u32>,
    /// The message without its location and traceback.
    pub message: String,
    /// Times it was raised (1 for load errors).
    pub count: u64,
}

/// Everything a code view shows beside one file's lines.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FileMarks {
    /// A script runs this file, or one `require`s it.
    pub loaded: bool,
    /// The loaded version (it bumps on every hot reload); 0 when not loaded.
    pub version: u32,
    /// Script instances running this file.
    pub instances: usize,
    /// Their milliseconds last frame, together.
    pub ms: f32,
    /// Why the newest text on disk didn't load (the version before keeps running).
    pub error: Option<LineMark>,
    /// The last runtime error raised in this file's code since it last loaded.
    pub fault: Option<LineMark>,
    /// The newest logpoint values, by line.
    pub values: BTreeMap<u32, String>,
    /// Each line's share of the recorded script time, percent.
    pub heat: BTreeMap<u32, f32>,
    /// Changes whenever `values` or `heat` do.
    pub stamp: u64,
}

/// The marks on `path` (relative to the scripts folder). Empty when no runtime is running.
pub fn file_marks(world: &World, path: &str) -> FileMarks {
    let path = normalize(path);
    let Some(rt) = world.get_resource::<ScriptRuntime>() else {
        return FileMarks::default();
    };
    let mut marks = FileMarks::default();
    {
        let files = rt.files();
        if let Some(f) = files.map.get(&path) {
            marks.loaded = true;
            marks.version = f.version;
            marks.error = f.error.as_deref().map(|e| LineMark {
                line: error_location(e).map(|(_, line)| line),
                message: error_gist(e),
                count: 1,
            });
            marks.fault = f.fault.clone().map(|x| LineMark {
                line: Some(x.line),
                message: x.message,
                count: x.count,
            });
        }
    }
    if marks.fault.is_none() {
        marks.fault = rt.fault_of(&path).map(|x| LineMark {
            line: Some(x.line),
            message: x.message,
            count: x.count,
        });
    }
    for vm in rt.vms() {
        for i in vm.instances.values().filter(|i| i.path == path) {
            marks.instances += 1;
            marks.ms += i.ms;
        }
    }
    let (values_version, values) = rt.debugger.inline_values(&path);
    let (heat_version, heat) = crate::profiler::line_heat(&rt.luas(), &path);
    marks.values = values;
    marks.heat = heat;
    marks.stamp = values_version
        .wrapping_mul(1 << 20)
        .wrapping_add(heat_version);
    marks
}

/// Compiles `source` without running it: None when it compiles, else the line and message of
/// its first syntax error (what an editor marks while you type, before you save).
pub fn check(source: &str) -> Option<(u32, String)> {
    match mlua::chunk::Compiler::new().compile(source) {
        Ok(_) => None,
        Err(mlua::Error::SyntaxError { message, .. }) => {
            // Luau reports "LINE: message".
            let (line, text) = message.split_once(':').unwrap_or(("0", &message));
            Some((line.trim().parse().unwrap_or(0), text.trim().to_string()))
        }
        Err(other) => Some((0, other.to_string())),
    }
}

/// One line of script output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputLine {
    pub text: String,
    /// An error (else `print` or `warn` output).
    pub error: bool,
    /// The file and line it came from, when it says.
    pub at: Option<(String, u32)>,
}

/// How far a reader of [`ScriptRuntime::output_since`] got: log lines and errors seen, per VM.
#[derive(Debug, Clone, Default)]
pub struct OutputCursor {
    seen: Vec<(u64, u64)>,
}

impl ScriptRuntime {
    /// The `print`/`warn` lines and errors of every VM since `cursor`, which moves past them
    /// (each VM's lines first, then its errors). A runtime made anew starts over.
    pub fn output_since(&self, cursor: &mut OutputCursor) -> Vec<OutputLine> {
        let vms: Vec<&ScriptRuntime> = self.vms().collect();
        cursor.seen.resize(vms.len(), (0, 0));
        let mut out = Vec::new();
        for (vm, seen) in vms.iter().zip(cursor.seen.iter_mut()) {
            let s = shared(&vm.lua);
            if s.log_total < seen.0 || s.errors_total < seen.1 {
                *seen = (0, 0);
            }
            let new_lines = ((s.log_total - seen.0) as usize).min(s.log.len());
            for line in s.log.iter().skip(s.log.len() - new_lines) {
                out.push(OutputLine {
                    text: line.clone(),
                    error: false,
                    at: error_location(line),
                });
            }
            let new_errors = ((s.errors_total - seen.1) as usize).min(s.errors.len());
            for e in s.errors.iter().skip(s.errors.len() - new_errors) {
                let at = error_location(&e.message);
                let gist = error_gist(&e.message);
                let text = match &at {
                    Some((path, line)) => format!("{path}:{line}: {gist}"),
                    None => match &e.path {
                        Some(path) => format!("{path}: {gist}"),
                        None => gist,
                    },
                };
                out.push(OutputLine {
                    text,
                    error: true,
                    at,
                });
            }
            *seen = (s.log_total, s.errors_total);
        }
        out
    }
}

/// What a word is, for code completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WordKind {
    Keyword,
    /// A global function or value (`print`, `vec3`).
    Global,
    /// A table of functions (`fire`, `task`, `math`).
    Namespace,
    Function,
    Field,
    Method,
    /// A method the engine calls on scripts (`update`, `on_interact`).
    Hook,
    /// A registered component's name (for `e:get` and `e:set`).
    Component,
}

/// A word code completion offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    /// The word itself ("wait").
    pub name: String,
    /// Where it is reached: a global table or namespace ("task", "physics.proving_ground"), a
    /// class after `.` or `:` ("Entity", "Vec3", "Quat", "ScriptSelf"), "hook", "component", or
    /// "" (a global or keyword).
    pub scope: String,
    pub kind: WordKind,
    /// Its type or signature, "(seconds: number?) -> number", or an engine function's example
    /// arguments as a Luau table.
    pub detail: String,
    /// What it does.
    pub doc: String,
}

const HOOKS: &[(&str, &str, &str)] = &[
    (
        "start",
        "()",
        "Once, when the script starts. Runs as a task: it may task.wait.",
    ),
    (
        "update",
        "(dt)",
        "Every frame, with world seconds (slow motion aware). Must not wait.",
    ),
    ("fixed_update", "(dt)", "Every physics step."),
    (
        "parallel_update",
        "(dt)",
        "Every frame in an actor VM beside other scripts: the world is read-only until \
         task.synchronize().",
    ),
    (
        "on_interact",
        "(by)",
        "The player pressed E on this entity. Runs as a task.",
    ),
    (
        "on_contact",
        "(other, info)",
        "A physics contact with another entity began.",
    ),
    (
        "on_message",
        "(event, data)",
        "A message from e:send or script.send (or define a method named after the event).",
    ),
    ("on_props", "()", "Script.props changed."),
    (
        "on_reload",
        "()",
        "The file was hot reloaded; self kept its fields.",
    ),
    (
        "on_destroy",
        "()",
        "The entity or its Script is going away.",
    ),
    (
        "on_world_shift",
        "(shift)",
        "The world moved to a new frame: map kept positions with shift:point(p).",
    ),
];

const KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "type", "until",
    "while",
];

/// Luau's own globals and libraries.
const LUAU_GLOBALS: &[(&str, &str)] = &[
    ("print", "(...any) -> ()"),
    ("warn", "(...any) -> ()"),
    ("error", "(message: any, level: number?) -> never"),
    ("pcall", "(f, ...any) -> (boolean, ...any)"),
    ("tostring", "(value: any) -> string"),
    ("tonumber", "(value: any, base: number?) -> number?"),
    ("type", "(value: any) -> string"),
    ("typeof", "(value: any) -> string"),
    ("pairs", "(t: table) -> iterator"),
    ("ipairs", "(t: table) -> iterator"),
    ("next", "(t: table, key: any?) -> (any, any)"),
    ("select", "(n: number | \"#\", ...any) -> ...any"),
    ("assert", "(value: any, message: string?) -> any"),
    ("setmetatable", "(t: table, meta: table?) -> table"),
    ("getmetatable", "(t: any) -> table?"),
    ("rawget", "(t: table, key: any) -> any"),
    ("rawset", "(t: table, key: any, value: any) -> table"),
    ("rawequal", "(a: any, b: any) -> boolean"),
    ("require", "(path: string) -> any"),
];

const LUAU_LIBRARIES: &[(&str, &str)] = &[
    (
        "math",
        "abs acos asin atan atan2 ceil clamp cos cosh deg exp floor fmod huge lerp log log10 max \
         min modf noise pi pow rad random randomseed round sign sin sinh sqrt tan tanh",
    ),
    (
        "string",
        "byte char find format gmatch gsub len lower match rep reverse split sub upper",
    ),
    (
        "table",
        "clear clone concat create find freeze insert isfrozen move pack remove sort unpack",
    ),
    (
        "vector",
        "create magnitude normalize cross dot angle floor ceil abs sign clamp max min zero one",
    ),
    (
        "coroutine",
        "create resume running status wrap yield isyieldable close",
    ),
    ("os", "clock date difftime time"),
    (
        "bit32",
        "band bor bxor bnot btest extract replace lshift rshift arshift lrotate rrotate countlz \
         countrz byteswap",
    ),
    ("utf8", "char charpattern codepoint codes len offset"),
];

/// A JSON example as the Luau table a script writes: `{ at = {0, 1, 0}, radius = 1.5 }`.
fn luau_literal(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let plain = k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && !k.starts_with(|c: char| c.is_ascii_digit());
                    if plain {
                        format!("{k} = {}", luau_literal(v))
                    } else {
                        format!("[\"{k}\"] = {}", luau_literal(v))
                    }
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(luau_literal).collect();
            format!("{{{}}}", items.join(", "))
        }
        Value::Null => "nil".into(),
        other => other.to_string(),
    }
}

/// Splits `a: T, b: (x, y) -> U` at its top-level commas.
fn split_members(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' | '{' | '[' | '<' => depth += 1,
            ')' | '}' | ']' | '>' if !text[..i].ends_with('-') => depth -= 1,
            ',' if depth == 0 => {
                out.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(text[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// A table member line's words: `wait: (seconds: number?) -> number`.
fn table_member(scope: &str, member: &str, doc: &str, out: &mut Vec<Word>) {
    let Some((name, ty)) = member.split_once(':') else {
        return;
    };
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return;
    }
    let ty = ty.trim();
    let function = ty.starts_with('(') && ty.contains("->");
    out.push(Word {
        name: name.to_string(),
        scope: scope.to_string(),
        kind: if function {
            WordKind::Function
        } else {
            WordKind::Field
        },
        detail: ty.replace("self: any, ", "").replace("(self: any)", "()"),
        doc: doc.to_string(),
    });
}

/// The built-in part of the vocabulary, read from the type definitions scripts are checked
/// against (`types::BUILTINS`), so the two never disagree.
fn builtin_words(out: &mut Vec<Word>) {
    let mut doc = String::new();
    // The open block: a class ("Entity"), or a table's scope ("task").
    let mut class: Option<String> = None;
    let mut table: Option<String> = None;
    for raw in crate::types::BUILTINS.lines() {
        let line = raw.trim();
        if let Some(text) = line.strip_prefix("---") {
            if !doc.is_empty() {
                doc.push(' ');
            }
            doc.push_str(text.trim());
            continue;
        }
        if let Some(text) = line.strip_prefix("--") {
            if class.is_some() || table.is_some() {
                if !doc.is_empty() {
                    doc.push(' ');
                }
                doc.push_str(text.trim());
            }
            continue;
        }
        if let Some(name) = line.strip_prefix("declare class ") {
            class = Some(name.trim().to_string());
        } else if let Some(c) = &class {
            if line == "end" {
                class = None;
            } else if let Some(sig) = line.strip_prefix("function ") {
                if let Some((name, rest)) = sig.split_once('(')
                    && !name.starts_with("__")
                {
                    let rest = rest
                        .strip_prefix("self, ")
                        .or_else(|| rest.strip_prefix("self"))
                        .unwrap_or(rest);
                    out.push(Word {
                        name: name.to_string(),
                        scope: c.clone(),
                        kind: WordKind::Method,
                        detail: format!("({rest}"),
                        doc: std::mem::take(&mut doc),
                    });
                }
            } else {
                table_member(c, line, &doc, out);
                doc.clear();
            }
        } else if let Some(scope) = &table {
            if line.starts_with('}') {
                table = None;
            } else {
                for member in split_members(line) {
                    table_member(scope, member, &doc, out);
                }
                doc.clear();
            }
        } else if let Some(rest) = line.strip_prefix("declare function ") {
            if let Some((name, sig)) = rest.split_once('(') {
                out.push(Word {
                    name: name.to_string(),
                    scope: String::new(),
                    kind: WordKind::Global,
                    detail: format!("({sig}"),
                    doc: "Calls an engine function by a name built at run time.".into(),
                });
            }
        } else if let Some(rest) = line
            .strip_prefix("declare ")
            .or_else(|| line.strip_prefix("export type "))
        {
            let Some((name, ty)) = rest.split_once([':', '=']) else {
                continue;
            };
            let name = name.trim().to_string();
            let ty = ty.trim();
            if ty.ends_with('{') {
                table = Some(name.clone());
                if line.starts_with("declare ") {
                    let callable = ty.trim_end_matches('{').trim().trim_end_matches('&').trim();
                    out.push(Word {
                        name,
                        scope: String::new(),
                        kind: if callable.is_empty() {
                            WordKind::Namespace
                        } else {
                            WordKind::Global
                        },
                        detail: callable
                            .strip_prefix('(')
                            .and_then(|c| c.strip_suffix(')'))
                            .unwrap_or(callable)
                            .into(),
                        doc: std::mem::take(&mut doc),
                    });
                }
            }
        }
    }
}

/// Every word code completion offers in this world: Luau's keywords, globals and libraries,
/// the engine's own (`vec3`, `world`, `task`, entities, `self`), every registered engine
/// function with its summary and example arguments, script hooks and component names.
pub fn vocabulary(world: &World) -> Vec<Word> {
    let mut out: Vec<Word> = KEYWORDS
        .iter()
        .map(|k| Word {
            name: k.to_string(),
            scope: String::new(),
            kind: WordKind::Keyword,
            detail: String::new(),
            doc: String::new(),
        })
        .collect();
    for (name, sig) in LUAU_GLOBALS {
        out.push(Word {
            name: name.to_string(),
            scope: String::new(),
            kind: WordKind::Global,
            detail: sig.to_string(),
            doc: String::new(),
        });
    }
    for (lib, members) in LUAU_LIBRARIES {
        out.push(Word {
            name: lib.to_string(),
            scope: String::new(),
            kind: WordKind::Namespace,
            detail: "Luau library".into(),
            doc: String::new(),
        });
        for m in members.split_whitespace() {
            out.push(Word {
                name: m.to_string(),
                scope: lib.to_string(),
                kind: if matches!(m, "pi" | "huge" | "charpattern" | "zero" | "one") {
                    WordKind::Field
                } else {
                    WordKind::Function
                },
                detail: String::new(),
                doc: String::new(),
            });
        }
    }
    builtin_words(&mut out);
    for (name, ty) in [
        ("pressed", "(action: string) -> boolean"),
        ("just_pressed", "(action: string) -> boolean"),
        ("just_released", "(action: string) -> boolean"),
        ("value", "(action: string) -> number"),
        ("stick", "(action: string) -> (number, number)"),
    ] {
        out.push(Word {
            name: name.into(),
            scope: "input".into(),
            kind: WordKind::Function,
            detail: ty.into(),
            doc: "An input action's state this frame (actions from the input map: \"jump\", \"interact\").".into(),
        });
    }
    for (name, args, doc) in HOOKS {
        out.push(Word {
            name: name.to_string(),
            scope: "hook".into(),
            kind: WordKind::Hook,
            detail: args.to_string(),
            doc: doc.to_string(),
        });
    }
    if let Some(api) = world.get_resource::<engine_core::registry::ApiRegistry>() {
        let mut spaces = std::collections::BTreeSet::new();
        for e in api.list() {
            let Some((scope, name)) = e.name.rsplit_once('.') else {
                continue;
            };
            let mut prefix = String::new();
            for part in scope.split('.') {
                spaces.insert((prefix.clone(), part.to_string()));
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(part);
            }
            let detail = serde_json::from_str::<serde_json::Value>(&e.args_example)
                .map(|v| luau_literal(&v))
                .unwrap_or_default();
            out.push(Word {
                name: name.to_string(),
                scope: scope.to_string(),
                kind: WordKind::Function,
                detail,
                doc: e.summary.clone(),
            });
        }
        for (scope, name) in spaces {
            let known = out
                .iter()
                .any(|w| w.scope == scope && w.name == name && w.kind == WordKind::Namespace);
            if !known {
                out.push(Word {
                    name,
                    scope,
                    kind: WordKind::Namespace,
                    detail: "engine functions".into(),
                    doc: String::new(),
                });
            }
        }
    }
    if let Some(types) = world.get_resource::<engine_core::registry::TypeRegistry>() {
        for c in types.components() {
            out.push(Word {
                name: c.name.clone(),
                scope: "component".into(),
                kind: WordKind::Component,
                detail: "component".into(),
                doc: c.doc.lines().next().unwrap_or("").to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_reads_the_type_definitions() {
        let mut words = Vec::new();
        builtin_words(&mut words);
        let find = |scope: &str, name: &str| {
            words
                .iter()
                .find(|w| w.scope == scope && w.name == name)
                .unwrap_or_else(|| panic!("no {scope}.{name}"))
        };
        let wait = find("task", "wait");
        assert_eq!(wait.kind, WordKind::Function);
        assert!(wait.detail.contains("seconds"), "{wait:?}");
        assert!(wait.doc.contains("Pauses"), "{wait:?}");
        assert_eq!(find("Entity", "set").kind, WordKind::Method);
        assert!(find("Entity", "set").doc.contains("PointLight"));
        assert_eq!(find("Entity", "position").kind, WordKind::Field);
        assert_eq!(
            find("Vec3", "lerp").detail,
            "(other: Vec3, t: number): Vec3"
        );
        assert_eq!(find("vec3", "up").kind, WordKind::Field);
        assert_eq!(find("vec3", "from_angle").kind, WordKind::Function);
        assert_eq!(find("ScriptSelf", "every").kind, WordKind::Function);
        assert!(!find("ScriptSelf", "every").detail.contains("self"));
        assert_eq!(find("", "world").kind, WordKind::Namespace);
        assert_eq!(find("", "vec3").kind, WordKind::Global);
        assert_eq!(
            find("", "vec3").detail,
            "(x: number?, y: number?, z: number?) -> Vec3"
        );
        assert!(find("task", "synchronize").doc.contains("parallel_update"));
        assert!(words.iter().all(|w| !w.name.starts_with("__")));
    }

    #[test]
    fn examples_read_as_luau_tables() {
        let v = serde_json::json!({"at": [0, 1, 0], "radius": 1.5, "name": "x"});
        assert_eq!(
            luau_literal(&v),
            "{ at = {0, 1, 0}, name = \"x\", radius = 1.5 }"
        );
    }

    #[test]
    fn check_names_the_line_of_a_syntax_error() {
        assert_eq!(check("local x = 1\nreturn x"), None);
        let (line, message) =
            check("local S = {}\nfunction S:update(dt)\n  local x = (1 +\nend\nreturn S")
                .expect("an error");
        assert_eq!(line, 4, "{message}");
        assert!(!message.is_empty());
        let (line, _) = check("local a = 1\n\nif a then\n  print(a)\nreturn a").expect("an error");
        assert_eq!(line, 5);
    }
}
