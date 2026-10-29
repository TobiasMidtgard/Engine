//! Script properties as the editor's Details panel shows them (Unity's inspector fields,
//! Unreal's Instance Editable variables, Godot's `@export`): each prop a script's class declares
//! in `props`, in the file's order, with its kind, its default and the hints in its comment.
//!
//! ```lua
//! Lamp.props = {
//!     -- @group Light
//!     lumens = 1800, -- at full brightness @range 0 5000 @unit lm
//!     color = vec3(1, 0.72, 0.42), -- the flame's light
//!     -- @group Flame
//!     period = 2.4, -- seconds a breath @range 0.3 8 @unit s
//!     target = "Courtyard director", -- who hears it @entity
//! }
//! ```
//!
//! Hints, anywhere in the prop's comment (the rest of the comment is its tooltip):
//! `@range min max [step]` (a slider; the panel keeps values inside, as Unreal's `ClampMin`
//! and `ClampMax`), `@min v` / `@max v` (one end only), `@step v`, `@int`, `@unit text`,
//! `@angle` (radians, shown in degrees), `@color` (a `vec3` as linear RGB; props named
//! "color", "colour" or "tint" are colors anyway), `@enum a|b|c` (a string from a list),
//! `@entity` (a string naming an entity: the panel offers the scene's names, the script finds it
//! with `world.find`), `@hide`, and `@group Name` on a comment line of its own for the props
//! below it. Tables of named values fold out with their own props.
//!
//! The defaults are what the class's `props` holds after its file runs, so `math.pi / 4` and
//! `vec3.up * 2` work (Defold reads literals only). The editor reads them in a VM of its own,
//! once per version of the file on disk.
//!
//! The same reading gives the class's `signals` (each a name, its parameters as text and the
//! comment after it: `pulled = "by: Entity", -- the arm reached the end`) and its methods, for
//! wiring signals in the Details panel ([`declared`]).

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use bevy_ecs::prelude::*;
use mlua::{Table, Value};
use serde_json::Value as Json;

use crate::convert::to_json;
use crate::math::{Kind, kind};
use crate::runtime::{ScriptRuntime, normalize};

/// How a prop is edited.
#[derive(Debug, Clone, PartialEq)]
pub enum PropKind {
    Number,
    /// A whole number (`@int`).
    Integer,
    /// Radians, shown in degrees (`@angle`).
    Angle,
    Bool,
    Text,
    /// A string from a list (`@enum a|b|c`).
    Choice(Vec<String>),
    /// A string naming an entity (`@entity`).
    Entity,
    Vec3,
    /// A `vec3` as linear RGB.
    Color,
    /// A `quat`, edited as angles.
    Rotation,
    /// A table of named values: see [`Prop::fields`].
    Table,
    /// A list (or a table the panel can't take apart): edited as JSON.
    List,
    /// A function or anything else that isn't a setting (not shown).
    Other(String),
}

/// One prop of a script.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub name: String,
    pub kind: PropKind,
    /// The class's default, as JSON (vectors as `[x, y, z]`, as `Script::props` holds them).
    pub default: Json,
    /// Its comment, hints taken out.
    pub doc: String,
    /// The `@group` it is in.
    pub group: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Both ends were given with `@range`: a slider.
    pub slider: bool,
    pub step: Option<f64>,
    pub unit: Option<String>,
    pub hidden: bool,
    /// Where the file declares it (1-based; 0 when it isn't written in the `props` table).
    pub line: u32,
    /// A [`PropKind::Table`]'s own props.
    pub fields: Vec<Prop>,
}

impl Prop {
    /// A value kept inside the prop's `@range`, `@min` and `@max` (whole for `@int`).
    pub fn clamp(&self, v: f64) -> f64 {
        let mut v = v;
        if let Some(min) = self.min {
            v = v.max(min);
        }
        if let Some(max) = self.max {
            v = v.min(max);
        }
        if self.kind == PropKind::Integer {
            v = v.round();
        }
        v
    }
}

