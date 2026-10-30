//! Story: conversations written in Yarn Spinner's language (`.yarn` files), the facts they
//! read and change, and quests (`.quest` files) that move on by themselves when the facts do.
//!
//! A `.yarn` file compiles to Luau ([`compile_yarn`]), one Luau line per Yarn line, so a
//! mistake at run time points at the `.yarn` line. Its nodes run through a runner in
//! `story.luau`: lines up to the next options or waiting command are shown together (in the
//! UI module's conversation screen when it is there, else in the log) and each option sends
//! its pick back. Facts are one table ([`StoryFacts`]): `$name` in Yarn, `facts.name` in
//! scripts, `story.facts` for agents and the editor; what a save keeps. Quests ([`QuestBook`])
//! are data: objectives with `done_when` and `progress` expressions over the facts, ticked
//! every frame and mirrored into the UI's journal and HUD tracker through `quest.*`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::SystemTime;

use bevy_ecs::prelude::*;
use engine_core::registry::call_api;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// Where dialogue and quest files live in the scripts folder.
pub const DIALOGUE_FOLDER: &str = "dialogue";
pub const QUEST_FOLDER: &str = "quests";

/// Registry name of `story.luau`'s `__story_tick`.
pub(crate) const TICK: &str = "engine.story_tick";

// ------------------------------------------------------------------------------- facts

/// The story's facts: what has happened, as named values (Yarn's `$variables`, Valve's facts).
/// Read and written by dialogue (`$met_keeper`), scripts (`facts.runes_lit += 1`), quests'
/// conditions and agents (`story.facts`). Also how many times each node has been left
/// (`visited`), and which `<<once>>`s have run.
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryFacts {
    pub values: BTreeMap<String, Json>,
    /// Times each node (by title) was left.
    pub visits: BTreeMap<String, u32>,
    /// `<<once>>` lines, options and blocks that have run (`file:line`).
    pub once: Vec<String>,
    /// Skill checks rolled (`file:line` of the option): `{"shut": skill}` for a white check
    /// that failed at that skill (open again once it is higher), `{"red": true}` for a red one.
    pub checks: BTreeMap<String, Json>,
    /// Checks failed in a row (each is +1 luck on the next, while luck is on).
    pub luck: u32,
    /// Changes whenever any of it does.
    #[serde(skip)]
    pub version: u64,
}

impl StoryFacts {
    pub fn get(&self, name: &str) -> Option<&Json> {
        self.values.get(name)
    }

    /// Sets a fact (`null` removes it).
    pub fn set(&mut self, name: &str, value: Json) {
        let changed = if value.is_null() {
            self.values.remove(name).is_some()
        } else {
            self.values.insert(name.to_string(), value.clone()) != Some(value)
        };
        if changed {
            self.version += 1;
        }
    }

    /// Sets a fact unless it has a value already (`<<declare>>`, a quest's `facts`).
    pub fn default_to(&mut self, name: &str, value: Json) {
        if !self.values.contains_key(name) {
            self.set(name, value);
        }
    }
}

// ------------------------------------------------------------------------------ checks

/// How skill checks roll (scripting-research.md §29), set by the game or `story.settings`.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorySettings {
    /// "2d6" (Disco Elysium: double six passes, double one fails) or "d20" (Baldur's Gate 3:
    /// a natural 20 passes, a natural 1 fails).
    pub dice: String,
    /// "easy", "normal" or "hard": −2, 0 or +2 on every check's difficulty.
    pub difficulty: String,
    /// +1 on a check for each one failed in a row before it (Karmic Dice, shown).
    pub luck: bool,
}

impl Default for StorySettings {
    fn default() -> Self {
        Self {
            dice: "2d6".into(),
            difficulty: "normal".into(),
            luck: true,
        }
    }
}

impl StorySettings {
    /// What the difficulty setting adds to every check's difficulty.
    pub fn shift(&self) -> f64 {
        match self.difficulty.as_str() {
            "easy" => -2.0,
            "hard" => 2.0,
            _ => 0.0,
        }
    }
}

/// The chance (0..1) that `dice` ("2d6" or "d20") plus `modifier` reaches `target`, with the
/// automatic pass and fail at the top and bottom of the dice, counted over every way they fall.
pub fn check_chance(dice: &str, modifier: f64, target: f64) -> f64 {
    if dice == "d20" {
        let pass = (1..=20)
            .filter(|&r| r == 20 || (r != 1 && r as f64 + modifier >= target))
            .count();
        return pass as f64 / 20.0;
    }
    let mut pass = 0;
    for a in 1..=6 {
        for b in 1..=6 {
            if (a == 6 && b == 6) || (!(a == 1 && b == 1) && (a + b) as f64 + modifier >= target) {
                pass += 1;
            }
        }
    }
    pass as f64 / 36.0
}

/// A skill name as people read it: "persuade" → "Persuade", "half_truth" → "Half truth".
pub fn skill_title(skill: &str) -> String {
    let s = skill.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ------------------------------------------------------------------------- expressions

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Var(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '.';
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            let s: String = c[start..i].iter().collect();
            out.push(Tok::Num(
                s.parse().map_err(|_| format!("{s} is not a number"))?,
            ));
            continue;
        }
        if ch == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err("a string is missing its closing \"".into()),
                    Some('"') => break,
                    Some('\\') => {
                        match c.get(i + 1) {
                            Some('n') => s.push('\n'),
                            Some(x) => s.push(*x),
                            None => {}
                        }
                        i += 2;
                        continue;
                    }
                    Some(x) => s.push(*x),
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
            continue;
        }
        if ch == '$' {
            let start = i + 1;
            i += 1;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            if i == start {
                return Err("a $ with no variable name after it".into());
            }
            out.push(Tok::Var(c[start..i].iter().collect()));
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < c.len() && ident(c[i]) {
                i += 1;
            }
            let word: String = c[start..i].iter().collect();
            out.push(match word.as_str() {
                "and" => Tok::Op("and"),
                "or" => Tok::Op("or"),
                "not" => Tok::Op("not"),
                "xor" => Tok::Op("xor"),
                "is" | "eq" => Tok::Op("=="),
                "neq" => Tok::Op("!="),
                "gt" => Tok::Op(">"),
                "lt" => Tok::Op("<"),
                "gte" => Tok::Op(">="),
                "lte" => Tok::Op("<="),
                _ => Tok::Ident(word),
            });
            continue;
        }
        let two: String = c[i..(i + 2).min(c.len())].iter().collect();
        let op2 = ["==", "!=", ">=", "<=", "&&", "||"]
            .into_iter()
            .find(|o| *o == two);
        if let Some(o) = op2 {
            out.push(Tok::Op(match o {
                "&&" => "and",
                "||" => "or",
                o => o,
            }));
            i += 2;
            continue;
        }
        out.push(match ch {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            ',' => Tok::Comma,
            '>' => Tok::Op(">"),
            '<' => Tok::Op("<"),
            '+' => Tok::Op("+"),
            '-' => Tok::Op("-"),
            '*' => Tok::Op("*"),
            '/' => Tok::Op("/"),
            '%' => Tok::Op("%"),
            '!' => Tok::Op("not"),
            '^' => Tok::Op("xor"),
            '=' => Tok::Op("=="),
            other => return Err(format!("'{other}' can't be in an expression")),
        });
        i += 1;
    }
    Ok(out)
}

/// Turns a Yarn expression into Luau: `$x` reads the facts (`F.x`), comparisons and sums treat
/// a fact never set as 0 (`S.lt`, `S.add`), `+` joins text, functions go to the runner.
struct ExprParser {
    toks: Vec<Tok>,
    at: usize,
}

fn binding(op: &str) -> Option<u8> {
    Some(match op {
        "or" => 1,
        "xor" => 2,
        "and" => 3,
        "==" | "!=" => 4,
        "<" | ">" | "<=" | ">=" => 5,
        "+" | "-" => 6,
        "*" | "/" | "%" => 7,
        _ => return None,
    })
}

impl ExprParser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn expr(&mut self, min: u8) -> Result<String, String> {
        let mut left = self.unary()?;
        while let Some(Tok::Op(op)) = self.peek() {
            let op = *op;
            let Some(bp) = binding(op) else {
                break;
            };
            if bp < min {
                break;
            }
            self.at += 1;
            let right = self.expr(bp + 1)?;
            left = match op {
                "or" => format!("({left} or {right})"),
                "and" => format!("({left} and {right})"),
                "xor" => format!("S.xor({left}, {right})"),
                "==" => format!("S.eq({left}, {right})"),
                "!=" => format!("(not S.eq({left}, {right}))"),
                "<" => format!("S.lt({left}, {right})"),
                ">" => format!("S.lt({right}, {left})"),
                "<=" => format!("S.le({left}, {right})"),
                ">=" => format!("S.le({right}, {left})"),
                "+" => format!("S.add({left}, {right})"),
                "-" => format!("S.sub({left}, {right})"),
                "*" => format!("S.mul({left}, {right})"),
                "/" => format!("S.div({left}, {right})"),
                _ => format!("S.mod({left}, {right})"),
            };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(Tok::Op("not")) => {
                self.at += 1;
                Ok(format!("(not {})", self.expr(8)?))
            }
            Some(Tok::Op("-")) => {
                self.at += 1;
                Ok(format!("S.neg({})", self.expr(8)?))
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(lua_num(n)),
            Some(Tok::Str(s)) => Ok(lua_str(&s)),
            Some(Tok::Var(v)) => Ok(format!("F[{}]", lua_str(&v))),
            Some(Tok::Ident(w)) => match w.as_str() {
                "true" => Ok("true".into()),
                "false" => Ok("false".into()),
                "null" | "nil" => Ok("nil".into()),
                _ if self.peek() == Some(&Tok::LParen) => {
                    self.at += 1;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::RParen) {
                        self.at += 1;
                    } else {
                        loop {
                            args.push(self.expr(0)?);
                            match self.next() {
                                Some(Tok::Comma) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err(format!("{w}( is missing its )")),
                            }
                        }
                    }
                    let mut call = format!("S:call({}", lua_str(&w));
                    for a in args {
                        call.push_str(", ");
                        call.push_str(&a);
                    }
                    call.push(')');
                    Ok(call)
                }
                _ => Err(format!(
                    "'{w}' isn't a value: variables start with $ (${w}), text goes in quotes"
                )),
            },
            Some(Tok::LParen) => {
                let e = self.expr(0)?;
                match self.next() {
                    Some(Tok::RParen) => Ok(format!("({e})")),
                    _ => Err("a ( is missing its )".into()),
                }
            }
            Some(t) => Err(format!("unexpected {}", tok_text(&t))),
            None => Err("the expression ends too soon".into()),
        }
    }
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Num(n) => n.to_string(),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Var(v) => format!("${v}"),
        Tok::Ident(w) => w.clone(),
        Tok::Op(o) => (*o).to_string(),
        Tok::LParen => "(".into(),
        Tok::RParen => ")".into(),
        Tok::Comma => ",".into(),
    }
}

/// A Yarn expression as Luau (`S` is the runner, `F` the facts).
pub fn compile_expr(src: &str) -> Result<String, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("an empty expression".into());
    }
    let mut p = ExprParser { toks, at: 0 };
    let e = p.expr(0)?;
    if let Some(t) = p.peek() {
        return Err(format!("unexpected {} after the expression", tok_text(t)));
    }
    Ok(e)
}

/// How specific a condition is (Yarn's "best" saliency, Valve's rule score): one per
/// comparison or value tested.
fn specificity(src: &str) -> u32 {
    let toks = lex(src).unwrap_or_default();
    1 + toks.iter().filter(|t| matches!(t, Tok::Op("and"))).count() as u32
}

