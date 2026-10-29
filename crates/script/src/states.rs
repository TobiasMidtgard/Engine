//! State machines in scripts, for the editor: a class's `states` table as states and
//! transitions ([`Machine`], with the lines they are written on), and a running machine's
//! active states and log ([`live`]). The machine itself runs in Luau (`states.luau`, the crate
//! docs' "State machines"); engine-design/scripting-research.md §19.

use std::collections::{BTreeMap, BTreeSet};

use bevy_ecs::prelude::*;
use mlua::{Table, Value};
use serde_json::{Value as Json, json};

use crate::runtime::ScriptRuntime;

/// One state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub name: String,
    /// The group it is in (None: the outer level).
    pub parent: Option<String>,
    /// For a group: the state it starts in.
    pub initial: Option<String>,
    /// For a group: coming back resumes the state it was last in.
    pub history: bool,
    /// With `history`: the states inside that one resume where they were too (`history =
    /// "deep"`).
    pub deep: bool,
    /// For a group: all its states run at once (regions), not one after another.
    pub parallel: bool,
    /// Reaching it ends its group (`final = true`): the group's `done` follows; at the outer
    /// level it ends the machine.
    pub is_final: bool,
    pub doc: String,
    /// Which of `enter`, `update`, `exit` it has.
    pub hooks: Vec<String>,
    /// 1-based line of its entry in the file (0: not found).
    pub line: u32,
}

/// What makes a transition fire.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    /// An event: an interaction (`interact`), a message, a wired signal, `self:trigger`;
    /// with the condition that must hold.
    On {
        event: String,
        guard: Option<String>,
    },
    /// Seconds after the state started; `prop`: the prop that sets them (`after = { "hold",
    /// "closing" }`), its default in `secs` (0 when it has none).
    After { secs: f64, prop: Option<String> },
    /// When the state's enter task has finished; for a group, when it reaches one of its
    /// final states (a parallel group: every region has).
    Done,
    /// When a condition (a method or field of the script) holds; checked every frame.
    When(String),
}

/// One transition.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// None: the outer table's, which works in every state (PlayMaker's global transitions).
    pub from: Option<String>,
    pub to: String,
    pub trigger: Trigger,
    /// 1-based line it is written on (0: not found).
    pub line: u32,
}

impl Transition {
    /// What fires it, short: `interact`, `after 4 s`, `done`, `when blocked`.
    pub fn label(&self) -> String {
        match &self.trigger {
            Trigger::On { event, guard } => match guard {
                Some(g) => format!("{event} [{g}]"),
                None => event.clone(),
            },
            Trigger::After { secs, prop: None } => format!("after {} s", short(*secs)),
            Trigger::After {
                secs,
                prop: Some(p),
            } => format!("after {p} ({} s)", short(*secs)),
            Trigger::Done => "done".into(),
            Trigger::When(c) => format!("when {c}"),
        }
    }

    /// The trigger's family: `on`, `after`, `done`, `when`.
    pub fn kind(&self) -> &'static str {
        match self.trigger {
            Trigger::On { .. } => "on",
            Trigger::After { .. } => "after",
            Trigger::Done => "done",
            Trigger::When(_) => "when",
        }
    }
}

