//! # engine_script
//!
//! Gameplay scripting in [Luau](https://luau.org) (Roblox's fast, gradually typed, sandboxed
//! Lua), hot reloaded while the game runs. Research and design:
//! `engine-design/scripting-research.md`.
//!
//! ## A script
//!
//! A `Script { path, props }` component runs `assets/scripts/<path>` on its entity. The file
//! returns a class; each entity gets its own `self` with `self.entity` and `self.props`:
//!
//! ```lua
//! local Lantern = {}
//! Lantern.props = { color = vec3(1, 0.6, 0.3), period = 2 }   -- defaults; Script.props override
//!
//! function Lantern:start()              -- once; may wait (it runs as a task)
//!     self.t = 0
//! end
//!
//! function Lantern:update(dt)          -- every frame, world seconds (slow motion aware)
//!     self.t += dt
//!     local glow = 0.6 + 0.4 * math.sin(self.t * 2 * math.pi / self.props.period)
//!     self.entity:set("PointLight", { intensity_lm = 900 * glow })
//! end
//!
//! function Lantern:on_interact(by)     -- the player pressed E on it
//!     vfx.spawn { effect = "spell.fireball_impact", position = self.entity.position }
//! end
//!
//! return Lantern
//! ```
//!
//! Hooks (each optional): `start`, `update(dt)`, `fixed_update(dt)`, `parallel_update(dt)`
//! (below), `on_interact(actor)`,
//! `on_contact(other, info)`, `on_message(event, data)` (or a method named after the event),
//! `on_props()`, `on_reload()`, `on_destroy()`, `on_world_shift(shift)` (the world moved to a
//! new frame: map kept positions with `shift:point(p)`, `shift:direction(v)`,
//! `shift:orientation(q)`; entities move on their own). `start` and the event hooks (interact, contact,
//! messages) run as tasks of their own, so they may `task.wait`; `update` must not wait.
//!
//! ## In the editor's view
//!
//! Three more hooks show a script in the editor before Play, after Unreal's construction script
//! and Unity's gizmos:
//!
//! * `construct()` builds the parts the entity's look depends on. In the game it runs just
//!   before `start`; in the editor (while [`ScriptsHeld`]) it runs on a stand-in `self` of its
//!   own, and again whenever the file, the props or `enabled` change. What it spawns is marked
//!   [`Constructed`], and goes before each rebuild, on destroy and when Play starts.
//! * `draw_gizmos()` runs every frame while [`ScriptGizmos::show`] is on, and
//!   `draw_gizmos_selected()` for the entities in [`ScriptGizmos::selected`]; both start in the
//!   entity's own space. Scripts draw with the `draw` global (`draw.line`, `draw.sphere`,
//!   `draw.circle`, `draw.cone`, `draw.path`, ...; `draw` module docs), which records shapes and
//!   hands them to `engine_debug`'s `DebugDraw` after the frame, in any hook.
//! * A prop hinted `@handle` ([`props::Handle`]) is a handle the editor draws in the scene view
//!   and drags: a point for a `vec3`, a ring for a radius, or a distance along an axis.
//!
//! ## Tools
//!
//! A method with `@button [Label]` in the comment above it is a button in the editor's Details
//! panel (Unity's `[ContextMenu]`, Unreal's Call In Editor): `script.tool {entity, method}` runs
//! it, before Play on the script's editor stand-in, and keeps what it set in `self.props` in the
//! entity's `Script::props`. A module in `tools/` ([`TOOLS`]) with `run(selection)` is a tool for
//! the editor's Tools menu (Godot's EditorScript): `script.tool {file, selection}`. The editor
//! records each run's changes to the scene as one undo step.
//!
//! ## Props in the Details panel
//!
//! The editor shows each prop as a field of its kind ([`props`], after Unity's inspector,
//! Unreal's Instance Editable variables and Godot's `@export`): numbers, sliders, colors,
//! vectors, angles, toggles, text, lists of choices, entity pickers and tables that fold out,
//! with the prop's comment as its tooltip. Hints in the comment shape the field:
//!
//! ```lua
//! Beacon.props = {
//!     -- @group Light
//!     lumens = 2400, -- at the top of a pulse @range 0 8000 @unit lm
//!     color = vec3(0.35, 0.75, 1), -- linear RGB (props named color or tint are colors)
//!     -- @group Signal
//!     mode = "pulse", -- how it shines @enum steady|pulse|blink
//!     target = "Stone", -- who hears it flash @entity
//!     every = 4, -- flashes between messages @int @min 1
//! }
//! ```
//!
//! A value set there goes in `Script::props` (only what differs from the default), reaches a
//! running script at once (`on_props`), and a table prop set in part keeps its other defaults.
//!
//! ## Signals
//!
//! A class declares what it tells the world, and fires it with `self:emit`:
//!
//! ```lua
//! Lever.signals = {
//!     pulled = "by: Entity", -- the arm went over; who pulled it
//! }
//! -- in a method:
//! self:emit("pulled", by)
//! ```
//!
//! Who hears it is wired per object, in the editor's Details panel (the Script component's
//! [`Connection`]s: a scene object, a method of its script, optional fixed values, once), as
//! Godot connects signals and Unity's inspector wires events; code listens with
//! `entity:on(signal, f)`, `entity:once(signal, f)` (Roblox's `Connect` and `Once`, each
//! returning a connection with `disconnect()`) or waits with `task.wait_for(entity, signal)`.
//! Listeners run later in the same frame, each as a task of its own (Roblox's deferred
//! events), so one that fails or waits holds up neither the emitter nor the others.
//!
//! ## Type checking
//!
//! [`analysis`] (feature `analysis`, on by default) runs Luau's own type checker on the scripts
//! against the engine's definitions ([`definitions`]), knowing what a class script's
//! `self` is: problems, the type under the pointer, members, a call's parameters and where a
//! name is defined, for the editor's Scripts tab and `script.typecheck`.
//!
//! ## What scripts can use
//!
//! * Every registered engine function as `module.verb(args)`: `fire.ignite { at = p }`,
//!   `weather.set { rain = 0.8 }`, `npc.spawn { ... }`, and `api("name", args)` for names built
//!   at run time. Tables go in as JSON, results come back as tables.
//! * Entities (`entity` module docs): `world.find(name)`, `world.spawn(spec)`, `self:spawn(spec)`,
//!   `e.position`/`rotation`/`scale`, `e:get(component)`, `e:set(component, patch)`,
//!   `e:send(event, data)`, `e:on(signal, f)` / `e:once(signal, f)`, `e:material(patch)` (its own material, edited in place, for glows
//!   and fades), `e.script` (another entity's script `self`).
//! * `vec3` (f64, for world positions) and `quat` (`math` module docs).
//! * `task.wait/spawn/delay/defer/cancel/tween/wait_until/wait_for`, `self:every(s, "method")`,
//!   `self:after(s, "method")`, `self:emit(signal, ...)`, `ease.*` curves. Waits run on world
//!   time.
//! * `input.pressed(action)`, `input.just_pressed`, `input.value`, `input.stick`.
//! * `print`/`warn` (into the log and `script.log`), `require("lib/name")` for shared modules.
//!
//! ## Hot reload
//!
//! Every loaded file is checked a few times a second ([`ScriptSettings::poll_secs`]), in release
//! builds too. A file that changed is recompiled: if it fails, the old code keeps running and the
//! error is reported; if it compiles, live instances keep `self` and their tasks, get the new
//! methods and an `on_reload()` call, and the props they don't set in `Script::props` take the
//! new defaults (state belongs in `self` fields). `reload = "restart"` on a class restarts its
//! instances instead. Editing a `require`d module reloads its users.
//!
//! ## Parallel scripts
//!
//! `parallel_update(dt)` runs every frame like `update`, but on several cores at once: scripts
//! with it are spread over actor VMs ([`ScriptActor`]; [`ScriptSettings::parallel_vms`], by
//! default one fewer than the cores), and each VM runs its scripts on its own thread while the
//! others run theirs. In it a script reads the world as it was when the parallel phase began
//! (other scripts' changes this frame don't show, so the result is the same on any number of
//! cores), sees its own changes to `Transform`s at once, and its other changes (`e:set`,
//! `e.name =`, `e:material(patch)`, `e:despawn()`) are queued and applied right after the
//! phase, VM after VM. Engine calls that need the whole world (`module.verb`, spawning) fail
//! there with a hint: call `task.synchronize()` and the rest of `parallel_update` runs after
//! the phase, one script at a time, with the world. Roblox's Parallel Luau works the same way
//! with actors, but forbids the changes instead of queueing them.
//!
//! A VM has its own globals and `require`d modules, so `e.script` reaches only scripts in the
//! same VM; scripts talk across VMs with `e:send` (delivered the next frame) or share a VM by
//! being given the same `ScriptActor`. With `ENGINE_SCRIPT_DEBUG` set at start there are no
//! actor VMs, so breakpoints reach every script.
//!
//! ## Visual scripts
//!
//! A `.graph` file ([`graph`]) is a script drawn as nodes and wires: JSON text, one node, wire
//! or comment per line, so it diffs and merges like code. A `Script` whose path ends in
//! `.graph` compiles it to Luau ([`graph::compile`]) and runs that, so hot reload, errors,
//! the profiler and the debugger work as for `.luau`; a graph with mistakes doesn't load and the
//! old one keeps running. The nodes come from the engine: events, flow (branch, sequence,
//! delay, do once, flip flop, gate, loops, every, tween), variables and props, math, text,
//! entities, every registered engine function and Get/Set for every reflected component
//! ([`graph::library`]). [`graph::watch`] puts counting logpoints on a graph's lines, and
//! [`graph::live`] turns their hits and values into which nodes ran and what each wire
//! carries. A [`graph_board::GraphBoard`] draws a graph on a quad in the world, live.
//! `script.graph {path}` compiles one and returns its Luau and problems; `script.graph_nodes`
//! lists the library. The editor's Graphs tab edits them.
//!
//! ## Debugging
//!
//! A script debugger for VS Code (`crates/script/vscode`) and any other Debug Adapter Protocol
//! client: run the game with `ENGINE_SCRIPT_DEBUG=4711` (or call `script.debug`), attach, and
//! set breakpoints in `.luau` files: conditions, hit counts and logpoints, step in, over and
//! out, the call stack, locals, upvalues and `self` expandable, watches and hovers, and the debug
//! console evaluating in the stopped frame. "Uncaught errors" (on by default in VS Code's
//! breakpoint list) stops where an error nothing catches is raised, with its stack and locals.
//!
//! A stop doesn't freeze the program: the stopped script waits at its line (its coroutine
//! yields there), the other scripts wait for it and the world's time is held still
//! ([`TimeDilation`](engine_core::prelude::TimeDilation)), while rendering, the UI and an editor
//! in the same process keep running. The editor debugs through [`debug`] (no socket: the
//! Scripts tab's gutter breakpoints, call stack, variables, watches and stepping, the Graphs
//! tab's node breakpoints), and both can be attached at once. Where a script can't wait
//! (engine code between it and the frame, as in a `table.sort` comparator) or after an error,
//! the stop shows the values as they were and the code goes on. Code boards draw a stop: the
//! line, its arrow and each local's value beside the line that set it; `script.break` does all
//! this from the API (the tour's `script_paused` stop). Without a client,
//! `script.logpoint {path, line, message}` prints values from a line without stopping, and
//! code boards show the newest beside it. To try scripts and the debugger without the game (no
//! GPU): `cargo run -p engine_script --example headless [-- files...]`. How it works:
//! `src/debugger.rs`.
//!
//! ## Profiling
//!
//! `script.profile {seconds = 5}` samples the scripts while the game runs (1000 times a second
//! by default, `hz` up to 20000) and `script.profile {}` reports the functions and lines that
//! took the most time, self and total, with engine calls (`e:set`, `fire.ignite`) as their own
//! entries; `folded = true` adds the stacks for flame graph tools. Code boards draw each line's
//! share of the time beside it, and a [`board::ProfileBoard`] draws the flame graph. Not
//! recording, it costs nothing measurable. How it works: `src/profiler.rs`.
//!
//! Errors are marked where they happen: a code board shows the line an error came from in red,
//! with the message and how often it repeated, until the file is fixed.
//!
//! ## Rewind
//!
//! A [`ScriptRewind`] resource records what every script does, frame by frame, to scrub back
//! through afterwards (Unreal's Rewind Debugger and Visual Logger): each script's values
//! (`self`'s fields and props, kept when they change), its state machine's current states, where
//! it and the parts it holds were (`self.drone`: entities it built or that are below it),
//! what happened (prints, errors, messages sent and received, signals, hot reloads) and the
//! shapes it drew. It keeps the last two minutes (or about 96 MB) by default. With
//! `ScriptRewind::shown` set to a time, [`rewind::show_rewind`] draws the shapes as they were
//! then and each moving part's path around it. The editor's Rewind tab records each Play, and
//! scrubbing there pauses the game and puts the entities back where they were;
//! `script.rewind` records in a game and answers what was when (`{at}`, `{entity, value}`,
//! `{events, kind}`). Nothing is replayed: the world isn't recorded, so the game goes on from
//! where it is. How it works: `src/rewind.rs`.
//!
//! ## Tweens and timelines
//!
//! `self:tween(target, {position = ..., ["PointLight.intensity_lm"] = ...}, secs, opts)` moves
//! an entity's transform, any registered component's field, its material's fields or a
//! table's fields over time; `self:sequence()` chains steps (`to`, `join`, `at`, `interval`,
//! `call`, as DOTween's Append, Join and Insert); `self:spring(target, {position = goal})`
//! follows a goal that moves (an exact damped spring set by its halflife, as in Daniel
//! Holden's *Spring-It-On*). They run in Rust each frame after `update` ([`tween`]); a newer
//! tween of a value takes it over, and they stop with their script. Eases ([`ease`]) are
//! Penner's families, CSS's `cubic-bezier` and `steps`, a curve or a function. Curves
//! ([`curve`]) are `.curve` files of keys (monotone auto tangents, Fritsch and Carlson; user
//! and broken slopes; hold, loop and pingpong outside), kept loaded and hot reloaded by
//! [`ScriptCurves`]. A class's `timelines` table declares Unreal-style timelines: curves as
//! tracks and method names at times as events, played forward or backward from where they are
//! (`timelines.luau`). The editor's Curves tab edits the curves live and shows the timelines'
//! playheads and the tweens running.
//!
//! ## Cutscenes
//!
//! A `.cutscene` file ([`cutscene`]) animates several objects on one clock, cuts between
//! cameras and sends events at times, as Unreal's Sequencer and Unity's Timeline do: tracks
//! bound to objects by name (looked up under the player first), each axis of a value a curve
//! (`position`, `rotation` in degrees, `scale`, `"Component.field"`, `"material.field"`),
//! events sent to the bound object's script (`retro` ones also when a skip jumps past them,
//! `once` ones once across loops), and camera cuts that a view camera of its own follows,
//! blending between cuts, in from the game's camera and back out, letterboxed to `aspect`.
//! `cutscene.play(path, opts)` plays one from a script and returns a handle (`pause`, `play`,
//! `seek` to a time or mark, `set_rate`, `skip` to the end, `stop`, `wait`); a
//! [`CutscenePlayer`] plays one when the game starts. Values stay as it leaves them unless a
//! track says `"finished": "restore"` or the player restores. [`CutsceneFiles`] keeps the
//! files loaded and hot reloaded. The editor's Sequencer tab makes them: a dope sheet of the
//! tracks, scrubbing that poses the objects in edit mode ([`cutscene::pose`]), keys from the
//! selection (S, auto key) and the cut camera's view in the corner. `script.cutscene` reads,
//! plays, skips and stops them.
//!
//! ## State machines
//!
//! A class's `states` table (`initial`, nested `states`, `history`, and per state `on`, `after`,
//! `done`, `when`) is a hierarchical state machine the runtime runs with the script: the state's
//! work is `enter_<state>` (a task: `done` follows when it returns), `update_<state>(dt)` and
//! `exit_<state>`; interactions, messages, wired signals and `self:trigger` are its events
//! (`states.luau`). A group with `parallel = true` runs all its states at once (regions, as
//! SCXML's `<parallel>` and XState's parallel states): an event reaches every region, and a
//! change into one region leaves the others where they are. A state with `final = true` ends
//! its group: the group's `done` follows once it is in one of its final states (a parallel
//! group's once every region is), and a final state at the outer level ends the machine;
//! `history = "deep"` resumes a group all the way in. An event or `go` from an enter or exit
//! hook waits until the change it is part of is done (run to completion). [`states`] reads a
//! machine's shape and its live state for the editor (each change in the log keeps the states
//! active after it and the script's values, so the editor can show the machine as it was then),
//! and `script.states {entity}` serves both. State breakpoints
//! ([`states::StateBreakpoints`], `script.state_breaks`) pause the game as a state starts or
//! ends, as Unreal's StateTree debugger does. A [`state_board::StateBoard`] draws a running
//! machine on a quad in the world, from the editor's layout ([`diagram`]).
//!
//! ## Tests
//!
//! `*.test.luau` files are gameplay tests in Jest's shape (`describe`, `it`, `expect`), run one
//! by one as tasks in the live game: a test spawns what it needs, waits, presses E, listens to
//! signals and checks what the scripts did ([`testing`]). `script.test` starts a run and
//! `script.tests` reports it, with the line each failure stopped on.
//!
//! ## Rust hot patching
//!
//! With the `hotpatch` cargo feature, [`hotpatch::HotPatchPlugin`] (added by [`ScriptPlugin`])
//! lets `dx serve --hot-patch` swap the game crate's Rust systems while the game runs: see the
//! [`hotpatch`] module for the commands and what can and can't change live.
//!
//! ## Safety
//!
//! Sandboxed (read-only libraries, a private global table per file), a time limit per call
//! ([`ScriptSettings::call_limit_ms`]: an endless loop is stopped with an error, not a frozen
//! game), a memory limit, and every error caught and reported with file and line
//! (`script.errors`); one script's error never stops another.
//!
//! ## API
//!
//! `script.list`, `script.errors`, `script.log`, `script.eval {code}` (run Luau in the live
//! game), `script.reload {path}`, `script.restart {entity}`, `script.send {entity, event, data}`,
//! `script.source {path}`, `script.write {path, source}`, `script.edit {path, find, replace}`,
//! `script.types` (writes `engine.d.luau` for luau-lsp autocompletion), `script.debug {port}`
//! (start the debugger), `script.break {path, lines}` (breakpoints and stepping without a
//! client), `script.logpoint {path, line, message}`, `script.profile {seconds}`,
//! `script.marks {path}` (a file's errors, logpoint values and heat by line) and
//! `script.check {source}` (compile without running), `script.test {files, file, line,
//! filter}` (run the script tests), `script.tests` (their results) and `script.states {entity}`
//! (its state machine, live), `script.state_breaks {path, state, exit, on}` (pause on a state),
//! `script.rewind {record, at, entity, value, events, show}` (the rewind recorder).
//! An editor holds scripts still with
//! [`ScriptsHeld`] while a scene is edited ([`editing`]).
//!
//! ## Showcase
//!
//! [`demo`]: the Script courtyard, whose lanterns, orrery, lever ritual and swarm of 320
//! wisps (`parallel_update`, on every core) are scripts (`assets/scripts/showcase/`), with a
//! [`board::CodeBoard`] showing the director's source and a [`board::ProfileBoard`] its
//! profile.
//!
//! ## Reads and writes (for replacing this module)
//!
//! Reads `Script`, `CutscenePlayer`, `Time`, `LocalTimeScale`, `ActionState`, `Window`, the
//! `Interact` and `ContactEvent` messages, and whatever scripts read through the registry.
//! Writes only what scripts and cutscenes write, through the registry and `Transform`, and the
//! cutscene view camera (`Camera`, `Viewport`). Draws code boards ([`board::CodeBoard`]).

