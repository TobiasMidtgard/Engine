//! Luau's own type checker on the scripts, for the editor: problems as you type, the type of
//! what is under the pointer, the members that fit after a `.` or `:`, and a call's parameters
//! while its arguments are typed (Round 10; research in engine-design/scripting-research.md §15).
//!
//! The checker is `luau-analysis`, Luau's analysis frontend (type checker, linter, autocomplete)
//! ported to Rust: it runs in this process beside the mlua VM without clashing with it, on every
//! platform, and checks a script in a few milliseconds. It reads the engine's API as the
//! definitions [`crate::definitions`] writes (the same file luau-lsp reads in VS Code).
//!
//! ## What `self` is
//!
//! Luau infers each method's `self` on its own, from what the method does with it, so on its
//! own it knows nothing of `self.entity` or `self.props` and reports nonsense on ordinary
//! scripts. The engine knows what a script's `self` is: the class's instance (its methods and
//! `props`, typed from the props table's defaults), the engine's part ([`SCRIPT_BASE`]: the
//! entity, `every`, `after`, `spawn`) and the fields its methods set. So before checking, a
//! script is shaped: `local Lamp = {}` gets a type `__Self` after it on the same line, and each
//! `function Lamp:update(dt)` becomes `function Lamp.update(self: __Self, dt)`. Nothing moves
//! to another line, and positions map back to the file as written. The fields' types come
//! from a first check: `self.home = self.entity.position` makes `home` a `Vec3`.
//!
//! [`Analyzer`] does the work; [`Checker`] runs one on a thread of its own for the editor.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use luau_analysis::autocomplete::AutocompleteEntryKind;
use luau_analysis::{
    DefinitionEnvironment, FileResolver, Frontend, FrontendOptions, ModuleName, NullConfigResolver,
    SourceCode, SourceCodeType, TypeCheckLimits,
};
use luau_common::BString;
use luau_syntax::allocator::AstArena;
use luau_syntax::ast::{Expression, ExpressionKind, IndexNameOp, Statement, StatementView};
use luau_syntax::ast_names::AstNameTable;
use luau_syntax::location::Position;
use luau_syntax::parser::{ParseOptions, parse};

use crate::runtime::normalize;

/// The type alias a class script's methods get for `self`.
const SELF_TYPE: &str = "__Self";

/// What the engine adds to every script's `self` (declared beside the definitions).
pub const SCRIPT_BASE: &str = "ScriptBase";

/// A range in a file: 0-based lines, columns in chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// Something the checker found.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub span: Span,
    pub message: String,
    /// An error (a type error, or a lint set to fail); else a warning.
    pub error: bool,
    /// `TypeError`, `SyntaxError` or the lint's name (`LocalUnused`, ...).
    pub code: String,
    /// In a visual script: the node the line was made from.
    pub node: Option<u32>,
}

/// One file's check.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub path: String,
    pub problems: Vec<Problem>,
    /// The class's own fields (set by its methods) and the types found for them.
    pub fields: Vec<(String, String)>,
    pub micros: u64,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.problems.iter().filter(|p| p.error).count()
    }
}

/// What the pointer is over.
#[derive(Debug, Clone, PartialEq)]
pub struct Hover {
    pub span: Span,
    /// `rate: number`.
    pub text: String,
    /// The definition's doc comment, if it has one.
    pub doc: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestKind {
    Field,
    Method,
    Function,
    Local,
    Keyword,
    Type,
    Module,
}

/// A member or name that fits where the caret is.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub name: String,
    pub kind: SuggestKind,
    pub ty: String,
    pub doc: String,
}

/// The call the caret is in.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// `quat.axis_angle`.
    pub name: String,
    /// `axis: Vec3`, `angle: number`.
    pub params: Vec<String>,
    pub ret: String,
    /// The parameter being typed.
    pub active: usize,
    pub doc: String,
}

// ----------------------------------------------------------------------------- shaping

/// A script as the checker sees it.
#[derive(Debug, Clone, Default)]
struct Shape {
    text: String,
    /// What was inserted, in the file's coordinates: (line, byte column, bytes).
    inserts: Vec<(u32, u32, u32)>,
    /// The class's name and its own fields' names (none: not a class script).
    class: Option<(String, Vec<String>)>,
}

impl Shape {
    /// A position in the file to one in the shaped text (same line).
    fn to_shaped(&self, line: u32, col: u32) -> u32 {
        col + self
            .inserts
            .iter()
            .filter(|(l, c, _)| *l == line && *c <= col)
            .map(|(_, _, n)| n)
            .sum::<u32>()
    }

    /// Back to the file; `None` inside inserted text.
    fn to_file(&self, line: u32, col: u32) -> Option<u32> {
        let mut shift = 0;
        for (l, c, n) in self.inserts.iter().filter(|(l, _, _)| *l == line) {
            let _ = l;
            let at = c + shift;
            if col >= at + n {
                shift += n;
            } else if col >= at {
                return None;
            }
        }
        Some(col - shift)
    }
}

/// A (line, byte column) in the source.
type At = (u32, u32);

/// The class a script returns: `local Lamp = {}` ... `return Lamp`, with methods or props.
struct ClassSyntax {
    name: String,
    /// Where `local Lamp = ...` ends (line, byte column).
    after_local: (u32, u32),
    /// `function Lamp:m(` methods: where `(` is and whether the list is empty;
    /// `function Lamp.m(self` methods: where `self` ends.
    colon_methods: Vec<(At, At, bool)>,
    dot_selfs: Vec<(u32, u32)>,
    /// Own fields set as `self.x = ...` anywhere in the file.
    fields: Vec<String>,
    /// `Class.signals = { name = "params" }`: each signal and its values as written.
    signals: Option<Vec<(String, String)>>,
}

fn name_of(local: &luau_syntax::ast::Local) -> String {
    String::from_utf8_lossy(local.name.bytes()).into_owned()
}