// ----------------------------------------------------------------------------- reading

/// A signal a class declares: `Lever.signals = { pulled = "by: Entity", -- doc }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Signal {
    pub name: String,
    /// Its values as written: `"by: Entity"` (may be empty).
    pub params: String,
    pub doc: String,
    /// 1-based line of its entry.
    pub line: u32,
}

/// A method of a class (for the signal wiring's method lists).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Method {
    pub name: String,
    /// Its parameters as written, without `self`.
    pub params: String,
    /// 1-based line of `function Class:name(`, when found.
    pub line: u32,
    /// A hook the engine calls itself (`update`, `on_interact`, ...).
    pub hook: bool,
}

/// What a class declares for the editor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Declared {
    pub props: Vec<Prop>,
    pub signals: Vec<Signal>,
    pub methods: Vec<Method>,
    /// Its `states` table, or why that doesn't run ([`crate::states`]).
    pub machine: Option<Result<crate::states::Machine, crate::states::Problem>>,
}

/// Reads the classes' props for the editor, each file once per version on disk.
#[derive(Resource, Default)]
pub struct PropsReader {
    read: HashMap<String, (Stamp, Result<Declared, String>)>,
}

/// A file's modified time and length.
type Stamp = Option<(SystemTime, u64)>;

/// The props `path`'s class declares, in the order its file lists them (those it builds in
/// code after them, by name). An error says why the file didn't load.
pub fn of(world: &mut World, path: &str) -> Result<Vec<Prop>, String> {
    declared(world, path).map(|d| d.props)
}

/// Everything `path`'s class declares: props, signals and methods.
pub fn declared(world: &mut World, path: &str) -> Result<Declared, String> {
    let path = normalize(path);
    let full = crate::scripts_root(world).join(&path);
    let stamp = file_stamp(&full);
    if let Some((s, declared)) = world
        .get_resource::<PropsReader>()
        .and_then(|r| r.read.get(&path))
        && *s == stamp
    {
        return declared.clone();
    }
    let declared = read(world, &path);
    world
        .get_resource_or_insert_with(PropsReader::default)
        .read
        .insert(path, (stamp, declared.clone()));
    declared
}

fn file_stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Runs the file in a VM of its own (its top level only, as `require` would) and reads its
/// class's `props`, `signals` and methods.
fn read(world: &mut World, path: &str) -> Result<Declared, String> {
    let root = crate::scripts_root(world);
    let source =
        std::fs::read_to_string(root.join(path)).map_err(|e| format!("can't read {path}: {e}"))?;
    let source = crate::runtime::runnable(path, source)?;
    let settings = world
        .get_resource::<crate::ScriptSettings>()
        .cloned()
        .unwrap_or_default();
    let api = world
        .get_resource::<engine_core::prelude::ApiRegistry>()
        .cloned()
        .unwrap_or_default();
    let mut rt = ScriptRuntime::new(&api, root, &settings).map_err(|e| e.to_string())?;
    rt.ensure_loaded(world, path)?;
    let class = rt
        .class(path)
        .ok_or_else(|| format!("{path} didn't load"))?;
    let props = match class.raw_get::<Value>("props") {
        Ok(Value::Table(t)) => props_from(&t, &notes(&source, "props")),
        _ => Vec::new(),
    };
    let signals = match class.raw_get::<Value>("signals") {
        Ok(Value::Table(t)) => signals_from(&t, &notes(&source, "signals")),
        _ => Vec::new(),
    };
    let mut methods: Vec<Method> = class
        .pairs::<Value, Value>()
        .filter_map(|p| p.ok())
        .filter_map(|(k, v)| match (k, v) {
            (Value::String(k), Value::Function(_)) => Some(k.to_string_lossy().to_string()),
            _ => None,
        })
        .filter(|name| !name.starts_with("__"))
        .map(|name| {
            let (params, line) = method_params(&source, &name).unwrap_or_default();
            Method {
                hook: crate::editing::is_hook(&name),
                name,
                params,
                line,
            }
        })
        .collect();
    methods.sort_by(|a, b| (a.hook, a.line, &a.name).cmp(&(b.hook, b.line, &b.name)));
    let machine = crate::states::of_class(&rt.lua, &class).map(|m| match m {
        Ok(mut m) => {
            m.locate(&source);
            Ok(m)
        }
        Err(mut p) => {
            p.locate(&source);
            Err(p)
        }
    });
    Ok(Declared {
        props,
        signals,
        methods,
        machine,
    })
}