pub mod access;
#[cfg(feature = "analysis")]
pub mod analysis;
mod api;
mod bind;
pub mod board;
mod convert;
pub mod curve;
pub mod cutscene;
mod dap;
pub mod debug;
mod debugger;
pub mod demo;
pub mod diagram;
mod draw;
pub mod ease;
pub mod editing;
mod entity;
pub mod graph;
pub mod graph_board;
#[cfg(feature = "hotpatch")]
pub mod hotpatch;
mod math;
mod profiler;
pub mod props;
pub mod rewind;
mod runtime;
pub mod state_board;
pub mod states;
pub mod testing;
pub mod tween;
mod types;

use std::path::PathBuf;

use bevy_ecs::prelude::*;
use engine_core::prelude::*;

pub use access::ScriptError;
pub use curve::ScriptCurves;
pub use cutscene::{CutsceneFiles, CutscenePlayer, Cutscenes};
pub use editing::ScriptsHeld;
pub use entity::OwnMaterial;
pub use rewind::ScriptRewind;
pub use runtime::{ScriptRuntime, normalize};
pub use types::definitions;

/// Runs a Luau script on this entity (see the crate docs).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Script {
    /// The script file, relative to the scripts folder (`assets/scripts/`), e.g.
    /// `"showcase/lantern.luau"` (`.luau` may be left off).
    pub path: String,
    /// Values for the script's `props`, over the defaults its class declares. A JSON object.
    pub props: serde_json::Value,
    /// While false, the script's hooks and tasks pause (its state stays).
    pub enabled: bool,
    /// Who hears this script's signals: methods of scene objects' scripts, wired in the
    /// editor's Details panel (Godot's signal connections, Unity's events).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            path: String::new(),
            props: serde_json::Value::Object(Default::default()),
            enabled: true,
            connections: Vec::new(),
        }
    }
}