/// Finds the class in a parsed script.
fn class_syntax(source: &str) -> Option<ClassSyntax> {
    let arena = AstArena::new();
    let mut names = AstNameTable::new(&arena);
    let parsed = parse(source, &arena, &mut names, ParseOptions::default()).ok()?;
    let statements = parsed.root.as_slice();
    // `return Name` at the end, Name a local of this block.
    let returned = statements
        .last()
        .and_then(|s| s.as_return())
        .and_then(|r| {
            let [e] = r.expressions else { return None };
            match e.kind() {
                ExpressionKind::Local { local, .. } => Some(name_of(local)),
                _ => None,
            }
        })?;
    let mut after_local = None;
    let mut colon_methods = Vec::new();
    let mut dot_selfs = Vec::new();
    let mut has_members = false;
    let mut signals = None;
    for s in statements {
        match s.view() {
            StatementView::Local(l) => {
                if l.bindings.len() == 1
                    && name_of(l.bindings[0]) == returned
                    && l.values.len() == 1
                    && matches!(l.values[0].kind(), ExpressionKind::Table { .. })
                {
                    let end = s.location.end;
                    after_local = Some((end.line, end.column));
                }
            }
            StatementView::FunctionDeclaration(f) => {
                let ExpressionKind::IndexName { expr, op, .. } = f.name.kind() else {
                    continue;
                };
                let ExpressionKind::Local { local, .. } = expr.kind() else {
                    continue;
                };
                if name_of(local) != returned {
                    continue;
                }
                let Some(func) = f.function.as_function() else {
                    continue;
                };
                has_members = true;
                match op {
                    IndexNameOp::Colon => {
                        let Some(args) = func.arg_location else {
                            continue;
                        };
                        let open = (args.begin.line, args.begin.column);
                        let empty = func.args.is_empty() && !func.vararg;
                        let colon = f.name.location().end;
                        // The `:` sits right before the method's name.
                        colon_methods.push((open, (colon.line, colon.column), empty));
                    }
                    IndexNameOp::Dot => {
                        if let Some(first) = func.args.first()
                            && name_of(first) == "self"
                            && first.annotation.is_none()
                        {
                            let e = first.location.end;
                            dot_selfs.push((e.line, e.column));
                        }
                    }
                }
            }
            StatementView::Assign(a)
                if a.vars.iter().any(|v| {
                    matches!(v.kind(), ExpressionKind::IndexName { expr, .. }
                        if matches!(expr.kind(), ExpressionKind::Local { local, .. } if name_of(local) == returned))
                }) =>
            {
                has_members = true;
                for (var, value) in a.vars.iter().zip(a.values.iter()) {
                    if let ExpressionKind::IndexName { expr, index, .. } = var.kind()
                        && matches!(expr.kind(), ExpressionKind::Local { local, .. } if name_of(local) == returned)
                        && index.bytes() == b"signals"
                        && let ExpressionKind::Table { items } = value.kind()
                    {
                        signals = Some(table_strings(items));
                    }
                }
            }
            _ => {}
        }
    }
    let after_local = after_local?;
    if !has_members {
        return None;
    }
    let mut fields = Vec::new();
    let mut finder = SelfFields {
        out: &mut fields,
        types: None,
    };
    parsed.root.visit(&mut finder);
    Some(ClassSyntax {
        name: returned,
        after_local,
        colon_methods,
        dot_selfs,
        fields,
        signals,
    })
}

/// A table literal's `name = "text"` entries (other values give empty text).
fn table_strings(items: &[luau_syntax::ast::TableItem]) -> Vec<(String, String)> {
    items
        .iter()
        .filter_map(|item| {
            let luau_syntax::ast::TableItem::Record { key, value } = item else {
                return None;
            };
            let ExpressionKind::String { value: k, .. } = key.kind() else {
                return None;
            };
            let text = match value.kind() {
                ExpressionKind::String { value: v, .. } => {
                    String::from_utf8_lossy(v.as_bytes()).into_owned()
                }
                _ => String::new(),
            };
            Some((String::from_utf8_lossy(k.as_bytes()).into_owned(), text))
        })
        .collect()
}

/// `self:emit`'s type for a class's signals: one overload per signal, its name a string
/// singleton and its values typed as declared (`by: Entity`; a bare name is `any`).
fn emit_type(signals: Option<&[(String, String)]>) -> String {
    let Some(signals) = signals.filter(|s| !s.is_empty()) else {
        return "(self: any, signal: string, ...any) -> ()".into();
    };
    let overloads: Vec<String> = signals
        .iter()
        .map(|(name, params)| {
            let mut args = vec!["self: any".to_string(), format!("signal: \"{name}\"")];
            for p in split_top(params) {
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                match p.split_once(':') {
                    Some((n, t)) if plain_name(n.trim()) => {
                        args.push(format!("{}: {}", n.trim(), t.trim()));
                    }
                    _ if plain_name(p) => args.push(format!("{p}: any")),
                    _ => args.push("_: any".to_string()),
                }
            }
            format!("(({}) -> ())", args.join(", "))
        })
        .collect();
    overloads.join(" & ")
}

/// Collects `self.x = ...` and `self.x += ...` targets (names, in order of first use), and with
/// `types`, the right-hand sides' types from a checked module.
struct SelfFields<'a, 'm> {
    out: &'a mut Vec<String>,
    #[allow(clippy::type_complexity)]
    types: Option<(
        &'m luau_analysis::SemanticModel<'m>,
        &'a mut BTreeMap<String, Vec<String>>,
    )>,
}

fn self_field(e: Expression) -> Option<String> {
    let ExpressionKind::IndexName { expr, index, .. } = e.kind() else {
        return None;
    };
    let ExpressionKind::Local { local, .. } = expr.kind() else {
        return None;
    };
    (name_of(local) == "self").then(|| String::from_utf8_lossy(index.bytes()).into_owned())
}

impl<'ast> luau_syntax::ast::AstVisitor<'ast> for SelfFields<'_, '_> {
    fn visit_assign_statement(&mut self, statement: Statement<'ast>) -> bool {
        if let Some(a) = statement.as_assign() {
            for (k, v) in a.vars.iter().enumerate() {
                let Some(name) = self_field(*v) else { continue };
                if !self.out.contains(&name) {
                    self.out.push(name.clone());
                }
                if let Some((model, types)) = &mut self.types {
                    let ty = a
                        .values
                        .get(k)
                        .and_then(|value| {
                            model
                                .expression_type(*value)
                                .map(|t| t.to_string())
                                .or_else(|| {
                                    // A call's type is the pack it returns: its first value.
                                    model
                                        .expression_type_pack(*value)
                                        .map(|p| first_of_pack(&p.to_string()))
                                })
                        })
                        .unwrap_or_else(|| "any".into());
                    types.entry(name).or_default().push(ty);
                }
            }
        }
        true
    }

    fn visit_compound_assign_statement(&mut self, statement: Statement<'ast>) -> bool {
        if let Some(a) = statement.as_compound_assign()
            && let Some(name) = self_field(a.var)
            && !self.out.contains(&name)
        {
            self.out.push(name);
        }
        true
    }
}

/// An error about a member (`Key 'lumen' not found in ...`) is placed by Luau on the whole
/// `p.lumen`: it is the member that is wrong, so the mark goes under it alone.
fn member_span(span: Span, text: &str, message: &str) -> Span {
    let Some(name) = message
        .split('\'')
        .nth(1)
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'))
    else {
        return span;
    };
    let Some(line) = text.split('\n').nth(span.line as usize) else {
        return span;
    };
    let chars: Vec<char> = line.chars().collect();
    let (a, b) = (span.col as usize, span.end_col as usize);
    if span.end_line != span.line || b > chars.len() || a >= b {
        return span;
    }
    let inside: String = chars[a..b].iter().collect();
    let n = name.chars().count();
    if inside.len() > name.len() && inside.ends_with(name) {
        let before = chars[b - n - 1];
        if before == '.' || before == ':' {
            return Span {
                col: (b - n) as u32,
                ..span
            };
        }
    }
    span
}

/// The first value's type in a type pack as Luau writes it: `Entity` for `Entity` or
/// `(Entity, number)`, `any` for `...any`.
fn first_of_pack(pack: &str) -> String {
    let p = pack.trim();
    let inner = p
        .strip_prefix('(')
        .and_then(|q| q.strip_suffix(')'))
        .unwrap_or(p);
    let first = split_top(inner).into_iter().next().unwrap_or_default();
    let first = first.trim();
    if first.is_empty() || first.starts_with("...") {
        "any".into()
    } else {
        first.to_string()
    }
}