fn lua_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// A number as JSON: whole numbers as integers, as Luau's would be.
fn json_num(n: f64) -> Json {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// A Luau string literal.
pub fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --------------------------------------------------------------------------- the file

/// A problem in a `.yarn` or `.quest` file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub line: u32,
    pub message: String,
}

/// What the editor draws of a node.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NodeInfo {
    pub title: String,
    /// The `title:` line and the `===` line.
    pub line: u32,
    pub end_line: u32,
    pub when: Vec<String>,
    pub tags: Vec<String>,
    /// `position: x,y` from the header (Yarn's editors keep it there).
    pub position: Option<(f32, f32)>,
    /// Its lines and options, in order, as written (`Keeper: Welcome.`, `-> Ask`).
    pub lines: Vec<(u32, String)>,
    /// Where it goes: (target, how: "jump", "detour" or an option's text, line).
    pub links: Vec<(String, String, u32)>,
    /// Its options, with the spot each is walked to (`#spot:name`).
    pub options: Vec<OptionInfo>,
}

/// An option, for the editor (the plan of a choice made by walking).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OptionInfo {
    /// The line of the first option of its set (options shown together).
    pub set: u32,
    pub line: u32,
    pub text: String,
    /// `#spot:name`: picked by walking to the spot of that name.
    pub spot: Option<String>,
    /// `#fallback`: taken when the time to choose runs out.
    pub fallback: bool,
    /// `#check:skill:difficulty`: a skill check (the skill, its difficulty).
    pub check: Option<(String, f64)>,
    /// `#red`: a red check, rolled once.
    pub red: bool,
}

/// A compiled `.yarn` file.
#[derive(Clone, Debug, Default)]
pub struct Compiled {
    pub luau: String,
    pub nodes: Vec<NodeInfo>,
    /// `<<declare>>`d facts and their starting values.
    pub declares: Vec<(String, Json)>,
    pub problems: Vec<Problem>,
}

/// `persuade:8` → ("persuade", 8).
pub fn parse_check(tag: &str) -> Option<(String, f64)> {
    let (skill, n) = tag.split_once(':')?;
    let ok = !skill.is_empty() && skill.chars().all(|c| c.is_alphanumeric() || c == '_');
    let n = n.trim().parse::<f64>().ok()?;
    ok.then(|| (skill.to_string(), n))
}

/// One line of the body, split up.
#[derive(Clone, Debug)]
enum Body {
    Blank,
    Line {
        speaker: Option<String>,
        text: String,
        cond: Option<String>,
        once: bool,
        tags: Vec<String>,
    },
    Option {
        text: String,
        cond: Option<String>,
        once: bool,
        tags: Vec<String>,
    },
    Group {
        speaker: Option<String>,
        text: String,
        cond: Option<String>,
        once: bool,
        tags: Vec<String>,
    },
    Command(String),
}

/// Splits `text <<if cond>> <<once>> #tag #tag` into its parts.
fn split_line(raw: &str) -> (String, Option<String>, bool, Vec<String>) {
    let mut text = raw.to_string();
    let mut tags = Vec::new();
    // Tags: `#word` at the end, outside braces.
    loop {
        let t = text.trim_end();
        let Some(i) = t.rfind('#') else {
            break;
        };
        let tag = &t[i + 1..];
        let open = t[..i].matches('{').count() > t[..i].matches('}').count();
        if tag.is_empty() || tag.contains(char::is_whitespace) || open {
            break;
        }
        if i > 0 && !t[..i].ends_with(char::is_whitespace) {
            break;
        }
        tags.insert(0, tag.to_string());
        text = t[..i].to_string();
    }
    let mut cond = None;
    let mut once = false;
    loop {
        let t = text.trim_end();
        if !t.ends_with(">>") {
            break;
        }
        let Some(i) = t.rfind("<<") else {
            break;
        };
        let inner = t[i + 2..t.len() - 2].trim();
        if let Some(c) = inner.strip_prefix("if ") {
            cond = Some(c.trim().to_string());
        } else if inner == "once" {
            once = true;
        } else if let Some(c) = inner.strip_prefix("once if ") {
            once = true;
            cond = Some(c.trim().to_string());
        } else {
            break;
        }
        text = t[..i].to_string();
    }
    (text.trim().to_string(), cond, once, tags)
}

/// `Speaker: text` (a name before the first colon, without markup in it).
fn split_speaker(text: &str) -> (Option<String>, String) {
    if let Some(i) = text.find(':') {
        let name = text[..i].trim();
        let ok = !name.is_empty()
            && name.chars().count() <= 40
            && !name.contains(['{', '}', '<', '>', '[', ']', '#', '"', '$']);
        if ok && text[i + 1..].starts_with(char::is_whitespace) {
            return (Some(name.to_string()), text[i + 1..].trim().to_string());
        }
    }
    (None, text.to_string())
}

fn classify(trimmed: &str) -> Body {
    if trimmed.is_empty() || trimmed.starts_with("//") {
        return Body::Blank;
    }
    if let Some(rest) = trimmed.strip_prefix("->") {
        let (text, cond, once, tags) = split_line(rest);
        return Body::Option {
            text,
            cond,
            once,
            tags,
        };
    }
    if let Some(rest) = trimmed.strip_prefix("=>") {
        let (text, cond, once, tags) = split_line(rest);
        let (speaker, text) = split_speaker(&text);
        return Body::Group {
            speaker,
            text,
            cond,
            once,
            tags,
        };
    }
    if trimmed.starts_with("<<") && trimmed.ends_with(">>") && trimmed.matches("<<").count() == 1 {
        return Body::Command(trimmed[2..trimmed.len() - 2].trim().to_string());
    }
    let (text, cond, once, tags) = split_line(trimmed);
    let (speaker, text) = split_speaker(&text);
    Body::Line {
        speaker,
        text,
        cond,
        once,
        tags,
    }
}

fn indent_of(line: &str) -> usize {
    let mut n = 0;
    for ch in line.chars() {
        match ch {
            ' ' => n += 1,
            '\t' => n += 4,
            _ => break,
        }
    }
    n
}

/// Text with `{expressions}` in it, as a Luau string expression.
fn interpolate(text: &str) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    let mut lit = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(n) = chars.next() {
                    lit.push(n);
                }
            }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if !closed {
                    return Err("a { in the text is missing its }".into());
                }
                if !lit.is_empty() {
                    parts.push(lua_str(&std::mem::take(&mut lit)));
                }
                parts.push(format!("S.text({})", compile_expr(&inner)?));
            }
            c => lit.push(c),
        }
    }
    if !lit.is_empty() || parts.is_empty() {
        parts.push(lua_str(&lit));
    }
    Ok(parts.join(" .. "))
}

fn tags_lua(tags: &[String]) -> String {
    if tags.is_empty() {
        return "nil".into();
    }
    let items: Vec<String> = tags.iter().map(|t| lua_str(t)).collect();
    format!("{{{}}}", items.join(", "))
}

/// The Luau text, a slot per source line (code for line N goes on line N).
struct Out {
    slots: Vec<String>,
    /// The last line written to (code is never put before it, so it runs in order).
    cur: usize,
}

impl Out {
    fn put(&mut self, line: u32, code: &str) {
        let at = (line as usize).max(self.cur).max(1);
        while self.slots.len() < at {
            self.slots.push(String::new());
        }
        let slot = &mut self.slots[at - 1];
        if !slot.is_empty() {
            slot.push(' ');
        }
        slot.push_str(code);
        self.cur = at;
    }
}

/// An open block in a node body.
enum Open {
    /// `<<if>>` or `<<once>>`: whether `<<else>>` was seen.
    If { line: u32, once: bool },
    /// Options (or a line group) at this indent, inside an item's body now; `start` is the
    /// first one's line.
    Choices {
        indent: usize,
        group: bool,
        start: u32,
    },
}

/// The compiled module's first statement.
const HEAD: &str = "local __nodes, __declares = {}, {}";