/// One listener of a script's signal: when the script runs `self:emit(signal, ...)`, the
/// script on `target` gets `method` called with the signal's values (or with `args`, when
/// set), later in the same frame, as a task of its own.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    /// The signal, one its class declares in `signals`.
    pub signal: String,
    /// The scene object whose script hears it, by name; empty: this one.
    pub target: String,
    /// The method of that script to call.
    pub method: String,
    /// Values to call it with instead of the signal's own (Unity's static parameters).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<serde_json::Value>,
    /// Only the first time the signal fires (each play).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub once: bool,
}

impl Script {
    /// A script at `path` with default props.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    /// The same script with these props (a JSON object).
    pub fn with_props(mut self, props: serde_json::Value) -> Self {
        self.props = props;
        self
    }

    /// The same script with `signal` wired to `method` on the scene object named `target`.
    pub fn connect(mut self, signal: &str, target: &str, method: &str) -> Self {
        self.connections.push(Connection {
            signal: signal.into(),
            target: target.into(),
            method: method.into(),
            ..Default::default()
        });
        self
    }
}

/// The actor VM (1, 2, ...) that runs the script on this entity. Added by the runtime when a
/// script with `parallel_update` starts (to the VM with the fewest scripts); set it yourself to
/// keep scripts that work together in one VM, where they run one after another and can reach
/// each other's `self` (`entity.script`). Numbers wrap around the VMs there are; 0 is the main
/// VM.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptActor(pub u32);