/// A field's type from the types assigned to it: one type, `T?` with a nil among them, or
/// `any` when one of them is still unknown to the checker.
fn field_type(types: &[String]) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    let mut optional = false;
    for t in types {
        let t = t.trim();
        if t == "nil" {
            optional = true;
            continue;
        }
        // `any` fits any declared type, so it doesn't widen one.
        if t == "any" {
            continue;
        }
        // `self.list = {}` is filled later: its literal's type says nothing yet.
        let vague = t.is_empty()
            || t.chars().all(|c| matches!(c, '{' | '}' | ' '))
            || t.len() > 160
            || t.contains("unknown")
            || t.contains("error")
            || t.contains("never")
            || t.contains('<')
            || t.contains(SELF_TYPE)
            || t.split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|w| w.len() == 1 && w.chars().all(|c| c.is_ascii_lowercase()));
        if vague {
            return "any".into();
        }
        if !kinds.contains(&t) {
            kinds.push(t);
        }
    }
    match kinds.as_slice() {
        [] => "any".into(),
        [one] if optional && !one.ends_with('?') => format!("({one})?"),
        [one] => (*one).to_string(),
        many => {
            let u = many.join(" | ");
            if optional { format!("({u})?") } else { u }
        }
    }
}

/// A Luau field name (else it is quoted in the type).
fn plain_name(n: &str) -> bool {
    n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !n.starts_with(|c: char| c.is_ascii_digit())
}