/// Compiles a `.yarn` file to a Luau module: `return { nodes = { {title, when, score, run}... },
/// declares = {...} }`.
pub fn compile_yarn(text: &str, path: &str) -> Compiled {
    let mut out = Out {
        slots: Vec::new(),
        cur: 0,
    };
    let mut nodes = Vec::new();
    let mut declares = Vec::new();
    let mut problems = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    out.put(1, HEAD);
    let mut i = 0;
    while i < lines.len() {
        // Headers.
        let mut info = NodeInfo {
            line: i as u32 + 1,
            ..Default::default()
        };
        let mut when = Vec::new();
        let mut header_start = None;
        while i < lines.len() && lines[i].trim() != "---" {
            let l = lines[i].trim();
            let n = i as u32 + 1;
            if l.is_empty() || l.starts_with("//") {
                i += 1;
                continue;
            }
            if header_start.is_none() {
                header_start = Some(n);
                info.line = n;
            }
            match l.split_once(':') {
                Some((k, v)) => {
                    let v = v.trim();
                    match k.trim() {
                        "title" => info.title = v.to_string(),
                        "tags" => info.tags = v.split_whitespace().map(str::to_string).collect(),
                        "when" => when.push((n, v.to_string())),
                        "position" => {
                            let mut xy = v.split(',').map(|s| s.trim().parse::<f32>());
                            if let (Some(Ok(x)), Some(Ok(y))) = (xy.next(), xy.next()) {
                                info.position = Some((x, y));
                            }
                        }
                        _ => {}
                    }
                }
                None => problems.push(Problem {
                    line: n,
                    message: format!(
                        "'{l}' isn't a header (key: value); a node's body starts after ---"
                    ),
                }),
            }
            i += 1;
        }
        if i >= lines.len() {
            if header_start.is_some() {
                problems.push(Problem {
                    line: info.line,
                    message: "a node's headers with no --- after them".into(),
                });
            }
            break;
        }
        let start_line = i as u32 + 1;
        if info.title.is_empty() {
            problems.push(Problem {
                line: info.line,
                message: "a node with no title: header".into(),
            });
            info.title = format!("Untitled{}", nodes.len() + 1);
        }
        if info.title.contains(char::is_whitespace) {
            problems.push(Problem {
                line: info.line,
                message: format!("the title '{}' has a space in it", info.title),
            });
        }
        // The node's table, its when: conditions on their own lines.
        let mut score = 0;
        out.put(
            info.line,
            &format!(
                "__nodes[#__nodes + 1] = {{ title = {}, line = {}, when = {{",
                lua_str(&info.title),
                info.line
            ),
        );
        for (n, w) in &when {
            info.when.push(w.clone());
            let code = match w.as_str() {
                "always" => "function() return true end,".to_string(),
                "once" => {
                    score += 1;
                    format!(
                        "function(S) return not S:has_once({}) end,",
                        lua_str(&format!("{path}:{n}"))
                    )
                }
                c => match compile_expr(c) {
                    Ok(e) => {
                        score += specificity(c);
                        format!("function(S, F) return {e} end,")
                    }
                    Err(e) => {
                        problems.push(Problem {
                            line: *n,
                            message: format!("when: {e}"),
                        });
                        "function() return false end,".to_string()
                    }
                },
            };
            out.put(*n, &code);
        }
        let once_keys: Vec<String> = when
            .iter()
            .filter(|(_, w)| w == "once")
            .map(|(n, _)| lua_str(&format!("{path}:{n}")))
            .collect();
        out.put(
            start_line,
            &format!(
                "}}, score = {score}, once = {{{}}}, run = function(S, F)",
                once_keys.join(", ")
            ),
        );
        i += 1;
        // The body.
        let mut stack: Vec<Open> = Vec::new();
        let mut closed = false;
        while i < lines.len() {
            let raw = lines[i];
            let n = i as u32 + 1;
            if raw.trim() == "===" {
                close_choices(&mut out, &mut stack, 0, n);
                if let Some(Open::If { line, .. }) =
                    stack.iter().rev().find(|o| matches!(o, Open::If { .. }))
                {
                    problems.push(Problem {
                        line: *line,
                        message: "an <<if>> or <<once>> with no <<endif>> or <<endonce>>".into(),
                    });
                    for _ in stack.iter().filter(|o| matches!(o, Open::If { .. })) {
                        out.put(n, "end");
                    }
                }
                out.put(n, "end }");
                info.end_line = n;
                closed = true;
                i += 1;
                break;
            }
            let body = classify(raw.trim());
            if matches!(body, Body::Blank) {
                i += 1;
                continue;
            }
            let indent = indent_of(raw);
            // Leaving option bodies: a line at or left of the options' own indent.
            let is_item = matches!(body, Body::Option { .. } | Body::Group { .. });
            close_choices(&mut out, &mut stack, indent + usize::from(is_item), n);
            // Options right after a line group at the same indent end it (and the other way
            // round): `=> Back again?` then `-> Ask`.
            let group_item = matches!(body, Body::Group { .. });
            if is_item
                && matches!(stack.last(), Some(Open::Choices { indent: at, group, .. })
                    if *at == indent && *group != group_item)
            {
                close_choices(&mut out, &mut stack, indent, n);
            }
            match body {
                Body::Blank => {}
                Body::Line {
                    speaker,
                    text,
                    cond,
                    once,
                    tags,
                } => {
                    info.lines.push((
                        n,
                        match &speaker {
                            Some(s) => format!("{s}: {text}"),
                            None => text.clone(),
                        },
                    ));
                    match interpolate(&text) {
                        Ok(t) => {
                            let call = format!(
                                "S:line({}, {t}, {}, {n})",
                                speaker.as_deref().map_or("nil".into(), lua_str),
                                tags_lua(&tags)
                            );
                            let guard = guard(&cond, once, path, n, &mut problems);
                            match guard {
                                Some(g) => out.put(n, &format!("if {g} then {call} end")),
                                None => out.put(n, &call),
                            }
                        }
                        Err(e) => problems.push(Problem {
                            line: n,
                            message: e,
                        }),
                    }
                }
                Body::Option {
                    text,
                    cond,
                    once,
                    tags,
                } => {
                    info.lines.push((n, format!("-> {text}")));
                    let open = matches!(stack.last(), Some(Open::Choices { indent: at, group: false, .. }) if *at == indent);
                    if open {
                        out.put(n, "end)");
                    } else {
                        out.put(n, "do local __o = S:options()");
                        stack.push(Open::Choices {
                            indent,
                            group: false,
                            start: n,
                        });
                    }
                    let set = match stack.last() {
                        Some(Open::Choices { start, .. }) => *start,
                        _ => n,
                    };
                    let tag = |key: &str| {
                        tags.iter().find_map(|t| {
                            if t == key {
                                Some(String::new())
                            } else {
                                t.strip_prefix(key)
                                    .and_then(|r| r.strip_prefix(':'))
                                    .map(str::to_string)
                            }
                        })
                    };
                    info.options.push(OptionInfo {
                        set,
                        line: n,
                        text: text.clone(),
                        spot: tag("spot").filter(|s| !s.is_empty()),
                        fallback: tag("fallback").is_some(),
                        check: tag("check").and_then(|c| parse_check(&c)),
                        red: tag("red").is_some(),
                    });
                    if let Some(c) = tag("check")
                        && parse_check(&c).is_none()
                    {
                        problems.push(Problem {
                            line: n,
                            message: format!(
                                "#check:{c}: write #check:skill:difficulty, like #check:persuade:8"
                            ),
                        });
                    }
                    let t = interpolate(&text).unwrap_or_else(|e| {
                        problems.push(Problem {
                            line: n,
                            message: e,
                        });
                        "\"\"".into()
                    });
                    let ok = match &cond {
                        Some(c) => compile_expr(c).unwrap_or_else(|e| {
                            problems.push(Problem {
                                line: n,
                                message: e,
                            });
                            "false".into()
                        }),
                        None => "true".into(),
                    };
                    let key = if once {
                        lua_str(&format!("{path}:{n}"))
                    } else {
                        "nil".into()
                    };
                    out.put(
                        n,
                        &format!(
                            "__o:add({t}, {ok}, {key}, {}, {n}, function()",
                            tags_lua(&tags)
                        ),
                    );
                }
                Body::Group {
                    speaker,
                    text,
                    cond,
                    once,
                    tags,
                } => {
                    info.lines.push((
                        n,
                        match &speaker {
                            Some(s) => format!("=> {s}: {text}"),
                            None => format!("=> {text}"),
                        },
                    ));
                    let open = matches!(stack.last(), Some(Open::Choices { indent: at, group: true, .. }) if *at == indent);
                    if open {
                        out.put(n, "end)");
                    } else {
                        out.put(n, "do local __g = S:group()");
                        stack.push(Open::Choices {
                            indent,
                            group: true,
                            start: n,
                        });
                    }
                    let score = cond.as_deref().map_or(0, specificity) + u32::from(once);
                    let ok = match &cond {
                        Some(c) => compile_expr(c).unwrap_or_else(|e| {
                            problems.push(Problem {
                                line: n,
                                message: e,
                            });
                            "false".into()
                        }),
                        None => "true".into(),
                    };
                    let key = if once {
                        lua_str(&format!("{path}:{n}"))
                    } else {
                        "nil".into()
                    };
                    let t = interpolate(&text).unwrap_or_else(|e| {
                        problems.push(Problem {
                            line: n,
                            message: e,
                        });
                        "\"\"".into()
                    });
                    out.put(
                        n,
                        &format!(
                            "__g:add({ok}, {score}, {key}, {n}, function() S:line({}, {t}, {}, {n})",
                            speaker.as_deref().map_or("nil".into(), lua_str),
                            tags_lua(&tags)
                        ),
                    );
                }
                Body::Command(cmd) => {
                    command(
                        &cmd,
                        n,
                        path,
                        &mut out,
                        &mut stack,
                        &mut info,
                        &mut declares,
                        &mut problems,
                    );
                }
            }
            i += 1;
        }
        if !closed {
            problems.push(Problem {
                line: start_line,
                message: format!("the node '{}' has no === at its end", info.title),
            });
            close_choices(&mut out, &mut stack, 0, lines.len() as u32);
            out.put(lines.len() as u32, "end }");
            info.end_line = lines.len() as u32;
        }
        nodes.push(info);
    }
    out.put(
        lines.len() as u32 + 1,
        "return { nodes = __nodes, declares = __declares }",
    );
    // The declares go on the first line, right after the tables they fill (a node's header
    // may follow on that line).
    let mut decl = String::new();
    for (name, v) in &declares {
        let lit = match v {
            Json::String(s) => lua_str(s),
            Json::Bool(b) => b.to_string(),
            Json::Number(n) => n.to_string(),
            _ => "nil".into(),
        };
        decl.push_str(&format!(" __declares[{}] = {lit}", lua_str(name)));
    }
    out.slots[0].insert_str(HEAD.len(), &decl);
    let mut seen = HashMap::new();
    for n in &nodes {
        if n.when.is_empty()
            && let Some(first) = seen.insert(n.title.clone(), n.line)
        {
            problems.push(Problem {
                line: n.line,
                message: format!(
                    "a second node called '{}' (line {first}); nodes sharing a title need when: \
                     headers",
                    n.title
                ),
            });
        }
    }
    let titles: Vec<&str> = nodes.iter().map(|n| n.title.as_str()).collect();
    for n in &nodes {
        for (target, _, line) in &n.links {
            if !target.starts_with('{') && !titles.contains(&target.as_str()) {
                problems.push(Problem {
                    line: *line,
                    message: format!("there is no node called '{target}' in this file"),
                });
            }
        }
    }
    Compiled {
        luau: out.slots.join("\n"),
        nodes,
        declares,
        problems,
    }
}

/// The `if` guard of a line: its condition and `once`.
fn guard(
    cond: &Option<String>,
    once: bool,
    path: &str,
    n: u32,
    problems: &mut Vec<Problem>,
) -> Option<String> {
    let c = cond.as_ref().map(|c| {
        compile_expr(c).unwrap_or_else(|e| {
            problems.push(Problem {
                line: n,
                message: e,
            });
            "false".into()
        })
    });
    let o = once.then(|| format!("S:once({})", lua_str(&format!("{path}:{n}"))));
    match (c, o) {
        (Some(c), Some(o)) => Some(format!("({c}) and {o}")),
        (Some(c), None) => Some(c),
        (None, Some(o)) => Some(o),
        (None, None) => None,
    }
}

/// Closes the option groups at or right of `indent`: their last body, then the pick.
fn close_choices(out: &mut Out, stack: &mut Vec<Open>, indent: usize, n: u32) {
    while let Some(Open::Choices {
        indent: at, group, ..
    }) = stack.last()
    {
        if *at < indent {
            break;
        }
        let code = if *group {
            "end) local __r = __g:run() if __r then return __r end end"
        } else {
            "end) local __r = __o:pick() if __r then return __r end end"
        };
        // After the last body's code (on its line), before the line that ends the options.
        let line = (out.cur.max(1) as u32).min(n.max(1));
        out.put(line, code);
        stack.pop();
    }
}