/// How scripts are found, reloaded and limited.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptSettings {
    /// Folder under the asset root that script paths are relative to.
    pub folder: String,
    /// Reload scripts when their files change (`ENGINE_SCRIPT_HOT_RELOAD=0` turns it off).
    pub hot_reload: bool,
    /// Seconds between checks of the loaded files' modification times.
    pub poll_secs: f32,
    /// Longest one call into a script may run before it is stopped, milliseconds.
    pub call_limit_ms: f32,
    /// Memory the scripts may use, MiB, per VM (the main one and each actor VM).
    pub memory_limit_mb: u32,
    /// Port the script debugger listens on for VS Code and other DAP clients (localhost only;
    /// `ENGINE_SCRIPT_DEBUG=4711`, or `script.debug` at run time). None: not listening.
    pub debug_port: Option<u16>,
    /// Actor VMs that run `parallel_update` side by side (`ENGINE_SCRIPT_VMS`). None: one fewer
    /// than the cores, at most 8, or none when `debug_port` is set at start (the debugger sees
    /// the main VM). 0: `parallel_update` runs in the main VM, with the same rules.
    pub parallel_vms: Option<u32>,
}

impl Default for ScriptSettings {
    fn default() -> Self {
        Self {
            folder: "scripts".into(),
            hot_reload: std::env::var("ENGINE_SCRIPT_HOT_RELOAD").map_or(true, |v| v != "0"),
            poll_secs: 0.25,
            call_limit_ms: 250.0,
            memory_limit_mb: 512,
            debug_port: std::env::var("ENGINE_SCRIPT_DEBUG")
                .ok()
                .and_then(|v| v.parse().ok()),
            parallel_vms: std::env::var("ENGINE_SCRIPT_VMS")
                .ok()
                .and_then(|v| v.parse().ok()),
        }
    }
}

