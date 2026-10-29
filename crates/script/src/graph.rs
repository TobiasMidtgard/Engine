//! Visual scripts: graphs of nodes, wired like Unreal's Blueprints, that compile to Luau.
//!
//! A `.graph` file in the scripts folder is a script like a `.luau` one: put it on an entity
//! with `Script { path: "showcase/crystal.graph" }`. The runtime compiles it to Luau when it
//! loads (about a millisecond for a hundred nodes) and runs that, so a graph hot reloads
//! keeping its state, its errors are marked (on the node that raised them), the profiler times
//! it, and logpoints watch its wires, all as for code (engine-design/scripting-research.md §12).
//!
//! The file is JSON written one node and one wire per line, so a change to a graph is a short
//! text diff and two people's edits merge like code (Unreal's binary Blueprints can't be merged
//! at all):
//!
//! ```json
//! {
//!   "version": 1,
//!   "props": {"speed": 25},
//!   "vars": {"lit": false},
//!   "nodes": [
//!     {"id": 1, "kind": "event.update", "at": [-320, 0]},
//!     {"id": 2, "kind": "entity.rotate_y", "at": [0, 0]},
//!     {"id": 3, "kind": "math.multiply", "at": [-160, 120]},
//!     {"id": 4, "kind": "prop.get", "of": "speed", "at": [-320, 160]}
//!   ],
//!   "links": [
//!     {"from": [1, "then"], "to": [2, "in"]},
//!     {"from": [1, "dt"], "to": [3, "a"]},
//!     {"from": [4, "value"], "to": [3, "b"]},
//!     {"from": [3, "value"], "to": [2, "degrees"]}
//!   ]
//! }
//! ```
//!
//! Nodes are events (where a script's hooks start), flow (branch, sequence, delay, do once,
//! flip flop, gate, loops, tween), actions on entities, variables and props, math, and one node
//! for every engine function and component the running game has registered ([`library`]), so
//! a module's new function is a node the moment it exists, with its documentation. Data pins
//! are pure: a pure node (no white execution pins) is worked out where its value is used, as
//! in Blueprints. A latent node (Delay, Tween) lets the rest run on later frames.
//!
//! [`compile`] gives the Luau, which node each line came from, the problems it found
//! (Unity-style predictive checks: unwired conditions, nodes nothing leads to, values used
//! where they don't exist yet, loops without a Delay), and a Luau expression for every data
//! wire, which the editor watches with logpoints to show the values along the wires while
//! the game runs.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The file format's version.
pub const VERSION: u32 = 1;

/// A graph file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graph {
    pub version: u32,
    /// What the graph does (the generated file's header).
    pub doc: String,
    /// The script's props: settings per entity, `Script::props` overrides them.
    pub props: BTreeMap<String, Value>,
    /// What each prop is for: the comment after it in the Luau, so its hints (`@range 0 10`,
    /// `@unit m`, ...) shape its field in the Details panel (`crate::props`).
    pub prop_notes: BTreeMap<String, String>,
    /// The script's own state (`self.<name>`), set to these values when it starts.
    pub vars: BTreeMap<String, Value>,
    /// What the script tells the scene (`Class.signals`; Emit nodes fire them): each signal's
    /// values, `count: number, by: Entity`. Wired per object in the Details panel.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub signals: BTreeMap<String, String>,
    /// What each signal is for (the Details panel shows it).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub signal_notes: BTreeMap<String, String>,
    /// Custom events: methods of the script that signals wired in the Details panel, Bind
    /// nodes, Call nodes and messages of their name run. Each one's values, as for signals.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, String>,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub comments: Vec<Comment>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            version: VERSION,
            doc: String::new(),
            props: BTreeMap::new(),
            prop_notes: BTreeMap::new(),
            vars: BTreeMap::new(),
            signals: BTreeMap::new(),
            signal_notes: BTreeMap::new(),
            events: BTreeMap::new(),
            nodes: Vec::new(),
            links: Vec::new(),
            comments: Vec::new(),
        }
    }
}

/// One node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Unique in the graph; links name it.
    pub id: u32,
    /// What it is: `flow.branch`, `api.call`, `comp.set`, ...
    pub kind: String,
    /// What a node of a family is about: the engine function (`fire.ignite`), the component
    /// (`PointLight`), the variable or prop.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub of: String,
    /// Top-left corner on the canvas.
    pub at: [i32; 2],
    /// Values of inputs that aren't wired, by pin.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
}

/// A wire from an output pin to an input pin: `(node id, pin name)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub from: (u32, String),
    pub to: (u32, String),
}

/// A note boxed around a group of nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub text: String,
    /// sRGB.
    #[serde(default = "comment_color")]
    pub color: [u8; 3],
}

fn comment_color() -> [u8; 3] {
    [70, 90, 120]
}

impl Graph {
    /// Reads a graph file.
    pub fn parse(text: &str) -> Result<Graph, String> {
        let g: Graph = serde_json::from_str(text).map_err(|e| format!("not a graph file: {e}"))?;
        if g.version > VERSION {
            return Err(format!(
                "graph version {} is newer than this engine reads ({VERSION})",
                g.version
            ));
        }
        Ok(g)
    }

    /// The file's text: one node, wire and comment per line, so changes diff and merge.
    pub fn to_text(&self) -> String {
        let one = |v: &dyn erased::Ser| v.json();
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"version\": {},\n", self.version.max(1)));
        if !self.doc.is_empty() {
            out.push_str(&format!("  \"doc\": {},\n", json!(self.doc)));
        }
        out.push_str(&format!("  \"props\": {},\n", json!(self.props)));
        if !self.prop_notes.is_empty() {
            out.push_str(&format!("  \"prop_notes\": {},\n", json!(self.prop_notes)));
        }
        out.push_str(&format!("  \"vars\": {},\n", json!(self.vars)));
        for (name, map) in [
            ("signals", &self.signals),
            ("signal_notes", &self.signal_notes),
            ("events", &self.events),
        ] {
            if !map.is_empty() {
                out.push_str(&format!("  \"{name}\": {},\n", json!(map)));
            }
        }
        let list = |out: &mut String, name: &str, items: Vec<String>, last: bool| {
            if items.is_empty() {
                out.push_str(&format!("  \"{name}\": []"));
            } else {
                out.push_str(&format!("  \"{name}\": [\n"));
                let n = items.len();
                for (i, item) in items.into_iter().enumerate() {
                    out.push_str("    ");
                    out.push_str(&item);
                    out.push_str(if i + 1 < n { ",\n" } else { "\n" });
                }
                out.push_str("  ]");
            }
            out.push_str(if last { "\n" } else { ",\n" });
        };
        let mut nodes: Vec<&Node> = self.nodes.iter().collect();
        nodes.sort_by_key(|n| n.id);
        list(
            &mut out,
            "nodes",
            nodes.iter().map(|n| one(*n)).collect(),
            false,
        );
        list(
            &mut out,
            "links",
            self.links.iter().map(|l| one(l)).collect(),
            false,
        );
        list(
            &mut out,
            "comments",
            self.comments.iter().map(|c| one(c)).collect(),
            true,
        );
        out.push_str("}\n");
        out
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: u32) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// An id no node has.
    pub fn next_id(&self) -> u32 {
        self.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1
    }
}

/// serde_json on one line, for `to_text`.
mod erased {
    pub trait Ser {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap_or_default()
        }
    }
}

// ------------------------------------------------------------------------------- node specs

/// What a pin carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinType {
    /// Execution order (white wires).
    Exec,
    Any,
    Number,
    Bool,
    Text,
    Vec3,
    Entity,
    Table,
}

impl PinType {
    /// Whether a wire may join an output of this type to an input of `to`.
    pub fn fits(self, to: PinType) -> bool {
        match (self, to) {
            (PinType::Exec, PinType::Exec) => true,
            (PinType::Exec, _) | (_, PinType::Exec) => false,
            (PinType::Any, _) | (_, PinType::Any) => true,
            // Anything prints.
            (_, PinType::Text) => true,
            (a, b) => a == b,
        }
    }

    /// The pin type for a Luau type written in a signal's or event's values (`number`,
    /// `Entity?`, ...).
    pub fn of_luau(ty: &str) -> PinType {
        match ty.trim().trim_end_matches('?') {
            "number" => PinType::Number,
            "boolean" => PinType::Bool,
            "string" => PinType::Text,
            "Vec3" => PinType::Vec3,
            "Entity" => PinType::Entity,
            t if t.starts_with('{') => PinType::Table,
            _ => PinType::Any,
        }
    }

    /// This type as Luau writes it.
    pub fn luau(self) -> &'static str {
        match self {
            PinType::Number => "number",
            PinType::Bool => "boolean",
            PinType::Text => "string",
            PinType::Vec3 => "Vec3",
            PinType::Entity => "Entity",
            PinType::Table => "{ [any]: any }",
            PinType::Exec | PinType::Any => "any",
        }
    }

    /// An unwired input's value of this type.
    pub fn zero(self) -> Value {
        match self {
            PinType::Number => json!(0),
            PinType::Bool => json!(false),
            PinType::Text => json!(""),
            PinType::Vec3 => json!([0, 0, 0]),
            _ => Value::Null,
        }
    }

    /// The type a JSON example value shows.
    pub fn of_value(v: &Value) -> PinType {
        match v {
            Value::Bool(_) => PinType::Bool,
            Value::Number(_) => PinType::Number,
            Value::String(_) => PinType::Text,
            Value::Array(a) if a.len() == 3 && a.iter().all(Value::is_number) => PinType::Vec3,
            Value::Array(_) | Value::Object(_) => PinType::Table,
            Value::Null => PinType::Any,
        }
    }
}

/// One pin of a node.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PinSpec {
    pub name: String,
    pub ty: PinType,
    /// The value an unwired input has.
    pub default: Value,
    /// A setting typed into the node, not a socket (an event's message name).
    pub fixed: bool,
    /// Choices for a text setting (an ease curve, a component).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// What a kind of node is: its pins and documentation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeSpec {
    pub kind: String,
    /// The engine function, component, variable or prop it is about ("" for the rest).
    pub of: String,
    pub title: String,
    /// Where it is listed: "Events", "Flow", "Math", "Engine: fire", ...
    pub category: String,
    pub doc: String,
    /// No execution pins: worked out where its value is used.
    pub pure: bool,
    pub inputs: Vec<PinSpec>,
    pub outputs: Vec<PinSpec>,
}