#[allow(clippy::too_many_arguments)]
fn command(
    cmd: &str,
    n: u32,
    path: &str,
    out: &mut Out,
    stack: &mut Vec<Open>,
    info: &mut NodeInfo,
    declares: &mut Vec<(String, Json)>,
    problems: &mut Vec<Problem>,
) {
    let (word, rest) = match cmd.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim()),
        None => (cmd, ""),
    };
    let mut err = |m: String| {
        problems.push(Problem {
            line: n,
            message: m,
        })
    };
    let expr = |src: &str, err: &mut dyn FnMut(String)| match compile_expr(src) {
        Ok(e) => e,
        Err(e) => {
            err(e);
            "nil".into()
        }
    };
    match word {
        "set" => {
            let (var, op, value) = match split_assign(rest) {
                Some(x) => x,
                None => {
                    err(format!(
                        "<<set {rest}>>: write <<set $name to value>> (or = value)"
                    ));
                    return;
                }
            };
            let e = expr(&value, &mut err);
            let target = format!("F[{}]", lua_str(&var));
            let code = match op {
                "+=" => format!("{target} = S.add({target}, {e})"),
                "-=" => format!("{target} = S.sub({target}, {e})"),
                "*=" => format!("{target} = S.mul({target}, {e})"),
                "/=" => format!("{target} = S.div({target}, {e})"),
                _ => format!("{target} = {e}"),
            };
            out.put(n, &code);
        }
        "declare" => match split_assign(rest) {
            Some((var, _, value)) => {
                let v = match lex(&value).ok().as_deref() {
                    Some([Tok::Num(x)]) => json_num(*x),
                    Some([Tok::Op("-"), Tok::Num(x)]) => json_num(-x),
                    Some([Tok::Str(s)]) => json!(s),
                    Some([Tok::Ident(b)]) if b == "true" || b == "false" => json!(b == "true"),
                    _ => {
                        err(format!(
                            "<<declare ${var} = {value}>>: the value must be a number, text, true \
                             or false"
                        ));
                        return;
                    }
                };
                declares.push((var, v));
            }
            None => err(format!(
                "<<declare {rest}>>: write <<declare $name = value>>"
            )),
        },
        "if" => {
            let e = expr(rest, &mut err);
            out.put(n, &format!("if {e} then"));
            stack.push(Open::If {
                line: n,
                once: false,
            });
        }
        "elseif" => {
            if !matches!(stack.last(), Some(Open::If { once: false, .. })) {
                err("an <<elseif>> with no <<if>> before it".into());
                return;
            }
            let e = expr(rest, &mut err);
            out.put(n, &format!("elseif {e} then"));
        }
        "else" => {
            if !matches!(stack.last(), Some(Open::If { .. })) {
                err("an <<else>> with no <<if>> or <<once>> before it".into());
                return;
            }
            out.put(n, "else");
        }
        "endif" | "endonce" => {
            if !matches!(stack.last(), Some(Open::If { .. })) {
                err(format!("an <<{word}>> with nothing to end"));
                return;
            }
            stack.pop();
            out.put(n, "end");
        }
        "once" => {
            let key = lua_str(&format!("{path}:{n}"));
            let code = match rest.strip_prefix("if ") {
                Some(c) => format!("if ({}) and S:once({key}) then", expr(c, &mut err)),
                None => format!("if S:once({key}) then"),
            };
            out.put(n, &code);
            stack.push(Open::If {
                line: n,
                once: true,
            });
        }
        "jump" | "detour" => {
            let target = rest.trim();
            if target.is_empty() {
                err(format!("<<{word}>> needs a node's title"));
                return;
            }
            let t = if target.starts_with('{') && target.ends_with('}') {
                format!("S.text({})", expr(&target[1..target.len() - 1], &mut err))
            } else {
                lua_str(target)
            };
            let how = match stack.iter().rev().find_map(|o| match o {
                Open::Choices { group: false, .. } => Some(()),
                _ => None,
            }) {
                Some(()) => info
                    .lines
                    .iter()
                    .rev()
                    .find_map(|(_, l)| l.strip_prefix("-> ").map(str::to_string))
                    .unwrap_or_else(|| word.to_string()),
                None => word.to_string(),
            };
            info.links.push((target.to_string(), how, n));
            if word == "jump" {
                out.put(n, &format!("do return S:jump({t}) end"));
            } else {
                out.put(
                    n,
                    &format!("do local __r = S:detour({t}) if __r then return __r end end"),
                );
            }
        }
        "return" => out.put(n, "do return S:back() end"),
        "stop" => out.put(n, "do return S:stop() end"),
        "wait" => {
            let e = expr(rest, &mut err);
            out.put(n, &format!("S:wait({e}, {n})"));
        }
        _ => {
            let args: Vec<String> = command_args(rest)
                .into_iter()
                .map(|a| match a {
                    Arg::Word(w) => match w.parse::<f64>() {
                        Ok(x) => lua_num(x),
                        Err(_) => match w.as_str() {
                            "true" | "false" => w,
                            _ => lua_str(&w),
                        },
                    },
                    Arg::Quoted(s) => lua_str(&s),
                    Arg::Expr(e) => format!("({})", expr(&e, &mut err)),
                })
                .collect();
            info.lines.push((n, format!("<<{cmd}>>")));
            if (word == "bark" || word == "barks")
                && let Some(target) = rest.split_whitespace().next()
            {
                info.links.push((target.to_string(), word.to_string(), n));
            }
            out.put(
                n,
                &format!(
                    "do local __r = S:command({}, {{{}}}, {n}) if __r then return __r end end",
                    lua_str(word),
                    args.join(", ")
                ),
            );
        }
    }
}

/// `$name to value`, `$name = value`, `$name += value`...
fn split_assign(rest: &str) -> Option<(String, &'static str, String)> {
    let rest = rest.trim();
    let name_end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))?;
    let var = rest[..name_end].trim().strip_prefix('$')?.to_string();
    let after = rest[name_end..].trim_start();
    for op in ["+=", "-=", "*=", "/=", "to ", "="] {
        if let Some(v) = after.strip_prefix(op) {
            let op: &'static str = match op {
                "to " | "=" => "=",
                "+=" => "+=",
                "-=" => "-=",
                "*=" => "*=",
                _ => "/=",
            };
            return Some((var, op, v.trim().to_string()));
        }
    }
    None
}

enum Arg {
    Word(String),
    Quoted(String),
    Expr(String),
}

fn command_args(rest: &str) -> Vec<Arg> {
    let mut out = Vec::new();
    let mut chars = rest.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut s = String::new();
            for ch in chars.by_ref() {
                if ch == '"' {
                    break;
                }
                s.push(ch);
            }
            out.push(Arg::Quoted(s));
        } else if c == '{' {
            chars.next();
            let mut s = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                s.push(ch);
            }
            out.push(Arg::Expr(s));
        } else {
            let mut s = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                s.push(ch);
                chars.next();
            }
            out.push(Arg::Word(s));
        }
    }
    out
}

// ------------------------------------------------------------------------------ files

/// A `.yarn` file as last compiled: its Luau, nodes and problems, and the file's stamp.
pub struct YarnFile {
    pub compiled: Compiled,
    pub version: u64,
    stamp: Option<(SystemTime, u64)>,
}

/// The `.yarn` files conversations have used (compiled once, again when the file changes).
#[derive(Resource, Default)]
pub struct DialogueFiles {
    map: HashMap<String, YarnFile>,
    /// Changes whenever a file is compiled again.
    pub version: u64,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// `dialogue/keeper` -> `dialogue/keeper.yarn`.
pub fn normalize_yarn(path: &str) -> String {
    let mut p = path.trim().replace('\\', "/");
    while let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    let p = p.trim_start_matches('/');
    if p.ends_with(".yarn") {
        p.to_string()
    } else {
        format!("{p}.yarn")
    }
}

/// The first problem in a `.yarn` file's text, as the Scripts tab marks it: (line, message).
pub fn check_yarn(text: &str, path: &str) -> Option<(u32, String)> {
    let c = compile_yarn(text, &normalize_yarn(path));
    c.problems.first().map(|p| (p.line, p.message.clone()))
}

/// The first problem in a `.quest` file's text: bad JSON, or a condition that doesn't parse.
pub fn check_quest(text: &str) -> Option<(u32, String)> {
    let def: QuestDef = match serde_json::from_str(text) {
        Ok(d) => d,
        Err(e) => return Some((e.line() as u32, e.to_string())),
    };
    let line_of = |needle: &str| {
        text.lines()
            .position(|l| l.contains(needle))
            .map_or(0, |i| i as u32 + 1)
    };
    let mut conds: Vec<(&str, &Option<String>)> = vec![
        ("start_when", &def.start_when),
        ("fail_when", &def.fail_when),
    ];
    for o in &def.objectives {
        conds.push(("done_when", &o.done_when));
        conds.push(("progress", &o.progress));
    }
    for (what, c) in conds {
        if let Some(c) = c
            && let Err(e) = compile_expr(c)
        {
            return Some((line_of(c), format!("{what}: {e}")));
        }
    }
    None
}

impl DialogueFiles {
    /// The file at `path` (in `root`), compiled again if it changed on disk.
    pub fn get(&mut self, root: &Path, path: &str) -> Result<&YarnFile, String> {
        let path = normalize_yarn(path);
        let full = root.join(&path);
        let now = stamp(&full);
        let fresh = self.map.get(&path).is_some_and(|f| f.stamp == now);
        if !fresh {
            let text = std::fs::read_to_string(&full)
                .map_err(|e| format!("{path}: can't read it ({e})"))?;
            self.version += 1;
            let compiled = compile_yarn(&text, &path);
            self.map.insert(
                path.clone(),
                YarnFile {
                    compiled,
                    version: self.version,
                    stamp: now,
                },
            );
        }
        Ok(&self.map[&path])
    }

    /// Compiles `text` as the file at `path` without reading it (the editor, as it edits).
    pub fn put(&mut self, path: &str, text: &str) {
        let path = normalize_yarn(path);
        self.version += 1;
        let compiled = compile_yarn(text, &path);
        self.map.insert(
            path,
            YarnFile {
                compiled,
                version: self.version,
                stamp: None,
            },
        );
    }
}

/// Every file with `ext` under `root/folder`, as paths relative to `root`.
pub fn find_files(root: &Path, folder: &str, ext: &str) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(dir: &Path, root: &Path, ext: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, ext, out);
            } else if p.extension().is_some_and(|x| x == ext)
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    walk(&root.join(folder), root, ext, &mut out);
    out.sort();
    out
}

// ------------------------------------------------------------------------------ quests

/// One step of a quest (a `.quest` file's `objectives`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectiveDef {
    pub text: String,
    /// Done once this is true (`"$runes_lit >= 3"`).
    pub done_when: Option<String>,
    /// How far along it is, counted against `count` (`"$runes_lit"`).
    pub progress: Option<String>,
    pub count: u32,
    pub optional: bool,
    /// The map place it happens at (the UI's map waypoint).
    pub place: String,
}

/// A `.quest` file: a quest that moves on by itself as the facts change (Skyrim's stages, with
/// conditions instead of calls to SetStage).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuestDef {
    pub id: String,
    pub title: String,
    /// main, side or contract.
    pub kind: String,
    pub giver: String,
    pub giver_title: String,
    pub giver_portrait: String,
    pub region: String,
    pub summary: String,
    /// Starts by itself once this is true (else from dialogue, a script or the API).
    pub start_when: Option<String>,
    /// Fails once this is true.
    pub fail_when: Option<String>,
    /// Facts it starts with, unless they are set already.
    pub facts: BTreeMap<String, Json>,
    pub objectives: Vec<ObjectiveDef>,
    /// As the UI's journal shows them (`{"xp": 150}`).
    pub rewards: Json,
    /// Facts set when it is completed.
    pub on_done: BTreeMap<String, Json>,
    /// Facts set when it fails.
    pub on_fail: BTreeMap<String, Json>,
    /// Tracked on the HUD when it starts.
    pub track: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestState {
    #[default]
    Inactive,
    Active,
    Completed,
    Failed,
}

impl QuestState {
    pub fn name(self) -> &'static str {
        match self {
            QuestState::Inactive => "inactive",
            QuestState::Active => "active",
            QuestState::Completed => "completed",
            QuestState::Failed => "failed",
        }
    }
}

/// Where a quest is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuestRun {
    pub state: QuestState,
    pub done: Vec<bool>,
    pub progress: Vec<u32>,
    /// The last error evaluating its conditions.
    #[serde(skip)]
    pub error: Option<String>,
}

/// The quests: their files and where each one is. Saved with the facts.
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QuestBook {
    pub runs: BTreeMap<String, QuestRun>,
    #[serde(skip)]
    pub defs: Vec<QuestDef>,
    #[serde(skip)]
    pub problems: Vec<(String, Problem)>,
    /// The quest files' Luau (each condition a function) and its version.
    #[serde(skip)]
    luau: String,
    #[serde(skip)]
    pub version: u64,
    #[serde(skip)]
    stamps: Vec<(String, Option<(SystemTime, u64)>)>,
    #[serde(skip)]
    checked: f64,
    /// Changes for the UI's journal, sent after the frame's tick.
    #[serde(skip)]
    mirror: Vec<(String, Json)>,
}

impl QuestBook {
    pub fn def(&self, id: &str) -> Option<&QuestDef> {
        self.defs.iter().find(|d| d.id == id)
    }

    pub fn state(&self, id: &str) -> QuestState {
        self.runs.get(id).map(|r| r.state).unwrap_or_default()
    }

    /// Reads the quest files under `root` again if any changed.
    pub fn reload(&mut self, root: &Path) {
        let files = find_files(root, QUEST_FOLDER, "quest");
        let stamps: Vec<_> = files
            .iter()
            .map(|f| (f.clone(), stamp(&root.join(f))))
            .collect();
        if stamps == self.stamps && self.version > 0 {
            return;
        }
        self.stamps = stamps;
        self.defs.clear();
        self.problems.clear();
        for f in files {
            let text = std::fs::read_to_string(root.join(&f)).unwrap_or_default();
            match serde_json::from_str::<QuestDef>(&text) {
                Ok(mut d) => {
                    if d.id.is_empty() {
                        d.id = Path::new(&f)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                    }
                    self.defs.push(d);
                }
                Err(e) => self.problems.push((
                    f.clone(),
                    Problem {
                        line: e.line() as u32,
                        message: e.to_string(),
                    },
                )),
            }
        }
        self.compile();
    }