fn short(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{x:.0}")
    } else {
        let s = format!("{x:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A class's state machine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Machine {
    /// The state it starts in (at the outer level).
    pub initial: String,
    /// In the file's order: groups before the states inside them.
    pub states: Vec<State>,
    pub transitions: Vec<Transition>,
    /// 1-based line of `Class.states =` (0: not found).
    pub line: u32,
}

impl Machine {
    pub fn state(&self, name: &str) -> Option<&State> {
        self.states.iter().find(|s| s.name == name)
    }

    /// The states directly inside `group` (None: the outer level), in order.
    pub fn children<'a>(&'a self, group: Option<&'a str>) -> impl Iterator<Item = &'a State> {
        self.states
            .iter()
            .filter(move |s| s.parent.as_deref() == group)
    }

    /// Whether the group `name` (None: the machine) is done with `active` the active states:
    /// in one of its final states, a parallel group with every region done.
    pub fn complete(&self, group: Option<&str>, active: &[String]) -> bool {
        let parallel = group
            .and_then(|g| self.state(g))
            .is_some_and(|g| g.parallel);
        let mut kids = self.children(group).peekable();
        if kids.peek().is_none() {
            return group
                .and_then(|g| self.state(g))
                .is_some_and(|g| g.is_final);
        }
        if parallel {
            return kids.all(|k| self.complete(Some(&k.name), active));
        }
        kids.find(|k| active.contains(&k.name))
            .is_some_and(|k| k.is_final)
    }

    /// Whether `name` holds states.
    pub fn is_group(&self, name: &str) -> bool {
        self.states
            .iter()
            .any(|s| s.parent.as_deref() == Some(name))
    }

    /// The events some state takes.
    pub fn events(&self) -> BTreeSet<String> {
        self.transitions
            .iter()
            .filter_map(|t| match &t.trigger {
                Trigger::On { event, .. } => Some(event.clone()),
                _ => None,
            })
            .collect()
    }

    /// From `__states_shape`'s data (or a graph's machine, the same shape).
    pub fn from_json(v: &Json) -> Result<Machine, Problem> {
        if let Some(problem) = v["problem"].as_str() {
            return Err(Problem {
                message: problem.to_string(),
                about: v["about"].as_str().map(str::to_string),
                line: 0,
            });
        }
        let text = |v: &Json| v.as_str().map(str::to_string);
        let states = v["states"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|s| State {
                name: text(&s["name"]).unwrap_or_default(),
                parent: text(&s["parent"]),
                initial: text(&s["initial"]),
                history: s["history"].as_bool().unwrap_or(false) || s["history"] == "deep",
                deep: s["history"] == "deep",
                parallel: s["parallel"].as_bool().unwrap_or(false),
                is_final: s["final"].as_bool().unwrap_or(false),
                doc: text(&s["doc"]).unwrap_or_default(),
                hooks: s["hooks"]
                    .as_array()
                    .map(|h| h.iter().filter_map(text).collect())
                    .unwrap_or_default(),
                line: 0,
            })
            .collect();
        let transitions = v["transitions"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                let trigger = match t["kind"].as_str()? {
                    "on" => Trigger::On {
                        event: text(&t["event"])?,
                        guard: text(&t["guard"]),
                    },
                    "after" => Trigger::After {
                        secs: t["secs"].as_f64().unwrap_or(0.0),
                        prop: text(&t["prop"]),
                    },
                    "done" => Trigger::Done,
                    "when" => Trigger::When(text(&t["cond"])?),
                    _ => return None,
                };
                let from = text(&t["from"]).filter(|f| f != "root");
                Some(Transition {
                    from,
                    to: text(&t["to"])?,
                    trigger,
                    line: 0,
                })
            })
            .collect();
        Ok(Machine {
            initial: text(&v["initial"]).unwrap_or_default(),
            states,
            transitions,
            line: 0,
        })
    }

    /// From a `states` table written as JSON (a graph file's machine): the states and
    /// transitions it can make sense of, and what is wrong with the rest. The same reading as
    /// the runtime's (`states.luau`); hooks are left for the caller.
    pub fn from_definition(def: &Json) -> (Machine, Vec<String>) {
        let mut m = Machine::default();
        let mut problems = Vec::new();
        fn walk(
            name: Option<&str>,
            d: &Json,
            m: &mut Machine,
            problems: &mut Vec<String>,
        ) -> Option<String> {
            let kids = d.get("states").and_then(Json::as_object);
            let names: Vec<&String> = kids.map(|k| k.keys().collect()).unwrap_or_default();
            let parallel = d["parallel"].as_bool().unwrap_or(false);
            let initial = d["initial"]
                .as_str()
                .map(str::to_string)
                .or_else(|| (names.len() == 1).then(|| names[0].clone()))
                .filter(|_| !parallel);
            if !names.is_empty() && !parallel {
                match &initial {
                    Some(i) if names.contains(&i) => {}
                    Some(i) => problems.push(format!(
                        "{}: first state \"{i}\" isn't one of its states",
                        name.unwrap_or("states")
                    )),
                    None => problems.push(format!(
                        "{} has {} states: pick the one it starts in",
                        name.unwrap_or("states"),
                        names.len()
                    )),
                }
            }
            for k in names {
                let kd = &kids.expect("has states")[k];
                m.states.push(State {
                    name: k.clone(),
                    parent: name.map(str::to_string),
                    history: kd["history"].as_bool().unwrap_or(false) || kd["history"] == "deep",
                    deep: kd["history"] == "deep",
                    parallel: kd["parallel"].as_bool().unwrap_or(false),
                    is_final: kd["final"].as_bool().unwrap_or(false),
                    doc: kd["doc"].as_str().unwrap_or_default().to_string(),
                    ..Default::default()
                });
                let at = m.states.len() - 1;
                let first = walk(Some(k), kd, m, problems);
                m.states[at].initial = first;
            }
            if names_empty(d) { None } else { initial }
        }
        fn names_empty(d: &Json) -> bool {
            d.get("states")
                .and_then(Json::as_object)
                .is_none_or(|k| k.is_empty())
        }
        m.initial = walk(None, def, &mut m, &mut problems).unwrap_or_default();
        let known: BTreeSet<String> = m.states.iter().map(|s| s.name.clone()).collect();
        let mut push =
            |from: Option<&str>, to: &Json, trigger: Trigger, m: &mut Machine| match to.as_str() {
                Some(t) if known.contains(t) => m.transitions.push(Transition {
                    from: from.map(str::to_string),
                    to: t.to_string(),
                    trigger,
                    line: 0,
                }),
                _ => problems.push(format!(
                    "{}: goes to {to}, which isn't a state",
                    from.unwrap_or("any state")
                )),
            };
        let mut defs: Vec<(Option<String>, Json)> = vec![(None, def.clone())];
        let mut stack = vec![def];
        while let Some(d) = stack.pop() {
            if let Some(kids) = d.get("states").and_then(Json::as_object) {
                for (k, v) in kids {
                    defs.push((Some(k.clone()), v.clone()));
                    stack.push(v);
                }
            }
        }
        for (from, d) in &defs {
            let from = from.as_deref();
            if let Some(on) = d["on"].as_object() {
                for (event, v) in on {
                    let choices: Vec<(Json, Option<String>)> = match v {
                        Json::String(_) => vec![(v.clone(), None)],
                        Json::Object(o) => vec![(
                            o.get("to").cloned().unwrap_or(Json::Null),
                            o.get("guard").and_then(Json::as_str).map(str::to_string),
                        )],
                        Json::Array(list) => list
                            .iter()
                            .map(|c| match c {
                                Json::Object(o) => (
                                    o.get("to").cloned().unwrap_or(Json::Null),
                                    o.get("guard").and_then(Json::as_str).map(str::to_string),
                                ),
                                other => (other.clone(), None),
                            })
                            .collect(),
                        other => vec![(other.clone(), None)],
                    };
                    for (to, guard) in choices {
                        let trigger = Trigger::On {
                            event: event.clone(),
                            guard,
                        };
                        push(from, &to, trigger, &mut m);
                    }
                }
            }
            if let Some(a) = d["after"].as_array() {
                let (secs, prop) = match a.first() {
                    Some(Json::String(p)) => (0.0, Some(p.clone())),
                    Some(n) => (n.as_f64().unwrap_or(0.0), None),
                    None => (0.0, None),
                };
                push(
                    from,
                    a.get(1).unwrap_or(&Json::Null),
                    Trigger::After { secs, prop },
                    &mut m,
                );
            }
            if !d["done"].is_null() {
                push(from, &d["done"], Trigger::Done, &mut m);
            }
            if let Some(w) = d["when"].as_object() {
                for (cond, to) in w {
                    push(from, to, Trigger::When(cond.clone()), &mut m);
                }
            }
        }
        (m, problems)
    }

    /// Finds the lines states and transitions are written on in `source`.
    pub fn locate(&mut self, source: &str) {
        let lines: Vec<&str> = source.lines().collect();
        let start = lines
            .iter()
            .position(|l| {
                let t = l.trim_start();
                !t.starts_with("--") && t.contains(".states") && t.contains('=')
            })
            .unwrap_or(0);
        self.line = if lines.is_empty() {
            0
        } else {
            start as u32 + 1
        };
        for s in &mut self.states {
            s.line = entry_line(&lines, start, &s.name).unwrap_or(0);
        }
        let state_lines: BTreeMap<String, u32> = self
            .states
            .iter()
            .map(|s| (s.name.clone(), s.line))
            .collect();
        for t in &mut self.transitions {
            let from_line = t
                .from
                .as_ref()
                .and_then(|f| state_lines.get(f).copied())
                .filter(|l| *l > 0)
                .unwrap_or(self.line.max(1));
            let token = match &t.trigger {
                Trigger::On { event, .. } => event.clone(),
                Trigger::After { .. } => "after".into(),
                Trigger::Done => "done".into(),
                Trigger::When(c) => c.clone(),
            };
            let quoted = format!("\"{}\"", t.to);
            t.line = lines
                .iter()
                .enumerate()
                .skip(from_line as usize - 1)
                .find(|(_, l)| l.contains(&quoted) && l.contains(&token))
                .map_or(from_line, |(k, _)| k as u32 + 1);
        }
    }
}