impl NodeSpec {
    fn new(kind: &str, title: &str, category: &str, doc: &str) -> Self {
        Self {
            kind: kind.into(),
            of: String::new(),
            title: title.into(),
            category: category.into(),
            doc: doc.into(),
            pure: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn pure(mut self) -> Self {
        self.pure = true;
        self
    }

    /// An execution input named `in`.
    fn run(self) -> Self {
        self.exec_in("in")
    }

    fn exec_in(mut self, name: &str) -> Self {
        self.inputs.push(pin(name, PinType::Exec, Value::Null));
        self
    }

    /// An execution output named `then`.
    fn then(self) -> Self {
        self.exec_out("then")
    }

    fn exec_out(mut self, name: &str) -> Self {
        self.outputs.push(pin(name, PinType::Exec, Value::Null));
        self
    }

    fn input(mut self, name: &str, ty: PinType, default: Value) -> Self {
        self.inputs.push(pin(name, ty, default));
        self
    }

    fn setting(mut self, name: &str, ty: PinType, default: Value, choices: &[&str]) -> Self {
        let mut p = pin(name, ty, default);
        p.fixed = true;
        p.choices = choices.iter().map(|c| c.to_string()).collect();
        self.inputs.push(p);
        self
    }

    fn output(mut self, name: &str, ty: PinType) -> Self {
        self.outputs.push(pin(name, ty, Value::Null));
        self
    }

    pub fn input_pin(&self, name: &str) -> Option<&PinSpec> {
        self.inputs.iter().find(|p| p.name == name)
    }

    pub fn output_pin(&self, name: &str) -> Option<&PinSpec> {
        self.outputs.iter().find(|p| p.name == name)
    }

    /// Whether this is where execution starts (an event).
    pub fn is_event(&self) -> bool {
        self.kind.starts_with("event.")
    }
}

fn pin(name: &str, ty: PinType, default: Value) -> PinSpec {
    PinSpec {
        name: name.into(),
        ty,
        default,
        fixed: false,
        choices: Vec::new(),
    }
}

/// The easing curves a Tween node offers (`ease.<name>` in Luau).
pub const EASES: &[&str] = &[
    "smooth",
    "linear",
    "in_out",
    "in_quad",
    "out_quad",
    "out_back",
    "out_elastic",
];

/// The nodes every graph has, whatever the game registered.
pub fn builtins() -> Vec<NodeSpec> {
    use PinType::*;
    let entity = || Value::Null;
    let v3 = |x: f64, y: f64, z: f64| json!([x, y, z]);
    vec![
        // Events.
        NodeSpec::new(
            "event.start",
            "Start",
            "Events",
            "Once, when the script starts.",
        )
        .then(),
        NodeSpec::new(
            "event.update",
            "Update",
            "Events",
            "Every frame. dt: the world seconds since the last frame (slow motion aware).",
        )
        .then()
        .output("dt", Number),
        NodeSpec::new(
            "event.fixed_update",
            "Fixed update",
            "Events",
            "Every physics step, with its length.",
        )
        .then()
        .output("dt", Number),
        NodeSpec::new(
            "event.interact",
            "Interact",
            "Events",
            "The player pressed E on this entity (give it an Interactable). by: who pressed.",
        )
        .then()
        .output("by", Entity),
        NodeSpec::new(
            "event.contact",
            "Contact",
            "Events",
            "A physics contact with another entity began.",
        )
        .then()
        .output("other", Entity)
        .output("info", Table),
        NodeSpec::new(
            "event.message",
            "Message",
            "Events",
            "A message sent to this entity (Send message, e:send, script.send), by name; \
             empty: every message.",
        )
        .setting("name", Text, json!(""), &[])
        .then()
        .output("event", Text)
        .output("data", Any),
        NodeSpec::new(
            "event.every",
            "Every",
            "Events",
            "Every so many world seconds while the script runs.",
        )
        .setting("seconds", Number, json!(1.0), &[])
        .then(),
        NodeSpec::new(
            "event.destroy",
            "Destroyed",
            "Events",
            "The entity or its script is going away.",
        )
        .then(),
        // Flow.
        NodeSpec::new(
            "flow.branch",
            "Branch",
            "Flow",
            "Goes on through true or false (an unwired condition is false).",
        )
        .run()
        .input("condition", Bool, json!(false))
        .exec_out("true")
        .exec_out("false"),
        NodeSpec::new(
            "flow.sequence",
            "Sequence",
            "Flow",
            "Runs its outputs in order.",
        )
        .run()
        .exec_out("then 1")
        .exec_out("then 2")
        .exec_out("then 3"),
        NodeSpec::new(
            "flow.delay",
            "Delay",
            "Flow",
            "Goes on after world seconds, while the frame runs on. Triggered again while it \
             waits, it is ignored (as Unreal's Delay).",
        )
        .run()
        .input("seconds", Number, json!(1.0))
        .then(),
        NodeSpec::new(
            "flow.do_once",
            "Do once",
            "Flow",
            "Goes on the first time only, until reset.",
        )
        .run()
        .exec_in("reset")
        .then(),
        NodeSpec::new(
            "flow.flip_flop",
            "Flip flop",
            "Flow",
            "Goes on through A and B in turn.",
        )
        .run()
        .exec_out("a")
        .exec_out("b")
        .output("is_a", Bool),
        NodeSpec::new(
            "flow.gate",
            "Gate",
            "Flow",
            "Goes on from enter only while open; open, close and toggle set it.",
        )
        .exec_in("enter")
        .exec_in("open")
        .exec_in("close")
        .exec_in("toggle")
        .setting("start_open", Bool, json!(true), &[])
        .exec_out("exit"),
        NodeSpec::new(
            "flow.for_loop",
            "For loop",
            "Flow",
            "Runs body for each whole number from first to last, then completed.",
        )
        .run()
        .input("first", Number, json!(1))
        .input("last", Number, json!(3))
        .exec_out("body")
        .output("index", Number)
        .exec_out("completed"),
        NodeSpec::new(
            "flow.for_each",
            "For each",
            "Flow",
            "Runs body for each item of a list (a table), then completed.",
        )
        .run()
        .input("list", Table, json!([]))
        .exec_out("body")
        .output("item", Any)
        .output("index", Number)
        .exec_out("completed"),
        NodeSpec::new(
            "flow.tween",
            "Tween",
            "Flow",
            "Runs update every frame for seconds with alpha going 0 to 1 along the ease \
             curve, then finished; the frame runs on meanwhile.",
        )
        .run()
        .input("seconds", Number, json!(1.0))
        .setting("ease", Text, json!("smooth"), EASES)
        .exec_out("update")
        .output("alpha", Number)
        .exec_out("finished"),
        // Variables and props.
        NodeSpec::new(
            "var.get",
            "Get",
            "Variables",
            "A variable of this script (self.<name>).",
        )
        .pure()
        .output("value", Any),
        NodeSpec::new(
            "var.set",
            "Set",
            "Variables",
            "Sets a variable of this script.",
        )
        .run()
        .input("value", Any, Value::Null)
        .then(),
        NodeSpec::new(
            "prop.get",
            "Prop",
            "Variables",
            "A prop: a setting per entity, from the Script component's props.",
        )
        .pure()
        .output("value", Any),
        // Entities.
        NodeSpec::new(
            "entity.self",
            "Self",
            "Entity",
            "The entity this script runs on.",
        )
        .pure()
        .output("entity", Entity),
        NodeSpec::new(
            "entity.find",
            "Find entity",
            "Entity",
            "The first entity with this name (nil when none).",
        )
        .pure()
        .input("name", Text, json!(""))
        .output("entity", Entity),
        NodeSpec::new(
            "entity.position",
            "Position",
            "Entity",
            "Where an entity is (relative to its parent) and where it ends up in the world.",
        )
        .pure()
        .input("entity", Entity, entity())
        .output("position", Vec3)
        .output("world", Vec3),
        NodeSpec::new("entity.name", "Name", "Entity", "An entity's name.")
            .pure()
            .input("entity", Entity, entity())
            .output("name", Text),
        NodeSpec::new(
            "entity.set_position",
            "Set position",
            "Entity",
            "Moves an entity.",
        )
        .run()
        .input("entity", Entity, entity())
        .input("position", Vec3, v3(0.0, 0.0, 0.0))
        .then(),
        NodeSpec::new(
            "entity.translate",
            "Move by",
            "Entity",
            "Moves an entity by an offset.",
        )
        .run()
        .input("entity", Entity, entity())
        .input("by", Vec3, v3(0.0, 0.0, 0.0))
        .then(),
        NodeSpec::new(
            "entity.rotate_y",
            "Turn",
            "Entity",
            "Turns an entity about the up axis by degrees.",
        )
        .run()
        .input("entity", Entity, entity())
        .input("degrees", Number, json!(0))
        .then(),
        NodeSpec::new(
            "entity.look_at",
            "Look at",
            "Entity",
            "Turns an entity to face a point.",
        )
        .run()
        .input("entity", Entity, entity())
        .input("target", Vec3, v3(0.0, 0.0, 0.0))
        .then(),
        NodeSpec::new(
            "entity.material",
            "Set material",
            "Entity",
            "Changes this entity's own copy of its material (only the wired or typed fields).",
        )
        .run()
        .input("entity", Entity, entity())
        .input("base_color", Vec3, v3(1.0, 1.0, 1.0))
        .input("emissive", Vec3, v3(1.0, 1.0, 1.0))
        .input("emissive_strength", Number, json!(0))
        .then(),
        NodeSpec::new(
            "entity.send",
            "Send message",
            "Entity",
            "Sends a message to an entity's script (its Message event or method).",
        )
        .run()
        .input("entity", Entity, entity())
        .input("event", Text, json!("ping"))
        .input("data", Any, Value::Null)
        .then(),
        NodeSpec::new(
            "entity.destroy",
            "Destroy",
            "Entity",
            "Despawns an entity and its children.",
        )
        .run()
        .input("entity", Entity, entity())
        .then(),
        // Math.
        binary("math.add", "+", "Adds numbers or vec3s."),
        binary("math.subtract", "-", "Subtracts."),
        binary("math.multiply", "×", "Multiplies (a vec3 by a number too)."),
        binary("math.divide", "÷", "Divides."),
        NodeSpec::new("math.sin", "Sine", "Math", "sin(x), x in radians.")
            .pure()
            .input("x", Number, json!(0))
            .output("value", Number),
        NodeSpec::new("math.cos", "Cosine", "Math", "cos(x), x in radians.")
            .pure()
            .input("x", Number, json!(0))
            .output("value", Number),
        NodeSpec::new("math.lerp", "Lerp", "Math", "a + (b - a) × t.")
            .pure()
            .input("a", Any, json!(0))
            .input("b", Any, json!(1))
            .input("t", Number, json!(0.5))
            .output("value", Any),
        NodeSpec::new("math.clamp", "Clamp", "Math", "x kept between min and max.")
            .pure()
            .input("x", Number, json!(0))
            .input("min", Number, json!(0))
            .input("max", Number, json!(1))
            .output("value", Number),
        NodeSpec::new(
            "math.random",
            "Random",
            "Math",
            "A random number between min and max (a new one each time it is used).",
        )
        .pure()
        .input("min", Number, json!(0))
        .input("max", Number, json!(1))
        .output("value", Number),
        NodeSpec::new(
            "math.time",
            "Time",
            "Math",
            "World seconds since the game started.",
        )
        .pure()
        .output("seconds", Number),
        compare("math.greater", ">", "a > b"),
        compare("math.less", "<", "a < b"),
        compare("math.equal", "=", "a equals b"),
        NodeSpec::new("logic.and", "And", "Math", "Both true.")
            .pure()
            .input("a", Bool, json!(false))
            .input("b", Bool, json!(false))
            .output("value", Bool),
        NodeSpec::new("logic.or", "Or", "Math", "Either true.")
            .pure()
            .input("a", Bool, json!(false))
            .input("b", Bool, json!(false))
            .output("value", Bool),
        NodeSpec::new("logic.not", "Not", "Math", "The opposite.")
            .pure()
            .input("a", Bool, json!(false))
            .output("value", Bool),
        NodeSpec::new(
            "logic.select",
            "Select",
            "Math",
            "a when the condition holds, else b.",
        )
        .pure()
        .input("condition", Bool, json!(false))
        .input("a", Any, json!(1))
        .input("b", Any, json!(0))
        .output("value", Any),
        NodeSpec::new("vec3.make", "Make vec3", "Math", "A vec3 from x, y and z.")
            .pure()
            .input("x", Number, json!(0))
            .input("y", Number, json!(0))
            .input("z", Number, json!(0))
            .output("vec3", Vec3),
        NodeSpec::new("vec3.break", "Break vec3", "Math", "A vec3's x, y and z.")
            .pure()
            .input("vec3", Vec3, v3(0.0, 0.0, 0.0))
            .output("x", Number)
            .output("y", Number)
            .output("z", Number),
        NodeSpec::new("vec3.length", "Length", "Math", "How long a vec3 is.")
            .pure()
            .input("vec3", Vec3, v3(0.0, 0.0, 0.0))
            .output("value", Number),
        NodeSpec::new(
            "vec3.distance",
            "Distance",
            "Math",
            "How far apart two points are.",
        )
        .pure()
        .input("a", Vec3, v3(0.0, 0.0, 0.0))
        .input("b", Vec3, v3(0.0, 0.0, 0.0))
        .output("value", Number),
        // Text.
        NodeSpec::new("text.join", "Join text", "Text", "a then b, as text.")
            .pure()
            .input("a", Text, json!(""))
            .input("b", Text, json!(""))
            .output("text", Text),
        NodeSpec::new(
            "text.number",
            "Number to text",
            "Text",
            "A number with so many decimals.",
        )
        .pure()
        .input("number", Number, json!(0))
        .input("decimals", Number, json!(2))
        .output("text", Text),
        NodeSpec::new(
            "debug.print",
            "Print",
            "Text",
            "Writes to the Output and the log.",
        )
        .run()
        .input("text", Text, json!("Hello"))
        .then(),
    ]
}

fn binary(kind: &str, title: &str, doc: &str) -> NodeSpec {
    NodeSpec::new(kind, title, "Math", doc)
        .pure()
        .input("a", PinType::Any, json!(0))
        .input("b", PinType::Any, json!(0))
        .output("value", PinType::Any)
}

fn compare(kind: &str, title: &str, doc: &str) -> NodeSpec {
    NodeSpec::new(kind, title, "Math", doc)
        .pure()
        .input("a", PinType::Any, json!(0))
        .input("b", PinType::Any, json!(0))
        .output("value", PinType::Bool)
}

/// Every node a graph can use in this game: [`builtins`], one per engine function (its
/// arguments from its JSON example) and a get and a set per component (fields from an entity
/// that has one, when any does), and the graph's own variables and props.
pub fn library(world: &bevy_ecs::world::World, graph: Option<&Graph>) -> Vec<NodeSpec> {
    let mut out = builtins();
    if let Some(api) = world.get_resource::<engine_core::registry::ApiRegistry>() {
        for e in api.list() {
            out.push(api_spec(&e.name, &e.summary, &e.args_example));
        }
    }
    if let Some(types) = world.get_resource::<engine_core::registry::TypeRegistry>() {
        for c in types.components() {
            let sample = (c.component_id)(world).and_then(|id| {
                world
                    .archetypes()
                    .iter()
                    .find(|a| !a.is_empty() && a.contains(id))
                    .and_then(|a| a.entities().first().map(|e| e.id()))
                    .and_then(|e| (c.get)(world, e))
            });
            let doc = c.doc.lines().next().unwrap_or("").to_string();
            out.extend(component_specs(&c.name, &doc, sample.as_ref()));
        }
    }
    if let Some(g) = graph {
        out.extend(own_specs(g));
    }
    out
}

/// A signal's or custom event's values, from `count: number, by: Entity`: (name, pin type).
/// A value without a type is Any; what isn't a name is left out.
pub fn params(text: &str) -> Vec<(String, PinType)> {
    text.split(',')
        .filter_map(|p| {
            let (name, ty) = p.split_once(':').unwrap_or((p, ""));
            let name = name.trim();
            is_ident(name).then(|| (name.to_string(), PinType::of_luau(ty)))
        })
        .collect()
}

/// The variable, prop, custom event and signal nodes of `g`: a Get and a Set per variable, a
/// Prop per prop; per custom event its event node, a Call and a Bind; an Emit per signal.
pub fn own_specs(g: &Graph) -> Vec<NodeSpec> {
    let mut out = Vec::new();
    for (name, values) in &g.events {
        let values = params(values);
        let mut ev = NodeSpec::new(
            "event.custom",
            name,
            "Events",
            "A custom event: runs when a signal wired to it fires (the Details panel's \
             Signals, or a Bind node), a Call node calls it, or a message of its name arrives.",
        )
        .then();
        let mut call = NodeSpec::new(
            "call.event",
            &format!("Call {name}"),
            "Events",
            "Runs this script's custom event now, then goes on.",
        )
        .run();
        for (p, ty) in &values {
            ev = ev.output(p, *ty);
            call = call.input(p, *ty, ty.zero());
        }
        ev.of = name.clone();
        out.push(ev);
        call = call.then();
        call.of = name.clone();
        out.push(call);
        let mut bind = NodeSpec::new(
            "signal.bind",
            &format!("Bind signal to {name}"),
            "Signals",
            "From now on, when the entity's script fires the signal, this script's custom \
             event runs with the signal's values (once: only the next time).",
        )
        .run()
        .input("entity", PinType::Entity, Value::Null)
        .setting("signal", PinType::Text, json!(""), &[])
        .setting("once", PinType::Bool, json!(false), &[])
        .then();
        bind.of = name.clone();
        out.push(bind);
    }
    for (name, values) in &g.signals {
        let doc = g
            .signal_notes
            .get(name)
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| {
                "Fires this script's signal: every method wired to it in the Details panel and \
                 every Bind on it runs, this frame."
                    .into()
            });
        let mut emit = NodeSpec::new("signal.emit", &format!("Emit {name}"), "Signals", &doc).run();
        for (p, ty) in params(values) {
            emit = emit.input(&p, ty, ty.zero());
        }
        emit = emit.then();
        emit.of = name.clone();
        out.push(emit);
    }
    for (name, v) in &g.vars {
        let ty = PinType::of_value(v);
        let mut get = NodeSpec::new("var.get", name, "Variables", "A variable of this script.")
            .pure()
            .output("value", ty);
        get.of = name.clone();
        out.push(get);
        let mut set = NodeSpec::new("var.set", &format!("Set {name}"), "Variables", "")
            .run()
            .input("value", ty, v.clone())
            .then();
        set.of = name.clone();
        out.push(set);
    }
    for (name, v) in &g.props {
        let mut get = NodeSpec::new("prop.get", name, "Props", "A prop of this script.")
            .pure()
            .output("value", PinType::of_value(v));
        get.of = name.clone();
        out.push(get);
    }
    out
}

/// An engine function's node: an input per field of its example, the result out.
pub fn api_spec(name: &str, summary: &str, example: &str) -> NodeSpec {
    let space = name.rsplit_once('.').map(|(s, _)| s).unwrap_or("engine");
    let mut s = NodeSpec::new("api.call", name, &format!("Engine: {space}"), summary).run();
    s.of = name.to_string();
    if let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(example) {
        for (k, v) in fields {
            s = s.input(&k, PinType::of_value(&v), v);
        }
    }
    s.then().output("result", PinType::Any)
}

/// A component's Get (its fields out) and Set (its fields in; only the wired or typed ones
/// change) nodes. `sample`: the component's JSON on some entity, for its fields.
pub fn component_specs(name: &str, doc: &str, sample: Option<&Value>) -> Vec<NodeSpec> {
    let fields: Vec<(String, Value)> = match sample {
        Some(Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => Vec::new(),
    };
    let category = format!("Components: {name}");
    let mut get = NodeSpec::new("comp.get", &format!("Get {name}"), &category, doc)
        .pure()
        .input("entity", PinType::Entity, Value::Null);
    let mut set = NodeSpec::new("comp.set", &format!("Set {name}"), &category, doc)
        .run()
        .input("entity", PinType::Entity, Value::Null);
    for (k, v) in &fields {
        get = get.output(k, PinType::of_value(v));
        set = set.input(k, PinType::of_value(v), v.clone());
    }
    get.of = name.to_string();
    set = set.then();
    set.of = name.to_string();
    vec![get, set]
}

/// The spec a node is drawn and checked with: from `library` by kind and `of`, else from the
/// pins its wires and values use (an engine function this game doesn't have).
pub fn spec_for(library: &[NodeSpec], node: &Node, graph: &Graph) -> NodeSpec {
    let own = |s: &NodeSpec| s.kind == node.kind && (s.of.is_empty() || s.of == node.of);
    if let Some(s) = library
        .iter()
        .find(|s| own(s) && s.of == node.of)
        .or_else(|| library.iter().find(|s| own(s)))
    {
        let mut s = s.clone();
        if !node.of.is_empty() && s.of.is_empty() {
            s.of = node.of.clone();
            if matches!(node.kind.as_str(), "var.get" | "prop.get") {
                s.title = node.of.clone();
            } else if node.kind == "var.set" {
                s.title = format!("Set {}", node.of);
            }
        }
        return s;
    }
    let (title, category) = match node.kind.as_str() {
        "comp.get" => (format!("Get {}", node.of), "Components"),
        "comp.set" => (format!("Set {}", node.of), "Components"),
        "api.call" => (node.of.clone(), "Engine"),
        _ => (format!("{} {}", node.kind, node.of), "Missing"),
    };
    let mut s = NodeSpec::new(&node.kind, title.trim(), category, "Not in this game.");
    s.of = node.of.clone();
    let action = node.kind != "comp.get";
    if action {
        s = s.run();
    }
    if node.kind.starts_with("comp.") {
        s = s.input("entity", PinType::Entity, Value::Null);
    }
    for (k, v) in &node.values {
        if s.input_pin(k).is_none() {
            s = s.input(k, PinType::of_value(v), v.clone());
        }
    }
    if action {
        s = s.then();
    }
    if node.kind == "api.call" {
        s = s.output("result", PinType::Any);
    }
    for l in &graph.links {
        if l.to.0 == node.id && s.input_pin(&l.to.1).is_none() {
            s = s.input(&l.to.1, PinType::Any, Value::Null);
        }
        if l.from.0 == node.id && s.output_pin(&l.from.1).is_none() {
            s = s.output(&l.from.1, PinType::Any);
        }
    }
    s
}

// ------------------------------------------------------------------------------- the look

/// How a node is laid out and colored (graph units: pixels at zoom 1), shared by the editor's
/// Graphs tab and graph boards so the two draw a graph alike. Colors follow Blueprints': red
/// events, white execution wires, data pins colored by type.
pub mod look {
    use super::{Node, NodeSpec, PinSpec, PinType};

    pub const HEADER: f32 = 26.0;
    pub const ROW: f32 = 22.0;
    pub const PAD: f32 = 8.0;
    /// About the width of one character of the node font (13 px).
    pub const CHAR: f32 = 7.0;
    /// Room for a socket beside a label.
    pub const SOCKET: f32 = 16.0;
    pub const MIN_WIDTH: f32 = 110.0;

    /// A node header's color by its category (sRGB).
    pub fn category_color(category: &str) -> [u8; 3] {
        let top = category.split(':').next().unwrap_or(category);
        match top {
            "Events" => [150, 32, 32],
            "Signals" => [124, 76, 190],
            "Flow" => [78, 80, 88],
            "Variables" | "Props" => [36, 112, 74],
            "Entity" => [38, 88, 156],
            "Math" => [56, 118, 76],
            "Text" => [150, 60, 112],
            "Engine" => [24, 112, 120],
            "Components" => [102, 64, 148],
            _ => [120, 40, 40],
        }
    }

    /// A pin's and its wires' color (sRGB).
    pub fn pin_color(ty: PinType) -> [u8; 3] {
        match ty {
            PinType::Exec => [235, 235, 235],
            PinType::Bool => [214, 54, 54],
            PinType::Number => [148, 226, 92],
            PinType::Text => [236, 92, 214],
            PinType::Vec3 => [250, 196, 52],
            PinType::Entity => [70, 156, 255],
            PinType::Table => [90, 214, 214],
            PinType::Any => [176, 176, 184],
        }
    }

    /// The width of the value field an unwired input shows (0: none).
    /// Nodes whose fields are a patch (Set component, Set material, engine functions): a field
    /// nobody set or wired is left as it is, not set to its default, and shows as a dash.
    pub fn patches(kind: &str) -> bool {
        matches!(kind, "comp.set" | "entity.material" | "api.call")
    }

    pub fn field_width(pin: &PinSpec) -> f32 {
        match pin.ty {
            PinType::Exec | PinType::Table => 0.0,
            PinType::Bool => 18.0,
            PinType::Number | PinType::Any => 52.0,
            PinType::Text if !pin.choices.is_empty() => 92.0,
            PinType::Text | PinType::Entity => 84.0,
            PinType::Vec3 => 150.0,
        }
    }

    /// A pin's label: execution pins named `in` and `then` go without, as in Blueprints, and
    /// so does a pure node's single `value`.
    pub fn label(pin: &PinSpec, output: bool, outputs: usize) -> String {
        let plain = match pin.ty {
            PinType::Exec => pin.name == "in" || pin.name == "then",
            _ => output && pin.name == "value" && outputs == 1,
        };
        if plain {
            String::new()
        } else {
            pin.name.replace('_', " ")
        }
    }

    /// One pin's row in a node.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Row {
        pub pin: PinSpec,
        pub label: String,
        /// Down from the node's top to the row's middle.
        pub y: f32,
        /// It has a socket (settings typed into the node don't).
        pub socket: bool,
        /// Width of its value field (0 when wired, or none).
        pub field: f32,
    }

    /// A node's size and pins.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Shape {
        pub size: [f32; 2],
        pub inputs: Vec<Row>,
        pub outputs: Vec<Row>,
    }