    /// Adds or replaces a quest (tests, agents).
    pub fn put(&mut self, def: QuestDef) {
        self.defs.retain(|d| d.id != def.id);
        self.defs.push(def);
        self.compile();
    }

    fn compile(&mut self) {
        let mut src = String::from("return {\n");
        let cond = |what: &str, e: &Option<String>, id: &str, problems: &mut Vec<_>| match e {
            Some(e) => match compile_expr(e) {
                Ok(c) => format!("{what} = function(S, F) return {c} end, "),
                Err(m) => {
                    problems.push((
                        id.to_string(),
                        Problem {
                            line: 0,
                            message: format!("{what}: {m}"),
                        },
                    ));
                    String::new()
                }
            },
            None => String::new(),
        };
        for d in &self.defs {
            src.push_str(&format!("[{}] = {{ ", lua_str(&d.id)));
            src.push_str(&cond(
                "start_when",
                &d.start_when,
                &d.id,
                &mut self.problems,
            ));
            src.push_str(&cond("fail_when", &d.fail_when, &d.id, &mut self.problems));
            src.push_str("objectives = { ");
            for o in &d.objectives {
                src.push_str("{ ");
                src.push_str(&cond("done", &o.done_when, &d.id, &mut self.problems));
                src.push_str(&cond("n", &o.progress, &d.id, &mut self.problems));
                src.push_str("}, ");
            }
            src.push_str("} },\n");
        }
        src.push('}');
        self.luau = src;
        // Never repeats, so a book loaded from a save is compiled into the VM again.
        static VERSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        self.version = VERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    }

    /// Starts a quest: its facts, then active (and into the journal).
    pub fn start(&mut self, facts: &mut StoryFacts, id: &str) -> Result<(), String> {
        let def = self
            .def(id)
            .cloned()
            .ok_or_else(|| format!("there is no quest '{id}' (quests/{id}.quest)"))?;
        if matches!(
            self.state(id),
            QuestState::Active | QuestState::Completed | QuestState::Failed
        ) {
            return Ok(());
        }
        for (k, v) in &def.facts {
            facts.default_to(k, v.clone());
        }
        let n = def.objectives.len();
        self.runs.insert(
            id.to_string(),
            QuestRun {
                state: QuestState::Active,
                done: vec![false; n],
                progress: vec![0; n],
                error: None,
            },
        );
        facts.version += 1;
        let mut q = self.ui_quest(&def);
        if def.track || def.kind != "contract" {
            q["track"] = json!(true);
        }
        self.mirror.push(("quest.add".into(), q));
        Ok(())
    }

    /// Ends a quest, setting its `on_done` or `on_fail` facts.
    pub fn finish(&mut self, facts: &mut StoryFacts, id: &str, failed: bool) {
        let Some(def) = self.def(id).cloned() else {
            return;
        };
        let run = self.runs.entry(id.to_string()).or_default();
        if matches!(run.state, QuestState::Completed | QuestState::Failed) {
            return;
        }
        run.state = if failed {
            QuestState::Failed
        } else {
            QuestState::Completed
        };
        if !failed {
            run.done = vec![true; def.objectives.len()];
        }
        for (k, v) in if failed { &def.on_fail } else { &def.on_done } {
            facts.set(k, v.clone());
        }
        facts.version += 1;
        self.mirror.push((
            "quest.complete".into(),
            json!({ "id": id, "failed": failed }),
        ));
    }

    /// The quest as the UI's journal takes it (`quest.add`).
    fn ui_quest(&self, def: &QuestDef) -> Json {
        let run = self.runs.get(&def.id).cloned().unwrap_or_default();
        let objectives: Vec<Json> = def
            .objectives
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let count = o.count.max(1);
                let done = run.done.get(i).copied().unwrap_or(false);
                let progress = if done {
                    count
                } else {
                    run.progress.get(i).copied().unwrap_or(0).min(count - 1)
                };
                json!({
                    "text": o.text,
                    "count": count,
                    "progress": progress,
                    "optional": o.optional,
                    "place": o.place,
                })
            })
            .collect();
        json!({
            "id": def.id,
            "title": def.title,
            "kind": if def.kind.is_empty() { "main" } else { def.kind.as_str() },
            "giver": def.giver,
            "giver_title": def.giver_title,
            "giver_portrait": def.giver_portrait,
            "region": def.region,
            "summary": def.summary,
            "objectives": objectives,
            "rewards": if def.rewards.is_null() { json!({}) } else { def.rewards.clone() },
        })
    }

    /// Applies what the conditions say now (from `__story_tick`).
    fn apply(&mut self, facts: &mut StoryFacts, id: &str, eval: &Json) {
        let Some(def) = self.def(id).cloned() else {
            return;
        };
        if let Some(e) = eval.get("error").and_then(Json::as_str) {
            let r = self.runs.entry(id.to_string()).or_default();
            if r.error.as_deref() != Some(e) {
                engine_core::log::warn!("quest {id}: {e}");
                r.error = Some(e.to_string());
            }
            return;
        }
        let state = self.state(id);
        if state == QuestState::Inactive {
            if eval["start"].as_bool() == Some(true) {
                let _ = self.start(facts, id);
            } else {
                return;
            }
        }
        if self.state(id) != QuestState::Active {
            return;
        }
        if eval["fail"].as_bool() == Some(true) {
            self.finish(facts, id, true);
            return;
        }
        let run = self.runs.get_mut(id).expect("started above");
        run.error = None;
        let mut changed = Vec::new();
        for (i, o) in def.objectives.iter().enumerate() {
            let count = o.count.max(1);
            let n = eval["n"][i].as_f64().unwrap_or(0.0).max(0.0) as u32;
            let mut done = eval["done"][i].as_bool().unwrap_or(false);
            if o.done_when.is_none() && o.progress.is_some() {
                done = n >= count;
            }
            // A step once done stays done (the journal never takes it back).
            let done = done || run.done[i];
            if done != run.done[i] || n != run.progress[i] {
                run.done[i] = done;
                run.progress[i] = n;
                changed.push(i);
            }
        }
        let all = def
            .objectives
            .iter()
            .zip(&run.done)
            .all(|(o, d)| o.optional || *d);
        if !changed.is_empty() {
            facts.version += 1;
            for i in changed {
                let count = def.objectives[i].count.max(1);
                let p = if run.done[i] {
                    count
                } else {
                    run.progress[i].min(count - 1)
                };
                self.mirror.push((
                    "quest.progress".into(),
                    json!({ "id": id, "objective": i, "progress": p }),
                ));
            }
        }
        if all {
            self.finish(facts, id, false);
        }
    }

    /// Ids worth evaluating: active quests, and inactive ones that start by themselves.
    fn pending(&self) -> Vec<String> {
        self.defs
            .iter()
            .filter(|d| match self.state(&d.id) {
                QuestState::Active => true,
                QuestState::Inactive => d.start_when.is_some(),
                _ => false,
            })
            .map(|d| d.id.clone())
            .collect()
    }
}

// ------------------------------------------------------------------------- conversations

/// What a conversation is doing (for the editor and `story.state`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunInfo {
    pub id: u64,
    pub path: String,
    pub node: String,
    /// The `title:` line of the node it is in (which of a node group's nodes).
    pub node_line: u32,
    /// The line shown last.
    pub speaker: String,
    pub text: String,
    /// The options shown now (their text).
    pub options: Vec<String>,
    /// What it waits for: "options", "spots" (a choice made by walking), "reading", "until",
    /// "time" or "".
    pub waiting: String,
    /// The nodes it went through, in order.
    pub taken: Vec<String>,
    /// What was said and picked, in order (the last 60).
    pub lines: Vec<RunLine>,
    pub done: bool,
    pub error: Option<String>,
    /// Barks (`<<barks>>`, `dialogue.bark`): said over the speakers' heads while play goes on.
    pub bark: bool,
    /// The skill checks among the options shown now.
    pub checks: Vec<CheckInfo>,
    /// Options left out for now: white checks failed, until the skill is higher.
    pub shut: Vec<ShutInfo>,
    /// The last check rolled.
    pub roll: Option<RollInfo>,
}

/// A skill check on an option (`#check:persuade:8`), as the player sees it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckInfo {
    /// The option (1 is the first).
    pub option: usize,
    pub skill: String,
    /// As written, and after the difficulty setting.
    pub difficulty: f64,
    pub target: f64,
    /// The chooser's skill.
    pub value: f64,
    /// Everything else added: (amount, why).
    pub bonuses: Vec<(f64, String)>,
    /// "2d6" or "d20".
    pub dice: String,
    pub chance: f64,
    pub red: bool,
}

/// An option a failed white check keeps shut.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShutInfo {
    pub text: String,
    pub skill: String,
    /// It opens once the skill is above this.
    pub above: f64,
}

/// A check rolled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RollInfo {
    pub skill: String,
    pub option: String,
    /// The dice (none when the result was forced or the conversation screen rolled).
    pub dice: Vec<u32>,
    /// Skill and bonuses.
    pub modifier: f64,
    pub total: f64,
    pub target: f64,
    pub passed: bool,
    /// How it was decided when not by these dice: "forced", "screen".
    pub how: String,
}

/// A line said in a conversation, or an option picked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunLine {
    pub speaker: String,
    pub text: String,
    /// The player picked this option.
    pub choice: bool,
    /// A check's roll (`text` says it; `passed` is how it went).
    pub roll: bool,
    pub passed: bool,
    /// The .yarn line.
    pub line: u32,
}

/// The conversations running, and picks waiting to be taken by them.
#[derive(Resource, Default)]
pub struct StoryRuns {
    pub runs: Vec<RunInfo>,
    picks: HashMap<u64, usize>,
    /// Checks forced to pass or fail with a pick (`story.choose {pass}`).
    forced: HashMap<u64, bool>,
    stops: Vec<u64>,
    /// The last id given out (ids stay unique when Stop drops the script VM).
    next_id: u64,
    /// Changes whenever a run reports.
    pub version: u64,
}

impl StoryRuns {
    pub fn get(&self, id: u64) -> Option<&RunInfo> {
        self.runs.iter().find(|r| r.id == id)
    }

    /// The newest conversation still going (not a bark).
    pub fn current(&self) -> Option<&RunInfo> {
        self.runs.iter().rev().find(|r| !r.done && !r.bark)
    }

    /// Picks option `index` (from 1) in conversation `id`.
    pub fn choose(&mut self, id: u64, index: usize) {
        self.picks.insert(id, index);
    }

    /// Picks option `index`, its check (if it has one) passing or failing as `pass` says.
    pub fn choose_forced(&mut self, id: u64, index: usize, pass: Option<bool>) {
        self.picks.insert(id, index);
        match pass {
            Some(p) => self.forced.insert(id, p),
            None => self.forced.remove(&id),
        };
    }
}

// ---------------------------------------------------------------------------------- barks

/// A bark line being said: a speech bubble over its speaker for a few seconds.
#[derive(Clone, Debug, Serialize)]
pub struct BarkLine {
    /// The bark's run (`StoryRuns`).
    pub run: u64,
    pub speaker: String,
    #[serde(skip)]
    pub entity: Option<Entity>,
    pub text: String,
    /// Real seconds it has been up, and how long it stays.
    pub age: f32,
    pub secs: f32,
    #[serde(skip)]
    marker: Option<Entity>,
}