/// The folder of tool files under the scripts folder: modules with `run(selection)` that the
/// editor's Tools menu runs on the selection (`script.tool {file, selection}`).
pub const TOOLS: &str = "tools";

/// Marks what a script's `construct` built (Unreal's construction script, Unity's
/// `[ExecuteAlways]` scripts): `by` is the scripted entity. In an editor's edit mode the parts
/// are rebuilt whenever the script's props or file change; while playing they are built once,
/// before `start`, and go when the script stops or restarts. Not saved with a scene (the
/// script builds them again), and an editor selects `by` when one is clicked.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Constructed {
    pub by: Entity,
}

/// Which scripts draw gizmos: `draw_gizmos` on every script while `show` is on (the editor's
/// Gizmos toggle), `draw_gizmos_selected` on the `selected` ones (the editor's selection). Both
/// run in edit mode too, on each script's stand-in (see the crate docs, "In the editor's
/// view"). `script.gizmos` sets it from the API.
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptGizmos {
    pub show: bool,
    #[serde(skip)]
    pub selected: Vec<Entity>,
    /// Hides them all for now, `show` kept (an editor's Game view).
    #[serde(skip)]
    pub hide: bool,
}

impl ScriptGizmos {
    /// Whether `draw_gizmos` runs.
    pub fn all(&self) -> bool {
        self.show && !self.hide
    }

