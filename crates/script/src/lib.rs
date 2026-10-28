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
//! Hooks (each optional): `start`, `update(dt)`, `fixed_update(dt)`, `on_interact(actor)`,
//! `on_contact(other, info)`, `on_message(event, data)` (or a method named after the event),
//! `on_props()`, `on_reload()`, `on_destroy()`, `on_world_shift(shift)` (the world moved to a
//! new frame: map kept positions with `shift:point(p)`, `shift:direction(v)`,
//! `shift:orientation(q)`; entities move on their own). `start` and the event hooks (interact, contact,
//! messages) run as tasks of their own, so they may `task.wait`; `update` must not wait.
//!
//! ## What scripts can use
//!
//! * Every registered engine function as `module.verb(args)`: `fire.ignite { at = p }`,
//!   `weather.set { rain = 0.8 }`, `npc.spawn { ... }`, and `api("name", args)` for names built
//!   at run time. Tables go in as JSON, results come back as tables.
//! * Entities (`entity` module docs): `world.find(name)`, `world.spawn(spec)`, `self:spawn(spec)`,
//!   `e.position`/`rotation`/`scale`, `e:get(component)`, `e:set(component, patch)`,
//!   `e:send(event, data)`, `e:material(patch)` (its own material, edited in place, for glows
//!   and fades), `e.script` (another entity's script `self`).
//! * `vec3` (f64, for world positions) and `quat` (`math` module docs).
//! * `task.wait/spawn/delay/defer/cancel/tween/wait_until`, `self:every(s, "method")`,
//!   `self:after(s, "method")`, `ease.*` curves. Waits run on world time.
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
//! ## Debugging
//!
//! A script debugger for VS Code (`crates/script/vscode`) and any other Debug Adapter Protocol
//! client: run the game with `ENGINE_SCRIPT_DEBUG=4711` (or call `script.debug`), attach, and
//! set breakpoints in `.luau` files: conditions, hit counts and logpoints, step in, over and
//! out, the call stack, locals, upvalues and `self` expandable, watches and hovers, and the debug
//! console evaluating in the stopped frame. A stop holds the game still until you continue.
//! "Uncaught errors" (on by default in VS Code's breakpoint list) stops where an error nothing
//! catches is raised, with its stack and locals, before the call fails. Without a client,
//! `script.logpoint {path, line, message}` prints values from a line without stopping, and
//! code boards show the newest beside it. How it works: `src/debugger.rs`.
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
//! (start the debugger), `script.logpoint {path, line, message}`, `script.profile {seconds}`.
//!
//! ## Showcase
//!
//! [`demo`]: the Script courtyard, whose lanterns, orrery and lever ritual are scripts
//! (`assets/scripts/showcase/`), with a [`board::CodeBoard`] showing the director's source.
//!
//! ## Reads and writes (for replacing this module)
//!
//! Reads `Script`, `Time`, `LocalTimeScale`, `ActionState`, the `Interact` and `ContactEvent`
//! messages, and whatever scripts read through the registry. Writes only what scripts write,
//! through the registry and `Transform`. Draws code boards ([`board::CodeBoard`]).

pub mod access;
mod api;
mod bind;
pub mod board;
mod convert;
mod dap;
mod debugger;
pub mod demo;
mod entity;
#[cfg(feature = "hotpatch")]
pub mod hotpatch;
mod math;
mod profiler;
mod runtime;
mod types;

use std::path::PathBuf;

use bevy_ecs::prelude::*;
use engine_core::prelude::*;

pub use access::ScriptError;
pub use entity::OwnMaterial;
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
}

impl Default for Script {
    fn default() -> Self {
        Self {
            path: String::new(),
            props: serde_json::Value::Object(Default::default()),
            enabled: true,
        }
    }
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
}

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
    /// Memory the scripts may use in all, MiB.
    pub memory_limit_mb: u32,
    /// Port the script debugger listens on for VS Code and other DAP clients (localhost only;
    /// `ENGINE_SCRIPT_DEBUG=4711`, or `script.debug` at run time). None: not listening.
    pub debug_port: Option<u16>,
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
        }
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
        app.register_component::<Script>(
            "Runs a Luau script from assets/scripts on this entity: path (\"showcase/lantern.luau\"), \
             props (JSON over the script's defaults), enabled.",
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
        .register_resource::<ScriptSettings>(
            "Scripting: folder under the asset root, hot_reload, poll_secs, call_limit_ms (per \
             call), memory_limit_mb, debug_port (the debugger's, for VS Code).",
        )
        .add_systems(Update, run_scripts.in_set(UpdateSet::Logic))
        .add_systems(FixedUpdate, run_fixed_scripts.in_set(SimSet::Gameplay))
        .add_systems(Update, board::draw_boards.after(run_scripts))
        .add_systems(
            Update,
            board::draw_profile_boards.after(board::draw_boards),
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
    let has_scripts = !world
        .query_filtered::<(), With<Script>>()
        .iter(world)
        .next()
        .is_none();
    let has_work = world.get_resource::<ScriptRuntime>().is_some_and(|rt| {
        // A debugger client may attach before the first script spawns.
        rt.task_count() > 0 || !rt.instances.is_empty() || rt.debugger.listening()
    });
    if !has_scripts && !has_work {
        return;
    }
    let _ = with_runtime(world, |world, rt, settings| rt.run_frame(world, settings));
    if let Some(rt) = world.get_resource::<ScriptRuntime>() {
        let (ms, instances, tasks) = (rt.frame_ms, rt.instances.len(), rt.task_count());
        let memory_mb = rt.lua.used_memory() as f64 / (1024.0 * 1024.0);
        let errors = access::shared(&rt.lua).errors_total;
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("script.ms", ms as f64);
            d.set("script.instances", instances as f64);
            d.set("script.tasks", tasks as f64);
            d.set("script.memory_mb", memory_mb);
            d.set("script.errors", errors as f64);
        }
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