    impl Shape {
        /// Where a pin's socket is, from the node's top-left.
        pub fn socket(&self, pin: &str, output: bool) -> Option<[f32; 2]> {
            let rows = if output { &self.outputs } else { &self.inputs };
            let r = rows.iter().find(|r| r.pin.name == pin)?;
            Some([if output { self.size[0] } else { 0.0 }, r.y])
        }
    }

    /// Lays out `node` drawn as `spec`; `wired(pin)` tells which inputs have a wire (they show
    /// no value field).
    pub fn shape(spec: &NodeSpec, node: &Node, wired: &dyn Fn(&str) -> bool) -> Shape {
        let _ = node;
        let row_y = |i: usize| HEADER + PAD / 2.0 + ROW * (i as f32 + 0.5);
        let inputs: Vec<Row> = spec
            .inputs
            .iter()
            .enumerate()
            .map(|(i, p)| Row {
                pin: p.clone(),
                label: label(p, false, spec.outputs.len()),
                y: row_y(i),
                socket: !p.fixed,
                field: if p.fixed || !wired(&p.name) {
                    field_width(p)
                } else {
                    0.0
                },
            })
            .collect();
        let outputs: Vec<Row> = spec
            .outputs
            .iter()
            .enumerate()
            .map(|(i, p)| Row {
                pin: p.clone(),
                label: label(p, true, spec.outputs.len()),
                y: row_y(i),
                socket: true,
                field: 0.0,
            })
            .collect();
        let text = |s: &str| s.chars().count() as f32 * CHAR;
        let in_w = inputs
            .iter()
            .map(|r| SOCKET + text(&r.label) + if r.field > 0.0 { r.field + 6.0 } else { 0.0 })
            .fold(0.0, f32::max);
        let out_w = outputs
            .iter()
            .map(|r| SOCKET + text(&r.label))
            .fold(0.0, f32::max);
        let width = (text(&spec.title) + 2.0 * PAD + 8.0)
            .max(in_w + out_w + 2.0 * PAD + 6.0)
            .max(MIN_WIDTH);
        let rows = inputs.len().max(outputs.len()).max(1);
        Shape {
            size: [width.ceil(), HEADER + PAD + ROW * rows as f32],
            inputs,
            outputs,
        }
    }
}