    /// Whether `e`'s `draw_gizmos_selected` runs.
    pub fn picked(&self, e: Entity) -> bool {
        !self.hide && self.selected.contains(&e)
    }
}

/// Present while the runner holds the runtime (scripts are running), so `script.*` calls made
/// from a script fail clearly instead of finding no runtime.
#[derive(Resource)]
struct ScriptsRunning;

/// Adds Luau scripting: the [`Script`] component, the runtime, code boards and the `script.*`
/// API.
pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        if !app.world.contains_resource::<ScriptSettings>() {
            app.insert_resource(ScriptSettings::default());
        }
        app.init_resource::<states::StateBreakpoints>()
            .init_resource::<ScriptGizmos>()
            .init_resource::<ScriptCurves>()
            .init_resource::<CutsceneFiles>()
            .init_resource::<Cutscenes>();
        app.register_component::<Script>(
            "Runs a Luau script from assets/scripts on this entity: path (\"showcase/lantern.luau\"), \
             props (JSON over the script's defaults), enabled.",
        )
        .register_component::<CutscenePlayer>(
            "Plays a cutscene (a .cutscene file: objects' values as curves, events, camera cuts) \
             on this entity: path (\"cutscenes/intro\"), autoplay (when the game starts), loop, \
             rate, restore (put back what it changed when it ends), cameras (look through its \
             camera cuts). The names its tracks bind are looked up below this entity first.",
        )
        .register_component::<ScriptActor>(
            "The actor VM (1, 2, ...) that runs this entity's script; set by the runtime for \
             scripts with parallel_update, or by hand to keep scripts in one VM.",
        )
        .register_component::<board::CodeBoard>(
            "Shows a script's source on this entity's mesh (its material's emissive texture), \
             syntax-colored, redrawn whenever the file reloads, with logpoint values, the line \
             of the last error and the profiler's heat per line.",
        )
        .register_component::<board::ProfileBoard>(
            "Shows the script profiler's flame graph and hottest lines on this entity's mesh; \
             live: records by itself, window_secs at a time, while no script.profile \
             recording is shown.",
        )
        .register_component::<graph_board::GraphBoard>(
            "Shows a visual script (a .graph file) on this entity's mesh as the editor draws \
             it; live: while it runs, the nodes that run light up and each wire shows its value.",
        )
        .register_component::<state_board::StateBoard>(
            "Shows the state machine of the script on the entity named target on this \
             entity's mesh as the editor draws it, live: the states it is in with their time, \
             the change just made, after timers filling and the last changes.",
        )
        .register_resource::<ScriptGizmos>(
            "Script gizmos: show runs every script's draw_gizmos (in an editor's edit mode too); \
             the editor's selection runs draw_gizmos_selected.",
        )
        .register_resource::<ScriptSettings>(
            "Scripting: folder under the asset root, hot_reload, poll_secs, call_limit_ms (per \
             call), memory_limit_mb (per VM), debug_port (the debugger's, for VS Code), \
             parallel_vms (actor VMs for parallel_update; null: cores - 1).",
        )
        .add_systems(Update, run_scripts.in_set(UpdateSet::Logic))
        .add_systems(Update, preview_scripts.in_set(UpdateSet::Logic))
        .add_systems(
            Update,
            cutscene::run_cutscenes
                .in_set(UpdateSet::Logic)
                .after(run_scripts),
        )
        .add_systems(FixedUpdate, run_fixed_scripts.in_set(SimSet::Gameplay))
        .add_systems(
            Update,
            rewind::show_rewind
                .after(run_scripts)
                .after(preview_scripts),
        )
        .add_systems(Update, board::draw_boards.after(run_scripts))
        .add_systems(
            Update,
            board::draw_profile_boards.after(board::draw_boards),
        )
        .add_systems(
            Update,
            graph_board::draw_graph_boards.after(board::draw_boards),
        )
        .add_systems(
            Update,
            state_board::draw_state_boards.after(board::draw_boards),
        )
        .add_systems(
            PostUpdate,
            shift_scripts.in_set(engine_world::WorldShiftSet::Apply),
        )
        .init_resource::<entity::OwnMaterials>()
        .add_systems(Last, entity::free_own_materials);
        #[cfg(feature = "hotpatch")]
        app.add_plugins(hotpatch::HotPatchPlugin);
        api::register(app);
        cutscene::register(app);
    }
}