/// The barks being said now (the UI's world markers show them in the game, the editor draws
/// them in its view).
#[derive(Resource, Default)]
pub struct StoryBarks {
    pub lines: Vec<BarkLine>,
    /// Changes whenever a line comes or goes.
    pub version: u64,
}

/// The entity a line's speaker names: one of `owner`'s descendants of that name, else the
/// first in the world.
fn speaker_entity(world: &mut World, name: &str, owner: Option<Entity>) -> Option<Entity> {
    if let Some(o) = owner {
        let mut stack = vec![o];
        while let Some(e) = stack.pop() {
            if world.get::<Name>(e).map(|n| n.as_str()) == Some(name) {
                return Some(e);
            }
            if let Some(ch) = world.get::<Children>(e) {
                stack.extend(ch.iter());
            }
        }
    }
    named(world, name)
}

/// `speaker` says `text` for `secs` real seconds, over their head (over `owner` when no one
/// has that name); a line they were still saying goes.
pub fn bark_say(
    world: &mut World,
    run: u64,
    speaker: &str,
    text: &str,
    secs: f32,
    owner: Option<Entity>,
) {
    let entity = speaker_entity(world, speaker, owner).or(owner);
    let gone: Vec<Option<Entity>> = {
        let mut barks = world.get_resource_or_insert_with(StoryBarks::default);
        let mut gone = Vec::new();
        barks.lines.retain(|l| {
            let same = l.entity.is_some() && l.entity == entity || l.speaker == speaker;
            if same {
                gone.push(l.marker);
            }
            !same
        });
        gone
    };
    for m in gone.into_iter().flatten() {
        if let Ok(e) = world.get_entity_mut(m) {
            e.despawn();
        }
    }
    let at = entity.and_then(|e| crate::cutscene::world_pose(world, e).map(|t| t.translation));
    let has_markers = world
        .get_resource::<engine_core::registry::ApiRegistry>()
        .is_some_and(|r| r.get("hud.marker").is_some());
    let marker = match at {
        Some(p) if has_markers => call_api(
            world,
            "hud.marker",
            json!({
                "title": speaker,
                "subtitle": text,
                "icon": "dot",
                "color": "#ece6d6",
                "position": [p.x, p.y + 2.05, p.z],
                "distance": false,
                "always": true,
            }),
        )
        .ok()
        .and_then(|out| out["entity"].as_u64())
        .and_then(Entity::try_from_bits),
        _ => None,
    };
    let mut barks = world.get_resource_or_insert_with(StoryBarks::default);
    barks.lines.push(BarkLine {
        run,
        speaker: speaker.to_string(),
        entity,
        text: text.to_string(),
        age: 0.0,
        secs: secs.max(0.5),
        marker,
    });
    barks.version += 1;
}

/// Lines whose time is up go (every frame, real time).
fn age_barks(world: &mut World) {
    let dt = world
        .get_resource::<engine_core::prelude::Time>()
        .map(|t| t.real_delta_secs())
        .unwrap_or(1.0 / 60.0)
        .min(0.1);
    let Some(mut barks) = world.get_resource_mut::<StoryBarks>() else {
        return;
    };
    if barks.lines.is_empty() {
        return;
    }
    let mut gone = Vec::new();
    barks.lines.retain_mut(|l| {
        l.age += dt;
        let keep = l.age < l.secs;
        if !keep {
            gone.push(l.marker);
        }
        keep
    });
    if !gone.is_empty() {
        barks.version += 1;
    }
    for m in gone.into_iter().flatten() {
        if let Ok(e) = world.get_entity_mut(m) {
            e.despawn();
        }
    }
}

/// Every bark goes at once (the editor's Stop, scripts held).
pub fn clear_barks(world: &mut World) {
    let Some(mut barks) = world.get_resource_mut::<StoryBarks>() else {
        return;
    };
    if barks.lines.is_empty() {
        return;
    }
    let all = std::mem::take(&mut barks.lines);
    barks.version += 1;
    for m in all.into_iter().filter_map(|l| l.marker) {
        if let Ok(e) = world.get_entity_mut(m) {
            e.despawn();
        }
    }
}

// ------------------------------------------------------------------------- the runtime

fn json_to_lua(lua: &mlua::Lua, v: &Json) -> mlua::Result<mlua::Value> {
    crate::convert::to_lua(lua, v)
}