// ------------------------------------------------------------------------------- compiling

/// What [`compile`] found wrong (or suspicious) and where.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub node: Option<u32>,
    pub pin: Option<String>,
    pub message: String,
    /// Stops the graph from loading (else a warning).
    pub error: bool,
}

/// A data wire's value as Luau, at the line where it is used.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Watch {
    /// Index into the graph's links.
    pub link: usize,
    /// 1-based line of the generated code.
    pub line: u32,
    pub expr: String,
}

/// A graph as Luau.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Compiled {
    pub luau: String,
    /// The node each line came from (index: line - 1).
    pub lines: Vec<Option<u32>>,
    /// Each node's first line (1-based), where it runs: logpoints there count its runs.
    pub node_lines: BTreeMap<u32, u32>,
    pub problems: Vec<Problem>,
    pub watches: Vec<Watch>,
}

impl Compiled {
    pub fn errors(&self) -> impl Iterator<Item = &Problem> {
        self.problems.iter().filter(|p| p.error)
    }

    /// The node line `line` (1-based) belongs to.
    pub fn node_at(&self, line: u32) -> Option<u32> {
        self.lines
            .get(line.checked_sub(1)? as usize)
            .copied()
            .flatten()
    }
}

// ------------------------------------------------------------------------------- watching

/// Between the wire values in one watch logpoint's text.
const SEP: char = '\u{1f}';

impl Compiled {
    /// Where a running graph is watched: every line where a node runs or a wire's value is
    /// used, with the wires used there.
    pub fn watch_lines(&self) -> BTreeMap<u32, Vec<&Watch>> {
        let mut out: BTreeMap<u32, Vec<&Watch>> = BTreeMap::new();
        for line in self.node_lines.values() {
            out.entry(*line).or_default();
        }
        for w in &self.watches {
            out.entry(w.line).or_default().push(w);
        }
        out
    }
}

/// Puts logpoints on a loaded graph's lines (replacing the API logpoints it had), so [`live`]
/// can tell what it does: they count each node's runs and read the wires' values, without
/// stopping anything (and at most every 50 ms per line). Call it again when the graph
/// reloads, since its lines move. False when no scripts are running.
pub fn watch(world: &bevy_ecs::world::World, path: &str, compiled: &Compiled) -> bool {
    match world.get_resource::<crate::ScriptRuntime>() {
        Some(rt) => {
            watch_in(rt, path, compiled);
            true
        }
        None => false,
    }
}

/// The version of `path` the runtime has loaded, when it is the code `compiled` holds and not
/// an older or newer edit: [`watch`]'s lines fit only that one.
pub fn loaded_version(
    world: &bevy_ecs::world::World,
    path: &str,
    compiled: &Compiled,
) -> Option<u32> {
    loaded_in(
        world.get_resource::<crate::ScriptRuntime>()?,
        path,
        compiled,
    )
}

pub(crate) fn loaded_in(rt: &crate::ScriptRuntime, path: &str, compiled: &Compiled) -> Option<u32> {
    rt.files()
        .map
        .get(path)
        .filter(|f| f.source == compiled.luau)
        .map(|f| f.version)
}

pub(crate) fn watch_in(rt: &crate::ScriptRuntime, path: &str, compiled: &Compiled) {
    rt.debugger.clear_logpoints(&rt.lua, Some(path));
    for (line, watches) in compiled.watch_lines() {
        let message: Vec<String> = watches.iter().map(|w| format!("{{{}}}", w.expr)).collect();
        let sep = SEP.to_string();
        rt.debugger
            .add_logpoint(&rt.lua, path, line, message.join(&sep), false);
    }
}

/// What a watched graph is doing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    /// How many times each node has run since it was watched (by node id).
    pub runs: BTreeMap<u32, u64>,
    /// Each data wire's newest value (by index into the graph's links).
    pub values: BTreeMap<usize, String>,
}

/// Reads a watched graph's marks back onto its nodes and wires: `hits` and `values` by line,
/// as [`crate::editing::FileMarks`] has them.
pub fn live(
    compiled: &Compiled,
    hits: &BTreeMap<u32, u64>,
    values: &BTreeMap<u32, String>,
) -> Live {
    let mut out = Live::default();
    for (node, line) in &compiled.node_lines {
        if let Some(n) = hits.get(line) {
            out.runs.insert(*node, *n);
        }
    }
    for (line, watches) in compiled.watch_lines() {
        let Some(text) = values.get(&line) else {
            continue;
        };
        for (w, value) in watches.iter().zip(text.split(SEP)) {
            out.values.insert(w.link, short_value(value));
        }
    }
    out
}

/// A value as a wire shows it: numbers to about three digits (`279.5393` -> `280`,
/// `0.0191` -> `0.019`), the rest as it is (`vec3(1, 0.72, 0.42)`).
pub fn short_value(text: &str) -> String {
    let short = |x: f64| {
        if x != 0.0 && x.abs() < 0.001 {
            return format!("{x:.1e}");
        }
        let decimals = match x.abs() {
            a if a >= 100.0 || x.fract() == 0.0 => 0,
            a if a >= 10.0 => 1,
            a if a >= 1.0 => 2,
            _ => 3,
        };
        let s = format!("{x:.decimals$}");
        let s = if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        };
        if s == "-0" { "0".to_string() } else { s }
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let after_word = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        let starts = c.is_ascii_digit()
            || (c == '-' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()));
        if starts && !after_word {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            // An exponent: 1e-05.
            if j < chars.len() && (chars[j] == 'e' || chars[j] == 'E') {
                let mut k = j + 1;
                if k < chars.len() && (chars[k] == '-' || chars[k] == '+') {
                    k += 1;
                }
                if k < chars.len() && chars[k].is_ascii_digit() {
                    while k < chars.len() && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    j = k;
                }
            }
            let token: String = chars[i..j].iter().collect();
            let word = j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_');
            match token.parse::<f64>() {
                Ok(x) if !word => out.push_str(&short(x)),
                _ => out.push_str(&token),
            }
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The hooks events become, with their parameters.
const HOOKS: &[(&str, &str, &[&str])] = &[
    ("event.start", "start", &[]),
    ("event.update", "update", &["dt"]),
    ("event.fixed_update", "fixed_update", &["dt"]),
    ("event.interact", "on_interact", &["by"]),
    ("event.contact", "on_contact", &["other", "info"]),
    ("event.message", "on_message", &["event", "data"]),
    ("event.destroy", "on_destroy", &[]),
];

/// A hook parameter's Luau type (strict code needs them to do arithmetic on `dt`).
fn hook_param_type(name: &str) -> &'static str {
    match name {
        "dt" => "number",
        "by" | "other" => "Entity",
        "event" => "string",
        _ => "any",
    }
}

/// Compiles a graph file's text (`path`, relative to the scripts folder, names the class and
/// the header).
pub fn compile_text(text: &str, path: &str) -> Result<Compiled, String> {
    Ok(compile(&Graph::parse(text)?, path))
}

/// The class name for a file: `showcase/glow_crystal.graph` -> `GlowCrystal`.
pub fn class_name(path: &str) -> String {
    let stem = path.rsplit('/').next().unwrap_or(path);
    let stem = stem.split('.').next().unwrap_or(stem);
    let mut out = String::new();
    for part in stem.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "Graph");
    }
    out
}

/// Compiles a graph to a Luau script.
pub fn compile(graph: &Graph, path: &str) -> Compiled {
    let mut g = Gen::new(graph, path);
    g.run();
    g.finish()
}

/// A value usable in the code being written: an event's parameter, a loop's variable, an
/// action's result.
type Scope = Vec<HashMap<(u32, String), String>>;

struct Gen<'a> {
    g: &'a Graph,
    class: String,
    path: String,
    nodes: HashMap<u32, &'a Node>,
    specs: HashMap<u32, NodeSpec>,
    /// Exec wires out of (node, pin), in file order.
    exec_out: HashMap<(u32, String), Vec<(u32, String)>>,
    /// How many exec wires come into (node, pin).
    exec_in: HashMap<(u32, String), usize>,
    /// The wire into a data input, by (node, pin): its source and its index.
    data_in: HashMap<(u32, String), ((u32, String), usize)>,
    /// Data outputs used somewhere.
    used: BTreeSet<(u32, String)>,
    lines: Vec<(String, Option<u32>)>,
    indent: usize,
    problems: Vec<Problem>,
    watches: Vec<Watch>,
    pending: Vec<(usize, String)>,
    /// (node, in pin) emitted as a method, waiting to be written.
    methods: Vec<(u32, String)>,
    methods_done: BTreeSet<(u32, String)>,
    /// Nodes written so far (the rest never run).
    emitted: BTreeSet<u32>,
    node_lines: BTreeMap<u32, u32>,
    /// Pure nodes being worked out, against data loops.
    evaluating: Vec<u32>,
}