/// `name = {` (or `["name"] = {`) at or after `start`.
fn entry_line(lines: &[&str], start: usize, name: &str) -> Option<u32> {
    let plain = format!("{name} = {{");
    let tight = format!("{name}={{");
    let bracketed = format!("[\"{name}\"] = {{");
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, l)| {
            let t = l.trim_start();
            t.starts_with(&plain) || t.starts_with(&tight) || t.starts_with(&bracketed)
        })
        .map(|(k, _)| k as u32 + 1)
}

/// Why a machine doesn't run.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub message: String,
    /// The name or value it is about.
    pub about: Option<String>,
    /// 1-based line (0: not found).
    pub line: u32,
}

impl Problem {
    /// Finds the line it is about in `source`: the quoted name, else its entry.
    pub fn locate(&mut self, source: &str) {
        let lines: Vec<&str> = source.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.contains(".states") && l.contains('='))
            .unwrap_or(0);
        self.line = start as u32 + 1;
        let Some(about) = &self.about else { return };
        let quoted = format!("\"{about}\"");
        if let Some(k) = lines
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, l)| l.contains(&quoted))
            .map(|(k, _)| k)
        {
            self.line = k as u32 + 1;
        } else if let Some(l) = entry_line(&lines, start, about) {
            self.line = l;
        }
    }
}