/// Shapes a script for the checker; `types` are the fields' types (else `any`).
fn shape(source: &str, types: Option<&BTreeMap<String, String>>) -> Shape {
    let Some(class) = class_syntax(source) else {
        return Shape {
            text: source.to_string(),
            ..Shape::default()
        };
    };
    let fields: Vec<String> = class
        .fields
        .iter()
        .map(|f| {
            let ty = types
                .and_then(|t| t.get(f))
                .cloned()
                .unwrap_or_else(|| "any".into());
            if plain_name(f) {
                format!("{f}: {ty}")
            } else {
                format!("[\"{f}\"]: {ty}")
            }
        })
        .collect();
    let mut fields = fields;
    if !class.fields.iter().any(|f| f == "emit") {
        fields.push(format!("emit: {}", emit_type(class.signals.as_deref())));
    }
    let alias = format!(
        "; type {SELF_TYPE} = {SCRIPT_BASE} & typeof({}) & {{ {} }}",
        class.name,
        fields.join(", ")
    );
    // (line, byte column, text) to insert, and the `:`s to turn into `.`s.
    let mut edits: Vec<(u32, u32, String)> =
        vec![(class.after_local.0, class.after_local.1, alias)];
    let mut colons = Vec::new();
    for (open, colon, empty) in &class.colon_methods {
        let insert = if *empty {
            format!("self: {SELF_TYPE}")
        } else {
            format!("self: {SELF_TYPE}, ")
        };
        edits.push((open.0, open.1 + 1, insert));
        colons.push(*colon);
    }
    for at in &class.dot_selfs {
        edits.push((at.0, at.1, format!(": {SELF_TYPE}")));
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut text = String::with_capacity(source.len() + 512);
    let mut inserts = Vec::new();
    for (k, line) in source.split('\n').enumerate() {
        if k > 0 {
            text.push('\n');
        }
        let k = k as u32;
        let mut line = line.to_string();
        // `Lamp:update` to `Lamp.update`: the name expression ends at the method's name, so
        // its `:` is the last one before that name.
        for (l, end) in &colons {
            if *l == k
                && let Some(c) = line[..(*end as usize).min(line.len())].rfind(':')
            {
                line.replace_range(c..c + 1, ".");
            }
        }
        let mut done = 0;
        for (l, c, insert) in edits.iter().filter(|e| e.0 == k) {
            let at = (*c as usize).min(line.len());
            text.push_str(&line[done..at]);
            text.push_str(insert);
            inserts.push((*l, *c, insert.len() as u32));
            done = at;
        }
        text.push_str(&line[done..]);
    }
    Shape {
        text,
        inserts,
        class: Some((class.name, class.fields)),
    }
}

// ----------------------------------------------------------------------------- positions

/// A char column on a line of `text` to its byte column.
fn byte_col(text: &str, line: u32, col: u32) -> u32 {
    let l = text.split('\n').nth(line as usize).unwrap_or("");
    l.char_indices()
        .nth(col as usize)
        .map_or(l.len(), |(b, _)| b) as u32
}

/// A byte column on a line of `text` to its char column.
fn char_col(text: &str, line: u32, col: u32) -> u32 {
    let l = text.split('\n').nth(line as usize).unwrap_or("");
    let b = (col as usize).min(l.len());
    l.get(..b).map_or(b, |s| s.chars().count()) as u32
}

// ----------------------------------------------------------------------------- the files

/// The scripts the checker reads: the editor's open texts first, then the files on disk.
struct Sources {
    root: PathBuf,
    /// Unsaved texts by path.
    open: HashMap<String, String>,
    /// What the checker saw for each path, shaped.
    shaped: HashMap<String, Shape>,
    /// The fields' types found for each class script (from its last check).
    fields: HashMap<String, BTreeMap<String, String>>,
}

impl Sources {
    /// The file as written (a visual script's graph).
    fn raw(&self, path: &str) -> Option<String> {
        match self.open.get(path) {
            Some(t) => Some(t.clone()),
            None => std::fs::read_to_string(self.root.join(path)).ok(),
        }
    }

    /// The Luau to check: the file, or the Luau a visual script compiles to.
    fn text(&self, path: &str) -> Option<String> {
        let text = self.raw(path)?;
        if path.ends_with(".graph") {
            return crate::runtime::runnable(path, text).ok();
        }
        Some(text)
    }

    /// A visual script's node for each line of its Luau.
    fn graph_lines(&self, path: &str) -> Vec<Option<u32>> {
        if !path.ends_with(".graph") {
            return Vec::new();
        }
        self.raw(path)
            .and_then(|t| crate::graph::compile_text(&t, path).ok())
            .map(|c| c.lines)
            .unwrap_or_default()
    }
}

impl FileResolver for Sources {
    fn read_source(&mut self, name: &ModuleName) -> Option<SourceCode> {
        let path = String::from_utf8_lossy(name).into_owned();
        let text = self.text(&path)?;
        let shaped = shape(&text, self.fields.get(&path));
        let source = BString::from(shaped.text.as_str());
        self.shaped.insert(path, shaped);
        Some(SourceCode {
            source,
            kind: SourceCodeType::Module,
        })
    }

    fn resolve_module(
        &mut self,
        context: Option<&luau_analysis::frontend::resolver::ModuleInfo>,
        expr: Expression<'_>,
        _limits: &TypeCheckLimits,
    ) -> Option<luau_analysis::frontend::resolver::ModuleInfo> {
        // `require("./lib/wave")` (next to the requiring file) or `require("lib/tween")`
        // (from the scripts folder), as the runtime's `require`.
        let s = expr.as_string()?;
        let name = String::from_utf8_lossy(s.value.as_bytes()).into_owned();
        let path = if let Some(rel) = name.strip_prefix("./") {
            let from = context.map(|c| String::from_utf8_lossy(&c.name).into_owned());
            let dir = from
                .as_deref()
                .and_then(|f| f.rsplit_once('/').map(|(d, _)| d.to_string()))
                .unwrap_or_default();
            normalize(&if dir.is_empty() {
                rel.to_string()
            } else {
                format!("{dir}/{rel}")
            })
        } else {
            normalize(&name)
        };
        Some(luau_analysis::frontend::resolver::ModuleInfo {
            name: BString::from(path),
            optional: false,
        })
    }
}

// ----------------------------------------------------------------------------- docs

/// The doc comments and declared signatures in the definitions, by `Owner.member` (`Entity.set`,
/// `world.find`, `fire.ignite`).
#[derive(Default)]
struct Docs {
    doc: HashMap<String, String>,
    /// The member's declared type, with parameter names: `(name: string) -> Entity?`.
    sig: HashMap<String, String>,
}

impl Docs {
    fn read(defs: &str) -> Self {
        let mut docs = Docs::default();
        let mut owners: Vec<String> = Vec::new();
        let mut pending: Vec<String> = Vec::new();
        for raw in defs.lines() {
            let line = raw.trim();
            if let Some(d) = line.strip_prefix("---") {
                pending.push(d.trim().to_string());
                continue;
            }
            let take = |pending: &mut Vec<String>| std::mem::take(pending).join(" ");
            if let Some(rest) = line.strip_prefix("declare extern type ") {
                owners = vec![rest.trim_end_matches(" with").trim().to_string()];
                pending.clear();
                continue;
            }
            if let Some(rest) = line.strip_prefix("declare ") {
                if let Some((name, ty)) = rest.split_once(':') {
                    let name = name.trim().trim_start_matches("function ").to_string();
                    owners = vec![name];
                    if !ty.trim().ends_with('{') {
                        owners.clear();
                    }
                }
                pending.clear();
                continue;
            }
            if line == "end" || line == "}" || line == "}," {
                if line == "end" {
                    owners.clear();
                } else {
                    owners.pop();
                }
                pending.clear();
                continue;
            }
            let Some(owner) = owners.last().cloned() else {
                pending.clear();
                continue;
            };
            let prefix = owners.join(".");
            let _ = owner;
            if let Some(rest) = line.strip_prefix("function ") {
                // `function set(self, component: string, patch: any): ()`
                if let Some((name, params)) = rest.split_once('(') {
                    let key = format!("{prefix}.{}", name.trim());
                    let (args, ret) = params.rsplit_once("):").unwrap_or((params, " ()"));
                    let args: Vec<&str> = split_top(args.trim_end_matches(')'))
                        .into_iter()
                        .filter(|a| a.trim() != "self")
                        .collect();
                    docs.sig.insert(
                        key.clone(),
                        format!("({}) -> {}", args.join(", "), ret.trim()),
                    );
                    let d = take(&mut pending);
                    if !d.is_empty() {
                        docs.doc.insert(key, d);
                    }
                }
                continue;
            }
            if let Some((name, ty)) = line.split_once(':') {
                let name = name.trim().trim_start_matches("read ").to_string();
                if !plain_name(&name) {
                    pending.clear();
                    continue;
                }
                let key = format!("{prefix}.{name}");
                let ty = ty.trim().trim_end_matches(',').trim();
                if ty.ends_with('{') {
                    owners.push(name);
                } else {
                    docs.sig.insert(key.clone(), ty.to_string());
                }
                let d = take(&mut pending);
                if !d.is_empty() {
                    docs.doc.insert(key, d);
                }
                continue;
            }
            pending.clear();
        }
        docs
    }
}

/// Splits on commas outside brackets.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut last = ' ';
    for (i, c) in s.char_indices() {
        let arrow = last == '-';
        last = c;
        match c {
            '(' | '{' | '[' | '<' => depth += 1,
            '>' if arrow => {}
            ')' | '}' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if !s[start..].trim().is_empty() {
        out.push(s[start..].trim());
    }
    out
}

// ----------------------------------------------------------------------------- the analyzer

/// Luau's checker over the scripts folder.
pub struct Analyzer {
    frontend: Frontend<Sources, NullConfigResolver>,
    docs: Docs,
    /// The text last checked for each path (what hover and completion read).
    checked: HashMap<String, String>,
    reports: HashMap<String, Report>,
}

impl Analyzer {
    /// A checker for the scripts under `root`, knowing the engine from `definitions` (see
    /// [`crate::definitions`]).
    pub fn new(root: PathBuf, definitions: &str) -> Result<Self, String> {
        let sources = Sources {
            root,
            open: HashMap::new(),
            shaped: HashMap::new(),
            fields: HashMap::new(),
        };
        let mut frontend = Frontend::new(
            sources,
            NullConfigResolver::default(),
            FrontendOptions {
                retain_full_type_graphs: true,
                run_lint_checks: true,
                module_time_limit_sec: Some(2.0),
                ..FrontendOptions::default()
            },
        );
        for env in [
            DefinitionEnvironment::Globals,
            DefinitionEnvironment::Autocomplete,
        ] {
            frontend
                .register_builtin_globals(env)
                .map_err(|e| format!("Luau's builtins: {e:?}"))?;
            let scope = frontend.global_scope(env);
            let loaded = frontend
                .load_definition_file(scope, definitions.as_bytes(), "@engine", false)
                .map_err(|e| format!("engine definitions: {e:?}"))?;
            if !loaded.success {
                let first = loaded
                    .source_module
                    .parse_errors
                    .first()
                    .map(|e| format!("{e:?}"))
                    .unwrap_or_else(|| "type errors".into());
                return Err(format!("engine definitions don't load: {first}"));
            }
        }
        Ok(Self {
            frontend,
            docs: Docs::read(definitions),
            checked: HashMap::new(),
            reports: HashMap::new(),
        })
    }

    /// A checker for this world's engine and scripts folder.
    pub fn for_world(world: &bevy_ecs::world::World) -> Result<Self, String> {
        Self::new(
            crate::scripts_root(world),
            &crate::types::definitions(world),
        )
    }

    /// The text to check for `path`: the editor's unsaved text, or the file (`None`).
    pub fn set_text(&mut self, path: &str, text: Option<String>) {
        let sources = self.frontend.file_resolver_mut();
        let changed = match text {
            Some(t) => sources.open.insert(path.to_string(), t.clone()) != Some(t),
            None => sources.open.remove(path).is_some(),
        };
        if changed {
            self.frontend.mark_dirty(&BString::from(path), None);
        }
    }

    /// The file changed on disk.
    pub fn touched(&mut self, path: &str) {
        self.frontend.mark_dirty(&BString::from(path), None);
        self.checked.remove(path);
    }

    fn text(&self, path: &str) -> Option<String> {
        self.frontend.file_resolver().text(path)
    }

    /// Checks one file (its unsaved text if set).
    pub fn check(&mut self, path: &str) -> Report {
        let start = Instant::now();
        let Some(text) = self.text(path) else {
            return Report {
                path: path.into(),
                problems: vec![Problem {
                    span: Span::default(),
                    message: format!("{path} can't be read"),
                    error: true,
                    code: "SyntaxError".into(),
                    node: None,
                }],
                ..Report::default()
            };
        };
        if self.checked.get(path) == Some(&text)
            && let Some(r) = self.reports.get(path)
        {
            return r.clone();
        }
        let name = BString::from(path);
        // First the fields as `any`, to find their types from what is assigned to them.
        self.frontend.file_resolver_mut().fields.remove(path);
        self.frontend.mark_dirty(&name, None);
        let first = self.frontend.check(&name, None);
        let mut fields = Vec::new();
        let is_class = self
            .frontend
            .file_resolver()
            .shaped
            .get(path)
            .is_some_and(|s| s.class.as_ref().is_some_and(|c| !c.1.is_empty()));
        let result = if is_class && first.is_ok() {
            let mut assigned: BTreeMap<String, Vec<String>> = BTreeMap::new();
            if let Some(model) = self.frontend.semantic_model(&name)
                && let Some(root) = model.module().root()
            {
                let mut names = Vec::new();
                let mut finder = SelfFields {
                    out: &mut names,
                    types: Some((&model, &mut assigned)),
                };
                root.visit(&mut finder);
            }
            let types: BTreeMap<String, String> = assigned
                .iter()
                .map(|(k, v)| (k.clone(), field_type(v)))
                .collect();
            fields = types.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            self.frontend
                .file_resolver_mut()
                .fields
                .insert(path.to_string(), types);
            self.frontend.mark_dirty(&name, None);
            self.frontend.check(&name, None)
        } else {
            first
        };
        let shape = self
            .frontend
            .file_resolver()
            .shaped
            .get(path)
            .cloned()
            .unwrap_or_default();
        let mut problems = Vec::new();
        match result {
            Err(e) => problems.push(Problem {
                span: Span::default(),
                message: format!("the type checker stopped: {e:?}"),
                error: false,
                code: "Checker".into(),
                node: None,
            }),
            Ok(result) => {
                let model = self.frontend.semantic_model(&name);
                for e in result.errors.iter().filter(|e| e.module_name == name) {
                    let message = match &model {
                        Some(m) => m.error_message(e).to_string(),
                        None => format!("{:?}", e.data),
                    };
                    let Some(span) = file_span(&shape, &text, e.location) else {
                        continue;
                    };
                    let span = member_span(span, &text, &message);
                    // The kind of error, as Luau names it (`UnknownProperty`, `TypeMismatch`).
                    let data = format!("{:?}", e.data);
                    let code = data
                        .split(|c: char| !c.is_alphanumeric())
                        .next()
                        .filter(|k| !k.is_empty())
                        .unwrap_or("TypeError")
                        .to_string();
                    problems.push(Problem {
                        span,
                        message: tidy(&message, shape.class.as_ref().map(|c| c.0.as_str())),
                        error: true,
                        code,
                        node: None,
                    });
                }
                for (w, error) in result
                    .lint_result
                    .errors
                    .iter()
                    .map(|w| (w, true))
                    .chain(result.lint_result.warnings.iter().map(|w| (w, false)))
                {
                    let Some(span) = file_span(&shape, &text, w.location) else {
                        continue;
                    };
                    problems.push(Problem {
                        span,
                        message: w.text.to_string(),
                        error,
                        code: format!("{:?}", w.code),
                        node: None,
                    });
                }
            }
        }
        let nodes = self.frontend.file_resolver().graph_lines(path);
        for p in &mut problems {
            p.node = nodes.get(p.span.line as usize).copied().flatten();
        }
        problems.sort_by_key(|p| (p.span.line, p.span.col, !p.error));
        problems.dedup_by(|a, b| a.span == b.span && a.message == b.message);
        let report = Report {
            path: path.into(),
            problems,
            fields,
            micros: start.elapsed().as_micros() as u64,
        };
        self.checked.insert(path.to_string(), text);
        self.reports.insert(path.to_string(), report.clone());
        report
    }

    /// Checks `path` if its text changed since; then the shape it was checked with.
    fn ready(&mut self, path: &str) -> Option<(Shape, String)> {
        self.check(path);
        let text = self.checked.get(path)?.clone();
        let shape = self.frontend.file_resolver().shaped.get(path)?.clone();
        Some((shape, text))
    }

    /// The type of what is at `line`, `col` (0-based, chars), and its docs.
    pub fn hover(&mut self, path: &str, line: u32, col: u32) -> Option<Hover> {
        let (shape, text) = self.ready(path)?;
        let (word, start, end, chain) = word_at(&text, line, col)?;
        let name = BString::from(path);
        let model = self.frontend.semantic_model(&name)?;
        let at = |c: u32| Position {
            line,
            column: shape.to_shaped(line, byte_col(&text, line, c)),
        };
        let ty = model.type_at(at(start)).map(|t| t.to_string())?;
        let ty = tidy(&ty, shape.class.as_ref().map(|c| c.0.as_str()));
        // The owner's docs: `world.find` by its name, `self.entity.position` by `Entity`'s.
        let mut doc = String::new();
        if !chain.is_empty() {
            doc = self
                .docs
                .doc
                .get(&format!("{chain}.{word}"))
                .cloned()
                .unwrap_or_default();
            if doc.is_empty()
                && start >= 2
                && let Some(owner) = model.type_at(at(start - 2)).map(|t| t.to_string())
            {
                let owner = owner.trim_end_matches('?');
                doc = self
                    .docs
                    .doc
                    .get(&format!("{owner}.{word}"))
                    .cloned()
                    .unwrap_or_default();
            }
        }
        Some(Hover {
            span: Span {
                line,
                col: start,
                end_line: line,
                end_col: end,
            },
            text: format!("{word}: {ty}"),
            doc,
        })
    }

    /// What fits at `line`, `col`: members after `.` and `:`, else names in scope.
    pub fn complete(&mut self, path: &str, line: u32, col: u32) -> Vec<Suggestion> {
        let Some((shape, text)) = self.ready(path) else {
            return Vec::new();
        };
        let pos = Position {
            line,
            column: shape.to_shaped(line, byte_col(&text, line, col)),
        };
        let name = BString::from(path);
        let mut strings = |_: &luau_common::BStr,
                           _: Option<&luau_analysis::types::ExternType>,
                           _: Option<&luau_common::BStr>| None;
        let result = self.frontend.autocomplete(name.as_ref(), pos, &mut strings);
        let Some(model) = result.semantic_model(&self.frontend) else {
            return Vec::new();
        };
        // `self.entity:` and such: the docs owner, by name or by type.
        let before: String = text
            .split('\n')
            .nth(line as usize)
            .unwrap_or("")
            .chars()
            .take(col as usize)
            .collect();
        let chain = chain_before(&before);
        let owner_type = chain_end(&before).and_then(|c| {
            model
                .type_at(Position {
                    line,
                    column: shape.to_shaped(line, byte_col(&text, line, c)),
                })
                .map(|t| t.to_string().trim_end_matches('?').to_string())
        });
        let colon = before
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
            .ends_with(':');
        let class = shape.class.as_ref().map(|c| c.0.clone());
        let mut out: Vec<Suggestion> = result
            .entries
            .iter()
            .filter(|(n, e)| {
                !e.wrong_index_type && !n.starts_with(b"__") && n.as_slice() != SELF_TYPE.as_bytes()
            })
            .map(|(n, e)| {
                let name = String::from_utf8_lossy(n).into_owned();
                let ty =
                    e.ty.and_then(|t| model.type_ref(t))
                        .map(|t| tidy(&t.to_string(), class.as_deref()))
                        .unwrap_or_default();
                let function = ty.starts_with('(') && ty.contains("->");
                let kind = match e.kind {
                    AutocompleteEntryKind::Property if function && colon => SuggestKind::Method,
                    AutocompleteEntryKind::Property if function => SuggestKind::Function,
                    AutocompleteEntryKind::Property => SuggestKind::Field,
                    AutocompleteEntryKind::Binding if function => SuggestKind::Function,
                    AutocompleteEntryKind::Binding => SuggestKind::Local,
                    AutocompleteEntryKind::Keyword => SuggestKind::Keyword,
                    AutocompleteEntryKind::Type => SuggestKind::Type,
                    _ => SuggestKind::Module,
                };
                let doc = [chain.clone(), owner_type.clone().unwrap_or_default()]
                    .iter()
                    .filter(|o| !o.is_empty())
                    .find_map(|o| self.docs.doc.get(&format!("{o}.{name}")).cloned())
                    .unwrap_or_default();
                Suggestion {
                    name,
                    kind,
                    ty,
                    doc,
                }
            })
            .filter(|s| !colon || s.kind == SuggestKind::Method)
            .collect();
        out.sort_by(|a, b| {
            (a.kind == SuggestKind::Keyword)
                .cmp(&(b.kind == SuggestKind::Keyword))
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    /// The call the caret is in, with its parameters.
    pub fn signature(&mut self, path: &str, line: u32, col: u32) -> Option<Signature> {
        let (shape, text) = self.ready(path)?;
        let (callee_line, callee_end, active) = open_call(&text, line, col)?;
        let line_text = text.split('\n').nth(callee_line as usize).unwrap_or("");
        let before: String = line_text.chars().take(callee_end as usize).collect();
        let tail = before.trim_end();
        let name_start = tail
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == ':')
            .last()
            .map(|(i, _)| tail[..i].chars().count())?;
        let full: String = tail.chars().skip(name_start).collect();
        if full.is_empty() {
            return None;
        }
        let last = tail.chars().count() as u32 - 1;
        let name = BString::from(path);
        let model = self.frontend.semantic_model(&name)?;
        let ty = model
            .type_at(Position {
                line: callee_line,
                column: shape.to_shaped(callee_line, byte_col(&text, callee_line, last)),
            })
            .map(|t| t.to_string())
            .unwrap_or_default();
        let method = full.contains(':');
        let word = full.rsplit(['.', ':']).next().unwrap_or(&full).to_string();
        let owner = full[..full.len() - word.len()]
            .trim_end_matches(['.', ':'])
            .to_string();
        let owner_type = if owner.is_empty() || full.len() < word.len() + 1 {
            None
        } else {
            let c = name_start as u32 + owner.chars().count() as u32 - 1;
            model
                .type_at(Position {
                    line: callee_line,
                    column: shape.to_shaped(callee_line, byte_col(&text, callee_line, c)),
                })
                .map(|t| t.to_string().trim_end_matches('?').to_string())
        };
        let keys: Vec<String> = [Some(owner.clone()), owner_type]
            .into_iter()
            .flatten()
            .filter(|o| !o.is_empty())
            .map(|o| format!("{o}.{word}"))
            .collect();
        let declared = keys.iter().find_map(|k| self.docs.sig.get(k).cloned());
        let doc = keys
            .iter()
            .find_map(|k| self.docs.doc.get(k).cloned())
            .unwrap_or_default();
        let sig = declared.unwrap_or(ty);
        let class = shape.class.as_ref().map(|c| c.0.as_str());
        let sig = tidy(&sig, class);
        let (params, ret) = split_function(&sig)?;
        let mut params: Vec<String> = params.into_iter().map(str::to_string).collect();
        // A method's type has `self` first; the call doesn't pass it.
        if method
            && params
                .first()
                .is_some_and(|p| p.starts_with("self") || !p.contains(':'))
            && !self.docs.sig.keys().any(|k| keys.contains(k))
        {
            params.remove(0);
        }
        Some(Signature {
            name: full,
            params,
            ret: ret.to_string(),
            active,
            doc,
        })
    }

    /// Where the name at `line`, `col` is declared: (path, line, col).
    pub fn definition(&mut self, path: &str, line: u32, col: u32) -> Option<(String, u32, u32)> {
        let (shape, text) = self.ready(path)?;
        let (_, start, _, _) = word_at(&text, line, col)?;
        let name = BString::from(path);
        let model = self.frontend.semantic_model(&name)?;
        let b = model.binding_at(Position {
            line,
            column: shape.to_shaped(line, byte_col(&text, line, start)),
        })?;
        let span = file_span(&shape, &text, b.location)?;
        Some((path.to_string(), span.line, span.col))
    }
}

/// A checker location back in the file, in chars; `None` inside inserted text.
fn file_span(shape: &Shape, text: &str, at: luau_syntax::location::Location) -> Option<Span> {
    let begin = shape.to_file(at.begin.line, at.begin.column)?;
    let end = shape
        .to_file(at.end.line, at.end.column)
        .unwrap_or(begin + 1);
    Some(Span {
        line: at.begin.line,
        col: char_col(text, at.begin.line, begin),
        end_line: at.end.line,
        end_col: char_col(text, at.end.line, end),
    })
}

/// A message in the script's words: `__Self` is `self`.
fn tidy(message: &str, class: Option<&str>) -> String {
    let mut m = message.replace(SELF_TYPE, "self");
    if let Some(c) = class {
        m = m.replace(&format!("typeof({c})"), c);
    }
    m.replace(" *TRUNCATED*", "…")
}

/// The word under `line`, `col`: (word, start, end, the dotted chain before it).
fn word_at(text: &str, line: u32, col: u32) -> Option<(String, u32, u32, String)> {
    let chars: Vec<char> = text.split('\n').nth(line as usize)?.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut a = (col as usize).min(chars.len());
    if a == chars.len() || !is_word(chars[a]) {
        if a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        } else {
            return None;
        }
    }
    let mut start = a;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = a;
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    if chars[start].is_ascii_digit() {
        return None;
    }
    let before: String = chars[..start].iter().collect();
    let chain = chain_before(&before);
    Some((
        chars[start..end].iter().collect(),
        start as u32,
        end as u32,
        chain,
    ))
}

/// `self.entity` in `self.entity.posi` (the text up to the word being typed or pointed at).
fn chain_before(before: &str) -> String {
    let t = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
    let Some(t) = t.strip_suffix(['.', ':']) else {
        return String::new();
    };
    let chain: String = t
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    chain.trim_start_matches('.').to_string()
}

/// The char column of the last char of the expression before a trailing `.`/`:`.
fn chain_end(before: &str) -> Option<u32> {
    let t = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
    let t = t.strip_suffix(['.', ':'])?;
    let n = t.chars().count();
    (n > 0).then(|| n as u32 - 1)
}

/// The innermost call whose `(` is still open before the caret: (line, char column of the `(`,
/// the argument the caret is in).
fn open_call(text: &str, line: u32, col: u32) -> Option<(u32, u32, usize)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut depth = 0i32;
    let mut commas = 0usize;
    // Back through this line and up to 8 before it.
    for l in (line.saturating_sub(8)..=line).rev() {
        let chars: Vec<char> = lines.get(l as usize)?.chars().collect();
        let upto = if l == line {
            (col as usize).min(chars.len())
        } else {
            chars.len()
        };
        // Skip strings and comments on the line (a rough pass, for typing in arguments).
        let mut code = vec![true; upto];
        let mut quote = None;
        let mut i = 0;
        while i < upto {
            let c = chars[i];
            if let Some(q) = quote {
                code[i] = false;
                if c == '\\' {
                    i += 1;
                    if i < upto {
                        code[i] = false;
                    }
                } else if c == q {
                    quote = None;
                }
            } else if c == '"' || c == '\'' || c == '`' {
                quote = Some(c);
                code[i] = false;
            } else if c == '-' && chars.get(i + 1) == Some(&'-') {
                for x in code.iter_mut().skip(i) {
                    *x = false;
                }
                break;
            }
            i += 1;
        }
        for i in (0..upto).rev() {
            if !code[i] {
                continue;
            }
            match chars[i] {
                ')' | '}' | ']' => depth += 1,
                '{' | '[' => {
                    if depth == 0 {
                        // Inside a table constructor: not in the call's own arguments.
                        return None;
                    }
                    depth -= 1;
                }
                '(' => {
                    if depth == 0 {
                        return Some((l, i as u32, commas));
                    }
                    depth -= 1;
                }
                ',' if depth == 0 => commas += 1,
                _ => {}
            }
        }
    }
    None
}

/// `(a: T, b: U) -> R` to its parameters and result.
fn split_function(sig: &str) -> Option<(Vec<&str>, &str)> {
    let sig = sig.trim();
    let sig = sig.strip_prefix('(')?;
    let mut depth = 1i32;
    for (i, c) in sig.char_indices() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    let params = split_top(&sig[..i]);
                    let ret = sig[i + 1..].trim().trim_start_matches("->").trim();
                    return Some((params, ret));
                }
            }
            _ => {}
        }
    }
    None
}