impl<'a> Gen<'a> {
    fn new(g: &'a Graph, path: &str) -> Self {
        let library = builtins();
        let mut s = Self {
            g,
            class: class_name(path),
            path: path.to_string(),
            nodes: HashMap::new(),
            specs: HashMap::new(),
            exec_out: HashMap::new(),
            exec_in: HashMap::new(),
            data_in: HashMap::new(),
            used: BTreeSet::new(),
            lines: Vec::new(),
            indent: 0,
            problems: Vec::new(),
            watches: Vec::new(),
            pending: Vec::new(),
            methods: Vec::new(),
            methods_done: BTreeSet::new(),
            emitted: BTreeSet::new(),
            node_lines: BTreeMap::new(),
            evaluating: Vec::new(),
        };
        let mut own = own_specs(g);
        own.extend(library);
        for n in &g.nodes {
            if s.nodes.insert(n.id, n).is_some() {
                s.problem(
                    Some(n.id),
                    None,
                    format!("two nodes have id {}", n.id),
                    true,
                );
            }
            s.specs.insert(n.id, spec_for(&own, n, g));
        }
        for (i, l) in g.links.iter().enumerate() {
            let (Some(from), Some(to)) = (s.specs.get(&l.from.0), s.specs.get(&l.to.0)) else {
                s.problem(
                    None,
                    None,
                    format!(
                        "a wire joins a node that isn't there ({} to {})",
                        l.from.0, l.to.0
                    ),
                    false,
                );
                continue;
            };
            let exec = from.output_pin(&l.from.1).map(|p| p.ty) == Some(PinType::Exec);
            let into_exec = to.input_pin(&l.to.1).is_some_and(|p| p.ty == PinType::Exec);
            if exec {
                s.exec_out
                    .entry(l.from.clone())
                    .or_default()
                    .push(l.to.clone());
                *s.exec_in.entry(l.to.clone()).or_default() += 1;
            } else {
                if into_exec {
                    s.problem(
                        Some(l.to.0),
                        Some(&l.to.1),
                        "a value wired into an execution pin".into(),
                        false,
                    );
                    continue;
                }
                if s.data_in
                    .insert(l.to.clone(), (l.from.clone(), i))
                    .is_some()
                {
                    s.problem(
                        Some(l.to.0),
                        Some(&l.to.1),
                        "two wires into one input; the last one counts".into(),
                        false,
                    );
                }
                s.used.insert(l.from.clone());
            }
        }
        s
    }

    fn problem(&mut self, node: Option<u32>, pin: Option<&str>, message: String, error: bool) {
        self.problems.push(Problem {
            node,
            pin: pin.map(str::to_string),
            message,
            error,
        });
    }

    fn line(&mut self, text: impl AsRef<str>, node: Option<u32>) {
        let text = text.as_ref();
        let line = if text.is_empty() {
            String::new()
        } else {
            format!("{}{text}", "\t".repeat(self.indent))
        };
        self.lines.push((line, node));
        let n = self.lines.len() as u32;
        for (link, expr) in std::mem::take(&mut self.pending) {
            self.watches.push(Watch {
                link,
                line: n,
                expr,
            });
        }
        if let Some(id) = node {
            self.node_lines.entry(id).or_insert(n);
        }
    }

    fn run(&mut self) {
        let c = self.class.clone();
        // Strict, so Luau's type checker (the editor's) checks the values it passes around.
        self.line("--!strict", None);
        self.line(
            format!(
                "-- Generated from {} by the engine's graph compiler: edit the graph, not this.",
                self.path
            ),
            None,
        );
        for doc in self.g.doc.lines() {
            self.line(format!("-- {doc}"), None);
        }
        self.line(format!("local {c} = {{}}"), None);
        if !self.g.props.is_empty() {
            self.line(format!("{c}.props = {{"), None);
            self.indent += 1;
            let props: Vec<(String, Value)> = self
                .g
                .props
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (k, v) in props {
                if !is_ident(&k) {
                    self.problem(
                        None,
                        None,
                        format!("prop '{k}' isn't a name (letters, digits, _)"),
                        true,
                    );
                    continue;
                }
                // Its note on one line, as the comment the Details panel reads.
                let note = self
                    .g
                    .prop_notes
                    .get(&k)
                    .map(|n| n.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" -- {n}"))
                    .unwrap_or_default();
                self.line(format!("{k} = {},{note}", literal(&v)), None);
            }
            self.indent -= 1;
            self.line("}", None);
        }
        if !self.g.signals.is_empty() {
            self.line(format!("{c}.signals = {{"), None);
            self.indent += 1;
            let signals: Vec<(String, String)> = self
                .g
                .signals
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (k, v) in signals {
                if !is_ident(&k) {
                    self.problem(
                        None,
                        None,
                        format!("signal '{k}' isn't a name (letters, digits, _)"),
                        true,
                    );
                    continue;
                }
                let note = self
                    .g
                    .signal_notes
                    .get(&k)
                    .map(|n| n.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|n| !n.is_empty())
                    .map(|n| format!(" -- {n}"))
                    .unwrap_or_default();
                let values = params(&v)
                    .iter()
                    .map(|(p, ty)| format!("{p}: {}", ty.luau()))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(format!("{k} = {},{note}", text_literal(&values)), None);
            }
            self.indent -= 1;
            self.line("}", None);
        }
        for k in self.g.vars.keys() {
            if !is_ident(k) {
                self.problem(
                    None,
                    None,
                    format!("variable '{k}' isn't a name (letters, digits, _)"),
                    true,
                );
            }
        }
        self.check_loops();