/// The class's `signals`, in the file's order.
fn signals_from(table: &Table, notes: &[Note]) -> Vec<Signal> {
    let mut out: Vec<Signal> = table
        .pairs::<Value, Value>()
        .filter_map(|p| p.ok())
        .filter_map(|(k, v)| {
            let Value::String(k) = k else { return None };
            let name = k.to_string_lossy().to_string();
            let params = match v {
                Value::String(s) => s.to_string_lossy().trim().to_string(),
                _ => String::new(),
            };
            let note = notes.iter().find(|n| n.name == name);
            Some(Signal {
                params,
                doc: note.map(|n| n.doc.clone()).unwrap_or_default(),
                line: note.map_or(0, |n| n.line),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| (a.line == 0, a.line, &a.name).cmp(&(b.line == 0, b.line, &b.name)));
    out
}

/// `function Class:name(a, b)` or `function Class.name(self, a)` in the source: the
/// parameters after `self`, and the 1-based line.
fn method_params(source: &str, name: &str) -> Option<(String, u32)> {
    for (k, line) in source.lines().enumerate() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("function ") else {
            continue;
        };
        let Some(open) = rest.find('(') else { continue };
        let head = &rest[..open];
        let colon = head.ends_with(&format!(":{name}"));
        if !colon && !head.ends_with(&format!(".{name}")) {
            continue;
        }
        let close = rest[open..].find(')').map(|c| open + c)?;
        let mut params = rest[open + 1..close].trim();
        if !colon {
            params = params
                .strip_prefix("self")
                .map(|p| p.trim_start_matches([',', ' ']))
                .unwrap_or(params);
        }
        return Some((params.to_string(), k as u32 + 1));
    }
    None
}

/// The table's entries as props, in the order of `notes` (the rest after them, by name).
fn props_from(table: &Table, notes: &[Note]) -> Vec<Prop> {
    let mut entries: Vec<(String, Value)> = table
        .pairs::<Value, Value>()
        .filter_map(|p| p.ok())
        .filter_map(|(k, v)| match k {
            Value::String(s) => Some((s.to_string_lossy().to_string(), v)),
            _ => None,
        })
        .collect();
    let rank = |name: &str| {
        notes
            .iter()
            .position(|n| n.name == name)
            .unwrap_or(usize::MAX)
    };
    entries.sort_by(|a, b| (rank(&a.0), &a.0).cmp(&(rank(&b.0), &b.0)));
    entries
        .into_iter()
        .map(|(name, v)| {
            let note = notes.iter().find(|n| n.name == name);
            prop(name, &v, note)
        })
        .collect()
}

fn prop(name: String, v: &Value, note: Option<&Note>) -> Prop {
    let empty = Note::default();
    let note = note.unwrap_or(&empty);
    let tag = |t: &str| {
        note.tags
            .iter()
            .find(|(k, _)| k == t)
            .map(|(_, a)| a.as_str())
    };
    let num = |t: &str| tag(t).and_then(|a| a.trim().parse::<f64>().ok());
    let mut p = Prop {
        name: name.clone(),
        kind: PropKind::Other(crate::math::type_name(v).to_string()),
        default: to_json(v).unwrap_or(Json::Null),
        doc: note.doc.clone(),
        group: note.group.clone(),
        min: num("min"),
        max: num("max"),
        slider: false,
        step: num("step"),
        unit: tag("unit")
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
        hidden: tag("hide").is_some(),
        line: note.line,
        fields: Vec::new(),
    };
    if let Some(range) = tag("range") {
        let n: Vec<f64> = range
            .split([' ', ',', '\t'])
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if n.len() >= 2 {
            p.min = Some(n[0].min(n[1]));
            p.max = Some(n[0].max(n[1]));
            p.slider = true;
        }
        if n.len() >= 3 {
            p.step = Some(n[2]);
        }
    }
    let lower = name.to_ascii_lowercase();
    p.kind = match v {
        Value::Boolean(_) => PropKind::Bool,
        Value::Integer(_) | Value::Number(_) => {
            if tag("int").is_some() {
                PropKind::Integer
            } else if tag("angle").is_some() {
                PropKind::Angle
            } else {
                PropKind::Number
            }
        }
        Value::String(_) => {
            if let Some(list) = tag("enum") {
                PropKind::Choice(
                    list.split('|')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                )
            } else if tag("entity").is_some() {
                PropKind::Entity
            } else {
                PropKind::Text
            }
        }
        Value::Table(t) => match kind(t) {
            Some(Kind::Vec3)
                if tag("color").is_some()
                    || ["color", "colour", "tint"]
                        .iter()
                        .any(|c| lower.contains(c)) =>
            {
                PropKind::Color
            }
            Some(Kind::Vec3) => PropKind::Vec3,
            Some(Kind::Quat) => PropKind::Rotation,
            None if p.default.is_object() => {
                p.fields = props_from(t, &note.fields);
                PropKind::Table
            }
            None => PropKind::List,
        },
        _ => p.kind,
    };
    p
}

/// The running instance's own values on `e` (`self.t`, `self.lit`, ...; not `entity` and
/// `props`), as the debugger shows them, by name. None when no script runs on it.
pub fn state(world: &World, e: Entity) -> Option<Vec<crate::debug::Variable>> {
    let rt = world.get_resource::<ScriptRuntime>()?;
    rt.vms().find_map(|vm| {
        let selves: Table = vm.lua.named_registry_value(crate::entity::SELVES).ok()?;
        let this: Table = selves.raw_get(e.to_bits() as f64).ok()?;
        let mut out: Vec<crate::debug::Variable> = this
            .pairs::<Value, Value>()
            .filter_map(|p| p.ok())
            .filter_map(|(k, v)| {
                let Value::String(k) = k else { return None };
                let name = k.to_string_lossy().to_string();
                (name != "entity"
                    && name != "props"
                    && !name.starts_with("__")
                    && !matches!(v, Value::Function(_)))
                .then(|| crate::debug::Variable {
                    name,
                    value: crate::debugger::display(&v, true),
                    kind: crate::math::type_name(&v).to_string(),
                    reference: 0,
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Some(out)
    })
}

/// The script files under the scripts folder (`.luau` and `.graph`, not the shared modules in
/// `lib/` folders nor `engine.d.luau`), sorted.
pub fn files(world: &World) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if path.is_dir() {
                if name != "lib" && !name.starts_with('.') {
                    walk(&path, &format!("{prefix}{name}/"), out);
                }
            } else if (name.ends_with(".luau") && !name.ends_with(".d.luau"))
                || name.ends_with(".graph")
            {
                out.push(format!("{prefix}{name}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(&crate::scripts_root(world), "", &mut out);
    out.sort();
    out
}

// ----------------------------------------------------------------------------- the source

/// What the source says about one prop.
#[derive(Debug, Clone, Default, PartialEq)]
struct Note {
    name: String,
    line: u32,
    doc: String,
    /// `@name args`, in order.
    tags: Vec<(String, String)>,
    group: Option<String>,
    /// A table value's own entries.
    fields: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    /// A string, number or anything else that is only skipped over.
    Atom,
    Sym(char),
    /// A comment's text; true when nothing but spaces comes before it on its line.
    Comment(String, bool),
}

/// The notes on `X.<field> = { ... }`'s entries (the first such table in the file).
fn notes(source: &str, field: &str) -> Vec<Note> {
    let toks = lex(source);
    let start = toks.windows(5).position(|w| {
        matches!(
            (&w[0].0, &w[1].0, &w[2].0, &w[3].0, &w[4].0),
            (Tok::Name(_), Tok::Sym('.'), Tok::Name(p), Tok::Sym('='), Tok::Sym('{')) if p == field
        )
    });
    match start {
        Some(i) => table(&toks, i + 5).0,
        None => Vec::new(),
    }
}

/// The entries of a table whose `{` is just before `i`; returns them and the index after its `}`.
fn table(toks: &[(Tok, u32)], mut i: usize) -> (Vec<Note>, usize) {
    let mut out = Vec::new();
    let mut lead: Vec<String> = Vec::new();
    let mut group: Option<String> = None;
    while i < toks.len() {
        match &toks[i].0 {
            Tok::Comment(text, _) => {
                let t = text.trim();
                if let Some(g) = t.strip_prefix("@group") {
                    group = Some(g.trim().to_string()).filter(|g| !g.is_empty());
                    lead.clear();
                } else {
                    lead.push(t.to_string());
                }
                i += 1;
                continue;
            }
            Tok::Sym('}') => return (out, i + 1),
            Tok::Sym(',') | Tok::Sym(';') => {
                i += 1;
                continue;
            }
            _ => {}
        }
        // One entry: `name = value`, `[key] = value` or a bare value.
        let line = toks[i].1;
        let name = match (&toks[i].0, toks.get(i + 1).map(|t| &t.0)) {
            (Tok::Name(n), Some(Tok::Sym('='))) => {
                i += 2;
                Some(n.clone())
            }
            _ => None,
        };
        let mut fields = Vec::new();
        if matches!(toks.get(i).map(|t| &t.0), Some(Tok::Sym('{'))) {
            let (f, next) = table(toks, i + 1);
            fields = f;
            i = next;
        }
        // The rest of the value, up to the `,` that ends the entry.
        let mut depth = 0i32;
        while i < toks.len() {
            match &toks[i].0 {
                Tok::Sym('(' | '[' | '{') => depth += 1,
                Tok::Sym(')' | ']') => depth -= 1,
                Tok::Sym('}') if depth == 0 => break,
                Tok::Sym('}') => depth -= 1,
                Tok::Sym(',' | ';') if depth == 0 => break,
                Tok::Comment(..) if depth == 0 => break,
                _ => {}
            }
            i += 1;
        }
        if matches!(toks.get(i).map(|t| &t.0), Some(Tok::Sym(',' | ';'))) {
            i += 1;
        }
        // A comment after code on its line is the entry's.
        let mut docs = std::mem::take(&mut lead);
        if let Some((Tok::Comment(text, false), _)) = toks.get(i) {
            docs.push(text.trim().to_string());
            i += 1;
        }
        if let Some(name) = name {
            let (doc, tags) = hints(&docs.join(" "));
            out.push(Note {
                name,
                line,
                doc,
                tags,
                group: group.clone(),
                fields,
            });
        }
    }
    (out, i)
}

/// A comment split into its text and its `@hints`.
fn hints(comment: &str) -> (String, Vec<(String, String)>) {
    let mut parts = comment.split('@');
    let doc = parts.next().unwrap_or("").trim().to_string();
    let tags = parts
        .filter_map(|p| {
            let p = p.trim();
            let name: String = p
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then(|| (name.clone(), p[name.len()..].trim().to_string()))
        })
        .collect();
    (doc, tags)
}

/// Luau split into the tokens [`table`] needs, each with its line (1-based).
fn lex(source: &str) -> Vec<(Tok, u32)> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    // Only spaces so far on this line.
    let mut bare = true;
    // `[[`, `[=[`, ...: the number of `=`, if a long bracket opens at `at`.
    let long = |at: usize| -> Option<usize> {
        if chars.get(at) != Some(&'[') {
            return None;
        }
        let eq = chars[at + 1..].iter().take_while(|c| **c == '=').count();
        (chars.get(at + 1 + eq) == Some(&'[')).then_some(eq)
    };
    // Skips a long bracket opening at `at` with `eq` signs; returns its text and the index
    // after it.
    let skip_long = |at: usize, eq: usize, line: &mut u32| -> (String, usize) {
        let mut j = at + eq + 2;
        let start = j;
        while j < chars.len() {
            if chars[j] == ']'
                && chars[j + 1..].iter().take(eq).all(|c| *c == '=')
                && chars.get(j + 1 + eq) == Some(&']')
            {
                return (chars[start..j].iter().collect(), j + eq + 2);
            }
            if chars[j] == '\n' {
                *line += 1;
            }
            j += 1;
        }
        (chars[start..].iter().collect(), chars.len())
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            bare = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let at = line;
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            let own = bare;
            if let Some(eq) = long(i + 2) {
                let (text, next) = skip_long(i + 2, eq, &mut line);
                out.push((Tok::Comment(text, own), at));
                i = next;
            } else {
                let end = chars[i..]
                    .iter()
                    .position(|c| *c == '\n')
                    .map_or(chars.len(), |p| i + p);
                let text: String = chars[i + 2..end].iter().collect();
                out.push((Tok::Comment(text, own), at));
                i = end;
            }
            continue;
        }
        bare = false;
        if let Some(eq) = long(i) {
            let (_, next) = skip_long(i, eq, &mut line);
            out.push((Tok::Atom, at));
            i = next;
        } else if c == '"' || c == '\'' || c == '`' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                if chars[j] == '\\' {
                    j += 1;
                } else if chars[j] == '\n' {
                    line += 1;
                }
                j += 1;
            }
            out.push((Tok::Atom, at));
            i = j + 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let end = chars[i..]
                .iter()
                .position(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
                .map_or(chars.len(), |p| i + p);
            out.push((Tok::Name(chars[i..end].iter().collect()), at));
            i = end;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let end = chars[i..]
                .iter()
                .position(|c| !(c.is_ascii_alphanumeric() || *c == '.' || *c == '_'))
                .map_or(chars.len(), |p| i + p);
            out.push((Tok::Atom, at));
            i = end;
        } else if (matches!(c, '=' | '~' | '<' | '>') && chars.get(i + 1) == Some(&'='))
            || (c == '.' && chars.get(i + 1) == Some(&'.'))
        {
            // `==`, `~=`, `<=`, `>=`, `..`: not a `=` or `.` that [`table`] looks for.
            out.push((Tok::Atom, at));
            i += 2;
        } else {
            out.push((Tok::Sym(c), at));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_read_comments_hints_groups_and_nested_tables() {
        let source = r#"--!strict
local Lamp = {}
-- not these: Lamp.other = { x = 1 }
Lamp.props = {
	-- @group Light
	lumens = 1800, -- at full brightness @range 0 5000 @unit lm
	color = vec3(1, 0.72, 0.42),
	-- how it moves
	-- in the wind
	sway = { speed = 2, -- turns a second @min 0
		axis = vec3.up },
	[1] = "skipped",
	-- @group Flame
	period = if true then 2.4 else 3, -- seconds a breath @range 0.3 8 0.1 @unit s
	mode = "calm", -- @enum calm | storm|  fog
	label = [[a long
string]], -- @hide
	name = "x", ["odd key"] = 3, target = "Courtyard director" -- who hears it @entity
}
function Lamp:start() end
return Lamp
"#;
        let n = notes(source, "props");
        let names: Vec<&str> = n.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "lumens", "color", "sway", "period", "mode", "label", "name", "target"
            ]
        );
        assert_eq!(n[0].doc, "at full brightness");
        assert_eq!(
            n[0].tags,
            [
                ("range".into(), "0 5000".into()),
                ("unit".into(), "lm".into())
            ]
        );
        assert_eq!(n[0].group.as_deref(), Some("Light"));
        assert_eq!(n[0].line, 6);
        assert_eq!(n[1].doc, "");
        assert_eq!(n[2].doc, "how it moves in the wind");
        assert_eq!(n[2].fields.len(), 2);
        assert_eq!(n[2].fields[0].doc, "turns a second");
        assert_eq!(n[2].fields[0].tags, [("min".into(), "0".into())]);
        assert_eq!(n[3].group.as_deref(), Some("Flame"));
        assert_eq!(n[3].tags[0], ("range".into(), "0.3 8 0.1".into()));
        assert_eq!(n[4].tags, [("enum".into(), "calm | storm|  fog".into())]);
        assert_eq!(n[5].tags, [("hide".into(), String::new())]);
        assert_eq!(n[5].line, 16);
        // Several on a line: the comment after the last one is its.
        assert_eq!(n[6].doc, "");
        assert_eq!(n[7].doc, "who hears it");
        assert_eq!(n[7].tags, [("entity".into(), String::new())]);
    }

    #[test]
    fn props_take_their_kinds_from_the_defaults_and_the_hints() {
        let lua = mlua::Lua::new();
        crate::math::install(&lua).unwrap();
        let source = r#"local C = {}
C.props = {
	lumens = 1800, -- @range 0 5000 @unit lm
	color = vec3(1, 0.5, 0.2),
	glow = vec3(1, 1, 1), -- @color
	offset = vec3(0, 1, 0),
	turn = quat.identity,
	swing = math.pi / 4, -- radians @angle
	count = 12, -- @int @min 1
	lit = true,
	mode = "calm", -- @enum calm|storm
	target = "Door", -- @entity
	title = "hello",
	-- @group Wave
	wave = { speed = 2, height = 0.5 },
	path = { 1, 2, 3 },
	built = 1, -- not in order below
}
C.props.extra = 5
return C
"#;
        let class: Table = lua.load(source).eval().unwrap();
        let defaults: Table = class.raw_get("props").unwrap();
        let props = props_from(&defaults, &notes(source, "props"));
        let get = |n: &str| props.iter().find(|p| p.name == n).unwrap();
        assert_eq!(
            props.last().unwrap().name,
            "extra",
            "props built in code come last"
        );
        assert_eq!(props[0].name, "lumens");
        let lumens = get("lumens");
        assert_eq!(lumens.kind, PropKind::Number);
        assert!(lumens.slider);
        assert_eq!((lumens.min, lumens.max), (Some(0.0), Some(5000.0)));
        assert_eq!(lumens.unit.as_deref(), Some("lm"));
        assert_eq!(lumens.clamp(9000.0), 5000.0);
        assert_eq!(get("color").kind, PropKind::Color);
        assert_eq!(get("color").default, serde_json::json!([1, 0.5, 0.2]));
        assert_eq!(get("glow").kind, PropKind::Color);
        assert_eq!(get("offset").kind, PropKind::Vec3);
        assert_eq!(get("turn").kind, PropKind::Rotation);
        assert_eq!(get("swing").kind, PropKind::Angle);
        assert_eq!(get("swing").doc, "radians");
        let count = get("count");
        assert_eq!(count.kind, PropKind::Integer);
        assert!(!count.slider);
        assert_eq!(count.clamp(0.4), 1.0);
        assert_eq!(count.clamp(7.6), 8.0);
        assert_eq!(get("lit").kind, PropKind::Bool);
        assert_eq!(
            get("mode").kind,
            PropKind::Choice(vec!["calm".into(), "storm".into()])
        );
        assert_eq!(get("target").kind, PropKind::Entity);
        assert_eq!(get("title").kind, PropKind::Text);
        let wave = get("wave");
        assert_eq!(wave.kind, PropKind::Table);
        assert_eq!(wave.group.as_deref(), Some("Wave"));
        assert_eq!(wave.fields.len(), 2);
        assert_eq!(get("path").kind, PropKind::List);
    }
}