/// The scripts folder: `ScriptSettings::folder` under the asset server's root (or the
/// configured asset root when there is no asset server).
pub fn scripts_root(world: &World) -> PathBuf {
    let folder = world
        .get_resource::<ScriptSettings>()
        .map(|s| s.folder.clone())
        .unwrap_or_else(|| "scripts".into());
    let assets = match world.get_resource::<engine_assets::AssetServer>() {
        Some(server) => server.root(),
        None => PathBuf::from(std::env::var("ENGINE_ASSET_ROOT").ok().unwrap_or_else(|| {
            world
                .get_resource::<EngineConfig>()
                .map(|c| c.asset_root.clone())
                .unwrap_or_else(|| "assets".into())
        })),
    };
    assets.join(folder)
}

/// Makes the runtime if there is none yet. Returns false (and logs why) if the VM couldn't be
/// made.
pub(crate) fn ensure_runtime(world: &mut World) -> bool {
    if world.contains_resource::<ScriptRuntime>() {
        return true;
    }
    if world.contains_resource::<ScriptsRunning>() {
        return false;
    }
    let settings = world
        .get_resource::<ScriptSettings>()
        .cloned()
        .unwrap_or_default();
    let root = scripts_root(world);
    let made = {
        let api = world
            .get_resource::<ApiRegistry>()
            .cloned()
            .unwrap_or_default();
        ScriptRuntime::new(&api, root, &settings)
    };
    match made {
        Ok(rt) => {
            // Its clock starts where the world's is (`world.time()`, task waits and test limits
            // read it before the first frame sets it).
            if let Some(now) = world.get_resource::<Time>().map(|t| t.elapsed_secs_f64()) {
                for vm in rt.vms() {
                    crate::access::shared(&vm.lua).world_secs = now;
                }
            }
            if let Some(port) = settings.debug_port
                && let Err(err) = rt.debugger.listen(port)
            {
                tracing::warn!(target: "script", "the script debugger can't listen on port {port}: {err}");
            }
            world.insert_resource(rt);
            true
        }
        Err(err) => {
            tracing::error!(target: "script", "the script VM could not start: {err}");
            false
        }
    }
}