        // Events, grouped by hook, top to bottom on the canvas.
        let mut events: Vec<&Node> = self
            .g
            .nodes
            .iter()
            .filter(|n| n.kind.starts_with("event."))
            .collect();
        events.sort_by_key(|n| (n.at[1], n.at[0], n.id));
        let gates: Vec<&Node> = self
            .g
            .nodes
            .iter()
            .filter(|n| n.kind == "flow.gate")
            .collect();
        for (kind, hook, params) in HOOKS {
            let these: Vec<&Node> = events.iter().copied().filter(|n| n.kind == *kind).collect();
            let start = *kind == "event.start";
            let everies: Vec<&Node> = events
                .iter()
                .copied()
                .filter(|n| n.kind == "event.every")
                .collect();
            let setup = !self.g.vars.is_empty() || !gates.is_empty() || !everies.is_empty();
            if these.is_empty() && !(start && setup) {
                continue;
            }
            self.line("", None);
            let typed: Vec<String> = params
                .iter()
                .map(|p| format!("{p}: {}", hook_param_type(p)))
                .collect();
            self.line(format!("function {c}:{hook}({})", typed.join(", ")), None);
            self.indent += 1;
            if start {
                let vars: Vec<(String, Value)> = self
                    .g
                    .vars
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (k, v) in vars {
                    if is_ident(&k) {
                        self.line(format!("self.{k} = {}", literal(&v)), None);
                    }
                }
                for gate in &gates {
                    self.line(
                        format!("self.__gate{} = {}", gate.id, gate_open(gate)),
                        None,
                    );
                }
                for every in &everies {
                    let (id, secs) = (every.id, every_secs(every));
                    self.line(
                        format!("self.__tick{id}, self.__secs{id} = self:every({secs}, \"__every{id}\"), {secs}"),
                        None,
                    );
                }
            }
            for e in these {
                self.emitted.insert(e.id);
                let mut scope: Scope = vec![HashMap::new()];
                for p in params.iter() {
                    scope[0].insert((e.id, p.to_string()), p.to_string());
                }
                let filter = (*kind == "event.message")
                    .then(|| e.values.get("name").and_then(Value::as_str).unwrap_or(""))
                    .filter(|n| !n.is_empty());
                match filter {
                    Some(name) => {
                        self.line(
                            format!("if event == {} then", text_literal(name)),
                            Some(e.id),
                        );
                        self.indent += 1;
                        self.chain(e.id, "then", &mut scope);
                        self.indent -= 1;
                        self.line("end", Some(e.id));
                    }
                    None => self.chain(e.id, "then", &mut scope),
                }
            }
            self.indent -= 1;
            self.line("end", None);
        }
        for every in events.iter().filter(|n| n.kind == "event.every") {
            self.emitted.insert(every.id);
            self.line("", None);
            self.line(format!("function {c}:__every{}()", every.id), None);
            self.indent += 1;
            self.chain(every.id, "then", &mut vec![HashMap::new()]);
            self.indent -= 1;
            self.line("end", None);
        }
        self.custom_events(&events);
        // After a hot reload: new variables and gates start at their values, new Every timers
        // start and changed ones restart (the runtime stops the removed ones).
        let vars: Vec<(String, Value)> = self
            .g
            .vars
            .iter()
            .filter(|(k, _)| is_ident(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let everies: Vec<&Node> = events
            .iter()
            .copied()
            .filter(|n| n.kind == "event.every")
            .collect();
        if !vars.is_empty() || !gates.is_empty() || !everies.is_empty() {
            self.line("", None);
            self.line(format!("function {c}:on_reload()"), None);
            self.indent += 1;
            for (k, v) in vars {
                self.line(
                    format!("if self.{k} == nil then self.{k} = {} end", literal(&v)),
                    None,
                );
            }
            for gate in &gates {
                let id = gate.id;
                self.line(
                    format!(
                        "if self.__gate{id} == nil then self.__gate{id} = {} end",
                        gate_open(gate)
                    ),
                    None,
                );
            }
            for every in &everies {
                let (id, secs) = (every.id, every_secs(every));
                self.line(format!("if self.__secs{id} ~= {secs} then"), None);
                self.line(
                    format!("\tif self.__tick{id} then task.cancel(self.__tick{id}) end"),
                    None,
                );
                self.line(format!("\tself.__tick{id}, self.__secs{id} = self:every({secs}, \"__every{id}\"), {secs}"), None);
                self.line("end", None);
            }
            self.indent -= 1;
            self.line("end", None);
        }
        // Nodes reached from more than one place.
        while let Some((id, pin)) = self.methods.pop() {
            if !self.methods_done.insert((id, pin.clone())) {
                continue;
            }
            self.line("", None);
            self.line(
                format!("function {c}:{}()", method_name(id, &pin)),
                Some(id),
            );
            self.indent += 1;
            self.enter_inline(id, &pin, &mut vec![HashMap::new()]);
            self.indent -= 1;
            self.line("end", None);
        }
        self.line("", None);
        self.line(format!("return {c}"), None);

        // Actions nothing leads to.
        let mut idle: Vec<u32> = self
            .g
            .nodes
            .iter()
            .filter(|n| {
                let s = &self.specs[&n.id];
                !s.pure && !s.is_event() && !self.emitted.contains(&n.id)
            })
            .map(|n| n.id)
            .collect();
        idle.sort();
        for id in idle {
            self.problem(
                Some(id),
                None,
                "never runs: no execution wire leads into it".into(),
                false,
            );
        }
    }

    /// A method per custom event: its node's chain, or nothing yet (one with no node can
    /// still be wired, bound and called).
    fn custom_events(&mut self, events: &[&'a Node]) {
        let c = self.class.clone();
        let mut written = BTreeSet::new();
        for e in events.iter().filter(|n| n.kind == "event.custom") {
            let name = e.of.clone();
            if !self.g.events.contains_key(&name) {
                self.problem(
                    Some(e.id),
                    None,
                    format!("no custom event '{name}' (add it under Events)"),
                    true,
                );
                continue;
            }
            if !self.event_name_ok(&name) {
                continue;
            }
            if !written.insert(name.clone()) {
                self.problem(
                    Some(e.id),
                    None,
                    format!("a second node for custom event '{name}': only the first runs"),
                    true,
                );
                continue;
            }
            self.emitted.insert(e.id);
            let values = params(&self.g.events[&name]);
            let mut scope: Scope = vec![HashMap::new()];
            for (p, _) in &values {
                scope[0].insert((e.id, p.clone()), p.clone());
            }
            self.event_head(&c, &name, &values);
            self.indent += 1;
            self.chain(e.id, "then", &mut scope);
            self.indent -= 1;
            self.line("end", None);
        }
        let rest: Vec<(String, String)> = self
            .g
            .events
            .iter()
            .filter(|(k, _)| {
                !events
                    .iter()
                    .any(|n| n.kind == "event.custom" && &n.of == *k)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, values) in rest {
            if self.event_name_ok(&name) {
                self.event_head(&c, &name, &params(&values));
                self.line("end", None);
            }
        }
    }

    fn event_name_ok(&mut self, name: &str) -> bool {
        let hook = HOOKS.iter().any(|(_, h, _)| *h == name)
            || matches!(name, "on_reload" | "props" | "signals" | "emit" | "entity");
        if !is_ident(name) || name.starts_with("__") || hook {
            self.problem(
                None,
                None,
                format!("custom event '{name}': a hook's or the script's own name, or not a name"),
                true,
            );
            return false;
        }
        true
    }

    fn event_head(&mut self, c: &str, name: &str, values: &[(String, PinType)]) {
        let typed: Vec<String> = values
            .iter()
            .map(|(p, ty)| format!("{p}: {}", ty.luau()))
            .collect();
        self.line("", None);
        self.line(format!("function {c}:{name}({})", typed.join(", ")), None);
    }

    fn finish(self) -> Compiled {
        let mut luau = String::new();
        let mut lines = Vec::with_capacity(self.lines.len());
        for (text, node) in self.lines {
            luau.push_str(&text);
            luau.push('\n');
            lines.push(node);
        }
        Compiled {
            luau,
            lines,
            node_lines: self.node_lines,
            problems: self.problems,
            watches: self.watches,
        }
    }

    /// Loops of execution wires with no Delay or Tween in them would never end.
    fn check_loops(&mut self) {
        let mut edges: HashMap<u32, Vec<u32>> = HashMap::new();
        for ((from, _), tos) in &self.exec_out {
            let latent = self.nodes.get(from).is_some_and(|n| {
                matches!(n.kind.as_str(), "flow.delay" | "flow.tween" | "event.every")
            });
            if latent {
                continue;
            }
            for (to, _) in tos {
                edges.entry(*from).or_default().push(*to);
            }
        }
        let mut state: HashMap<u32, u8> = HashMap::new();
        let mut looped = BTreeSet::new();
        fn visit(
            n: u32,
            edges: &HashMap<u32, Vec<u32>>,
            state: &mut HashMap<u32, u8>,
            looped: &mut BTreeSet<u32>,
        ) {
            state.insert(n, 1);
            for &m in edges.get(&n).into_iter().flatten() {
                match state.get(&m) {
                    Some(1) => {
                        looped.insert(m);
                    }
                    Some(_) => {}
                    None => visit(m, edges, state, looped),
                }
            }
            state.insert(n, 2);
        }
        let mut starts: Vec<u32> = edges.keys().copied().collect();
        starts.sort();
        for n in starts {
            if !state.contains_key(&n) {
                visit(n, &edges, &mut state, &mut looped);
            }
        }
        for n in looped {
            self.problem(
                Some(n),
                None,
                "execution loops back here without a Delay, so it would never finish".into(),
                true,
            );
        }
    }

    /// Runs whatever the exec output (node, pin) leads to.
    fn chain(&mut self, node: u32, pin: &str, scope: &mut Scope) {
        let targets = self
            .exec_out
            .get(&(node, pin.to_string()))
            .cloned()
            .unwrap_or_default();
        if targets.len() > 1 {
            self.problem(
                Some(node),
                Some(pin),
                "an execution output leads to more than one node; they run in wire order (use a \
                 Sequence)"
                    .into(),
                false,
            );
        }
        for (to, to_pin) in targets {
            self.enter(to, &to_pin, scope);
        }
    }

    fn enter(&mut self, node: u32, pin: &str, scope: &mut Scope) {
        if self
            .problems
            .iter()
            .any(|p| p.error && p.node == Some(node) && p.message.starts_with("execution loops"))
        {
            return;
        }
        let shared = self
            .exec_in
            .get(&(node, pin.to_string()))
            .copied()
            .unwrap_or(0)
            > 1;
        if shared {
            self.line(format!("self:{}()", method_name(node, pin)), Some(node));
            self.emitted.insert(node);
            self.methods.push((node, pin.to_string()));
            return;
        }
        self.enter_inline(node, pin, scope);
    }

    /// The input's value as Luau (its wire's source, or its typed value, or the default).
    fn input(&mut self, node: u32, pin: &str, scope: &Scope) -> String {
        let key = (node, pin.to_string());
        if let Some((src, link)) = self.data_in.get(&key).cloned() {
            let expr = self.output_expr(src.0, &src.1, node, scope);
            self.pending.push((link, expr.clone()));
            return expr;
        }
        let n = self.nodes[&node];
        let spec = &self.specs[&node];
        let value = n
            .values
            .get(pin)
            .cloned()
            .or_else(|| spec.input_pin(pin).map(|p| p.default.clone()))
            .unwrap_or(Value::Null);
        let ty = spec.input_pin(pin).map(|p| p.ty).unwrap_or(PinType::Any);
        if ty == PinType::Entity && value.is_null() {
            return "self.entity".into();
        }
        if ty == PinType::Entity
            && let Some(name) = value.as_str()
        {
            // A missing object is an error where it is used; the checker needn't ask for nil.
            return format!("(world.find({}) :: Entity)", text_literal(name));
        }
        literal(&value)
    }

    /// Whether an input is wired or typed in (for nodes that only send those).
    fn given(&self, node: u32, pin: &str) -> bool {
        self.data_in.contains_key(&(node, pin.to_string()))
            || self.nodes[&node].values.contains_key(pin)
    }

    /// The value of output `pin` of `node`, used by `user`.
    fn output_expr(&mut self, node: u32, pin: &str, user: u32, scope: &Scope) -> String {
        for frame in scope.iter().rev() {
            if let Some(name) = frame.get(&(node, pin.to_string())) {
                return name.clone();
            }
        }
        let Some(n) = self.nodes.get(&node).copied() else {
            return "nil".into();
        };
        let spec = self.specs[&node].clone();
        if !spec.pure {
            if n.kind == "flow.flip_flop" && pin == "is_a" {
                return format!("(self.__flip{node} == true)");
            }
            let from = format!("{} ({})", spec.title, node);
            self.problem(
                Some(user),
                None,
                format!(
                    "uses a value of '{from}' that doesn't exist here: that node hasn't run on \
                     the way to this one"
                ),
                true,
            );
            return "nil".into();
        }
        if self.evaluating.contains(&node) {
            self.problem(Some(node), None, "its value depends on itself".into(), true);
            return "nil".into();
        }
        self.evaluating.push(node);
        let e = self.pure_expr(n, &spec, pin, scope);
        self.evaluating.pop();
        e
    }

    fn pure_expr(&mut self, n: &Node, spec: &NodeSpec, pin: &str, scope: &Scope) -> String {
        let id = n.id;
        let i = |s: &mut Self, p: &str| s.input(id, p, scope);
        match n.kind.as_str() {
            "var.get" => {
                if !self.g.vars.contains_key(&n.of) {
                    self.problem(
                        Some(id),
                        None,
                        format!("no variable named '{}'", n.of),
                        true,
                    );
                }
                format!("self.{}", n.of)
            }
            "prop.get" => {
                if !self.g.props.contains_key(&n.of) {
                    self.problem(Some(id), None, format!("no prop named '{}'", n.of), true);
                }
                format!("self.props.{}", n.of)
            }
            "entity.self" => "self.entity".into(),
            "entity.find" => format!("world.find({})", i(self, "name")),
            "entity.position" => {
                let e = i(self, "entity");
                let field = if pin == "world" {
                    "world_position"
                } else {
                    "position"
                };
                format!("{}.{field}", paren(&e))
            }
            "entity.name" => format!("{}.name", paren(&i(self, "entity"))),
            "math.add" => format!("({} + {})", i(self, "a"), i(self, "b")),
            "math.subtract" => format!("({} - {})", i(self, "a"), i(self, "b")),
            "math.multiply" => format!("({} * {})", i(self, "a"), i(self, "b")),
            "math.divide" => format!("({} / {})", i(self, "a"), i(self, "b")),
            "math.sin" => format!("math.sin({})", i(self, "x")),
            "math.cos" => format!("math.cos({})", i(self, "x")),
            "math.lerp" => {
                let (a, b, t) = (i(self, "a"), i(self, "b"), i(self, "t"));
                format!("({a} + ({b} - {a}) * {t})")
            }
            "math.clamp" => format!(
                "math.clamp({}, {}, {})",
                i(self, "x"),
                i(self, "min"),
                i(self, "max")
            ),
            "math.random" => {
                let (lo, hi) = (i(self, "min"), i(self, "max"));
                format!("({lo} + math.random() * ({hi} - {lo}))")
            }
            "math.time" => "world.time()".into(),
            "math.greater" => format!("({} > {})", i(self, "a"), i(self, "b")),
            "math.less" => format!("({} < {})", i(self, "a"), i(self, "b")),
            "math.equal" => format!("({} == {})", i(self, "a"), i(self, "b")),
            "logic.and" => format!("({} and {})", i(self, "a"), i(self, "b")),
            "logic.or" => format!("({} or {})", i(self, "a"), i(self, "b")),
            "logic.not" => format!("(not {})", i(self, "a")),
            "logic.select" => format!(
                "(if {} then {} else {})",
                i(self, "condition"),
                i(self, "a"),
                i(self, "b")
            ),
            "vec3.make" => format!("vec3({}, {}, {})", i(self, "x"), i(self, "y"), i(self, "z")),
            "vec3.break" => {
                let axis = if matches!(pin, "x" | "y" | "z") {
                    pin
                } else {
                    "x"
                };
                format!("{}.{axis}", paren(&i(self, "vec3")))
            }
            "vec3.length" => format!("{}:length()", paren(&i(self, "vec3"))),
            "vec3.distance" => format!("({} - {}):length()", i(self, "a"), i(self, "b")),
            "text.join" => format!("(tostring({}) .. tostring({}))", i(self, "a"), i(self, "b")),
            "text.number" => format!(
                "string.format(\"%.\" .. math.floor({}) .. \"f\", {})",
                i(self, "decimals"),
                i(self, "number")
            ),
            "comp.get" => {
                let e = i(self, "entity");
                if is_ident(pin) {
                    format!("{}:get({}).{pin}", paren(&e), text_literal(&n.of))
                } else {
                    format!(
                        "{}:get({})[{}]",
                        paren(&e),
                        text_literal(&n.of),
                        text_literal(pin)
                    )
                }
            }
            other => {
                let title = spec.title.clone();
                self.problem(
                    Some(id),
                    None,
                    format!("unknown node '{other}' ({title})"),
                    true,
                );
                "nil".into()
            }
        }
    }

    /// Writes a node entered through `pin` and what follows it.
    fn enter_inline(&mut self, id: u32, pin: &str, scope: &mut Scope) {
        let Some(n) = self.nodes.get(&id).copied() else {
            return;
        };
        self.emitted.insert(id);
        let spec = self.specs[&id].clone();
        let at = Some(id);
        match n.kind.as_str() {
            "flow.branch" => {
                let cond = if self.given(id, "condition") {
                    self.input(id, "condition", scope)
                } else {
                    self.problem(
                        Some(id),
                        Some("condition"),
                        "condition isn't wired, so it is always false".into(),
                        false,
                    );
                    "false".into()
                };
                self.line(format!("if {cond} then"), at);
                self.block(id, "true", scope);
                if self.exec_out.contains_key(&(id, "false".into())) {
                    self.line("else", at);
                    self.block(id, "false", scope);
                }
                self.line("end", at);
            }
            "flow.sequence" => {
                for out in ["then 1", "then 2", "then 3"] {
                    self.chain(id, out, scope);
                }
            }
            "flow.delay" => {
                let secs = self.input(id, "seconds", scope);
                self.line(format!("if not self.__wait{id} then"), at);
                self.indent += 1;
                self.line(format!("self.__wait{id} = true"), at);
                self.line(format!("task.delay({secs}, function()"), at);
                self.indent += 1;
                self.line(format!("self.__wait{id} = nil"), at);
                scope.push(HashMap::new());
                self.chain(id, "then", scope);
                scope.pop();
                self.indent -= 1;
                self.line("end)", at);
                self.indent -= 1;
                self.line("end", at);
            }
            "flow.do_once" => {
                if pin == "reset" {
                    self.line(format!("self.__once{id} = nil"), at);
                    return;
                }
                self.line(format!("if not self.__once{id} then"), at);
                self.indent += 1;
                self.line(format!("self.__once{id} = true"), at);
                scope.push(HashMap::new());
                self.chain(id, "then", scope);
                scope.pop();
                self.indent -= 1;
                self.line("end", at);
            }
            "flow.flip_flop" => {
                self.line(format!("self.__flip{id} = not self.__flip{id}"), at);
                self.line(format!("if self.__flip{id} then"), at);
                self.block(id, "a", scope);
                if self.exec_out.contains_key(&(id, "b".into())) {
                    self.line("else", at);
                    self.block(id, "b", scope);
                }
                self.line("end", at);
            }
            "flow.gate" => match pin {
                "open" => self.line(format!("self.__gate{id} = true"), at),
                "close" => self.line(format!("self.__gate{id} = false"), at),
                "toggle" => self.line(format!("self.__gate{id} = not self.__gate{id}"), at),
                _ => {
                    self.line(format!("if self.__gate{id} then"), at);
                    self.block(id, "exit", scope);
                    self.line("end", at);
                }
            },
            "flow.for_loop" => {
                let (first, last) = (
                    self.input(id, "first", scope),
                    self.input(id, "last", scope),
                );
                self.line(format!("for i{id} = {first}, {last} do"), at);
                self.indent += 1;
                scope.push(HashMap::from([(
                    (id, "index".to_string()),
                    format!("i{id}"),
                )]));
                self.chain(id, "body", scope);
                scope.pop();
                self.indent -= 1;
                self.line("end", at);
                self.chain(id, "completed", scope);
            }
            "flow.for_each" => {
                let list = self.input(id, "list", scope);
                self.line(format!("for i{id}, item{id} in {list} do"), at);
                self.indent += 1;
                scope.push(HashMap::from([
                    ((id, "index".to_string()), format!("i{id}")),
                    ((id, "item".to_string()), format!("item{id}")),
                ]));
                self.chain(id, "body", scope);
                scope.pop();
                self.indent -= 1;
                self.line("end", at);
                self.chain(id, "completed", scope);
            }
            "flow.tween" => {
                let secs = self.input(id, "seconds", scope);
                let ease = n
                    .values
                    .get("ease")
                    .and_then(Value::as_str)
                    .filter(|e| EASES.contains(e))
                    .unwrap_or("smooth");
                self.line("task.spawn(function()", at);
                self.indent += 1;
                self.line(format!("task.tween({secs}, function(alpha{id})"), at);
                self.indent += 1;
                scope.push(HashMap::from([(
                    (id, "alpha".to_string()),
                    format!("alpha{id}"),
                )]));
                self.chain(id, "update", scope);
                scope.pop();
                self.indent -= 1;
                self.line(format!("end, ease.{ease})"), at);
                scope.push(HashMap::new());
                self.chain(id, "finished", scope);
                scope.pop();
                self.indent -= 1;
                self.line("end)", at);
            }
            "var.set" => {
                if !self.g.vars.contains_key(&n.of) {
                    self.problem(
                        Some(id),
                        None,
                        format!("no variable named '{}'", n.of),
                        true,
                    );
                }
                let v = self.input(id, "value", scope);
                self.line(format!("self.{} = {v}", n.of), at);
                self.chain(id, "then", scope);
            }
            "entity.set_position" => {
                let (e, p) = (
                    self.input(id, "entity", scope),
                    self.input(id, "position", scope),
                );
                self.line(format!("{}.position = {p}", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "entity.translate" => {
                let (e, by) = (self.input(id, "entity", scope), self.input(id, "by", scope));
                self.line(format!("{}:translate({by})", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "entity.rotate_y" => {
                let (e, d) = (
                    self.input(id, "entity", scope),
                    self.input(id, "degrees", scope),
                );
                self.line(format!("{}:rotate_y(math.rad({d}))", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "entity.look_at" => {
                let (e, t) = (
                    self.input(id, "entity", scope),
                    self.input(id, "target", scope),
                );
                self.line(format!("{}:look_at({t})", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "entity.material" => {
                let e = self.input(id, "entity", scope);
                let fields = self.fields(id, &spec, &["entity"], scope);
                self.line(format!("{}:material({fields})", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "entity.send" => {
                let e = self.input(id, "entity", scope);
                let ev = self.input(id, "event", scope);
                let data = self.input(id, "data", scope);
                self.line(format!("{}:send({ev}, {data})", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "entity.destroy" => {
                let e = self.input(id, "entity", scope);
                self.line(format!("{}:despawn()", paren(&e)), at);
                self.chain(id, "then", scope);
            }
            "debug.print" => {
                let t = self.input(id, "text", scope);
                self.line(format!("print({t})"), at);
                self.chain(id, "then", scope);
            }
            "comp.set" => {
                let e = self.input(id, "entity", scope);
                let fields = self.fields(id, &spec, &["entity"], scope);
                self.line(
                    format!("{}:set({}, {fields})", paren(&e), text_literal(&n.of)),
                    at,
                );
                self.chain(id, "then", scope);
            }
            "call.event" | "signal.emit" => {
                let emit = n.kind == "signal.emit";
                let (declared, what) = if emit {
                    (self.g.signals.get(&n.of), "signal")
                } else {
                    (self.g.events.get(&n.of), "custom event")
                };
                let Some(values) = declared.map(|v| params(v)) else {
                    let under = if emit { "Signals" } else { "Events" };
                    self.problem(
                        Some(id),
                        None,
                        format!("no {what} '{}' (add it under {under})", n.of),
                        true,
                    );
                    return;
                };
                let mut args: Vec<String> = values
                    .iter()
                    .map(|(p, _)| self.input(id, p, scope))
                    .collect();
                if emit {
                    args.insert(0, text_literal(&n.of));
                    self.line(format!("self:emit({})", args.join(", ")), at);
                } else {
                    self.line(format!("self:{}({})", n.of, args.join(", ")), at);
                }
                self.chain(id, "then", scope);
            }
            "signal.bind" => {
                if !self.g.events.contains_key(&n.of) {
                    self.problem(
                        Some(id),
                        None,
                        format!("no custom event '{}' to bind (add it under Events)", n.of),
                        true,
                    );
                    return;
                }
                let signal = n.values.get("signal").and_then(Value::as_str).unwrap_or("");
                if !is_ident(signal) {
                    self.problem(
                        Some(id),
                        Some("signal"),
                        "which signal? Type its name (the entity's script declares it)".into(),
                        true,
                    );
                    return;
                }
                let once = n.values.get("once").and_then(Value::as_bool) == Some(true);
                let e = self.input(id, "entity", scope);
                self.line(
                    format!(
                        "{}:{}({}, function(...) self:{}(...) end)",
                        paren(&e),
                        if once { "once" } else { "on" },
                        text_literal(signal),
                        n.of
                    ),
                    at,
                );
                self.chain(id, "then", scope);
            }
            "api.call" => {
                let call = n.of.clone();
                let valid = !call.is_empty() && call.split('.').all(is_ident) && call.contains('.');
                if !valid {
                    self.problem(
                        Some(id),
                        None,
                        format!("'{call}' isn't an engine function name"),
                        true,
                    );
                    return;
                }
                let args = self.fields(id, &spec, &[], scope);
                if self.used.contains(&(id, "result".to_string())) {
                    self.line(format!("local r{id} = {call}({args})"), at);
                    if let Some(top) = scope.last_mut() {
                        top.insert((id, "result".into()), format!("r{id}"));
                    }
                } else {
                    self.line(format!("{call}({args})"), at);
                }
                self.chain(id, "then", scope);
            }
            other => {
                let title = spec.title.clone();
                self.problem(
                    Some(id),
                    None,
                    format!("unknown node '{other}' ({title})"),
                    true,
                );
            }
        }
    }

    /// An indented block: what exec output `pin` leads to.
    fn block(&mut self, id: u32, pin: &str, scope: &mut Scope) {
        self.indent += 1;
        scope.push(HashMap::new());
        self.chain(id, pin, scope);
        scope.pop();
        self.indent -= 1;
    }

    /// A table of the node's wired or typed inputs (all but exec and `skip`).
    fn fields(&mut self, id: u32, spec: &NodeSpec, skip: &[&str], scope: &Scope) -> String {
        let mut names: Vec<String> = spec
            .inputs
            .iter()
            .filter(|p| p.ty != PinType::Exec && !skip.contains(&p.name.as_str()))
            .map(|p| p.name.clone())
            .collect();
        // Pins this game's spec doesn't list (an older or newer engine function).
        for k in self.nodes[&id].values.keys() {
            if !names.contains(k) && !skip.contains(&k.as_str()) {
                names.push(k.clone());
            }
        }
        let mut parts = Vec::new();
        for name in names {
            if !self.given(id, &name) {
                continue;
            }
            let v = self.input(id, &name, scope);
            if is_ident(&name) {
                parts.push(format!("{name} = {v}"));
            } else {
                parts.push(format!("[{}] = {v}", text_literal(&name)));
            }
        }
        if parts.is_empty() {
            "{}".into()
        } else {
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

fn gate_open(gate: &Node) -> bool {
    gate.values
        .get("start_open")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// An Every node's period as Luau.
fn every_secs(every: &Node) -> String {
    number(
        every
            .values
            .get("seconds")
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
            .max(0.0),
    )
}

fn method_name(id: u32, pin: &str) -> String {
    if pin == "in" {
        format!("__n{id}")
    } else {
        let p: String = pin
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("__n{id}_{p}")
    }
}

/// Wraps an expression for a method call or field (`(a + b).x`, but `self.entity:x`).
fn paren(e: &str) -> String {
    let simple = e
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if simple {
        e.to_string()
    } else {
        format!("({e})")
    }
}

/// Whether `s` can be a Luau name.
pub fn is_ident(s: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
        "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        "continue", "export", "type",
    ];
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && !KEYWORDS.contains(&s)
}

fn number(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else if x.is_finite() {
        format!("{x}")
    } else {
        "0".into()
    }
}

/// A Luau string literal.
pub fn text_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\{}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON value as Luau: `[x, y, z]` as a vec3, objects as tables.
pub fn literal(v: &Value) -> String {
    match v {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => text_literal(s),
        Value::Array(a) if a.len() == 3 && a.iter().all(Value::is_number) => format!(
            "vec3({}, {}, {})",
            number(a[0].as_f64().unwrap_or(0.0)),
            number(a[1].as_f64().unwrap_or(0.0)),
            number(a[2].as_f64().unwrap_or(0.0))
        ),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(literal).collect();
            format!("{{{}}}", items.join(", "))
        }
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    if is_ident(k) {
                        format!("{k} = {}", literal(v))
                    } else {
                        format!("[{}] = {}", text_literal(k), literal(v))
                    }
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wire_values_are_short() {
        assert_eq!(short_value("279.5393"), "280");
        assert_eq!(short_value("38.1152"), "38.1");
        assert_eq!(short_value("0.0191"), "0.019");
        assert_eq!(short_value("-2.50001"), "-2.5");
        assert_eq!(short_value("1800"), "1800");
        assert_eq!(
            short_value("vec3(1, 0.72345, 0.42)"),
            "vec3(1, 0.723, 0.42)"
        );
        assert_eq!(short_value("Lamp2"), "Lamp2");
        assert_eq!(short_value("1e-05"), "1.0e-5");
        assert_eq!(short_value("2.5e3"), "2500");
    }

    use super::*;

    fn node(id: u32, kind: &str, of: &str, values: Value) -> Node {
        Node {
            id,
            kind: kind.into(),
            of: of.into(),
            at: [0, id as i32 * 10],
            values: serde_json::from_value(values).unwrap_or_default(),
        }
    }

    fn link(a: u32, ap: &str, b: u32, bp: &str) -> Link {
        Link {
            from: (a, ap.into()),
            to: (b, bp.into()),
        }
    }

    /// The doc comment's graph: spin by speed × dt.
    fn spinner() -> Graph {
        Graph {
            props: BTreeMap::from([("speed".into(), json!(25))]),
            nodes: vec![
                node(1, "event.update", "", json!({})),
                node(2, "entity.rotate_y", "", json!({})),
                node(3, "math.multiply", "", json!({})),
                node(4, "prop.get", "speed", json!({})),
            ],
            links: vec![
                link(1, "then", 2, "in"),
                link(1, "dt", 3, "a"),
                link(4, "value", 3, "b"),
                link(3, "value", 2, "degrees"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_graph_becomes_a_script() {
        let c = compile(&spinner(), "examples/spinner.graph");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert!(c.luau.contains("local Spinner = {}"), "{}", c.luau);
        assert!(c.luau.contains("speed = 25,"), "{}", c.luau);
        assert!(
            c.luau.contains("function Spinner:update(dt: number)"),
            "{}",
            c.luau
        );
        assert!(
            c.luau
                .contains("self.entity:rotate_y(math.rad((dt * self.props.speed)))"),
            "{}",
            c.luau
        );
        // The rotate line belongs to node 2, and the wires into it are watched there.
        let line = c.node_lines[&2];
        assert_eq!(c.node_at(line), Some(2));
        assert!(
            c.watches
                .iter()
                .any(|w| w.link == 3 && w.line == line && w.expr == "(dt * self.props.speed)")
        );
        assert!(c.watches.iter().any(|w| w.link == 1 && w.expr == "dt"));
        // It loads as Luau.
        assert!(crate::editing::check(&c.luau).is_none(), "{}", c.luau);
        // A prop's note is its comment, so its hints reach the Details panel; the text keeps it.
        let mut g = spinner();
        g.prop_notes.insert(
            "speed".into(),
            "degrees a second\n@range 0 360 @unit °/s".into(),
        );
        let c = compile(&g, "examples/spinner.graph");
        assert!(
            c.luau
                .contains("speed = 25, -- degrees a second @range 0 360 @unit °/s"),
            "{}",
            c.luau
        );
        assert_eq!(Graph::parse(&g.to_text()).unwrap().prop_notes, g.prop_notes);
    }

    /// Blueprint-style talk between scripts: a custom event, a Bind to another object's
    /// signal, a Call, and an Emit of the graph's own signal.
    #[test]
    fn custom_events_and_signals_compile() {
        let g = Graph {
            signals: BTreeMap::from([("rang".into(), "times: number".into())]),
            signal_notes: BTreeMap::from([("rang".into(), "each ring".into())]),
            events: BTreeMap::from([
                ("ring".into(), "times: number, by: Entity".into()),
                ("hush".into(), String::new()),
            ]),
            nodes: vec![
                node(1, "event.custom", "ring", json!({})),
                node(2, "signal.emit", "rang", json!({})),
                node(3, "event.start", "", json!({})),
                node(
                    4,
                    "signal.bind",
                    "ring",
                    json!({"signal": "pulled", "entity": "Lever"}),
                ),
                node(5, "call.event", "hush", json!({})),
            ],
            links: vec![
                link(1, "then", 2, "in"),
                link(1, "times", 2, "times"),
                link(3, "then", 4, "in"),
                link(4, "then", 5, "in"),
            ],
            ..Default::default()
        };
        let c = compile(&g, "examples/bell.graph");
        assert!(c.problems.is_empty(), "{:?}\n{}", c.problems, c.luau);
        for want in [
            "Bell.signals = {",
            "rang = \"times: number\", -- each ring",
            "function Bell:ring(times: number, by: Entity)",
            "self:emit(\"rang\", times)",
            ":on(\"pulled\", function(...) self:ring(...) end)",
            "self:hush()",
            "function Bell:hush()",
        ] {
            assert!(c.luau.contains(want), "{want} in\n{}", c.luau);
        }
        assert!(crate::editing::check(&c.luau).is_none(), "{}", c.luau);
        assert_eq!(Graph::parse(&g.to_text()).unwrap(), g);
        // An Emit of a signal the graph doesn't declare, a custom event named like a hook and a
        // Bind with no signal name are errors on their nodes.
        let mut bad = g.clone();
        bad.nodes.push(node(6, "signal.emit", "rnag", json!({})));
        bad.links.push(link(5, "then", 6, "in"));
        bad.events.insert("update".into(), String::new());
        bad.nodes[3].values.remove("signal");
        let c = compile(&bad, "examples/bell.graph");
        let errors: Vec<(Option<u32>, &str)> =
            c.errors().map(|p| (p.node, p.message.as_str())).collect();
        assert!(
            errors.contains(&(
                Some(4),
                "which signal? Type its name (the entity's script declares it)"
            )),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.1.contains("custom event 'update'")),
            "{errors:?}"
        );
        // Node 6 hangs off the Bind, which no longer compiles; on its own it says why.
        bad.nodes[3].values.insert("signal".into(), json!("pulled"));
        let c = compile(&bad, "examples/bell.graph");
        assert!(
            c.errors()
                .any(|p| p.node == Some(6) && p.message.contains("no signal 'rnag'")),
            "{:?}",
            c.problems
        );
        assert_eq!(params("a: number, b, 3x: string, c: Entity?").len(), 3);
    }

    #[test]
    fn the_text_format_is_one_node_per_line_and_reads_back() {
        let g = spinner();
        let text = g.to_text();
        assert!(
            text.contains("\n    {\"id\":1,\"kind\":\"event.update\",\"at\":[0,10]},\n"),
            "{text}"
        );
        assert!(
            text.contains("{\"from\":[1,\"then\"],\"to\":[2,\"in\"]}"),
            "{text}"
        );
        let back = Graph::parse(&text).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn flow_nodes_compile_and_check() {
        let g = Graph {
            vars: BTreeMap::from([("lit".into(), json!(false))]),
            nodes: vec![
                node(1, "event.interact", "", json!({})),
                node(2, "flow.do_once", "", json!({})),
                node(3, "flow.flip_flop", "", json!({})),
                node(4, "var.set", "lit", json!({"value": true})),
                node(5, "flow.delay", "", json!({"seconds": 2})),
                node(
                    6,
                    "api.call",
                    "fire.ignite",
                    json!({"at": [0, 1, 0], "radius": 1.5}),
                ),
                node(7, "debug.print", "", json!({})),
                node(8, "flow.for_loop", "", json!({"last": 3})),
                node(
                    9,
                    "flow.tween",
                    "",
                    json!({"seconds": 0.5, "ease": "out_back"}),
                ),
                node(10, "comp.set", "PointLight", json!({"intensity_lm": 800})),
                node(11, "math.multiply", "", json!({"b": 1600})),
            ],
            links: vec![
                link(1, "then", 2, "in"),
                link(2, "then", 3, "in"),
                link(3, "a", 4, "in"),
                link(4, "then", 5, "in"),
                link(5, "then", 6, "in"),
                link(6, "result", 7, "text"),
                link(6, "then", 7, "in"),
                link(7, "then", 2, "reset"),
                link(3, "b", 8, "in"),
                link(8, "body", 9, "in"),
                link(9, "update", 10, "in"),
                link(9, "alpha", 11, "a"),
                link(11, "value", 10, "intensity_lm"),
            ],
            ..Default::default()
        };
        let c = compile(&g, "examples/torch.graph");
        assert!(c.errors().next().is_none(), "{:?}\n{}", c.problems, c.luau);
        for want in [
            "function Torch:on_interact(by: Entity)",
            "if not self.__once2 then",
            "self.__flip3 = not self.__flip3",
            "self.lit = true",
            "task.delay(2, function()",
            "local r6 = fire.ignite({ at = vec3(0, 1, 0), radius = 1.5 })",
            "print(r6)",
            "self.__once2 = nil",
            "for i8 = 1, 3 do",
            "task.tween(0.5, function(alpha9)",
            "self.entity:set(\"PointLight\", { intensity_lm = (alpha9 * 1600) })",
            "end, ease.out_back)",
            "self.lit = false",
        ] {
            assert!(c.luau.contains(want), "missing {want:?} in\n{}", c.luau);
        }
        assert!(crate::editing::check(&c.luau).is_none(), "{}", c.luau);
    }

    #[test]
    fn mistakes_are_found_on_their_nodes() {
        let g = Graph {
            nodes: vec![
                node(1, "event.start", "", json!({})),
                node(2, "flow.branch", "", json!({})),
                node(3, "debug.print", "", json!({})),
                node(4, "debug.print", "", json!({})),
                node(5, "api.call", "world.find_thing", json!({})),
                node(6, "debug.print", "", json!({})),
                node(7, "event.update", "", json!({})),
                node(8, "debug.print", "", json!({})),
            ],
            links: vec![
                link(1, "then", 2, "in"),
                link(2, "true", 3, "in"),
                // A loop with no Delay.
                link(3, "then", 4, "in"),
                link(4, "then", 3, "in"),
                // Node 6 uses a result from another event's path.
                link(5, "result", 6, "text"),
                link(7, "then", 6, "in"),
                // Node 8 never runs.
            ],
            ..Default::default()
        };
        let c = compile(&g, "x.graph");
        let on =
            |n: u32| -> Vec<&Problem> { c.problems.iter().filter(|p| p.node == Some(n)).collect() };
        assert!(
            on(2)
                .iter()
                .any(|p| !p.error && p.message.contains("always false")),
            "{:?}",
            c.problems
        );
        assert!(
            on(3)
                .iter()
                .chain(on(4).iter())
                .any(|p| p.error && p.message.contains("loops")),
            "{:?}",
            c.problems
        );
        assert!(
            on(6)
                .iter()
                .any(|p| p.error && p.message.contains("hasn't run")),
            "{:?}",
            c.problems
        );
        assert!(
            on(8)
                .iter()
                .any(|p| !p.error && p.message.contains("never runs")),
            "{:?}",
            c.problems
        );
    }

    #[test]
    fn shared_nodes_become_methods() {
        let g = Graph {
            nodes: vec![
                node(1, "event.start", "", json!({})),
                node(2, "event.interact", "", json!({})),
                node(3, "debug.print", "", json!({"text": "hi"})),
            ],
            links: vec![link(1, "then", 3, "in"), link(2, "then", 3, "in")],
            ..Default::default()
        };
        let c = compile(&g, "x.graph");
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert_eq!(c.luau.matches("self:__n3()").count(), 2, "{}", c.luau);
        assert!(
            c.luau.contains("function X:__n3()\n\tprint(\"hi\")\nend"),
            "{}",
            c.luau
        );
    }

    #[test]
    fn engine_functions_and_components_are_nodes() {
        let s = api_spec(
            "fire.ignite",
            "Lights a fire.",
            r#"{"at": [0, 1, 0], "radius": 1.5, "fuel": "wood"}"#,
        );
        assert_eq!(s.category, "Engine: fire");
        assert_eq!(s.input_pin("at").unwrap().ty, PinType::Vec3);
        assert_eq!(s.input_pin("radius").unwrap().ty, PinType::Number);
        assert_eq!(s.input_pin("fuel").unwrap().ty, PinType::Text);
        assert!(s.output_pin("result").is_some());
        let c = component_specs(
            "PointLight",
            "A light.",
            Some(&json!({"intensity_lm": 800, "color": [1, 1, 1]})),
        );
        assert_eq!(c[0].output_pin("color").unwrap().ty, PinType::Vec3);
        assert!(c[1].input_pin("intensity_lm").is_some());
        assert_eq!(class_name("showcase/glow_crystal.graph"), "GlowCrystal");
    }
}