/// The Luau side's helpers (`__story_*`, `__facts`, `__quest_*`), hidden after the preludes
/// load.
pub(crate) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    use crate::access::{read_world_as, shared, with_world_as};
    use mlua::{Table, Value};
    let g = lua.globals();

    // facts: a table whose reads and writes go to StoryFacts.
    let facts = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (_, k): (Table, String)| {
            let v = read_world_as(lua, "facts", |w| {
                Ok(w.get_resource::<StoryFacts>()
                    .and_then(|f| f.get(&k).cloned()))
            })?;
            match v {
                Some(v) => json_to_lua(lua, &v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    meta.set(
        "__newindex",
        lua.create_function(|lua, (_, k, v): (Table, String, Value)| {
            let j = crate::convert::to_json(&v)?;
            with_world_as(lua, "facts", |w| {
                w.get_resource_or_insert_with(StoryFacts::default)
                    .set(&k, j);
                Ok(())
            })
        })?,
    )?;
    meta.set(
        "__iter",
        lua.create_function(|lua, _: Table| {
            let all = read_world_as(lua, "facts", |w| {
                Ok(w.get_resource::<StoryFacts>()
                    .map(|f| f.values.clone())
                    .unwrap_or_default())
            })?;
            let t = lua.create_table()?;
            for (k, v) in all {
                t.set(k, json_to_lua(lua, &v)?)?;
            }
            let next: mlua::Function = lua.globals().get("next")?;
            Ok((next, t))
        })?,
    )?;
    facts.set_metatable(Some(meta))?;
    g.set("facts", facts)?;

    g.set(
        "__story_module",
        lua.create_function(|lua, path: String| {
            let root = read_world_as(lua, "dialogue", |w| Ok(crate::scripts_root(w)))?;
            let (src, version, problems, declares, name) = with_world_as(lua, "dialogue", |w| {
                let mut files = w.get_resource_or_insert_with(DialogueFiles::default);
                let f = files.get(&root, &path).map_err(mlua::Error::runtime)?;
                Ok((
                    f.compiled.luau.clone(),
                    f.version,
                    f.compiled.problems.clone(),
                    f.compiled.declares.clone(),
                    normalize_yarn(&path),
                ))
            })?;
            if let Some(p) = problems.first() {
                return Err(mlua::Error::runtime(format!(
                    "{name}:{}: {}{}",
                    p.line,
                    p.message,
                    if problems.len() > 1 {
                        format!(" (and {} more)", problems.len() - 1)
                    } else {
                        String::new()
                    }
                )));
            }
            with_world_as(lua, "dialogue", |w| {
                let mut facts = w.get_resource_or_insert_with(StoryFacts::default);
                for (k, v) in declares {
                    facts.default_to(&k, v);
                }
                Ok(())
            })?;
            let module: Table = lua.load(&src).set_name(format!("@{name}")).eval()?;
            Ok((module, version, name))
        })?,
    )?;
    g.set(
        "__story_version",
        lua.create_function(|lua, path: String| {
            let root = read_world_as(lua, "dialogue", |w| Ok(crate::scripts_root(w)))?;
            with_world_as(lua, "dialogue", |w| {
                let mut files = w.get_resource_or_insert_with(DialogueFiles::default);
                Ok(files.get(&root, &path).map(|f| f.version).unwrap_or(0))
            })
        })?,
    )?;
    g.set(
        "__story_visit",
        lua.create_function(|lua, (title, add): (String, bool)| {
            with_world_as(lua, "visited", |w| {
                let mut f = w.get_resource_or_insert_with(StoryFacts::default);
                if add {
                    *f.visits.entry(title.clone()).or_default() += 1;
                    f.version += 1;
                }
                Ok(f.visits.get(&title).copied().unwrap_or(0))
            })
        })?,
    )?;
    g.set(
        "__story_once",
        lua.create_function(|lua, (key, mark): (String, bool)| {
            with_world_as(lua, "once", |w| {
                let mut f = w.get_resource_or_insert_with(StoryFacts::default);
                let seen = f.once.contains(&key);
                if mark && !seen {
                    f.once.push(key);
                    f.version += 1;
                }
                Ok(seen)
            })
        })?,
    )?;
    g.set(
        "__story_owner",
        lua.create_function(|lua, ()| {
            let owner = shared(lua).owner;
            match owner {
                Some(e) => lua
                    .create_userdata(crate::entity::EntityRef(e))
                    .map(Value::UserData),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    g.set(
        "__story_set_owner",
        lua.create_function(|lua, owner: Value| {
            let e = match &owner {
                Value::UserData(ud) => ud.borrow::<crate::entity::EntityRef>().ok().map(|e| e.0),
                _ => None,
            };
            shared(lua).owner = e;
            Ok(())
        })?,
    )?;
    g.set(
        "__story_new_id",
        lua.create_function(|lua, ()| {
            with_world_as(lua, "dialogue", |w| {
                let mut runs = w.get_resource_or_insert_with(StoryRuns::default);
                runs.next_id += 1;
                Ok(runs.next_id)
            })
        })?,
    )?;
    g.set(
        "__story_has_api",
        lua.create_function(|lua, name: String| {
            read_world_as(lua, "dialogue", |w| {
                Ok(w.get_resource::<engine_core::registry::ApiRegistry>()
                    .is_some_and(|r| r.get(&name).is_some()))
            })
        })?,
    )?;
    g.set(
        "__story_error",
        lua.create_function(|lua, (path, message): (String, String)| {
            let owner = shared(lua).owner;
            crate::access::push_error(lua, Some(&path), owner, &message);
            Ok(())
        })?,
    )?;
    g.set(
        "__story_report",
        lua.create_function(|lua, (id, info): (u64, Table)| {
            let j = crate::convert::to_json(&Value::Table(info))?;
            with_world_as(lua, "dialogue", |w| {
                let mut runs = w.get_resource_or_insert_with(StoryRuns::default);
                let mut r: RunInfo = serde_json::from_value(j).unwrap_or_else(|e| {
                    engine_core::log::warn!("story: conversation {id}'s report: {e}");
                    RunInfo::default()
                });
                r.id = id;
                match runs.runs.iter_mut().find(|x| x.id == id) {
                    Some(x) => *x = r,
                    None => {
                        runs.runs.push(r);
                        // Keep the last few finished ones for the editor.
                        let done = runs.runs.iter().filter(|x| x.done).count();
                        if done > 8
                            && let Some(i) = runs.runs.iter().position(|x| x.done)
                        {
                            runs.runs.remove(i);
                        }
                    }
                }
                runs.version += 1;
                Ok(())
            })
        })?,
    )?;
    g.set(
        "__story_take",
        lua.create_function(|lua, id: u64| {
            with_world_as(lua, "dialogue", |w| {
                let mut runs = w.get_resource_or_insert_with(StoryRuns::default);
                let stop = runs.stops.contains(&id);
                runs.stops.retain(|s| *s != id);
                let pick = runs.picks.remove(&id);
                let forced = if pick.is_some() {
                    runs.forced.remove(&id)
                } else {
                    None
                };
                Ok((pick, stop, forced))
            })
        })?,
    )?;
    g.set(
        "__story_check_get",
        lua.create_function(|lua, key: String| {
            let v = read_world_as(lua, "dialogue", |w| {
                Ok(w.get_resource::<StoryFacts>()
                    .and_then(|f| f.checks.get(&key).cloned()))
            })?;
            match v {
                Some(v) => json_to_lua(lua, &v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    g.set(
        "__story_check_set",
        lua.create_function(|lua, (key, v): (String, Value)| {
            let j = crate::convert::to_json(&v)?;
            with_world_as(lua, "dialogue", |w| {
                let mut f = w.get_resource_or_insert_with(StoryFacts::default);
                if j.is_null() {
                    f.checks.remove(&key);
                } else {
                    f.checks.insert(key, j);
                }
                f.version += 1;
                Ok(())
            })
        })?,
    )?;
    g.set(
        "__story_luck",
        lua.create_function(|lua, op: String| {
            with_world_as(lua, "dialogue", |w| {
                let mut f = w.get_resource_or_insert_with(StoryFacts::default);
                match op.as_str() {
                    "fail" => f.luck += 1,
                    "pass" => f.luck = 0,
                    _ => return Ok(f.luck),
                }
                f.version += 1;
                Ok(f.luck)
            })
        })?,
    )?;
    g.set(
        "__story_settings",
        lua.create_function(|lua, ()| {
            let st = read_world_as(lua, "dialogue", |w| {
                Ok(w.get_resource::<StorySettings>()
                    .cloned()
                    .unwrap_or_default())
            })?;
            let t = lua.create_table()?;
            t.set("dice", st.dice.clone())?;
            t.set("difficulty", st.difficulty.clone())?;
            t.set("luck", st.luck)?;
            t.set("shift", st.shift())?;
            Ok(t)
        })?,
    )?;
    g.set(
        "__story_settings_set",
        lua.create_function(|lua, t: Table| {
            let j = crate::convert::to_json(&Value::Table(t))?;
            with_world_as(lua, "dialogue", |w| {
                apply_settings(w, &j)
                    .map(|_| ())
                    .map_err(mlua::Error::runtime)
            })
        })?,
    )?;
    g.set(
        "__story_real_dt",
        lua.create_function(|lua, ()| {
            read_world_as(lua, "dialogue", |w| {
                Ok(w.get_resource::<engine_core::prelude::Time>()
                    .map(|t| t.real_delta_secs() as f64)
                    .unwrap_or(1.0 / 60.0)
                    .min(0.1))
            })
        })?,
    )?;
    g.set(
        "__story_bark_say",
        lua.create_function(
            |lua, (run, speaker, text, secs, owner): (u64, String, String, f32, Value)| {
                let owner = match &owner {
                    Value::UserData(ud) => {
                        ud.borrow::<crate::entity::EntityRef>().ok().map(|e| e.0)
                    }
                    _ => None,
                };
                with_world_as(lua, "dialogue", |w| {
                    bark_say(w, run, &speaker, &text, secs, owner);
                    Ok(())
                })
            },
        )?,
    )?;
    g.set(
        "__story_dt",
        lua.create_function(|lua, ()| {
            read_world_as(lua, "dialogue", |w| {
                Ok(w.get_resource::<engine_core::prelude::Time>()
                    .map(|t| t.delta_secs() as f64)
                    .unwrap_or(1.0 / 60.0))
            })
        })?,
    )?;
    g.set(
        "__quest_module",
        lua.create_function(|lua, have: u64| {
            let (src, version) = read_world_as(lua, "quests", |w| {
                Ok(w.get_resource::<QuestBook>()
                    .map(|q| (q.luau.clone(), q.version))
                    .unwrap_or_default())
            })?;
            if version == have || src.is_empty() {
                return Ok((Value::Nil, version));
            }
            let t: Table = lua.load(&src).set_name("@quests").eval()?;
            Ok((Value::Table(t), version))
        })?,
    )?;
    g.set(
        "__quest_ctl",
        lua.create_function(|lua, (what, id): (String, String)| {
            with_world_as(lua, "quests", |w| {
                let root = crate::scripts_root(w);
                if let Some(mut book) = w.get_resource_mut::<QuestBook>() {
                    book.reload(&root);
                }
                let mut facts = w.remove_resource::<StoryFacts>().unwrap_or_default();
                let out = match w.get_resource_mut::<QuestBook>() {
                    Some(mut book) => match what.as_str() {
                        "start" => book.start(&mut facts, &id).map(|_| book.state(&id)),
                        "complete" => {
                            book.finish(&mut facts, &id, false);
                            Ok(book.state(&id))
                        }
                        "fail" => {
                            book.finish(&mut facts, &id, true);
                            Ok(book.state(&id))
                        }
                        _ => Ok(book.state(&id)),
                    },
                    None => Err("there are no quests".to_string()),
                };
                w.insert_resource(facts);
                out.map(|s| s.name()).map_err(mlua::Error::runtime)
            })
        })?,
    )?;
    Ok(())
}

/// Runs the conversations one frame and ticks the quests (after the scripts).
pub(crate) fn run_story(world: &mut World) {
    if world.contains_resource::<crate::ScriptsHeld>() {
        clear_barks(world);
        return;
    }
    age_barks(world);
    let now = world
        .get_resource::<engine_core::prelude::Time>()
        .map(|t| t.elapsed_secs() as f64)
        .unwrap_or(0.0);
    let root = crate::scripts_root(world);
    if let Some(mut book) = world.get_resource_mut::<QuestBook>()
        && (now - book.checked >= 0.5 || book.version == 0)
    {
        book.checked = now;
        book.reload(&root);
    }
    tick(world, false);
    send_to_journal(world);
}

/// Evaluates the quests' conditions and applies them, and (unless `quests_only`) runs the
/// conversations a frame: `story.luau`'s `__story_tick`.
fn tick(world: &mut World, quests_only: bool) {
    let pending = world
        .get_resource::<QuestBook>()
        .map(|b| b.pending())
        .unwrap_or_default();
    let talking = !quests_only
        && world
            .get_resource::<StoryRuns>()
            .is_some_and(|r| r.runs.iter().any(|x| !x.done));
    if pending.is_empty() && !talking {
        return;
    }
    let out = crate::with_runtime(world, |world, rt, _| {
        let _lent = crate::access::lend_world(&rt.lua, world);
        let f: mlua::Function = rt.lua.named_registry_value(TICK)?;
        let ids = rt.lua.create_sequence_from(pending.iter().cloned())?;
        let r: mlua::Value = f.call((ids, quests_only))?;
        crate::convert::to_json(&r)
    });
    let out = match out {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            engine_core::log::warn!("story: {e}");
            return;
        }
        Err(_) => return,
    };
    let evals = &out["quests"];
    let mut facts = world.remove_resource::<StoryFacts>().unwrap_or_default();
    if let Some(mut book) = world.get_resource_mut::<QuestBook>() {
        for id in &pending {
            if let Some(e) = evals.get(id) {
                book.apply(&mut facts, id, e);
            }
        }
    }
    world.insert_resource(facts);
    if quests_only {
        return;
    }
    // Conversations the VM no longer has (Stop dropped it) are over.
    let live: Vec<u64> = out["live"]
        .as_array()
        .map(|a| a.iter().filter_map(Json::as_u64).collect())
        .unwrap_or_default();
    if let Some(mut runs) = world.get_resource_mut::<StoryRuns>()
        && runs.runs.iter().any(|r| !r.done && !live.contains(&r.id))
    {
        for r in runs.runs.iter_mut() {
            if !r.done && !live.contains(&r.id) {
                r.done = true;
            }
        }
        runs.version += 1;
    }
}

/// Sends the quests' changes to the UI module's journal and HUD tracker (when it is there).
fn send_to_journal(world: &mut World) {
    let mirror = world
        .get_resource_mut::<QuestBook>()
        .map(|mut b| std::mem::take(&mut b.mirror))
        .unwrap_or_default();
    let has_ui = world
        .get_resource::<engine_core::registry::ApiRegistry>()
        .is_some_and(|r| r.get("quest.add").is_some());
    if has_ui {
        for (name, args) in mirror {
            if let Err(e) = call_api(world, &name, args) {
                engine_core::log::warn!("quests: {name}: {e}");
            }
        }
    }
}

/// `{dice, difficulty, luck}` (any of them) into [`StorySettings`].
fn apply_settings(world: &mut World, args: &Json) -> Result<StorySettings, String> {
    let mut st = world.get_resource_or_insert_with(StorySettings::default);
    if let Some(d) = args.get("dice").and_then(Json::as_str) {
        if d != "2d6" && d != "d20" {
            return Err(format!("dice \"{d}\": use \"2d6\" or \"d20\""));
        }
        st.dice = d.to_string();
    }
    if let Some(d) = args.get("difficulty").and_then(Json::as_str) {
        if !matches!(d, "easy" | "normal" | "hard") {
            return Err(format!(
                "difficulty \"{d}\": use \"easy\", \"normal\" or \"hard\""
            ));
        }
        st.difficulty = d.to_string();
    }
    if let Some(l) = args.get("luck").and_then(Json::as_bool) {
        st.luck = l;
    }
    Ok(st.clone())
}

/// The entity named `name`.
fn named(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query::<(Entity, &Name)>()
        .iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

// ------------------------------------------------------------------------------- the API

fn describe_quests(world: &World) -> Json {
    let Some(book) = world.get_resource::<QuestBook>() else {
        return json!([]);
    };
    let list: Vec<Json> = book
        .defs
        .iter()
        .map(|d| {
            let run = book.runs.get(&d.id).cloned().unwrap_or_default();
            json!({
                "id": d.id,
                "title": d.title,
                "state": run.state.name(),
                "start_when": d.start_when,
                "error": run.error,
                "objectives": d.objectives.iter().enumerate().map(|(i, o)| json!({
                    "text": o.text,
                    "done_when": o.done_when,
                    "progress": run.progress.get(i),
                    "count": o.count.max(1),
                    "done": run.done.get(i),
                    "optional": o.optional,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!(list)
}

pub(crate) fn register(app: &mut engine_core::App) {
    app.register_api(
        "story.facts",
        "The story's facts (what dialogue reads as $name and scripts as facts.name), how many \
         times each node was left, and the quests' states. {set: {name: value}} changes them \
         (null removes one); {clear: true} forgets them all.",
        r#"{"set": {"runes_lit": 2}}"#,
        |world, args| {
            let mut f = world.get_resource_or_insert_with(StoryFacts::default);
            if args.get("clear").and_then(Json::as_bool) == Some(true) {
                *f = StoryFacts {
                    version: f.version + 1,
                    ..Default::default()
                };
            }
            if let Some(set) = args.get("set").and_then(Json::as_object) {
                for (k, v) in set {
                    f.set(k, v.clone());
                }
            }
            let f = f.clone();
            Ok(json!({ "facts": f.values, "visited": f.visits }))
        },
    );
    app.register_api(
        "story.start",
        "Starts a conversation: {path: \"dialogue/keeper\", node: \"Start\", owner: \"Keeper\" \
         (the entity whose script runs its commands), chooser (the entity that walks to \
         #spot: options; the player), headless: false, step: false}. It shows \
         in the UI's conversation screen (the log with headless or without the UI module; \
         step waits after each line for story.choose) and returns its id.",
        r#"{"path": "dialogue/keeper", "node": "Start", "owner": "Keeper"}"#,
        |world, args| {
            let path = args
                .get("path")
                .and_then(Json::as_str)
                .ok_or("pass the .yarn file as \"path\"")?
                .to_string();
            let node = args
                .get("node")
                .and_then(Json::as_str)
                .unwrap_or("Start")
                .to_string();
            let headless = args
                .get("headless")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let step = args.get("step").and_then(Json::as_bool).unwrap_or(false);
            let owner = match args.get("owner").and_then(Json::as_str) {
                Some(name) => {
                    Some(named(world, name).ok_or_else(|| format!("there is no entity '{name}'"))?)
                }
                None => None,
            };
            let chooser = match args.get("chooser").and_then(Json::as_str) {
                Some(name) => {
                    Some(named(world, name).ok_or_else(|| format!("there is no entity '{name}'"))?)
                }
                None => None,
            };
            crate::with_runtime(world, |world, rt, _| {
                let _lent = crate::access::lend_world(&rt.lua, world);
                let start: mlua::Table = rt.lua.globals().get("dialogue")?;
                let f: mlua::Function = start.get("start")?;
                let opts = rt.lua.create_table()?;
                opts.set("headless", headless)?;
                opts.set("step", step)?;
                if let Some(e) = owner {
                    opts.set("owner", crate::entity::EntityRef(e))?;
                }
                if let Some(e) = chooser {
                    opts.set("chooser", crate::entity::EntityRef(e))?;
                }
                let h: mlua::Table = f.call((path, node, opts))?;
                h.get::<u64>("id")
            })?
            .map(|id| json!({ "id": id }))
            .map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "story.state",
        "The conversations: the newest one going (its node, the line shown, its options and \
         their skill checks with the odds, what it waits for, the nodes it went through, the \
         last roll) and the others, the choices made by walking open now (spots: each \
         option's spot, where it is, whether the player stands in it and how far its ring \
         filled, the seconds left, who is watching), rolls shown over rings, the barks being \
         said, and how checks roll. {id} for one.",
        "{}",
        |world, args| {
            let spots = crate::spots::describe(world);
            let rolls = crate::spots::describe_rolls(world);
            let barks = world
                .get_resource::<StoryBarks>()
                .map(|b| json!(b.lines))
                .unwrap_or_else(|| json!([]));
            let settings = world
                .get_resource::<StorySettings>()
                .cloned()
                .unwrap_or_default();
            let luck = world
                .get_resource::<StoryFacts>()
                .map(|f| f.luck)
                .unwrap_or(0);
            let runs = world.get_resource_or_insert_with(StoryRuns::default);
            if let Some(id) = args.get("id").and_then(Json::as_u64) {
                return runs
                    .get(id)
                    .map(|r| json!(r))
                    .ok_or_else(|| format!("no conversation {id}"));
            }
            Ok(json!({
                "current": runs.current(),
                "all": runs.runs,
                "spots": spots,
                "rolls": rolls,
                "barks": barks,
                "checks": { "settings": settings, "luck": luck },
            }))
        },
    );
    app.register_api(
        "story.settings",
        "How skill checks roll: {dice: \"2d6\" (Disco Elysium: double six passes, double one \
         fails) or \"d20\" (natural 20 and 1), difficulty: \"easy\" | \"normal\" | \"hard\" \
         (−2, 0, +2 on every difficulty), luck: true (+1 on a check for each failed in a \
         row)}. Returns them; {} reads.",
        r#"{"difficulty": "easy"}"#,
        |world, args| {
            let st = apply_settings(world, &args)?;
            Ok(json!(st))
        },
    );
    app.register_api(
        "story.bark",
        "Says a node's lines as barks, over each speaker's head, while play goes on: {path: \
         \"dialogue/muster\", node: \"Murmur\", owner: \"Captain\" (whose children are \
         looked in first for the speakers)}. Of a node group, the one that fits best. Returns \
         its id (null when no node fits).",
        r#"{"path": "dialogue/muster", "node": "Murmur", "owner": "Captain"}"#,
        |world, args| {
            let path = args
                .get("path")
                .and_then(Json::as_str)
                .ok_or("pass the .yarn file as \"path\"")?
                .to_string();
            let node = args
                .get("node")
                .and_then(Json::as_str)
                .ok_or("pass the node as \"node\"")?
                .to_string();
            let owner = match args.get("owner").and_then(Json::as_str) {
                Some(name) => {
                    Some(named(world, name).ok_or_else(|| format!("there is no entity '{name}'"))?)
                }
                None => None,
            };
            crate::with_runtime(world, |world, rt, _| {
                let _lent = crate::access::lend_world(&rt.lua, world);
                let d: mlua::Table = rt.lua.globals().get("dialogue")?;
                let f: mlua::Function = d.get("bark")?;
                let opts = rt.lua.create_table()?;
                if let Some(e) = owner {
                    opts.set("owner", crate::entity::EntityRef(e))?;
                }
                let h: Option<mlua::Table> = f.call((path, node, opts))?;
                match h {
                    Some(h) => h.get::<Option<u64>>("id"),
                    None => Ok(None),
                }
            })?
            .map(|id| json!({ "id": id }))
            .map_err(|e| e.to_string())
        },
    );
    app.register_api(
        "story.choose",
        "Picks an option (1 is the first) in a conversation: {index, id}; the newest one \
         without id. Index 0 goes on past a line of a conversation started with step. \
         {pass: true|false} decides the option's skill check instead of the dice.",
        r#"{"index": 1}"#,
        |world, args| {
            let index = args
                .get("index")
                .and_then(Json::as_u64)
                .ok_or("pass the option as \"index\" (1 is the first)")?
                as usize;
            let pass = args.get("pass").and_then(Json::as_bool);
            let mut runs = world.get_resource_or_insert_with(StoryRuns::default);
            let id = match args.get("id").and_then(Json::as_u64) {
                Some(id) => id,
                None => runs
                    .current()
                    .map(|r| r.id)
                    .ok_or("no conversation is going")?,
            };
            runs.choose_forced(id, index, pass);
            Ok(json!({ "ok": true, "id": id }))
        },
    );
    app.register_api(
        "story.stop",
        "Stops a conversation ({id}; the newest without one).",
        "{}",
        |world, args| {
            let mut runs = world.get_resource_or_insert_with(StoryRuns::default);
            let id = match args.get("id").and_then(Json::as_u64) {
                Some(id) => id,
                None => runs
                    .current()
                    .map(|r| r.id)
                    .ok_or("no conversation is going")?,
            };
            runs.stops.push(id);
            Ok(json!({ "ok": true }))
        },
    );
    app.register_api(
        "story.quests",
        "The quests (.quest files): each one's state, objectives, conditions and progress. \
         {start: id}, {complete: id} or {fail: id} moves one by hand; {reset: true} makes \
         them all inactive again.",
        r#"{"start": "sealed_bridge"}"#,
        |world, args| {
            let root = crate::scripts_root(world);
            let mut facts = world.remove_resource::<StoryFacts>().unwrap_or_default();
            let mut book = world.remove_resource::<QuestBook>().unwrap_or_default();
            book.reload(&root);
            if args.get("reset").and_then(Json::as_bool) == Some(true) {
                book.runs.clear();
            }
            let mut out = Ok(());
            if let Some(id) = args.get("start").and_then(Json::as_str) {
                out = book.start(&mut facts, id);
            }
            for (key, failed) in [("complete", false), ("fail", true)] {
                if let Some(id) = args.get(key).and_then(Json::as_str) {
                    book.finish(&mut facts, id, failed);
                }
            }
            world.insert_resource(facts);
            world.insert_resource(book);
            out?;
            // Where they stand now, in the journal now (not after the next frame).
            if !world.contains_resource::<crate::ScriptsRunning>() {
                tick(world, true);
            }
            send_to_journal(world);
            Ok(describe_quests(world))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_become_luau() {
        assert_eq!(compile_expr("$gold > 3").unwrap(), "S.lt(3, F[\"gold\"])");
        assert_eq!(
            compile_expr("$a and not $b or $c == \"x\"").unwrap(),
            "((F[\"a\"] and (not F[\"b\"])) or S.eq(F[\"c\"], \"x\"))"
        );
        assert_eq!(
            compile_expr("visited(\"Gate\") is false").unwrap(),
            "S.eq(S:call(\"visited\", \"Gate\"), false)"
        );
        assert_eq!(
            compile_expr("-$x * 2").unwrap(),
            "S.mul(S.neg(F[\"x\"]), 2)"
        );
        assert!(
            compile_expr("gold > 3").is_err(),
            "a bare word is a mistake"
        );
        assert!(compile_expr("($a").is_err());
        assert_eq!(specificity("$a and $b > 2"), 2);
    }

    /// Options right after a line group (or a line group after options) at the same indent
    /// are their own block: the group says its line, then the options show.
    #[test]
    fn options_after_a_line_group_are_their_own_block() {
        let src = "title: Start\n---\n=> Back again?\n=> Hello. <<if $met>>\n-> Ask\n    \
                   Ask away.\n-> Leave\n===\n";
        let c = compile_yarn(src, "dialogue/x.yarn");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        let lines: Vec<&str> = c.luau.lines().collect();
        assert!(
            lines[3].ends_with("__g:run() if __r then return __r end end"),
            "{}",
            c.luau
        );
        assert!(
            lines[4].starts_with("do local __o = S:options()"),
            "{}",
            c.luau
        );
    }

    #[test]
    fn options_name_their_spots_and_sets() {
        let src = "title: Start\n---\nWho speaks?\n-> Stay <<if $a>> #spot:rank #fallback\n    \
                   -> Inner #spot:x\n    -> Other\n-> Go #spot:front\n-> Talk\n===\n";
        let c = compile_yarn(src, "dialogue/x.yarn");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        let o = &c.nodes[0].options;
        let seen: Vec<(u32, u32, &str, Option<&str>, bool)> = o
            .iter()
            .map(|o| {
                (
                    o.set,
                    o.line,
                    o.text.as_str(),
                    o.spot.as_deref(),
                    o.fallback,
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (4, 4, "Stay", Some("rank"), true),
                (5, 5, "Inner", Some("x"), false),
                (5, 6, "Other", None, false),
                (4, 7, "Go", Some("front"), false),
                (4, 8, "Talk", None, false),
            ]
        );
    }

    #[test]
    fn a_yarn_file_compiles_line_for_line() {
        let src = "title: Start\n---\nKeeper: Hello, {$name}. #greet\n<<if $gold >= 3>>\n\
                   Keeper: Rich today.\n<<else>>\nKeeper: Poor today.\n<<endif>>\n-> Ask\n    \
                   Keeper: Ask away.\n    <<jump Other>>\n-> Leave <<if $x>>\n<<set $met to true>>\n\
                   ===\ntitle: Other\n---\n<<open_gate 2 \"slow\">>\n===\n";
        let c = compile_yarn(src, "dialogue/t.yarn");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        let lines: Vec<&str> = c.luau.lines().collect();
        assert!(
            lines[2].contains("S:line(\"Keeper\", \"Hello, \" .. S.text(F[\"name\"]) .. \".\"")
        );
        assert!(lines[2].contains("{\"greet\"}"));
        assert!(lines[3].starts_with("if S.le(3, F[\"gold\"]) then"));
        assert!(lines[8].contains("S:options()") && lines[8].contains("__o:add(\"Ask\""));
        assert!(lines[10].contains("S:jump(\"Other\")"));
        assert!(lines[11].starts_with("end) __o:add(\"Leave\", F[\"x\"]"));
        assert!(
            lines[12].starts_with("end) local __r = __o:pick()")
                || lines[11].contains("__o:pick()")
        );
        assert!(lines[16].contains("S:command(\"open_gate\", {2, \"slow\"}, 17)"));
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.nodes[0].links, vec![("Other".into(), "Ask".into(), 11)]);
        assert_eq!(c.nodes[0].lines[0], (3, "Keeper: Hello, {$name}.".into()));
    }

    #[test]
    fn mistakes_name_their_line() {
        let src = "title: A\n---\n<<if $x>>\nHi\n===\ntitle: A\n---\n<<jump Nowhere>>\n===\n";
        let c = compile_yarn(src, "t.yarn");
        let lines: Vec<u32> = c.problems.iter().map(|p| p.line).collect();
        assert!(lines.contains(&3), "unclosed if: {:?}", c.problems);
        assert!(lines.contains(&6), "second node called A: {:?}", c.problems);
        assert!(lines.contains(&8), "no node Nowhere: {:?}", c.problems);
    }
}