// ----------------------------------------------------------------------------- the thread

/// What to ask the checker.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Check,
    Hover {
        line: u32,
        col: u32,
    },
    Complete {
        line: u32,
        col: u32,
    },
    Signature {
        line: u32,
        col: u32,
    },
    Definition {
        line: u32,
        col: u32,
    },
    /// The file changed on disk.
    Touched,
    /// The editor closed the file: check it as it is on disk again.
    Closed,
}

pub struct Job {
    /// Handed back with the answer.
    pub ticket: u64,
    pub path: String,
    /// The editor's text for `path` (`None`: the file on disk).
    pub text: Option<String>,
    pub ask: Ask,
}

#[derive(Debug, Clone)]
pub enum Reply {
    Report(Report),
    Hover(Option<Hover>),
    Complete(Vec<Suggestion>),
    Signature(Option<Signature>),
    Definition(Option<(String, u32, u32)>),
    /// The checker couldn't start (the definitions don't load).
    Failed(String),
}

pub struct Answer {
    pub ticket: u64,
    pub path: String,
    pub reply: Reply,
}

/// An [`Analyzer`] on a thread of its own: jobs in, answers out, never blocking the caller.
pub struct Checker {
    jobs: Sender<Job>,
    /// In a mutex only so the checker can sit in an ECS resource (which must be `Sync`).
    answers: Mutex<Receiver<Answer>>,
}