/// The machine of a class table (in a VM that has the engine's base class).
pub(crate) fn of_class(lua: &mlua::Lua, class: &Table) -> Option<Result<Machine, Problem>> {
    if !matches!(class.raw_get::<Value>("states"), Ok(Value::Table(_))) {
        return None;
    }
    let shape = (|| -> mlua::Result<Json> {
        let base: Table = lua.named_registry_value(crate::runtime::BASE)?;
        let f: mlua::Function = base.get("__states_shape")?;
        let v: Value = f.call(class.clone())?;
        crate::convert::to_json(&v)
    })();
    Some(match shape {
        Ok(v) => Machine::from_json(&v),
        Err(e) => Err(Problem {
            message: e.to_string(),
            about: None,
            line: 0,
        }),
    })
}

/// One change of state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Change {
    /// Seconds on the script's clock.
    pub t: f64,
    /// Empty for the first state.
    pub from: String,
    pub to: String,
    /// `interact by Player`, `after 4 s`, `done`, `when blocked`, `go`, `start`, `reload`.
    pub why: String,
    /// Every state active after it, outermost first.
    pub active: Vec<String>,
    /// The script's values right after it, by name (`props.range`, `shots`): PlayMaker's
    /// Debug Flow.
    pub values: Vec<(String, String)>,
}

/// A running machine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Live {
    /// Outermost first (a group before the states in it).
    pub active: Vec<String>,
    /// The active states with no active state inside: the current state, or one per region of
    /// a parallel group.
    pub current: Vec<String>,
    /// When each active state started (script clock).
    pub since: BTreeMap<String, f64>,
    /// Active states whose enter task is still going.
    pub running: BTreeSet<String>,
    /// The script's clock now.
    pub clock: f64,
    /// The last changes, oldest first.
    pub log: Vec<Change>,
    /// Groups with history: the state each was last in.
    pub history: BTreeMap<String, String>,
    /// Active groups in one of their final states (a parallel group: every region).
    pub done: BTreeSet<String>,
    /// It reached a final state at the outer level: nothing changes it until a `go`.
    pub over: bool,
    /// The script's values now, as [`Change::values`].
    pub values: Vec<(String, String)>,
}