/// Runs `f` with the runtime taken out of the world (made first if needed).
pub(crate) fn with_runtime<R>(
    world: &mut World,
    f: impl FnOnce(&mut World, &mut ScriptRuntime, &ScriptSettings) -> R,
) -> Result<R, String> {
    if world.contains_resource::<ScriptsRunning>() {
        return Err(
            "scripts are running: call script.* from outside a script (from a script, use \
             entity:send and the script's own functions)"
                .into(),
        );
    }
    if !ensure_runtime(world) {
        return Err("the script VM could not start; see the log".into());
    }
    let settings = world
        .get_resource::<ScriptSettings>()
        .cloned()
        .unwrap_or_default();
    world.insert_resource(ScriptsRunning);
    let out =
        world.resource_scope(|world, mut rt: Mut<ScriptRuntime>| f(world, &mut rt, &settings));
    world.remove_resource::<ScriptsRunning>();
    Ok(out)
}

fn run_scripts(world: &mut World) {
    if world.contains_resource::<ScriptsHeld>() {
        return;
    }
    let has_scripts = !world
        .query_filtered::<(), With<Script>>()
        .iter(world)
        .next()
        .is_none();
    let has_work = world.get_resource::<ScriptRuntime>().is_some_and(|rt| {
        // A debugger client may attach before the first script spawns.
        rt.task_count() > 0
            || rt.instance_count() > 0
            || rt.tween_count() > 0
            || rt.debugger.listening()
    });
    if !has_scripts && !has_work {
        return;
    }
    let _ = with_runtime(world, |world, rt, settings| {
        rt.run_frame(world, settings);
        rewind::record(world, rt);
    });
    if let Some(rt) = world.get_resource::<ScriptRuntime>() {
        let (ms, instances, tasks) = (rt.frame_ms, rt.instance_count(), rt.task_count());
        let memory_mb = rt.used_memory() as f64 / (1024.0 * 1024.0);
        let (errors, vms, parallel_ms) = (rt.errors_total(), rt.vm_count(), rt.parallel_ms());
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("script.ms", ms as f64);
            d.set("script.instances", instances as f64);
            d.set("script.tasks", tasks as f64);
            d.set("script.memory_mb", memory_mb);
            d.set("script.errors", errors as f64);
            d.set("script.vms", vms as f64);
            d.set("script.parallel_ms", parallel_ms as f64);
        }
    }
}

/// Edit mode ([`ScriptsHeld`]): scripts don't run, but each builds what it looks like
/// (`construct`) and draws its gizmos, on a stand-in `self` ([`ScriptRuntime::preview_frame`]).
fn preview_scripts(world: &mut World) {
    if !world.contains_resource::<ScriptsHeld>() {
        return;
    }
    let wanted = world.contains_resource::<ScriptRuntime>()
        || world
            .query_filtered::<(), With<Script>>()
            .iter(world)
            .next()
            .is_some();
    if wanted {
        let _ = with_runtime(world, |world, rt, settings| {
            rt.preview_frame(world, settings)
        });
    }
}

/// The world moved to a new frame: scripts that keep world positions map them
/// (`on_world_shift`).
fn shift_scripts(
    world: &mut World,
    mut cursor: Local<Option<bevy_ecs::message::MessageCursor<engine_world::WorldShift>>>,
) {
    let Some(messages) =
        world.get_resource::<bevy_ecs::message::Messages<engine_world::WorldShift>>()
    else {
        return;
    };
    let cursor = cursor.get_or_insert_with(|| messages.get_cursor_current());
    let Some(shift) = cursor.read(messages).copied().reduce(|a, b| a.then(&b)) else {
        return;
    };
    rewind::forget_on_shift(world);
    if world
        .get_resource::<ScriptRuntime>()
        .is_some_and(|rt| rt.wants_world_shift())
    {
        let _ = with_runtime(world, |world, rt, settings| {
            rt.world_shift(world, shift, settings)
        });
    }
}

fn run_fixed_scripts(world: &mut World) {
    if world.contains_resource::<ScriptsHeld>() {
        return;
    }
    if !world
        .get_resource::<ScriptRuntime>()
        .is_some_and(|rt| rt.wants_fixed())
    {
        return;
    }
    let _ = with_runtime(world, |world, rt, settings| rt.run_fixed(world, settings));
}

#[cfg(test)]
mod tests;