impl Checker {
    /// A checker for this world's engine (its API registry) and scripts folder.
    pub fn for_world(world: &bevy_ecs::world::World) -> Self {
        Self::spawn(crate::scripts_root(world), crate::types::definitions(world))
    }

    pub fn spawn(root: PathBuf, definitions: String) -> Self {
        let (jobs, inbox) = channel::<Job>();
        let (outbox, answers) = channel::<Answer>();
        let _ = std::thread::Builder::new()
            .name("luau checker".into())
            // Luau's checker recurses deeply on nested code.
            .stack_size(64 << 20)
            .spawn(move || serve(root, definitions, inbox, outbox));
        Self {
            jobs,
            answers: Mutex::new(answers),
        }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// The answers so far.
    pub fn answers(&self) -> Vec<Answer> {
        self.answers
            .lock()
            .map(|a| a.try_iter().collect())
            .unwrap_or_default()
    }
}

fn serve(root: PathBuf, definitions: String, inbox: Receiver<Job>, outbox: Sender<Answer>) {
    let start = |root: &Path| Analyzer::new(root.to_path_buf(), &definitions);
    let mut analyzer = match start(&root) {
        Ok(a) => Some(a),
        Err(e) => {
            let _ = outbox.send(Answer {
                ticket: 0,
                path: String::new(),
                reply: Reply::Failed(e),
            });
            None
        }
    };
    while let Ok(first) = inbox.recv() {
        // Everything waiting: only the newest check of each file is worth doing.
        let mut jobs = vec![first];
        jobs.extend(inbox.try_iter());
        let mut last_check: HashMap<&str, usize> = HashMap::new();
        for (k, j) in jobs.iter().enumerate() {
            if j.ask == Ask::Check {
                last_check.insert(j.path.as_str(), k);
            }
        }
        let keep: Vec<bool> = jobs
            .iter()
            .enumerate()
            .map(|(k, j)| j.ask != Ask::Check || last_check.get(j.path.as_str()) == Some(&k))
            .collect();
        for (job, keep) in jobs.into_iter().zip(keep) {
            if !keep {
                continue;
            }
            let Some(a) = analyzer.as_mut() else { continue };
            let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match job.ask {
                    Ask::Touched => {
                        a.touched(&job.path);
                        return None;
                    }
                    Ask::Closed => {
                        a.set_text(&job.path, None);
                        return None;
                    }
                    _ => {}
                }
                if let Some(t) = &job.text {
                    a.set_text(&job.path, Some(t.clone()));
                }
                Some(match job.ask {
                    Ask::Check | Ask::Touched | Ask::Closed => Reply::Report(a.check(&job.path)),
                    Ask::Hover { line, col } => Reply::Hover(a.hover(&job.path, line, col)),
                    Ask::Complete { line, col } => {
                        Reply::Complete(a.complete(&job.path, line, col))
                    }
                    Ask::Signature { line, col } => {
                        Reply::Signature(a.signature(&job.path, line, col))
                    }
                    Ask::Definition { line, col } => {
                        Reply::Definition(a.definition(&job.path, line, col))
                    }
                })
            }));
            let reply = match reply {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => {
                    // A bug in the checker: start a fresh one (its state may be half-done).
                    analyzer = start(&root).ok();
                    Reply::Report(Report {
                        path: job.path.clone(),
                        ..Report::default()
                    })
                }
            };
            if outbox
                .send(Answer {
                    ticket: job.ticket,
                    path: job.path,
                    reply,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::world::World;
    use engine_core::registry::ApiRegistry;
    use serde_json::Value;

    fn scripts() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scripts")
    }

    /// The definitions, with the other modules' functions the shipped scripts call (their own
    /// crates register them in the game).
    fn definitions() -> String {
        let mut world = World::new();
        let mut api = ApiRegistry::default();
        for name in [
            "vfx.spawn",
            "vfx.beam",
            "ui.notify",
            "time.dilate",
            "spell.starburst",
            "spell.fireball_impact",
            "spell.charge",
            "impact.sparks",
            "fire.torch",
            "script.break",
        ] {
            api.register(name, "A stand-in.", "{}", |_: &mut World, _| {
                Ok(Value::Null)
            });
        }
        world.insert_resource(api);
        crate::types::definitions(&world)
    }

    fn analyzer() -> Analyzer {
        Analyzer::new(scripts(), &definitions()).expect("the definitions load")
    }

    fn beacon() -> String {
        std::fs::read_to_string(scripts().join("examples/beacon.luau")).unwrap()
    }

    /// (line, char column) of `needle` (its `nth` match) plus `offset` chars.
    fn at(text: &str, needle: &str, offset: u32) -> (u32, u32) {
        for (l, line) in text.split('\n').enumerate() {
            if let Some(b) = line.find(needle) {
                return (l as u32, line[..b].chars().count() as u32 + offset);
            }
        }
        panic!("{needle} is not in the text");
    }

    #[test]
    fn positions_map_through_the_shape() {
        let src = "local Lamp = {}\nfunction Lamp:update(dt: number)\n\tself.t = dt\nend\n\
                   function Lamp:start()\nend\nreturn Lamp\n";
        let s = shape(src, None);
        let lines: Vec<&str> = s.text.split('\n').collect();
        assert_eq!(lines.len(), src.split('\n').count(), "no line moved");
        assert!(lines[0].starts_with("local Lamp = {}; type __Self = ScriptBase"));
        assert!(lines[0].contains("{ t: any, emit: "), "{}", lines[0]);
        assert_eq!(lines[1], "function Lamp.update(self: __Self, dt: number)");
        assert_eq!(lines[4], "function Lamp.start(self: __Self)");
        // `dt` (column 21 in the file) sits after the insert in the shaped text.
        let shaped = s.to_shaped(1, 21);
        assert_eq!(&lines[1][shaped as usize..shaped as usize + 2], "dt");
        assert_eq!(s.to_file(1, shaped), Some(21));
        assert_eq!(s.to_file(1, 23), None, "inside `self: __Self, `");
        assert_eq!(s.to_file(2, 3), Some(3), "other lines are as written");
        // A module (no class) is checked as written.
        let m = shape("local m = {}\nfunction m.f() end\nreturn 1\n", None);
        assert!(m.class.is_none() && m.inserts.is_empty());
    }

    #[test]
    fn self_is_the_class_so_typos_in_props_and_entity_fields_are_caught() {
        let mut a = analyzer();
        let clean = a.check("examples/beacon.luau");
        assert_eq!(clean.errors(), 0, "{:#?}", clean.problems);
        // The fields its methods set got the types assigned to them.
        let field = |n: &str| {
            clean
                .fields
                .iter()
                .find(|f| f.0 == n)
                .map(|f| f.1.clone())
                .unwrap_or_default()
        };
        assert_eq!(field("home"), "Vec3");
        assert_eq!(field("t"), "number");
        assert_eq!(field("level"), "number");

        let typo = beacon()
            .replace("p.lumens * level", "p.lumen * level")
            .replace(
                "self.entity.position = self.home",
                "self.entity.positon = self.home",
            );
        a.set_text("examples/beacon.luau", Some(typo.clone()));
        let r = a.check("examples/beacon.luau");
        let errors: Vec<&Problem> = r.problems.iter().filter(|p| p.error).collect();
        assert_eq!(errors.len(), 2, "{errors:#?}");
        let (line, col) = at(&typo, "p.lumen ", 2);
        assert_eq!((errors[0].span.line, errors[0].span.col), (line, col));
        assert!(
            errors[0].message.contains("'lumen'"),
            "{}",
            errors[0].message
        );
        assert!(
            errors[1].message.contains("positon"),
            "{}",
            errors[1].message
        );
        assert_eq!(errors[1].span.line, at(&typo, "positon", 0).0);
        // `self:emit` knows the class's signals: a name it doesn't declare and a value of the
        // wrong type are both caught where they are.
        for (wrong, says) in [
            ("self:emit(\"flashed\", p.mode)", "string"),
            ("self:emit(\"flahsed\", self.flashes)", "flahsed"),
        ] {
            let typo = beacon().replace("self:emit(\"flashed\", self.flashes)", wrong);
            a.set_text("examples/beacon.luau", Some(typo.clone()));
            let r = a.check("examples/beacon.luau");
            let errors: Vec<&Problem> = r.problems.iter().filter(|p| p.error).collect();
            assert_eq!(errors.len(), 1, "{wrong}: {errors:#?}");
            assert_eq!(errors[0].span.line, at(&typo, wrong, 0).0, "{wrong}");
            assert!(errors[0].message.contains(says), "{}", errors[0].message);
        }
        // Back to the file on disk.
        a.set_text("examples/beacon.luau", None);
        assert_eq!(a.check("examples/beacon.luau").errors(), 0);
    }

    #[test]
    fn every_shipped_script_checks_clean() {
        let mut a = analyzer();
        let mut files = Vec::new();
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    walk(&e.path(), &format!("{prefix}{name}/"), out);
                } else if (name.ends_with(".luau") && !name.ends_with(".d.luau"))
                    || name.ends_with(".graph")
                {
                    out.push(format!("{prefix}{name}"));
                }
            }
        }
        walk(&scripts(), "", &mut files);
        assert!(files.len() >= 8, "{files:?}");
        let mut bad = Vec::new();
        for f in &files {
            let r = a.check(f);
            for p in r.problems.iter().filter(|p| p.error) {
                bad.push(format!("{f}:{}: {}", p.span.line + 1, p.message));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn hover_completion_and_signatures_know_the_types() {
        let mut a = analyzer();
        let text = beacon();
        let (l, c) = at(&text, "p.pulse.rate", 9);
        let h = a.hover("examples/beacon.luau", l, c).expect("a type");
        assert_eq!(h.text, "rate: number");
        let (l, c) = at(&text, "self.entity:set", 13);
        let h = a.hover("examples/beacon.luau", l, c).expect("a type");
        assert!(h.text.starts_with("set: "), "{}", h.text);
        assert!(h.doc.contains("Change a component"), "{h:?}");

        // Typing in a method (each on its own, the rest of the file after it): the class's
        // props, an entity's methods, a call's parameters.
        let typing = |line: &str| {
            let t = text.replace(
                "\tlocal p = self.props\n",
                &format!("\tlocal p = self.props\n\t{line}\n"),
            );
            let (l, c) = at(&t, line, line.chars().count() as u32);
            (t, l, c)
        };
        let (t, l, c) = typing("local q = self.props.");
        a.set_text("examples/beacon.luau", Some(t));
        let names: Vec<String> = a
            .complete("examples/beacon.luau", l, c)
            .into_iter()
            .map(|s| s.name)
            .collect();
        for n in ["lumens", "color", "pulse", "mode", "every"] {
            assert!(names.contains(&n.to_string()), "{n} in {names:?}");
        }
        let (t, l, c) = typing("self.entity:");
        a.set_text("examples/beacon.luau", Some(t));
        let methods = a.complete("examples/beacon.luau", l, c);
        assert!(
            methods.iter().all(|s| s.kind == SuggestKind::Method),
            "{methods:?}"
        );
        let set = methods.iter().find(|s| s.name == "set").expect("set");
        assert!(set.doc.contains("Change a component"), "{set:?}");
        let (t, l, c) = typing("local lean = quat.axis_angle(vec3.up, ");
        a.set_text("examples/beacon.luau", Some(t));
        let s = a
            .signature("examples/beacon.luau", l, c)
            .expect("in a call");
        assert_eq!(s.name, "quat.axis_angle");
        assert_eq!(s.params, ["axis: Vec3", "angle: number"]);
        assert_eq!((s.active, s.ret.as_str()), (1, "Quat"));
    }

    #[test]
    fn required_modules_are_typed_and_definitions_found() {
        let mut a = analyzer();
        let r = a.check("examples/float.luau");
        assert_eq!(r.errors(), 0, "{:#?}", r.problems);
        let text = std::fs::read_to_string(scripts().join("examples/float.luau")).unwrap();
        let (l, c) = at(&text, "wave.bob", 6);
        let h = a.hover("examples/float.luau", l, c).expect("a type");
        assert!(h.text.contains("number"), "{h:?}");
        // A local's definition.
        let (l, c) = at(&text, "local wave", 6);
        let uses = text
            .split('\n')
            .enumerate()
            .skip(l as usize + 1)
            .find_map(|(k, line)| line.find("wave.").map(|b| (k as u32, b as u32)))
            .expect("a use of wave");
        let d = a
            .definition("examples/float.luau", uses.0, uses.1)
            .expect("declared");
        assert_eq!((d.1, d.2), (l, c));
    }
}