impl Live {
    /// The current (innermost) state; with parallel regions, the first region's.
    pub fn current(&self) -> Option<&str> {
        self.current
            .first()
            .or(self.active.last())
            .map(String::as_str)
    }

    /// Whether `name` is a current state (innermost, in any region).
    pub fn is_current(&self, name: &str) -> bool {
        if self.current.is_empty() {
            return self.active.last().is_some_and(|a| a == name);
        }
        self.current.iter().any(|c| c == name)
    }

    /// The machine as it was right after change `k` of the log (PlayMaker's Debug Flow, the
    /// StateTree debugger's timeline): its active states, when each started, that change last.
    pub fn as_of(&self, k: usize, m: &Machine) -> Live {
        let k = k.min(self.log.len().saturating_sub(1));
        let log: Vec<Change> = self.log.iter().take(k + 1).cloned().collect();
        let Some(at) = log.last() else {
            return self.clone();
        };
        let mut since = BTreeMap::new();
        let mut before: &[String] = &[];
        // Each state's time from the change that entered it (one that stayed active through a
        // change, as a group around both states does, keeps its time).
        for c in &log {
            for name in &c.active {
                if !before.contains(name) || *name == c.to {
                    since.insert(name.clone(), c.t);
                }
            }
            since.retain(|name, _| c.active.contains(name));
            before = &c.active;
        }
        let current = at
            .active
            .iter()
            .filter(|a| {
                !at.active
                    .iter()
                    .any(|b| m.state(b).and_then(|s| s.parent.as_deref()) == Some(a.as_str()))
            })
            .cloned()
            .collect();
        let done = at
            .active
            .iter()
            .filter(|a| m.is_group(a) && m.complete(Some(a.as_str()), &at.active))
            .cloned()
            .collect();
        Live {
            active: at.active.clone(),
            current,
            since,
            running: BTreeSet::new(),
            clock: at.t,
            over: m.complete(None, &at.active),
            done,
            values: at.values.clone(),
            log,
            history: self.history.clone(),
        }
    }

    /// Seconds since the active state `name` started.
    pub fn secs_in(&self, name: &str) -> Option<f64> {
        self.since.get(name).map(|t| (self.clock - t).max(0.0))
    }

    fn from_json(v: &Json) -> Live {
        let names = |v: &Json| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        // [[name, value], ...]
        let pairs = |v: &Json| -> Vec<(String, String)> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let map = |v: &Json| -> Vec<(String, Json)> {
            v.as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default()
        };
        Live {
            active: names(&v["active"]),
            current: names(&v["current"]),
            since: map(&v["since"])
                .into_iter()
                .filter_map(|(k, t)| Some((k, t.as_f64()?)))
                .collect(),
            running: map(&v["running"]).into_iter().map(|(k, _)| k).collect(),
            clock: v["clock"].as_f64().unwrap_or(0.0),
            log: v["log"]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|c| Change {
                    t: c["t"].as_f64().unwrap_or(0.0),
                    from: c["from"].as_str().unwrap_or_default().to_string(),
                    to: c["to"].as_str().unwrap_or_default().to_string(),
                    why: c["why"].as_str().unwrap_or_default().to_string(),
                    active: names(&c["active"]),
                    values: pairs(&c["values"]),
                })
                .collect(),
            history: map(&v["history"])
                .into_iter()
                .filter_map(|(k, s)| Some((k, s.as_str()?.to_string())))
                .collect(),
            done: names(&v["done"]).into_iter().collect(),
            over: v["over"].as_bool().unwrap_or(false),
            values: pairs(&v["values"]),
        }
    }
}

/// Breakpoints on states, by script path, as the editor sets them (PlayMaker's "Toggle
/// Breakpoint", the StateTree debugger's "Break on Enter/Exit"). A machine that reaches one adds
/// a [`Hit`]; the editor pauses the game on it. Change `by_path` through [`Self::set`] so the
/// runtime sees the new ones.
#[derive(Resource, Debug, Clone, Default)]
pub struct StateBreakpoints {
    pub by_path: BTreeMap<String, Breaks>,
    /// Bumped by every change of `by_path`.
    pub version: u64,
    /// Reached since the editor last took them.
    pub hits: Vec<Hit>,
}

/// One script's breakpoints: the states whose entry, and whose exit, pause the game.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Breaks {
    pub enter: BTreeSet<String>,
    pub exit: BTreeSet<String>,
}

impl Breaks {
    pub fn is_empty(&self) -> bool {
        self.enter.is_empty() && self.exit.is_empty()
    }
}

impl StateBreakpoints {
    /// Sets (`on`) or clears the breakpoint on `state`'s entry (`exit`: its exit) in `path`.
    pub fn set(&mut self, path: &str, state: &str, exit: bool, on: bool) {
        let breaks = self.by_path.entry(path.to_string()).or_default();
        let set = if exit {
            &mut breaks.exit
        } else {
            &mut breaks.enter
        };
        let changed = if on {
            set.insert(state.to_string())
        } else {
            set.remove(state)
        };
        if breaks.is_empty() {
            self.by_path.remove(path);
        }
        if changed {
            self.version += 1;
        }
    }

    pub fn of(&self, path: &str) -> Option<&Breaks> {
        self.by_path.get(path)
    }
}

/// A breakpoint reached.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub entity: Entity,
    pub path: String,
    /// `enter` or `exit`.
    pub kind: String,
    pub state: String,
    /// What changed the state (`interact by Player`, `after hold (4 s)`, ...).
    pub why: String,
    /// World seconds.
    pub t: f64,
}

/// The state machine running on `e` (None: no script with states runs there, or it hasn't
/// started).
pub fn live(world: &World, e: Entity) -> Option<Live> {
    let rt = world.get_resource::<ScriptRuntime>()?;
    rt.vms().find_map(|vm| {
        let selves: Table = vm.lua.named_registry_value(crate::entity::SELVES).ok()?;
        let this: Table = selves.raw_get(e.to_bits() as f64).ok()?;
        let f: mlua::Function = this.get("__states_live").ok()?;
        let v: Value = f.call(this).ok()?;
        if v.is_nil() {
            return None;
        }
        crate::convert::to_json(&v)
            .ok()
            .map(|j| Live::from_json(&j))
    })
}

/// `script.states`: the machine of the script on `e` and its live state, as JSON.
pub(crate) fn describe(world: &mut World, e: Entity) -> Result<Json, String> {
    let path = world
        .get::<crate::Script>(e)
        .map(|s| s.path.clone())
        .ok_or("that entity runs no script")?;
    let declared = crate::props::declared(world, &path)?;
    let machine = match declared.machine {
        None => return Err(format!("{path} has no states table")),
        Some(Err(p)) => return Err(format!("{path}:{}: {}", p.line, p.message)),
        Some(Ok(m)) => m,
    };
    let live = live(world, e);
    let states: Vec<Json> = machine
        .states
        .iter()
        .map(|s| {
            json!({
                "name": s.name, "parent": s.parent, "initial": s.initial,
                "history": if s.deep { json!("deep") } else { json!(s.history) },
                "parallel": s.parallel, "final": s.is_final, "hooks": s.hooks, "line": s.line,
            })
        })
        .collect();
    let transitions: Vec<Json> = machine
        .transitions
        .iter()
        .map(|t| {
            json!({
                "from": t.from, "to": t.to, "trigger": t.label(), "line": t.line,
            })
        })
        .collect();
    Ok(json!({
        "path": path,
        "initial": machine.initial,
        "states": states,
        "transitions": transitions,
        "active": live.as_ref().map(|l| l.active.clone()),
        "current": live.as_ref().map(|l| l.current.clone()),
        "state_secs": live.as_ref().and_then(|l| l.current().and_then(|c| l.secs_in(c))),
        "done": live.as_ref().map(|l| l.done.iter().collect::<Vec<_>>()),
        "over": live.as_ref().map(|l| l.over),
        "values": live.as_ref().map(|l| l.values.clone()),
        "log": live.as_ref().map(|l| l.log.iter().map(|c| json!({
            "t": c.t, "from": c.from, "to": c.to, "why": c.why, "active": c.active,
            "values": c.values,
        })).collect::<Vec<_>>()),
    }))
}
