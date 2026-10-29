//! The script runtime: one Luau VM, the script files it loaded, the script instances on
//! entities, the task scheduler and hot reload; and the actor VMs that run `parallel_update`
//! side by side.
//!
//! The runner systems take [`ScriptRuntime`] out of the world for the length of their run and
//! lend the world to the VM (`access`). Lua callbacks reach shared bookkeeping (new tasks,
//! messages, errors, the file table for `require`) through the VM's app data, so nothing here is
//! borrowed while Lua code runs.
//!
//! A frame: the main VM's serial work (hot reload, new instances, events, `update`, tasks),
//! then each actor VM's, then the parallel phase, where every VM runs its instances'
//! `parallel_update` on its own thread with the world shared read-only; then each VM's queued
//! changes are applied, VM after VM, and the code that called `task.synchronize()` goes on, one
//! script at a time. Instances of a script with `parallel_update` are spread over the actor
//! VMs ([`crate::ScriptActor`] says which); everything else runs in the main VM.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use bevy_ecs::change_detection::Tick;
use bevy_ecs::prelude::*;
use engine_core::prelude::{LocalTimeScale, Name, Time, TimeDilation};
use engine_core::registry::ApiRegistry;
use mlua::{Function, Lua, MultiValue, RegistryKey, Table, Thread, Value, Variadic};

use crate::access::{
    KEEP_LINES, NewTask, Shared, Watchdog, apply_deferred, lend_world, lend_world_parallel,
    push_error, shared, with_world,
};
use crate::convert::{to_lua, to_lua_like};
use crate::entity::{EntityRef, SELVES};
use crate::profiler::Phase;
use crate::{Script, ScriptActor, ScriptSettings};

/// Registry name of the class every instance inherits helpers from (`every`, `after`, `spawn`).
pub(crate) const BASE: &str = "engine_script.base";

/// Hooks a script's class defines, looked up when it loads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Hooks {
    pub start: bool,
    pub update: bool,
    pub fixed_update: bool,
    pub parallel_update: bool,
    pub on_interact: bool,
    pub on_contact: bool,
    pub on_message: bool,
    pub on_reload: bool,
    pub on_props: bool,
    pub on_destroy: bool,
    pub on_world_shift: bool,
    /// `reload = "restart"`: hot reload restarts instances instead of keeping their state.
    pub restart_on_reload: bool,
    /// A `states` table: the engine runs the class's state machine (`states.luau`).
    pub states: bool,
}

impl Hooks {
    fn of(class: &Table) -> Self {
        let has = |name: &str| matches!(class.raw_get::<Value>(name), Ok(Value::Function(_)));
        Self {
            start: has("start"),
            update: has("update"),
            fixed_update: has("fixed_update"),
            parallel_update: has("parallel_update"),
            on_interact: has("on_interact"),
            on_contact: has("on_contact"),
            on_message: has("on_message"),
            on_reload: has("on_reload"),
            on_props: has("on_props"),
            on_destroy: has("on_destroy"),
            on_world_shift: has("on_world_shift"),
            restart_on_reload: matches!(
                class.raw_get::<Value>("reload"),
                Ok(Value::String(s)) if s.to_str().is_ok_and(|s| &*s == "restart")
            ),
            states: matches!(class.raw_get::<Value>("states"), Ok(Value::Table(_))),
        }
    }
}

/// Whether a file is a script (returns a class, runs on entities) or a module (`require`d).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileKind {
    Script,
    Module,
}

/// One `.luau` file the runtime loaded.
pub(crate) struct FileState {
    pub kind: FileKind,
    pub full: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    pub source: String,
    /// The class table (scripts) or the returned value (modules). Kept across reloads: a
    /// reload copies the new class into the same table, so everything holding it sees new code.
    value: Option<RegistryKey>,
    /// Instances' metatable: `{ __index = class }`.
    meta: Option<RegistryKey>,
    pub hooks: Hooks,
    pub error: Option<String>,
    /// Bumps on every successful load.
    pub version: u32,
    /// The compiled file of the current version (the debugger sets breakpoints on it).
    pub chunk: Option<mlua::Function>,
    /// The last error raised in this file's code since it last loaded (code boards mark it).
    pub fault: Option<Fault>,
}

/// A runtime error located in a file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fault {
    pub line: u32,
    /// The error without its location and traceback.
    pub message: String,
    /// Times it was raised.
    pub count: u64,
}

/// Where an error message says it happened: the first `file.luau:LINE:` in it (a traceback's
/// first Luau frame for errors raised by engine calls).
pub(crate) fn error_location(message: &str) -> Option<(String, u32)> {
    let mut rest = message;
    while let Some(i) = [".luau:", ".lua:", ".graph:"]
        .iter()
        .filter_map(|e| rest.find(e))
        .min()
    {
        let ext = [".luau:", ".lua:", ".graph:"]
            .iter()
            .find(|e| rest[i..].starts_with(**e))
            .map_or(5, |e| e.len());
        let start = rest[..i]
            .rfind(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '(')
            .map(|j| j + 1)
            .unwrap_or(0);
        let digits: String = rest[i + ext..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() && rest[i + ext + digits.len()..].starts_with(':') {
            let path = rest[start..i + ext - 1].trim_start_matches('@').to_string();
            return Some((path, digits.parse().ok()?));
        }
        rest = &rest[i + ext..];
    }
    None
}

/// An error message's gist: what an engine call failed with, or the message without the
/// `file:line:` in front and the traceback after.
pub(crate) fn error_gist(message: &str) -> String {
    let text = match message.rfind("caused by: ") {
        Some(i) => &message[i + "caused by: ".len()..],
        None => message,
    };
    let first = text.lines().next().unwrap_or_default();
    let first = first.strip_prefix("runtime error: ").unwrap_or(first);
    let first = first.strip_prefix("syntax error: ").unwrap_or(first);
    match error_location(first) {
        Some((path, line)) => {
            let at = format!("{path}:{line}:");
            match first.find(&at) {
                Some(i) => first[i + at.len()..].trim().to_string(),
                None => first.trim().to_string(),
            }
        }
        None => first.trim().to_string(),
    }
}

/// Notes a runtime error on the file and line it names (code boards mark it).
pub(crate) fn note_fault(lua: &Lua, message: &str) {
    let Some((path, line)) = error_location(message) else {
        return;
    };
    let Some(mut files) = lua.app_data_mut::<Files>() else {
        return;
    };
    let Some(f) = files.map.get_mut(&path) else {
        return;
    };
    let gist = error_gist(message);
    match &mut f.fault {
        Some(fault) if fault.line == line && fault.message == gist => fault.count += 1,
        slot => {
            *slot = Some(Fault {
                line,
                message: gist,
                count: 1,
            })
        }
    }
}

/// Every file loaded, by path relative to the scripts folder (`showcase/lantern.luau`).
#[derive(Default)]
pub(crate) struct Files {
    pub root: PathBuf,
    pub map: BTreeMap<String, FileState>,
}

/// A script running on one entity.
pub(crate) struct Instance {
    pub path: String,
    this: RegistryKey,
    pub started: bool,
    pub enabled: bool,
    /// Seconds on this instance's clock (world time scaled by its entity's LocalTimeScale).
    pub clock: f64,
    synced: Tick,
    pub hooks: Hooks,
    pub last_error: Option<String>,
    pub errors: u32,
    /// Milliseconds spent in this script last frame (smoothed).
    pub ms: f32,
}

/// A coroutine stopped by the debugger, waiting at its line ([`crate::debugger`]).
struct Held {
    thread: Thread,
    owner: Option<Entity>,
    path: String,
    /// A task (or an event or `start`, which may wait): when it waits again it goes back to
    /// the tasks. A hook like `update` can't wait.
    task: bool,
}

/// The key of the debugger's [`TimeDilation`] hold.
pub(crate) const DEBUG_HOLD: &str = "script-debugger";

struct Task {
    thread: Thread,
    owner: Option<Entity>,
    wake_at: f64,
    since: f64,
    /// Hasn't run yet (started with `task.delay` or `task.defer`).
    fresh: bool,
}

/// The scripting VM and everything running in it. A resource; see the crate docs.
#[derive(Resource)]
pub struct ScriptRuntime {
    pub(crate) lua: Lua,
    watchdog: Watchdog,
    pub(crate) debugger: crate::debugger::Debugger,
    pub(crate) profiler: crate::profiler::Profiler,
    pub(crate) instances: BTreeMap<Entity, Instance>,
    tasks: Vec<Task>,
    next_poll: Option<Instant>,
    /// Milliseconds the scripts took last frame.
    pub(crate) frame_ms: f32,
    /// Interact and contact messages read so far.
    interact_cursor: Option<bevy_ecs::message::MessageCursor<engine_world::Interact>>,
    contact_cursor: Option<bevy_ecs::message::MessageCursor<engine_world::physics::ContactEvent>>,
    pub(crate) reloads: u32,
    /// 0 for the main VM; `k` for actor VM `k` (1-based, its [`ScriptActor`] number).
    role: u32,
    /// For an actor VM: how many actor VMs there are.
    peers: u32,
    /// The actor VMs (the main VM's; made when the first `parallel_update` script starts).
    pub(crate) actors: Vec<ScriptRuntime>,
    /// A finished `parallel_update` coroutine, reused for the next call.
    spare: Option<Thread>,
    /// `parallel_update` calls that called `task.synchronize()`: they go on after the parallel
    /// phase.
    synced: Vec<(Thread, Entity, String)>,
    /// Wall-clock milliseconds of last frame's parallel phase (every VM).
    pub(crate) parallel_ms: f32,
    /// The coroutine stopped at a breakpoint or step.
    held: Option<Held>,
    /// The world's time is held still for a stop.
    holding_time: bool,
}

/// Normalizes a script path: forward slashes, no leading `./` or `/`, `.luau` added when it has
/// no extension.
pub fn normalize(path: &str) -> String {
    let mut p = path.trim().replace('\\', "/");
    while let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    let p = p.trim_start_matches('/').to_string();
    let file = p.rsplit('/').next().unwrap_or("");
    if file.contains('.') {
        p
    } else {
        format!("{p}.luau")
    }
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl ScriptRuntime {
    /// A fresh VM with the engine's globals, reading scripts from `root`.
    pub fn new(api: &ApiRegistry, root: PathBuf, settings: &ScriptSettings) -> mlua::Result<Self> {
        Self::vm(api, root, settings, (0, 0))
    }

    /// A VM for `role` (0: the main one; `k`: actor VM `k` of `peers`).
    fn vm(
        api: &ApiRegistry,
        root: PathBuf,
        settings: &ScriptSettings,
        (role, peers): (u32, u32),
    ) -> mlua::Result<Self> {
        let lua = Lua::new();
        lua.set_memory_limit(settings.memory_limit_mb as usize * 1024 * 1024)?;
        lua.set_app_data(Shared::default());
        lua.set_app_data(crate::testing::TestRun::default());
        lua.set_app_data(Files {
            root,
            map: BTreeMap::new(),
        });
        lua.set_named_registry_value(SELVES, lua.create_table()?)?;

        crate::math::install(&lua)?;
        crate::entity::install(&lua)?;
        crate::bind::install(&lua, api)?;
        install_tasks(&lua)?;
        install_require(&lua)?;
        let watchdog = Watchdog::new();
        watchdog.install(&lua)?;
        let debugger = crate::debugger::Debugger::install(&lua, watchdog.clone())?;
        let profiler = crate::profiler::Profiler::new(&lua, &watchdog);
        let g = lua.globals();
        lua.load(include_str!("prelude.luau"))
            .set_name("@engine/prelude.luau")
            .exec()?;
        lua.load(include_str!("states.luau"))
            .set_name("@engine/states.luau")
            .exec()?;
        let base: Table = g.get("__script_base")?;
        lua.set_named_registry_value(BASE, base)?;
        for internal in [
            "__script_base",
            "__task_spawn",
            "__task_spawn_as",
            "__task_delay",
            "__task_cancel",
            "__script_stopped",
            "__script_parallel",
            "__script_emit",
        ] {
            g.raw_set(internal, Value::Nil)?;
        }
        // As in Roblox (loadstring is off there by default): these let one script reach into
        // another's globals and switch off Luau's fast paths for the whole file.
        for unsafe_global in ["getfenv", "setfenv", "loadstring"] {
            g.raw_set(unsafe_global, Value::Nil)?;
        }
        // Read-only libraries and globals; each script gets its own global table (below).
        lua.sandbox(true)?;
        Ok(Self {
            lua,
            watchdog,
            debugger,
            profiler,
            instances: BTreeMap::new(),
            tasks: Vec::new(),
            next_poll: None,
            frame_ms: 0.0,
            interact_cursor: None,
            contact_cursor: None,
            reloads: 0,
            role,
            peers,
            actors: Vec::new(),
            spare: None,
            synced: Vec::new(),
            held: None,
            holding_time: false,
            parallel_ms: 0.0,
        })
    }

    /// Every VM: this one, then its actor VMs.
    pub(crate) fn vms(&self) -> impl Iterator<Item = &ScriptRuntime> {
        std::iter::once(self).chain(self.actors.iter())
    }

    /// The scripts folder (absolute).
    pub fn root(&self) -> PathBuf {
        self.lua
            .app_data_ref::<Files>()
            .map(|f| f.root.clone())
            .unwrap_or_default()
    }

    pub(crate) fn files(&self) -> mlua::AppDataRef<'_, Files> {
        self.lua
            .app_data_ref::<Files>()
            .expect("files are installed")
    }

    /// Number of live tasks, in every VM.
    pub fn task_count(&self) -> usize {
        self.vms().map(|v| v.tasks.len()).sum()
    }

    /// Number of script instances, in every VM.
    pub fn instance_count(&self) -> usize {
        self.vms().map(|v| v.instances.len()).sum()
    }

    /// Number of VMs: the main one and the actor VMs.
    pub fn vm_count(&self) -> usize {
        1 + self.actors.len()
    }

    /// Wall-clock milliseconds of last frame's parallel phase.
    pub fn parallel_ms(&self) -> f32 {
        self.parallel_ms
    }

    /// Recent errors of every VM, oldest first.
    pub fn errors(&self) -> Vec<crate::access::ScriptError> {
        let mut all: Vec<crate::access::ScriptError> = self
            .vms()
            .flat_map(|v| shared(&v.lua).errors.iter().cloned().collect::<Vec<_>>())
            .collect();
        all.sort_by(|a, b| a.at_secs.total_cmp(&b.at_secs));
        all
    }

    /// Empties every VM's error list.
    pub(crate) fn clear_errors(&self) {
        for v in self.vms() {
            shared(&v.lua).errors.clear();
        }
    }

    /// Errors ever reported, in every VM.
    pub(crate) fn errors_total(&self) -> u64 {
        self.vms().map(|v| shared(&v.lua).errors_total).sum()
    }

    /// Memory the VMs use, bytes.
    pub(crate) fn used_memory(&self) -> usize {
        self.vms().map(|v| v.lua.used_memory()).sum()
    }

    /// Recent `print` output, oldest first (the main VM's, then each actor VM's).
    pub fn log(&self) -> Vec<String> {
        self.vms()
            .flat_map(|v| shared(&v.lua).log.iter().cloned().collect::<Vec<_>>())
            .collect()
    }

    /// Every VM's Lua state, the main one first.
    pub(crate) fn luas(&self) -> Vec<&Lua> {
        self.vms().map(|v| &v.lua).collect()
    }

    /// The last runtime error in `path`'s code, in whichever VM ran it.
    pub(crate) fn fault_of(&self, path: &str) -> Option<Fault> {
        self.vms().find_map(|v| {
            v.lua
                .app_data_ref::<Files>()
                .and_then(|f| f.map.get(path).and_then(|f| f.fault.clone()))
        })
    }

    /// Starts a profiler recording in every VM.
    pub(crate) fn profile_start(&mut self, hz: u32, seconds: f64, auto: bool) {
        self.profiler.start(&self.lua, hz, seconds, auto);
        for a in &mut self.actors {
            a.profiler.start(&a.lua, hz, seconds, auto);
        }
    }

    /// Ends every VM's recording (the results stay).
    pub(crate) fn profile_stop(&mut self) {
        self.profiler.stop(&self.lua);
        for a in &mut self.actors {
            a.profiler.stop(&a.lua);
        }
    }

    /// Drops every VM's recording.
    pub(crate) fn profile_reset(&mut self) {
        self.profiler.reset(&self.lua);
        for a in &mut self.actors {
            a.profiler.reset(&a.lua);
        }
    }

    /// Some VM is recording.
    pub(crate) fn profile_recording(&self) -> bool {
        self.vms().any(|v| v.profiler.recording())
    }

    /// A live profile board may start a recording (see `Profiler::idle_for_boards`).
    pub(crate) fn profile_idle_for_boards(&self) -> bool {
        self.vms().all(|v| v.profiler.idle_for_boards(&v.lua))
    }

    /// Source and version of a loaded file, if it is loaded.
    pub fn file_source(&self, path: &str) -> Option<(String, u32)> {
        let path = normalize(path);
        self.files()
            .map
            .get(&path)
            .map(|f| (f.source.clone(), f.version))
    }

    /// `path`'s class table, once it has loaded.
    pub(crate) fn class(&self, path: &str) -> Option<Table> {
        let files = self.files();
        let key = files.map.get(&normalize(path))?.value.as_ref()?;
        self.lua.registry_value::<Table>(key).ok()
    }

    /// Loads `path` now (or returns the error that kept it from loading). Scripts load on their
    /// own when an entity gets a `Script`; this is for tools.
    pub fn ensure_loaded(&mut self, world: &mut World, path: &str) -> Result<(), String> {
        let path = normalize(path);
        let _lent = lend_world(&self.lua, world);
        load_file(&self.lua, &self.watchdog, &path, FileKind::Script)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    // ------------------------------------------------------------------------- the frame

    /// One frame's script work (`Update`): every VM's serial part (hot reload, instances,
    /// events, `update`, tasks), then the parallel phase (`parallel_update`), then messages
    /// move to the VMs of the scripts they are for.
    pub(crate) fn run_frame(&mut self, world: &mut World, settings: &ScriptSettings) {
        let started = Instant::now();
        if self.debug_hold(world, settings) {
            self.debugger.flush(&self.lua);
            self.frame_ms = started.elapsed().as_secs_f32() * 1000.0;
            return;
        }
        self.route_messages();
        self.run_serial(world, settings);
        for actor in &mut self.actors {
            actor.run_serial(world, settings);
        }
        if !self.debugger.stopped() {
            self.run_parallel(world, settings);
        }
        self.route_messages();
        self.debugger.flush(&self.lua);
        self.profiler.frame(&self.lua);
        for actor in &mut self.actors {
            actor.profiler.frame(&actor.lua);
        }
        self.frame_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    /// The debugger's DAP client: what it sent since last frame. Console code it sends runs
    /// like `script.eval` (which lends the world itself, so before the frame does).
    fn serve_debugger(&mut self, world: &mut World, settings: &ScriptSettings) {
        for request in self.debugger.poll(&self.lua) {
            let code = request["arguments"]["expression"]
                .as_str()
                .unwrap_or_default();
            let result = self.eval(world, code, settings).map(|v| {
                let text = match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Null => "nil".into(),
                    other => other.to_string(),
                };
                serde_json::json!({"result": text, "variablesReference": 0})
            });
            self.debugger.respond(&request, result);
        }
    }

    /// While the debugger is stopped: serves its client, holds the world's time still, and
    /// resumes the stopped coroutine when a client says go on. Returns true when the frame's
    /// scripts don't run (still stopped, or the stopped script just went on).
    fn debug_hold(&mut self, world: &mut World, settings: &ScriptSettings) -> bool {
        if !self.debugger.stopped() && self.held.is_none() {
            self.release_time(world);
            return false;
        }
        self.serve_debugger(world, settings);
        if !self.holding_time {
            if let Some(mut d) = world.get_resource_mut::<TimeDilation>() {
                d.hold(DEBUG_HOLD, 0.0, 0.0);
            }
            self.holding_time = true;
        }
        if !self.debugger.take_go() {
            return true;
        }
        if let Some(held) = self.held.take() {
            let world_ptr: *mut World = world;
            let _lent = lend_world(&self.lua, world_ptr);
            let Held {
                thread,
                owner,
                path,
                task,
            } = held;
            shared(&self.lua).owner = owner;
            self.debugger.resuming(&self.lua, &thread);
            self.watchdog.arm(settings.call_limit_ms);
            let result = thread.resume::<MultiValue>(());
            self.watchdog.disarm();
            self.debugger.resumed();
            shared(&self.lua).owner = None;
            self.after_resume(thread, owner, &path, result, task);
            self.adopt_new_tasks();
        }
        if !self.debugger.stopped() && self.held.is_none() {
            self.release_time(world);
        }
        true
    }

    /// Lets the world's time run again after a stop (it eases back in).
    fn release_time(&mut self, world: &mut World) {
        if std::mem::take(&mut self.holding_time)
            && let Some(mut d) = world.get_resource_mut::<TimeDilation>()
        {
            d.release(DEBUG_HOLD);
        }
    }

    /// One VM's serial work for the frame.
    fn run_serial(&mut self, world: &mut World, settings: &ScriptSettings) {
        let now = world.resource::<Time>().elapsed_secs_f64();
        shared(&self.lua).world_secs = now;
        self.serve_debugger(world, settings);
        let world_ptr: *mut World = world;
        let _lent = lend_world(&self.lua, world_ptr);
        // SAFETY (for every `unsafe { &mut *world_ptr }` below): the runner owns the world for
        // this call; borrows taken here end before any call into Lua, whose callbacks borrow it
        // through the same pointer (`access::with_world`).
        let w = || unsafe { &mut *world_ptr };

        if settings.hot_reload {
            let due = self.next_poll.is_none_or(|t| Instant::now() >= t);
            if due {
                self.next_poll =
                    Some(Instant::now() + std::time::Duration::from_secs_f32(settings.poll_secs));
                self.poll_files(w(), settings);
            }
        }
        self.sync_instances(w(), settings);
        self.advance_clocks(w());
        self.deliver_interactions(w(), settings);
        self.deliver_contacts(w(), settings);
        self.deliver_messages(settings);
        self.deliver_signals(w(), settings);

        let updating: Vec<Entity> = self
            .instances
            .iter()
            .filter(|(_, i)| i.enabled && i.started && (i.hooks.update || i.hooks.states))
            .map(|(e, _)| *e)
            .collect();
        for e in updating {
            if self.debugger.stopped() {
                // The others wait with the stopped script.
                break;
            }
            let dt_e = {
                let w = w();
                w.resource::<Time>().delta_for(w.get::<LocalTimeScale>(e)) as f64
            };
            let t0 = Instant::now();
            let (update, states, clock) = self.instances.get(&e).map_or((false, false, 0.0), |i| {
                (i.hooks.update, i.hooks.states, i.clock)
            });
            if update {
                self.call_hook(e, "update", dt_e, settings);
            }
            if states {
                // Its states' updates, timers and conditions, after `update`.
                self.call_hook(e, "__states_tick", (dt_e, clock), settings);
            }
            if let Some(i) = self.instances.get_mut(&e) {
                let ms = t0.elapsed().as_secs_f32() * 1000.0;
                i.ms = i.ms * 0.9 + ms * 0.1;
            }
        }
        self.run_tasks(now, settings);
    }

    // ------------------------------------------------------------------------- parallel

    /// Whether this VM has instances to run in the parallel phase.
    fn wants_parallel(&self) -> bool {
        self.instances
            .values()
            .any(|i| i.enabled && i.started && i.hooks.parallel_update)
    }

    /// The parallel phase: every VM runs its `parallel_update`s on its own thread (the compute
    /// pool's), reading the world as the serial phase left it; then the VMs' changes are
    /// applied in VM order, and the calls that synchronized go on.
    fn run_parallel(&mut self, world: &mut World, settings: &ScriptSettings) {
        let main = self.wants_parallel();
        let busy = self.actors.iter().filter(|a| a.wants_parallel()).count();
        if !main && busy == 0 {
            self.parallel_ms = 0.0;
            return;
        }
        let t0 = Instant::now();
        {
            let w: &World = world;
            let mut actors = std::mem::take(&mut self.actors);
            if usize::from(main) + busy == 1 {
                // One VM: no hand-off to another thread.
                if main {
                    self.parallel_part(w, settings);
                }
                for actor in actors.iter_mut().filter(|a| a.wants_parallel()) {
                    actor.parallel_part(w, settings);
                }
            } else {
                let pool = engine_core::tasks::ComputeTaskPool::get_or_init(Default::default);
                let this = &mut *self;
                pool.scope(|scope| {
                    for actor in actors.iter_mut().filter(|a| a.wants_parallel()) {
                        scope.spawn(async move { actor.parallel_part(w, settings) });
                    }
                    if main {
                        scope.spawn(async move { this.parallel_part(w, settings) });
                    }
                });
            }
            self.actors = actors;
        }
        self.parallel_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.apply_parallel(world);
        for actor in &mut self.actors {
            actor.apply_parallel(world);
        }
        self.resume_synced(world, settings);
        for actor in &mut self.actors {
            actor.resume_synced(world, settings);
        }
    }

    /// This VM's share of the parallel phase.
    fn parallel_part(&mut self, world: &World, settings: &ScriptSettings) {
        let ids: Vec<Entity> = self
            .instances
            .iter()
            .filter(|(_, i)| i.enabled && i.started && i.hooks.parallel_update)
            .map(|(e, _)| *e)
            .collect();
        let _lent = lend_world_parallel(&self.lua, world);
        let time = world.resource::<Time>();
        for e in ids {
            let dt = time.delta_for(world.get::<LocalTimeScale>(e)) as f64;
            let t0 = Instant::now();
            self.call_parallel(e, dt, settings);
            if let Some(i) = self.instances.get_mut(&e) {
                let ms = t0.elapsed().as_secs_f32() * 1000.0;
                i.ms = i.ms * 0.9 + ms * 0.1;
            }
        }
    }

    /// `self:parallel_update(dt)` on `e`'s instance, in a coroutine so it can
    /// `task.synchronize()`.
    fn call_parallel(&mut self, e: Entity, dt: f64, settings: &ScriptSettings) {
        let Some(i) = self.instances.get(&e) else {
            return;
        };
        let Ok(Value::Table(this)) = self.lua.registry_value::<Value>(&i.this) else {
            return;
        };
        let path = i.path.clone();
        let Ok(Value::Function(f)) = this.get::<Value>("parallel_update") else {
            return;
        };
        let thread = match self.spare.take() {
            Some(t) if t.reset(f.clone()).is_ok() => t,
            _ => match self.lua.create_thread(f) {
                Ok(t) => t,
                Err(err) => return self.report(e, &path, &err),
            },
        };
        self.profiler.phase(Phase::Parallel);
        shared(&self.lua).owner = Some(e);
        if self.debugger.attached() {
            self.debugger.prepare(&self.lua, &thread);
        }
        self.watchdog.arm(settings.call_limit_ms);
        let result = thread.resume::<MultiValue>((this, dt));
        self.watchdog.disarm();
        self.profiler.settle(&self.lua);
        shared(&self.lua).owner = None;
        if let Err(err) = &result
            && self.debugger.breaks_on_errors()
        {
            self.debugger.stop_on_task_error(&self.lua, &thread, err);
        }
        match result {
            Ok(values) if thread.is_resumable() => {
                let sync = matches!(values.front(), Some(Value::String(s)) if s.to_str().is_ok_and(|s| &*s == "sync"));
                if sync {
                    self.synced.push((thread, e, path));
                } else {
                    // Like `update`, it runs again next frame, so it can't also wait.
                    let err = mlua::Error::runtime(
                        "parallel_update can't wait (it runs again next frame); start a task \
                         with task.spawn for things that take time",
                    );
                    self.report(e, &path, &err);
                    self.spare = Some(thread);
                }
            }
            Ok(_) => self.spare = Some(thread),
            Err(err) => {
                self.report(e, &path, &err);
                self.spare = Some(thread);
            }
        }
        self.adopt_new_tasks();
    }

    /// Applies what this VM changed in the parallel phase.
    fn apply_parallel(&mut self, world: &mut World) {
        for (owner, message) in apply_deferred(&self.lua, world) {
            let err = mlua::Error::runtime(message);
            match owner.and_then(|e| self.instances.get(&e).map(|i| (e, i.path.clone()))) {
                Some((e, path)) => self.report(e, &path, &err),
                None => push_error(&self.lua, None, owner, &err.to_string()),
            }
        }
    }

    /// The `parallel_update` calls that called `task.synchronize()` go on, with the world.
    fn resume_synced(&mut self, world: &mut World, settings: &ScriptSettings) {
        if self.synced.is_empty() {
            return;
        }
        let _lent = lend_world(&self.lua, world);
        self.profiler.phase(Phase::Parallel);
        for (thread, e, path) in std::mem::take(&mut self.synced) {
            self.resume(thread, Some(e), &path, (), settings);
            self.adopt_new_tasks();
        }
    }

    /// Moves messages to the VM that runs the script they are for (the main VM when none
    /// does yet).
    fn route_messages(&mut self) {
        if self.actors.is_empty() {
            return;
        }
        let mut all = std::mem::take(&mut shared(&self.lua).outbox);
        for actor in &self.actors {
            all.append(&mut shared(&actor.lua).outbox);
        }
        for m in all {
            let lua = self
                .actors
                .iter()
                .find(|a| a.instances.contains_key(&m.to))
                .map_or(&self.lua, |a| &a.lua);
            shared(lua).outbox.push(m);
        }
    }

    /// How many actor VMs to make: `ScriptSettings::parallel_vms`, or by default one fewer than
    /// the cores (at most 8). None while a debugger was asked for at start, so breakpoints
    /// reach every script.
    fn actor_count(settings: &ScriptSettings) -> usize {
        match settings.parallel_vms {
            Some(n) => n as usize,
            None if settings.debug_port.is_some() => 0,
            None => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .saturating_sub(1)
                .clamp(1, 8),
        }
    }

    /// The actor VM to run a new `parallel_update` script on (making the actor VMs the first
    /// time): the one with the fewest instances. `planned` counts those handed out this frame.
    fn pick_actor(
        &mut self,
        world: &World,
        settings: &ScriptSettings,
        planned: &mut Vec<usize>,
    ) -> Option<u32> {
        if self.role != 0 {
            return None;
        }
        if self.actors.is_empty() {
            let n = Self::actor_count(settings);
            if n == 0 {
                return None;
            }
            let api = world
                .get_resource::<ApiRegistry>()
                .cloned()
                .unwrap_or_default();
            let root = self.root();
            for k in 1..=n as u32 {
                match Self::vm(&api, root.clone(), settings, (k, n as u32)) {
                    Ok(vm) => self.actors.push(vm),
                    Err(err) => {
                        tracing::warn!(target: "script", "actor VM {k} could not start: {err}");
                        break;
                    }
                }
            }
            if self.actors.is_empty() {
                return None;
            }
            tracing::info!(target: "script", "{} actor VMs run parallel_update", self.actors.len());
        }
        planned.resize(self.actors.len(), 0);
        let k =
            (0..self.actors.len()).min_by_key(|&k| self.actors[k].instances.len() + planned[k])?;
        planned[k] += 1;
        Some(k as u32 + 1)
    }

    /// The VM (0: main) that runs the script on an entity with this [`ScriptActor`].
    fn vm_of(&self, actor: Option<&ScriptActor>) -> u32 {
        let n = match self.role {
            0 => self.actors.len() as u32,
            _ => self.peers,
        };
        match actor {
            Some(a) if a.0 > 0 && n > 0 => (a.0 - 1) % n + 1,
            _ => 0,
        }
    }

    /// The fixed-step hook (`FixedUpdate`, `SimSet::Gameplay`), in every VM.
    pub(crate) fn run_fixed(&mut self, world: &mut World, settings: &ScriptSettings) {
        self.run_fixed_here(world, settings);
        for actor in &mut self.actors {
            actor.run_fixed_here(world, settings);
        }
    }

    fn run_fixed_here(&mut self, world: &mut World, settings: &ScriptSettings) {
        let world_ptr: *mut World = world;
        let _lent = lend_world(&self.lua, world_ptr);
        let fixed: Vec<Entity> = self
            .instances
            .iter()
            .filter(|(_, i)| i.enabled && i.started && i.hooks.fixed_update)
            .map(|(e, _)| *e)
            .collect();
        for e in fixed {
            // SAFETY: as in `run_frame`.
            let w = unsafe { &*world_ptr };
            let dt_e = w.resource::<Time>().delta_for(w.get::<LocalTimeScale>(e)) as f64;
            self.call_hook(e, "fixed_update", dt_e, settings);
        }
    }

    /// True when any instance keeps world positions of its own (`on_world_shift`).
    pub(crate) fn wants_world_shift(&self) -> bool {
        self.vms()
            .flat_map(|v| v.instances.values())
            .any(|i| i.started && i.hooks.on_world_shift)
    }

    /// The world moved to a new frame: `on_world_shift(shift)` on every script that has it, so
    /// positions it keeps follow (`WorldShiftSet::Apply`, before the next frame's scripts).
    pub(crate) fn world_shift(
        &mut self,
        world: &mut World,
        shift: engine_world::WorldShift,
        settings: &ScriptSettings,
    ) {
        self.world_shift_here(world, shift, settings);
        for actor in &mut self.actors {
            actor.world_shift_here(world, shift, settings);
        }
    }

    fn world_shift_here(
        &mut self,
        world: &mut World,
        shift: engine_world::WorldShift,
        settings: &ScriptSettings,
    ) {
        let _lent = lend_world(&self.lua, world);
        let shifted: Vec<Entity> = self
            .instances
            .iter()
            .filter(|(_, i)| i.started && i.hooks.on_world_shift)
            .map(|(e, _)| *e)
            .collect();
        for e in shifted {
            self.call_hook(e, "on_world_shift", crate::math::Shift(shift), settings);
        }
    }

    /// True when any instance wants the fixed-step hook.
    pub(crate) fn wants_fixed(&self) -> bool {
        self.vms()
            .flat_map(|v| v.instances.values())
            .any(|i| i.hooks.fixed_update)
    }

    /// Each instance's clock: world seconds times its entity's `LocalTimeScale` (real seconds
    /// for an entity that ignores world slow motion).
    fn advance_clocks(&mut self, world: &World) {
        let time = world.resource::<Time>();
        for (e, i) in self.instances.iter_mut() {
            if i.enabled {
                i.clock += time.delta_for(world.get::<LocalTimeScale>(*e)) as f64;
            }
        }
    }

    // ------------------------------------------------------------------------- instances

    fn sync_instances(&mut self, world: &mut World, settings: &ScriptSettings) {
        let this_run = world.change_tick();
        let mut seen: Vec<(Entity, Option<Script>, Tick, bool)> = Vec::new();
        {
            let mut q = world.query::<(Entity, Ref<Script>, Option<&ScriptActor>)>();
            for (e, s, actor) in q.iter(world) {
                if self.vm_of(actor) != self.role {
                    continue;
                }
                let changed = match self.instances.get(&e) {
                    None => true,
                    Some(i) => s.last_changed().is_newer_than(i.synced, this_run),
                };
                seen.push((
                    e,
                    changed.then(|| Script::clone(&s)),
                    s.last_changed(),
                    actor.is_some(),
                ));
            }
        }
        let live: std::collections::HashSet<Entity> = seen.iter().map(|(e, _, _, _)| *e).collect();
        let gone: Vec<Entity> = self
            .instances
            .keys()
            .filter(|e| !live.contains(e))
            .copied()
            .collect();
        for e in gone {
            self.destroy_instance(e, settings);
        }
        let mut planned = Vec::new();
        for (e, script, tick, placed) in seen {
            let Some(script) = script else { continue };
            let path = normalize(&script.path);
            // A new `parallel_update` script goes to an actor VM (unless placed already).
            if self.role == 0
                && !placed
                && !self.instances.contains_key(&e)
                && self.hooks_of(&path).is_some_and(|h| h.parallel_update)
                && let Some(k) = self.pick_actor(world, settings, &mut planned)
            {
                world.entity_mut(e).insert(ScriptActor(k));
                continue;
            }
            match self.instances.get(&e) {
                Some(i) if i.path == path => {
                    let props_changed = self.update_props(e, &script);
                    if let Some(i) = self.instances.get_mut(&e) {
                        i.enabled = script.enabled;
                        i.synced = tick;
                        if props_changed && i.hooks.on_props {
                            self.call_hook(e, "on_props", (), settings);
                        }
                    }
                }
                Some(_) => {
                    self.destroy_instance(e, settings);
                    self.create_instance(e, &script, tick, settings);
                }
                None => self.create_instance(e, &script, tick, settings),
            }
        }
    }

    /// The hooks of the script at `path`, loading it if it isn't yet; None if it doesn't load.
    fn hooks_of(&self, path: &str) -> Option<Hooks> {
        let loaded = self
            .files()
            .map
            .get(path)
            .filter(|f| f.value.is_some())
            .map(|f| f.hooks);
        loaded.or_else(|| load_file(&self.lua, &self.watchdog, path, FileKind::Script).ok())
    }

    /// Makes `self` for a script on `e` and starts it.
    fn create_instance(
        &mut self,
        e: Entity,
        script: &Script,
        tick: Tick,
        settings: &ScriptSettings,
    ) {
        let path = normalize(&script.path);
        let loaded = {
            let files = self.files();
            files
                .map
                .get(&path)
                .filter(|f| f.value.is_some())
                .map(|f| f.hooks)
        };
        let hooks = match loaded {
            Some(h) => h,
            None => match load_file(&self.lua, &self.watchdog, &path, FileKind::Script) {
                Ok(h) => h,
                Err(err) => {
                    // Still tracked, so fixing the file starts it.
                    push_error(&self.lua, Some(&path), Some(e), &err.to_string());
                    self.instances
                        .insert(e, self.placeholder(path, script, tick, &err));
                    return;
                }
            },
        };
        match self.make_self(e, &path, script) {
            Ok(this) => {
                self.instances.insert(
                    e,
                    Instance {
                        path: path.clone(),
                        this,
                        started: false,
                        enabled: script.enabled,
                        clock: 0.0,
                        synced: tick,
                        hooks,
                        last_error: None,
                        errors: 0,
                        ms: 0.0,
                    },
                );
                self.start_instance(e, settings);
            }
            Err(err) => {
                push_error(&self.lua, Some(&path), Some(e), &err.to_string());
                self.instances
                    .insert(e, self.placeholder(path, script, tick, &err));
            }
        }
    }

    /// An instance whose script didn't load: no hooks until a reload succeeds.
    fn placeholder(
        &self,
        path: String,
        script: &Script,
        tick: Tick,
        err: &mlua::Error,
    ) -> Instance {
        Instance {
            path,
            this: self
                .lua
                .create_registry_value(Value::Nil)
                .expect("a nil registry value"),
            started: false,
            enabled: script.enabled,
            clock: 0.0,
            synced: tick,
            hooks: Hooks::default(),
            last_error: Some(err.to_string()),
            errors: 1,
            ms: 0.0,
        }
    }

    fn make_self(&self, e: Entity, path: &str, script: &Script) -> mlua::Result<RegistryKey> {
        let lua = &self.lua;
        let (class, meta): (Table, Table) = {
            let files = self.files();
            let f = files
                .map
                .get(path)
                .ok_or_else(|| mlua::Error::runtime("script not loaded"))?;
            (
                lua.registry_value(f.value.as_ref().expect("loaded"))?,
                lua.registry_value(f.meta.as_ref().expect("loaded"))?,
            )
        };
        let this = lua.create_table()?;
        this.raw_set("entity", EntityRef(e))?;
        this.raw_set("props", props_table(lua, &class, &script.props)?)?;
        this.set_metatable(Some(meta))?;
        let selves: Table = lua.named_registry_value(SELVES)?;
        selves.raw_set(e.to_bits() as f64, this.clone())?;
        lua.create_registry_value(this)
    }

    /// Re-reads `props` from the component; true when they changed.
    fn update_props(&mut self, e: Entity, script: &Script) -> bool {
        let Some(i) = self.instances.get(&e) else {
            return false;
        };
        let Ok(Value::Table(this)) = self.lua.registry_value::<Value>(&i.this) else {
            return false;
        };
        let class = {
            let files = self.files();
            files
                .map
                .get(&i.path)
                .and_then(|f| f.value.as_ref())
                .and_then(|k| self.lua.registry_value::<Table>(k).ok())
        };
        let Some(class) = class else { return false };
        match props_table(&self.lua, &class, &script.props) {
            Ok(p) => this.raw_set("props", p).is_ok(),
            Err(_) => false,
        }
    }

    fn start_instance(&mut self, e: Entity, settings: &ScriptSettings) {
        let Some(i) = self.instances.get_mut(&e) else {
            return;
        };
        i.started = true;
        if !i.hooks.start {
            if i.hooks.states {
                let clock = i.clock;
                self.call_hook(e, "__states_start", clock, settings);
            }
            return;
        }
        let states = i.hooks.states;
        let path = i.path.clone();
        let this: Value = match self.lua.registry_value(&i.this) {
            Ok(v) => v,
            Err(_) => return,
        };
        let start: Function = match &this {
            Value::Table(t) => match t.get("start") {
                Ok(f) => f,
                Err(_) => return,
            },
            _ => return,
        };
        // `start` runs as a task, so it may wait.
        self.profiler.phase(Phase::Start);
        match self
            .lua
            .create_thread(crate::debugger::catching(&self.lua, start))
        {
            Ok(thread) => self.resume(thread, Some(e), &path, (this,), settings),
            Err(err) => push_error(&self.lua, Some(&path), Some(e), &err.to_string()),
        }
        self.adopt_new_tasks();
        // The state machine starts once `start` has set things up (or reached its first wait).
        if states && let Some(clock) = self.instances.get(&e).map(|i| i.clock) {
            self.call_hook(e, "__states_start", clock, settings);
        }
    }

    fn destroy_instance(&mut self, e: Entity, settings: &ScriptSettings) {
        let Some(i) = self.instances.get(&e) else {
            return;
        };
        if i.hooks.on_destroy && i.started {
            self.call_hook(e, "on_destroy", (), settings);
        }
        self.tasks.retain(|t| t.owner != Some(e));
        let dropped: Vec<RegistryKey> = {
            let mut s = shared(&self.lua);
            let (gone, kept) = std::mem::take(&mut s.listeners)
                .into_iter()
                .partition(|l| l.owner == Some(e));
            s.listeners = kept;
            gone.into_iter()
                .map(|l: crate::access::Listener| l.func)
                .collect()
        };
        for key in dropped {
            let _ = self.lua.remove_registry_value(key);
        }
        if let Some(i) = self.instances.remove(&e) {
            let _ = self.lua.remove_registry_value(i.this);
        }
        if let Ok(selves) = self.lua.named_registry_value::<Table>(SELVES) {
            let _ = selves.raw_set(e.to_bits() as f64, Value::Nil);
        }
    }

    /// Restarts the script on `e` from scratch (fresh `self`, `start` again), in the VM that
    /// runs it.
    pub(crate) fn restart(&mut self, world: &mut World, e: Entity, settings: &ScriptSettings) {
        if let Some(actor) = self
            .actors
            .iter_mut()
            .find(|a| a.instances.contains_key(&e))
        {
            return actor.restart_here(world, e, settings);
        }
        self.restart_here(world, e, settings);
    }

    fn restart_here(&mut self, world: &mut World, e: Entity, settings: &ScriptSettings) {
        let world_ptr: *mut World = world;
        let _lent = lend_world(&self.lua, world_ptr);
        // From the file as it is now, even if the poller hasn't seen an edit yet.
        // SAFETY: as in `run_frame`.
        self.poll_files(unsafe { &mut *world_ptr }, settings);
        self.destroy_instance(e, settings);
        // SAFETY: as in `run_frame`.
        let world = unsafe { &mut *world_ptr };
        if let Some(script) = world.get::<Script>(e).cloned() {
            let tick = world.change_tick();
            self.create_instance(e, &script, tick, settings);
        }
    }

    /// Calls `self:<hook>(args)` on `e`'s instance with the watchdog armed. Errors are reported
    /// and counted; the script keeps running.
    fn call_hook(
        &mut self,
        e: Entity,
        hook: &str,
        args: impl mlua::IntoLuaMulti,
        settings: &ScriptSettings,
    ) {
        let Some(i) = self.instances.get(&e) else {
            return;
        };
        let Ok(Value::Table(this)) = self.lua.registry_value::<Value>(&i.this) else {
            return;
        };
        let path = i.path.clone();
        let Ok(Value::Function(f)) = this.get::<Value>(hook) else {
            return;
        };
        self.profiler.phase(match hook {
            "update" => Phase::Update,
            "fixed_update" => Phase::FixedUpdate,
            _ => Phase::Events,
        });
        if self.debugger.attached() {
            // In a coroutine, so a breakpoint can stop it in place (and an error where it
            // is raised).
            let f = crate::debugger::catching(&self.lua, f);
            let thread = match self.spare.take() {
                Some(t) if t.reset(f.clone()).is_ok() => t,
                _ => match self.lua.create_thread(f) {
                    Ok(t) => t,
                    Err(err) => return self.report(e, &path, &err),
                },
            };
            let mut args = match args.into_lua_multi(&self.lua) {
                Ok(a) => a,
                Err(err) => return self.report(e, &path, &err),
            };
            args.push_front(Value::Table(this));
            shared(&self.lua).owner = Some(e);
            self.debugger.resuming(&self.lua, &thread);
            self.watchdog.arm(settings.call_limit_ms);
            let result = thread.resume::<MultiValue>(args);
            self.watchdog.disarm();
            self.debugger.resumed();
            self.profiler.settle(&self.lua);
            shared(&self.lua).owner = None;
            self.after_resume(thread, Some(e), &path, result, false);
            self.adopt_new_tasks();
            return;
        }
        shared(&self.lua).owner = Some(e);
        self.watchdog.arm(settings.call_limit_ms);
        let result = f.call::<()>((this, args));
        self.watchdog.disarm();
        self.profiler.settle(&self.lua);
        shared(&self.lua).owner = None;
        if let Err(err) = result {
            self.report(e, &path, &err);
        }
        self.adopt_new_tasks();
    }

    /// Runs `self:<hook>(args)` on `e`'s instance as a task of its own, so an event handler may
    /// wait (`task.wait`) without holding up the frame or the events after it.
    fn spawn_hook(
        &mut self,
        e: Entity,
        hook: &str,
        args: impl mlua::IntoLuaMulti,
        settings: &ScriptSettings,
    ) {
        let Some(i) = self.instances.get(&e) else {
            return;
        };
        let Ok(Value::Table(this)) = self.lua.registry_value::<Value>(&i.this) else {
            return;
        };
        let path = i.path.clone();
        let Ok(Value::Function(f)) = this.get::<Value>(hook) else {
            return;
        };
        let mut args = match args.into_lua_multi(&self.lua) {
            Ok(a) => a,
            Err(err) => return self.report(e, &path, &err),
        };
        args.push_front(Value::Table(this));
        self.profiler.phase(Phase::Events);
        match self
            .lua
            .create_thread(crate::debugger::catching(&self.lua, f))
        {
            Ok(thread) => self.resume(thread, Some(e), &path, args, settings),
            Err(err) => self.report(e, &path, &err),
        }
        self.adopt_new_tasks();
    }

    /// Records an error from `e`'s script, logging it once while it repeats.
    fn report(&mut self, e: Entity, path: &str, err: &mlua::Error) {
        let message = err.to_string();
        note_fault(&self.lua, &message);
        if let Some(i) = self.instances.get_mut(&e) {
            i.errors += 1;
            if i.last_error.as_deref() == Some(message.as_str()) {
                return;
            }
            i.last_error = Some(message.clone());
        }
        push_error(&self.lua, Some(path), Some(e), &message);
    }

    // ------------------------------------------------------------------------- events

    fn deliver_interactions(&mut self, world: &mut World, settings: &ScriptSettings) {
        let Some(messages) =
            world.get_resource::<bevy_ecs::message::Messages<engine_world::Interact>>()
        else {
            return;
        };
        let cursor = self
            .interact_cursor
            .get_or_insert_with(|| messages.get_cursor_current());
        let hits: Vec<(Entity, Entity)> =
            cursor.read(messages).map(|m| (m.target, m.actor)).collect();
        for (target, actor) in hits {
            let Some(hooks) = self
                .instances
                .get(&target)
                .filter(|i| i.started && i.enabled)
                .map(|i| i.hooks)
            else {
                continue;
            };
            if hooks.on_interact {
                self.spawn_hook(target, "on_interact", EntityRef(actor), settings);
            }
            if hooks.states {
                self.call_hook(
                    target,
                    "__states_event",
                    ("interact", EntityRef(actor)),
                    settings,
                );
            }
        }
    }

    fn deliver_contacts(&mut self, world: &mut World, settings: &ScriptSettings) {
        let Some(messages) = world
            .get_resource::<bevy_ecs::message::Messages<engine_world::physics::ContactEvent>>()
        else {
            return;
        };
        let cursor = self
            .contact_cursor
            .get_or_insert_with(|| messages.get_cursor_current());
        let wanted = self.instances.values().any(|i| i.hooks.on_contact);
        let contacts: Vec<engine_world::physics::ContactEvent> =
            cursor.read(messages).copied().collect();
        if !wanted {
            return;
        }
        for c in contacts {
            for (me, other) in [(c.a, c.b), (c.b, c.a)] {
                if !self
                    .instances
                    .get(&me)
                    .is_some_and(|i| i.started && i.enabled && i.hooks.on_contact)
                {
                    continue;
                }
                let info = (|| -> mlua::Result<Table> {
                    let t = self.lua.create_table()?;
                    let started = matches!(c.phase, engine_world::physics::ContactPhase::Started);
                    t.raw_set("phase", if started { "started" } else { "ended" })?;
                    t.raw_set("point", crate::math::Vec3(c.point))?;
                    let n = if me == c.a { c.normal } else { -c.normal };
                    t.raw_set("normal", crate::math::Vec3(n.as_dvec3()))?;
                    t.raw_set("impulse", c.impulse)?;
                    t.raw_set("sensor", c.sensor)?;
                    Ok(t)
                })();
                if let Ok(info) = info {
                    self.spawn_hook(me, "on_contact", (EntityRef(other), info), settings);
                }
            }
        }
    }

    fn deliver_messages(&mut self, settings: &ScriptSettings) {
        let outbox = std::mem::take(&mut shared(&self.lua).outbox);
        for m in outbox {
            let Some(i) = self.instances.get(&m.to) else {
                continue;
            };
            if !i.started {
                continue;
            }
            let data = to_lua(&self.lua, &m.data).unwrap_or(Value::Nil);
            let states = i.hooks.states;
            if i.hooks.on_message {
                self.spawn_hook(
                    m.to,
                    "on_message",
                    (m.event.clone(), data.clone()),
                    settings,
                );
            } else {
                // A message named after one of the class's methods calls it: `door:send("open")`.
                let has_method = self
                    .lua
                    .registry_value::<Value>(&i.this)
                    .ok()
                    .and_then(|v| match v {
                        Value::Table(t) => t.get::<Value>(m.event.as_str()).ok(),
                        _ => None,
                    })
                    .is_some_and(|v| matches!(v, Value::Function(_)));
                if has_method {
                    self.spawn_hook(m.to, &m.event, data.clone(), settings);
                }
            }
            if states {
                // The state machine hears it too: `door:send("open")` can be an `on` event.
                self.call_hook(m.to, "__states_event", (m.event.as_str(), data), settings);
            }
        }
    }

    /// Signals emitted since the last frame (`self:emit`), to the methods wired to them in the
    /// emitter's Script component and to `entity:on` listeners. Each runs as a task of its
    /// own, as Roblox's deferred events: an error in one stops neither the emitter nor the
    /// others.
    fn deliver_signals(&mut self, world: &mut World, settings: &ScriptSettings) {
        // Methods may emit again: those wait for the next frame, so a loop of signals can't
        // hang the game.
        let emitted = std::mem::take(&mut shared(&self.lua).emitted);
        for m in emitted {
            let packed: Option<Table> = self.lua.registry_value(&m.args).ok();
            let _ = self.lua.remove_registry_value(m.args);
            let Some(packed) = packed else { continue };
            let values = unpack(&packed);
            let from_path = self.instances.get(&m.from).map(|i| i.path.clone());
            let wired: Vec<(usize, crate::Connection)> = world
                .get::<Script>(m.from)
                .map(|s| {
                    s.connections
                        .iter()
                        .cloned()
                        .enumerate()
                        .filter(|(_, c)| c.signal == m.signal)
                        .collect()
                })
                .unwrap_or_default();
            let mut spent = Vec::new();
            for (k, c) in wired {
                let target = if c.target.is_empty() {
                    Some(m.from)
                } else {
                    self.named_instance(world, &c.target)
                };
                let problem = match target.and_then(|t| self.instances.get(&t).map(|i| (t, i))) {
                    None => Some(format!(
                        "signal {}: no scene object named \"{}\" runs a script",
                        m.signal, c.target
                    )),
                    Some((t, i)) if !i.started => {
                        let _ = t;
                        None
                    }
                    Some((t, _)) if !self.has_method(t, &c.method) && !self.hears(t, &c.method) => {
                        Some(format!(
                            "signal {}: {}'s script has no method or state event {}",
                            m.signal,
                            if c.target.is_empty() {
                                "this"
                            } else {
                                &c.target
                            },
                            c.method
                        ))
                    }
                    Some((t, _)) => {
                        let args = if c.args.is_empty() {
                            values.clone()
                        } else {
                            c.args
                                .iter()
                                .map(|v| to_lua(&self.lua, v).unwrap_or(Value::Nil))
                                .collect()
                        };
                        if self.has_method(t, &c.method) {
                            self.spawn_hook(t, &c.method, args.clone(), settings);
                        }
                        if self.instances.get(&t).is_some_and(|i| i.hooks.states)
                            && let Ok(event) = self.lua.create_string(&c.method)
                        {
                            let mut with_event = args;
                            with_event.push_front(Value::String(event));
                            self.call_hook(t, "__states_event", with_event, settings);
                        }
                        if c.once {
                            spent.push(k);
                        }
                        None
                    }
                };
                if let (Some(problem), Some(path)) = (problem, &from_path) {
                    self.report(m.from, &path.clone(), &mlua::Error::runtime(problem));
                }
            }
            if !spent.is_empty()
                && let Some(mut s) = world.get_mut::<Script>(m.from)
            {
                let mut k = 0;
                s.connections.retain(|_| {
                    k += 1;
                    !spent.contains(&(k - 1))
                });
            }
            // Functions listening with `entity:on` / `entity:once`.
            let heard: Vec<(Option<Entity>, Function)> = {
                let s = shared(&self.lua);
                s.listeners
                    .iter()
                    .filter(|l| l.from == m.from && l.signal == m.signal)
                    .filter_map(|l| Some((l.owner, self.lua.registry_value(&l.func).ok()?)))
                    .collect()
            };
            {
                let mut s = shared(&self.lua);
                let (spent, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut s.listeners)
                    .into_iter()
                    .partition(|l| l.once && l.from == m.from && l.signal == m.signal);
                s.listeners = kept;
                drop(s);
                for l in spent {
                    let _ = self.lua.remove_registry_value(l.func);
                }
            }
            for (owner, f) in heard {
                if owner.is_some_and(|o| !self.instances.contains_key(&o)) {
                    continue;
                }
                self.spawn_function(owner, f, values.clone(), settings);
            }
        }
    }

    /// The script instance on the scene object named `name` (the first, by entity).
    fn named_instance(&self, world: &World, name: &str) -> Option<Entity> {
        self.instances
            .keys()
            .copied()
            .find(|e| world.get::<Name>(*e).is_some_and(|n| n.as_str() == name))
    }

    /// Whether `e`'s script has a method called `name`.
    fn has_method(&self, e: Entity, name: &str) -> bool {
        self.instances
            .get(&e)
            .and_then(|i| self.lua.registry_value::<Value>(&i.this).ok())
            .and_then(|v| match v {
                Value::Table(t) => t.get::<Value>(name).ok(),
                _ => None,
            })
            .is_some_and(|v| matches!(v, Value::Function(_)))
    }

    /// Whether `e`'s script has a state machine with an `on` transition for `event`.
    fn hears(&self, e: Entity, event: &str) -> bool {
        let Some(i) = self.instances.get(&e).filter(|i| i.hooks.states) else {
            return false;
        };
        let Ok(Value::Table(this)) = self.lua.registry_value::<Value>(&i.this) else {
            return false;
        };
        this.get::<Function>("__states_hears")
            .and_then(|f| f.call::<bool>((this, event)))
            .unwrap_or(false)
    }

    /// Runs `f(args...)` as a task of `owner`'s script (a listener of a signal).
    fn spawn_function(
        &mut self,
        owner: Option<Entity>,
        f: Function,
        args: MultiValue,
        settings: &ScriptSettings,
    ) {
        let path = owner
            .and_then(|o| self.instances.get(&o))
            .map(|i| i.path.clone())
            .unwrap_or_else(|| "console".to_string());
        self.profiler.phase(Phase::Events);
        match self
            .lua
            .create_thread(crate::debugger::catching(&self.lua, f))
        {
            Ok(thread) => self.resume(thread, owner, &path, args, settings),
            Err(err) => {
                if let Some(o) = owner {
                    self.report(o, &path, &err);
                }
            }
        }
        self.adopt_new_tasks();
    }

    // ------------------------------------------------------------------------- tasks

    fn clock_of(&self, owner: Option<Entity>, world_secs: f64) -> f64 {
        match owner {
            Some(e) => self.instances.get(&e).map(|i| i.clock).unwrap_or(0.0),
            None => world_secs,
        }
    }

    /// Moves tasks callbacks started into the schedule, and drops cancelled ones.
    fn adopt_new_tasks(&mut self) {
        let (new, cancelled) = {
            let mut s = shared(&self.lua);
            (
                std::mem::take(&mut s.new_tasks),
                std::mem::take(&mut s.cancelled),
            )
        };
        let world_secs = shared(&self.lua).world_secs;
        for t in new {
            let clock = self.clock_of(t.owner, world_secs);
            let wait = t.delay.or(t.yielded).unwrap_or(0.0).max(0.0);
            if let Some(args) = t.args {
                // Arguments for a delayed task's first run, kept with its thread.
                let _ = t.thread.set_named_registry_args(&self.lua, args);
            }
            self.tasks.push(Task {
                thread: t.thread,
                owner: t.owner,
                wake_at: clock + wait,
                since: clock,
                fresh: t.yielded.is_none(),
            });
        }
        if !cancelled.is_empty() {
            self.tasks.retain(|t| !cancelled.contains(&t.thread));
        }
    }

    /// Resumes the tasks whose wait is over, each once a frame at most (a task that waits 0
    /// seconds comes back next frame). Tasks started during the pass with `task.defer` or
    /// `task.delay(0)` run in it too, up to a limit, so a chain of defers can't stall the frame.
    fn run_tasks(&mut self, world_secs: f64, settings: &ScriptSettings) {
        let mut ran = 0usize;
        let mut first_pass = true;
        loop {
            let mut due = Vec::new();
            let mut i = 0;
            while i < self.tasks.len() {
                let t = &self.tasks[i];
                let paused = t
                    .owner
                    .is_some_and(|e| self.instances.get(&e).is_none_or(|i| !i.enabled));
                let clock = self.clock_of(t.owner, world_secs);
                if paused || clock < t.wake_at || !(first_pass || t.fresh) {
                    i += 1;
                    continue;
                }
                due.push(self.tasks.swap_remove(i));
            }
            if due.is_empty() {
                break;
            }
            // In the order they were due, so the same scripts run the same way every time.
            due.sort_by(|a, b| a.wake_at.total_cmp(&b.wake_at));
            self.profiler.phase(Phase::Tasks);
            let mut due = due.into_iter();
            while let Some(t) = due.next() {
                if self.debugger.stopped() {
                    // They wait with the stopped script, still due.
                    self.tasks.push(t);
                    self.tasks.extend(due);
                    return;
                }
                let clock = self.clock_of(t.owner, world_secs);
                let path = t
                    .owner
                    .and_then(|e| self.instances.get(&e))
                    .map(|i| i.path.clone())
                    .unwrap_or_else(|| "console".into());
                match take_task_args(&self.lua, &t.thread) {
                    Some(args) => self.resume(t.thread, t.owner, &path, args, settings),
                    None => self.resume(t.thread, t.owner, &path, clock - t.since, settings),
                }
                self.adopt_new_tasks();
                ran += 1;
            }
            first_pass = false;
            if ran > 10_000 {
                break;
            }
        }
    }

    /// Resumes `thread` for `owner`; schedules it again when it waits.
    fn resume(
        &mut self,
        thread: Thread,
        owner: Option<Entity>,
        path: &str,
        args: impl mlua::IntoLuaMulti,
        settings: &ScriptSettings,
    ) {
        shared(&self.lua).owner = owner;
        let debugging = self.debugger.attached();
        if debugging {
            self.debugger.resuming(&self.lua, &thread);
        }
        self.watchdog.arm(settings.call_limit_ms);
        let result = thread.resume::<MultiValue>(args);
        self.watchdog.disarm();
        if debugging {
            self.debugger.resumed();
        }
        self.profiler.settle(&self.lua);
        shared(&self.lua).owner = None;
        self.after_resume(thread, owner, path, result, true);
    }

    /// What a resume of one of the runtime's coroutines came to: stopped by the debugger (held
    /// until it says go on), waiting (a task again; a hook can't wait), done, or failed.
    fn after_resume(
        &mut self,
        thread: Thread,
        owner: Option<Entity>,
        path: &str,
        result: mlua::Result<MultiValue>,
        task: bool,
    ) {
        if result.is_ok() && thread.is_resumable() && self.debugger.holds(&self.lua, &thread) {
            self.held = Some(Held {
                thread,
                owner,
                path: path.to_string(),
                task,
            });
            return;
        }
        if !thread.is_resumable() && self.debugger.attached() {
            self.debugger.step_left(&self.lua, &thread);
        }
        if let Err(err) = &result
            && self.debugger.breaks_on_errors()
        {
            // The coroutine's stack is still there (a coroutine that fails keeps its frames).
            self.debugger.stop_on_task_error(&self.lua, &thread, err);
        }
        let report = |rt: &mut Self, err: &mlua::Error| match owner {
            Some(e) => rt.report(e, path, err),
            None => push_error(&rt.lua, Some(path), None, &err.to_string()),
        };
        match result {
            Ok(values) if thread.is_resumable() => {
                if task {
                    let wait = wait_secs(&values);
                    let world_secs = shared(&self.lua).world_secs;
                    let clock = self.clock_of(owner, world_secs);
                    self.tasks.push(Task {
                        thread,
                        owner,
                        wake_at: clock + wait,
                        since: clock,
                        fresh: false,
                    });
                } else {
                    let err = mlua::Error::runtime(
                        "a hook like update can't wait (it runs again next frame); start a \
                         task with task.spawn for things that take time",
                    );
                    report(self, &err);
                }
            }
            Ok(_) => {
                if !task {
                    self.spare = Some(thread);
                }
            }
            Err(err) => report(self, &err),
        }
    }

    // ------------------------------------------------------------------------- hot reload

    /// Checks every loaded file's modification time and reloads those that changed.
    fn poll_files(&mut self, world: &mut World, settings: &ScriptSettings) {
        let _ = world;
        let changed: Vec<String> = {
            let mut files = self
                .lua
                .app_data_mut::<Files>()
                .expect("files are installed");
            let mut out = Vec::new();
            for (path, f) in files.map.iter_mut() {
                let now = stamp(&f.full);
                if now.is_some() && now != f.stamp {
                    f.stamp = now;
                    out.push(path.clone());
                }
            }
            out
        };
        for path in changed {
            self.reload(&path, settings, false);
        }
    }

    /// Reloads `paths` (every loaded file when empty) in every VM that loaded them. Returns
    /// the paths.
    pub(crate) fn reload_all(
        &mut self,
        world: &mut World,
        paths: Vec<String>,
        settings: &ScriptSettings,
    ) -> Vec<String> {
        let paths = if paths.is_empty() {
            let mut all: Vec<String> = self
                .vms()
                .flat_map(|v| v.files().map.keys().cloned().collect::<Vec<_>>())
                .collect();
            all.sort();
            all.dedup();
            all
        } else {
            paths
        };
        let world_ptr: *mut World = world;
        let reload = |vm: &mut ScriptRuntime| {
            let _lent = lend_world(&vm.lua, world_ptr);
            for p in &paths {
                if vm.files().map.contains_key(p) || vm.role == 0 {
                    vm.reload(p, settings, true);
                }
            }
        };
        reload(self);
        for a in &mut self.actors {
            reload(a);
        }
        paths
    }

    /// Reloads one file (and what depends on it). `force` reloads even when the text is the
    /// same.
    pub(crate) fn reload(&mut self, path: &str, settings: &ScriptSettings, force: bool) {
        let path = normalize(path);
        let (full, kind, old_source) = {
            let files = self.files();
            match files.map.get(&path) {
                Some(f) => (f.full.clone(), f.kind, f.source.clone()),
                None => return,
            }
        };
        let source = match std::fs::read_to_string(&full) {
            Ok(s) => s,
            Err(err) => {
                let msg =
                    format!("{path}: can't read the file ({err}); the old version keeps running");
                push_error(&self.lua, Some(&path), None, &msg);
                return;
            }
        };
        let source = match runnable(&path, source) {
            Ok(s) => s,
            Err(err) => {
                let message = format!("{err}\n(the previous version of {path} keeps running)");
                if let Some(f) = self
                    .lua
                    .app_data_mut::<Files>()
                    .expect("files")
                    .map
                    .get_mut(&path)
                {
                    f.error = Some(message.clone());
                }
                push_error(&self.lua, Some(&path), None, &message);
                return;
            }
        };
        if source == old_source && !force {
            // A graph put back as it was (its code didn't change) is fine again.
            if path.ends_with(".graph")
                && let Some(f) = self
                    .lua
                    .app_data_mut::<Files>()
                    .expect("files")
                    .map
                    .get_mut(&path)
            {
                f.error = None;
            }
            return;
        }
        match eval_file(&self.lua, &self.watchdog, &path, &source, kind) {
            Ok((chunk, value)) => {
                self.reloads += 1;
                let hooks = match swap_in(&self.lua, &path, value, source, chunk) {
                    Ok(h) => h,
                    Err(err) => {
                        push_error(&self.lua, Some(&path), None, &err.to_string());
                        return;
                    }
                };
                tracing::info!(target: "script", "reloaded {path}");
                match kind {
                    FileKind::Script => self.refresh_instances(&path, hooks, settings),
                    FileKind::Module => {
                        let users: Vec<String> = shared(&self.lua)
                            .required_by
                            .iter()
                            .filter(|(m, _)| *m == path)
                            .map(|(_, by)| by.clone())
                            .collect();
                        for by in users {
                            self.reload(&by, settings, true);
                        }
                    }
                }
            }
            Err(err) => {
                let message = format!("{err}\n(the previous version of {path} keeps running)");
                if let Some(f) = self
                    .lua
                    .app_data_mut::<Files>()
                    .expect("files")
                    .map
                    .get_mut(&path)
                {
                    f.error = Some(message.clone());
                    f.source = source;
                }
                push_error(&self.lua, Some(&path), None, &message);
            }
        }
    }

    /// After a script's class changed: new hooks, new default props, `on_reload` (or a restart).
    fn refresh_instances(&mut self, path: &str, hooks: Hooks, settings: &ScriptSettings) {
        let users: Vec<Entity> = self
            .instances
            .iter()
            .filter(|(_, i)| i.path == path)
            .map(|(e, _)| *e)
            .collect();
        for e in users {
            let placeholder = self.instances.get(&e).is_some_and(|i| !i.started);
            if hooks.restart_on_reload || placeholder {
                let script = with_world(&self.lua, |w| Ok(w.get::<Script>(e).cloned()))
                    .ok()
                    .flatten();
                self.destroy_instance(e, settings);
                if let Some(script) = script {
                    let tick = with_world(&self.lua, |w| Ok(w.change_tick())).unwrap_or_default();
                    self.create_instance(e, &script, tick, settings);
                }
                continue;
            }
            if let Some(i) = self.instances.get_mut(&e) {
                i.hooks = hooks;
                i.last_error = None;
            }
            self.merge_default_props(e);
            if path.ends_with(".graph") {
                self.stop_removed_timers(e);
            }
            if hooks.on_reload {
                self.call_hook(e, "on_reload", (), settings);
            }
            if hooks.states
                && let Some(clock) = self.instances.get(&e).map(|i| i.clock)
            {
                // Its states as they were, where they still exist (or a start, for new states).
                self.call_hook(e, "__states_reload", clock, settings);
            }
        }
    }

    /// Takes the props this instance doesn't override (in its `Script::props`) from the new
    /// version's defaults, so a default edited in the file shows at once. State belongs in
    /// `self` fields, which a reload keeps.
    fn merge_default_props(&self, e: Entity) {
        let Some(i) = self.instances.get(&e) else {
            return;
        };
        let (Ok(Value::Table(this)), Some(class)) = (
            self.lua.registry_value::<Value>(&i.this),
            self.files()
                .map
                .get(&i.path)
                .and_then(|f| f.value.as_ref())
                .and_then(|k| self.lua.registry_value::<Table>(k).ok()),
        ) else {
            return;
        };
        let (Ok(Value::Table(defaults)), Ok(Value::Table(props))) = (
            class.raw_get::<Value>("props"),
            this.raw_get::<Value>("props"),
        ) else {
            return;
        };
        let overrides = with_world(&self.lua, |w| {
            Ok(w.get::<Script>(e).map(|s| s.props.clone()))
        })
        .ok()
        .flatten()
        .unwrap_or_default();
        for (k, v) in defaults.pairs::<Value, Value>().flatten() {
            let overridden = match &k {
                Value::String(name) => name
                    .to_str()
                    .is_ok_and(|name| overrides.get(&*name).is_some()),
                _ => false,
            };
            if !overridden {
                let _ = props.raw_set(k, deep_copy(&self.lua, &v).unwrap_or(v));
            }
        }
    }

    /// A graph's Every timers are `self.__tick<N>` calling `__every<N>`: after a reload, those
    /// whose method is gone (the node was deleted) stop. The new version's `on_reload` starts
    /// the new ones and restarts the changed ones.
    fn stop_removed_timers(&self, e: Entity) {
        let Some(i) = self.instances.get(&e) else {
            return;
        };
        let (Ok(Value::Table(this)), Some(class)) = (
            self.lua.registry_value::<Value>(&i.this),
            self.files()
                .map
                .get(&i.path)
                .and_then(|f| f.value.as_ref())
                .and_then(|k| self.lua.registry_value::<Table>(k).ok()),
        ) else {
            return;
        };
        let gone: Vec<(String, Value)> = this
            .pairs::<Value, Value>()
            .flatten()
            .filter_map(|(k, t)| {
                let k = k.as_string()?.to_str().ok()?.to_string();
                let n = k.strip_prefix("__tick")?;
                class
                    .raw_get::<Value>(format!("__every{n}"))
                    .is_ok_and(|m| m.is_nil())
                    .then_some((k, t))
            })
            .collect();
        for (k, t) in gone {
            if let Value::Thread(t) = t {
                shared(&self.lua).cancelled.push(t);
            }
            let _ = this.raw_set(k, Value::Nil);
        }
    }

    /// Runs a snippet as the console: with the world, outside any script instance. Tasks it
    /// starts run on world time.
    pub(crate) fn eval(
        &mut self,
        world: &mut World,
        code: &str,
        settings: &ScriptSettings,
    ) -> Result<serde_json::Value, String> {
        let world_ptr: *mut World = world;
        let _lent = lend_world(&self.lua, world_ptr);
        let env = script_env(&self.lua).map_err(|e| e.to_string())?;
        let chunk_src = if code.trim_start().starts_with("return") || code.contains('\n') {
            code.to_string()
        } else {
            // A bare expression: show its value, like a REPL.
            format!("return {code}")
        };
        let func = match self
            .lua
            .load(&chunk_src)
            .set_name("@console")
            .set_environment(env.clone())
            .into_function()
        {
            Ok(f) => f,
            Err(_) => self
                .lua
                .load(code)
                .set_name("@console")
                .set_environment(env)
                .into_function()
                .map_err(|e| e.to_string())?,
        };
        let thread = self.lua.create_thread(func).map_err(|e| e.to_string())?;
        self.profiler.phase(Phase::Console);
        self.watchdog.arm(settings.call_limit_ms);
        let result = thread.resume::<MultiValue>(());
        self.watchdog.disarm();
        self.profiler.settle(&self.lua);
        let out = match result {
            Ok(values) => {
                if thread.is_resumable() {
                    let wait = wait_secs(&values);
                    let now = shared(&self.lua).world_secs;
                    self.tasks.push(Task {
                        thread,
                        owner: None,
                        wake_at: now + wait,
                        since: now,
                        fresh: false,
                    });
                    Ok(serde_json::json!({"waiting": true}))
                } else {
                    let list: Vec<serde_json::Value> = values
                        .iter()
                        .map(|v| {
                            crate::convert::to_json(v)
                                .unwrap_or_else(|_| serde_json::json!(crate::bind::display(v)))
                        })
                        .collect();
                    Ok(match list.len() {
                        0 => serde_json::Value::Null,
                        1 => list.into_iter().next().unwrap_or_default(),
                        _ => serde_json::Value::Array(list),
                    })
                }
            }
            Err(e) => Err(e.to_string()),
        };
        self.adopt_new_tasks();
        out
    }

    /// `script.test`: loads test files (all of them when `files` is empty) into a new run and
    /// starts it as a console task, stopping the last run if it still goes. `file`, `line` and
    /// `filter` pick which of their tests run (the test or `describe` on that line; titles
    /// containing the text).
    pub(crate) fn start_tests(
        &mut self,
        world: &mut World,
        files: Vec<String>,
        pick: (Option<String>, Option<u32>, Option<String>),
        settings: &ScriptSettings,
    ) -> Result<serde_json::Value, String> {
        use crate::testing::{BrokenFile, session, test_files, test_run};
        self.stop_tests();
        let root = self.root();
        let files = if files.is_empty() {
            test_files(&root)
        } else {
            files
        };
        if files.is_empty() {
            return Err(
                "there are no test files: write one as <name>.test.luau in the scripts \
                        folder (examples/lamp.test.luau is one)"
                    .into(),
            );
        }
        let world_ptr: *mut World = world;
        let _lent = lend_world(&self.lua, world_ptr);
        let now = shared(&self.lua).world_secs;
        {
            let mut run = test_run(&self.lua);
            run.id += 1;
            run.running = true;
            run.files = files.clone();
            run.results.clear();
            run.broken.clear();
            run.started = now;
            run.took = 0.0;
        }
        let err = |e: mlua::Error| e.to_string();
        let session = session(&self.lua, script_env(&self.lua).map_err(err)?).map_err(err)?;
        let globals: Function = session.get("globals").map_err(err)?;
        // Each file's describe bodies run now, collecting its tests.
        for path in &files {
            let loaded = std::fs::read_to_string(root.join(path))
                .map_err(|e| mlua::Error::runtime(format!("{path}: {e}")))
                .and_then(|source| {
                    let env = script_env(&self.lua)?;
                    for pair in globals
                        .call::<Table>(path.as_str())?
                        .pairs::<Value, Value>()
                    {
                        let (k, v) = pair?;
                        env.raw_set(k, v)?;
                    }
                    let chunk = self
                        .lua
                        .load(&source)
                        .set_name(format!("@{path}"))
                        .set_environment(env)
                        .into_function()?;
                    shared(&self.lua).loading.push(path.clone());
                    self.watchdog.arm(settings.call_limit_ms.max(1000.0));
                    let result = chunk.call::<()>(());
                    self.watchdog.disarm();
                    shared(&self.lua).loading.pop();
                    result
                });
            if let Err(e) = loaded {
                let message = e.to_string();
                let line = error_location(&message)
                    .filter(|(p, _)| p == path)
                    .map(|(_, l)| l);
                test_run(&self.lua).broken.push(BrokenFile {
                    file: path.clone(),
                    message,
                    line,
                });
            }
        }
        let pick_table = self.lua.create_table().map_err(err)?;
        let (file, line, filter) = pick;
        pick_table.raw_set("file", file).map_err(err)?;
        pick_table.raw_set("line", line).map_err(err)?;
        pick_table.raw_set("filter", filter).map_err(err)?;
        let run: Function = session.get("run").map_err(err)?;
        let thread = self.lua.create_thread(run).map_err(err)?;
        self.profiler.phase(Phase::Console);
        self.watchdog.arm(settings.call_limit_ms);
        let result = thread.resume::<MultiValue>(pick_table);
        self.watchdog.disarm();
        self.profiler.settle(&self.lua);
        match result {
            Ok(values) if thread.is_resumable() => {
                let wait = wait_secs(&values);
                test_run(&self.lua).runner = Some(thread.clone());
                self.tasks.push(Task {
                    thread,
                    owner: None,
                    wake_at: now + wait,
                    since: now,
                    fresh: false,
                });
            }
            Ok(_) => {}
            Err(e) => {
                let mut run = test_run(&self.lua);
                run.running = false;
                run.broken.push(BrokenFile {
                    file: "engine/testing.luau".into(),
                    message: e.to_string(),
                    line: None,
                });
            }
        }
        self.adopt_new_tasks();
        let run = test_run(&self.lua);
        Ok(serde_json::json!({
            "run": run.id,
            "files": run.files.len(),
            "tests": run.results.len(),
            "broken": run.broken,
        }))
    }

    /// Stops the test run if one goes (its tests stay as far as they got).
    pub(crate) fn stop_tests(&mut self) {
        let (runner, worker) = {
            let mut run = crate::testing::test_run(&self.lua);
            if !run.running {
                return;
            }
            run.running = false;
            run.took = shared(&self.lua).world_secs - run.started;
            for r in run.results.iter_mut().filter(|r| r.status == "running") {
                r.status = "stopped".into();
            }
            (run.runner.take(), run.worker.take())
        };
        let gone: Vec<Thread> = runner.into_iter().chain(worker).collect();
        self.tasks.retain(|t| !gone.contains(&t.thread));
        shared(&self.lua).cancelled.extend(gone);
    }

    /// `script.tests`: the current or last test run.
    pub(crate) fn tests(&self) -> serde_json::Value {
        let now = shared(&self.lua).world_secs;
        let mut run = crate::testing::test_run(&self.lua);
        if run.running && run.runner.as_ref().is_some_and(|t| !t.is_resumable()) {
            // The runner itself failed (its error is in the log).
            run.running = false;
            run.took = now - run.started;
            for r in run.results.iter_mut().filter(|r| r.status == "running") {
                r.status = "stopped".into();
            }
        }
        run.describe(now)
    }

    /// The JSON summary `script.list` returns.
    pub(crate) fn describe(&self, world: &World) -> serde_json::Value {
        let files = self.files();
        let file_list: Vec<serde_json::Value> = files
            .map
            .iter()
            .map(|(path, f)| {
                let users: usize = self
                    .vms()
                    .map(|v| v.instances.values().filter(|i| i.path == *path).count())
                    .sum();
                let fault = f.fault.clone().or_else(|| self.fault_of(path));
                serde_json::json!({
                    "path": path,
                    "kind": match f.kind { FileKind::Script => "script", FileKind::Module => "module" },
                    "version": f.version,
                    "error": f.error,
                    "fault": fault.map(|x| serde_json::json!({
                        "line": x.line, "message": x.message, "count": x.count,
                    })),
                    "instances": users,
                })
            })
            .collect();
        let instances: Vec<serde_json::Value> = self
            .vms()
            .flat_map(|v| {
                v.instances.iter().map(move |(e, i)| {
                    serde_json::json!({
                        "entity": e.to_bits(),
                        "name": world.get::<Name>(*e).map(|n| n.as_str().to_string()),
                        "path": i.path,
                        "vm": v.role,
                        "enabled": i.enabled,
                        "running": i.started,
                        "clock_secs": i.clock,
                        "ms": i.ms,
                        "errors": i.errors,
                        "last_error": i.last_error,
                        "tasks": v.tasks.iter().filter(|t| t.owner == Some(*e)).count(),
                    })
                })
            })
            .collect();
        serde_json::json!({
            "root": files.root.display().to_string(),
            "files": file_list,
            "instances": instances,
            "tasks": self.task_count(),
            "vms": self.vm_count(),
            "frame_ms": self.frame_ms,
            "parallel_ms": self.parallel_ms,
            "reloads": self.reloads,
            "memory_kb": self.used_memory() / 1024,
        })
    }
}

/// Seconds a task asked to wait (`task.wait` yields `"wait", seconds`); anything else waits a
/// frame.
fn wait_secs(values: &MultiValue) -> f64 {
    let mut it = values.iter();
    match (it.next(), it.next()) {
        (Some(Value::String(s)), Some(v)) if s.to_str().is_ok_and(|s| &*s == "wait") => match v {
            Value::Integer(i) => *i as f64,
            Value::Number(n) => *n,
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// A new global table for one script: reads fall through to the shared (read-only) globals,
/// writes stay in the script. It is marked safe, like the sandbox's own thread globals: Luau then
/// resolves `math.sin` and the like once and calls built-ins on its fast path (about 2x faster
/// for `math`; the compiler leaves out any built-in a file assigns to itself).
fn script_env(lua: &Lua) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__index", lua.globals())?;
    env.set_metatable(Some(meta))?;
    env.set_safeenv(true);
    Ok(env)
}

/// Compiles and runs `source` as `path`, returning the compiled file and what it returns.
fn eval_file(
    lua: &Lua,
    watchdog: &Watchdog,
    path: &str,
    source: &str,
    kind: FileKind,
) -> mlua::Result<(Function, Value)> {
    let env = script_env(lua)?;
    let chunk = lua
        .load(source)
        .set_name(format!("@{path}"))
        .set_environment(env)
        .into_function()?;
    shared(lua).loading.push(path.to_string());
    watchdog.clock.set_phase(Phase::Reload);
    watchdog.arm(1000.0);
    let result = chunk.call::<Value>(());
    watchdog.disarm();
    shared(lua).loading.pop();
    let value = result?;
    if kind == FileKind::Script && !matches!(value, Value::Table(_)) {
        return Err(mlua::Error::runtime(format!(
            "{path} must end by returning its class table, e.g. `return Lantern` (it returned {})",
            value.type_name()
        )));
    }
    Ok((chunk, value))
}

/// The Luau a file runs: its text, or for a `.graph` file the code it compiles to (an error
/// naming the nodes when the graph has mistakes that stop it).
pub(crate) fn runnable(path: &str, text: String) -> Result<String, String> {
    if !path.ends_with(".graph") {
        return Ok(text);
    }
    let compiled = crate::graph::compile_text(&text, path).map_err(|e| format!("{path}: {e}"))?;
    let errors: Vec<String> = compiled
        .errors()
        .map(|p| match p.node {
            Some(id) => format!("node {id}: {}", p.message),
            None => p.message.clone(),
        })
        .collect();
    if errors.is_empty() {
        Ok(compiled.luau)
    } else {
        Err(format!(
            "{path}: the graph has mistakes: {}",
            errors.join("; ")
        ))
    }
}

/// Loads `path` for the first time; returns its hooks.
fn load_file(lua: &Lua, watchdog: &Watchdog, path: &str, kind: FileKind) -> mlua::Result<Hooks> {
    let full = {
        let files = lua.app_data_ref::<Files>().expect("files");
        if let Some(f) = files.map.get(path)
            && f.value.is_some()
        {
            return Ok(f.hooks);
        }
        files.root.join(path)
    };
    let source = std::fs::read_to_string(&full).map_err(|err| {
        mlua::Error::runtime(format!(
            "can't read script {path} ({err}); scripts live in {}",
            full.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ))
    })?;
    let source = runnable(path, source).map_err(mlua::Error::runtime)?;
    {
        let mut files = lua.app_data_mut::<Files>().expect("files");
        files.map.entry(path.to_string()).or_insert(FileState {
            kind,
            full: full.clone(),
            stamp: stamp(&full),
            source: source.clone(),
            value: None,
            meta: None,
            hooks: Hooks::default(),
            error: None,
            version: 0,
            chunk: None,
            fault: None,
        });
    }
    match eval_file(lua, watchdog, path, &source, kind) {
        Ok((chunk, value)) => swap_in(lua, path, value, source, chunk),
        Err(err) => {
            if let Some(f) = lua
                .app_data_mut::<Files>()
                .expect("files")
                .map
                .get_mut(path)
            {
                f.error = Some(err.to_string());
            }
            Err(err)
        }
    }
}

/// Stores a freshly evaluated value for `path`. A table that replaces a table is copied into the
/// old one, so instances, metatables and other scripts' `require`s see the new code.
fn swap_in(
    lua: &Lua,
    path: &str,
    value: Value,
    source: String,
    chunk: Function,
) -> mlua::Result<Hooks> {
    let old: Option<Value> = {
        let files = lua.app_data_ref::<Files>().expect("files");
        files
            .map
            .get(path)
            .and_then(|f| f.value.as_ref())
            .map(|k| lua.registry_value::<Value>(k))
            .transpose()?
    };
    let kept = match (old, &value) {
        (Some(Value::Table(old)), Value::Table(new)) if !old.is_readonly() => {
            let keys: Vec<Value> = old
                .pairs::<Value, Value>()
                .filter_map(|p| p.ok().map(|(k, _)| k))
                .collect();
            for k in keys {
                old.raw_set(k, Value::Nil)?;
            }
            for pair in new.pairs::<Value, Value>() {
                let (k, v) = pair?;
                old.raw_set(k, v)?;
            }
            Value::Table(old)
        }
        _ => value,
    };
    let hooks = match &kept {
        Value::Table(class) => {
            if class.metatable().is_none() {
                let base: Table = lua.named_registry_value(BASE)?;
                class.set_metatable(Some(base))?;
            }
            Hooks::of(class)
        }
        _ => Hooks::default(),
    };
    {
        let mut files = lua.app_data_mut::<Files>().expect("files");
        let f = files
            .map
            .get_mut(path)
            .ok_or_else(|| mlua::Error::runtime("file vanished"))?;
        if f.meta.is_none()
            && let Value::Table(class) = &kept
        {
            let meta = lua.create_table()?;
            meta.raw_set("__index", class.clone())?;
            f.meta = Some(lua.create_registry_value(meta)?);
        }
        let key = lua.create_registry_value(kept)?;
        if let Some(old) = f.value.replace(key) {
            let _ = lua.remove_registry_value(old);
        }
        f.source = source;
        f.error = None;
        f.fault = None;
        f.version += 1;
        f.hooks = hooks;
        f.chunk = Some(chunk);
    }
    crate::debugger::loaded(lua, path);
    Ok(hooks)
}

/// A copy of a props default, so instances don't share one table. Frozen tables (`vec3`,
/// `quat`, `table.freeze`) can't change, so they are shared as they are.
pub(crate) fn deep_copy(lua: &Lua, v: &Value) -> mlua::Result<Value> {
    match v {
        Value::Table(t) if !t.is_readonly() => {
            let c = lua.create_table()?;
            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair?;
                c.raw_set(k, deep_copy(lua, &v)?)?;
            }
            Ok(Value::Table(c))
        }
        other => Ok(other.clone()),
    }
}

/// `self.props`: the class's `props` defaults with the component's values over them.
fn props_table(lua: &Lua, class: &Table, overrides: &serde_json::Value) -> mlua::Result<Table> {
    let defaults = match class.raw_get::<Value>("props")? {
        Value::Table(t) => t,
        _ => lua.create_table()?,
    };
    let props = match deep_copy(lua, &Value::Table(defaults.clone()))? {
        Value::Table(t) => t,
        _ => lua.create_table()?,
    };
    if let serde_json::Value::Object(map) = overrides {
        for (k, v) in map {
            let d: Value = defaults.raw_get(k.as_str())?;
            props.raw_set(k.as_str(), to_lua_like(lua, v, &d)?)?;
        }
    }
    Ok(props)
}

// ------------------------------------------------------------------------- task functions

/// Arguments for a delayed task's first run.
trait TaskArgs {
    fn set_named_registry_args(&self, lua: &Lua, args: RegistryKey) -> mlua::Result<()>;
}

/// Registry table: thread → packed first-run arguments.
const TASK_ARGS: &str = "engine_script.task_args";

impl TaskArgs for Thread {
    fn set_named_registry_args(&self, lua: &Lua, args: RegistryKey) -> mlua::Result<()> {
        let table: Table = match lua.named_registry_value::<Value>(TASK_ARGS)? {
            Value::Table(t) => t,
            _ => {
                let t = lua.create_table()?;
                // Weak keys: a cancelled task's arguments go with it.
                let meta = lua.create_table()?;
                meta.raw_set("__mode", "k")?;
                t.set_metatable(Some(meta))?;
                lua.set_named_registry_value(TASK_ARGS, t.clone())?;
                t
            }
        };
        let packed: Value = lua.registry_value(&args)?;
        lua.remove_registry_value(args)?;
        table.raw_set(self.clone(), packed)
    }
}

/// A `table.pack`ed table's values.
fn unpack(packed: &Table) -> MultiValue {
    let n: usize = packed.raw_get("n").unwrap_or(0);
    (1..=n)
        .map(|i| packed.raw_get(i).unwrap_or(Value::Nil))
        .collect()
}

/// The packed arguments a delayed task was given, removed once taken.
fn take_task_args(lua: &Lua, thread: &Thread) -> Option<MultiValue> {
    let table: Table = lua.named_registry_value(TASK_ARGS).ok()?;
    let packed: Table = table.raw_get(thread.clone()).ok()?;
    table.raw_set(thread.clone(), Value::Nil).ok()?;
    let n: usize = packed.raw_get("n").unwrap_or(0);
    let mut args = MultiValue::new();
    for i in 1..=n {
        args.push_back(packed.raw_get(i).unwrap_or(Value::Nil));
    }
    Some(args)
}

fn install_tasks(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "__task_spawn",
        lua.create_function(|lua, (f, args): (Function, Variadic<Value>)| {
            let thread = lua.create_thread(crate::debugger::catching(lua, f))?;
            let owner = shared(lua).owner;
            let result = thread.resume::<MultiValue>(MultiValue::from_iter(args));
            // A nested task may have cleared the owner when it finished.
            shared(lua).owner = owner;
            match result {
                Ok(values) => {
                    if thread.is_resumable() {
                        shared(lua).new_tasks.push(NewTask {
                            thread: thread.clone(),
                            owner,
                            delay: None,
                            yielded: Some(wait_secs(&values)),
                            args: None,
                        });
                    }
                }
                Err(err) => {
                    let at = crate::bind::caller(lua);
                    push_error(lua, at.as_deref(), owner, &err.to_string());
                }
            }
            Ok(thread)
        })?,
    )?;
    // A task of `owner`'s script, whoever starts it: a state's enter task belongs to the
    // script whose state it is (its clock, stopped with it) even when a test or another
    // script changed the state.
    g.set(
        "__task_spawn_as",
        lua.create_function(|lua, (owner, f): (EntityRef, Function)| {
            let thread = lua.create_thread(crate::debugger::catching(lua, f))?;
            let before = shared(lua).owner;
            shared(lua).owner = Some(owner.0);
            let result = thread.resume::<MultiValue>(());
            shared(lua).owner = before;
            match result {
                Ok(values) => {
                    if thread.is_resumable() {
                        shared(lua).new_tasks.push(NewTask {
                            thread: thread.clone(),
                            owner: Some(owner.0),
                            delay: None,
                            yielded: Some(wait_secs(&values)),
                            args: None,
                        });
                    }
                }
                Err(err) => {
                    let at = crate::bind::caller(lua);
                    push_error(lua, at.as_deref(), Some(owner.0), &err.to_string());
                }
            }
            Ok(thread)
        })?,
    )?;
    g.set(
        "__task_delay",
        lua.create_function(|lua, (secs, f, args): (f64, Function, Variadic<Value>)| {
            let thread = lua.create_thread(crate::debugger::catching(lua, f))?;
            let packed = lua.create_table()?;
            packed.raw_set("n", args.len())?;
            for (i, a) in args.into_iter().enumerate() {
                packed.raw_set(i + 1, a)?;
            }
            let owner = shared(lua).owner;
            let key = lua.create_registry_value(packed)?;
            shared(lua).new_tasks.push(NewTask {
                thread: thread.clone(),
                owner,
                delay: Some(secs),
                yielded: None,
                args: Some(key),
            });
            Ok(thread)
        })?,
    )?;
    g.set(
        "__script_parallel",
        lua.create_function(|lua, ()| Ok(crate::access::in_parallel(lua)))?,
    )?;
    g.set(
        "__script_emit",
        lua.create_function(|lua, (from, signal, packed): (EntityRef, String, Table)| {
            let args = lua.create_registry_value(packed)?;
            shared(lua).emitted.push(crate::access::Emitted {
                from: from.0,
                signal,
                args,
            });
            Ok(())
        })?,
    )?;
    g.set(
        "__task_cancel",
        lua.create_function(|lua, thread: Thread| {
            shared(lua).cancelled.push(thread);
            Ok(())
        })?,
    )?;
    Ok(())
}

/// `require("lib/tween")`: loads a module from the scripts folder once and returns its value;
/// `./name` is relative to the requiring file. Editing a module reloads the files that use it.
fn install_require(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "require",
        lua.create_function(|lua, name: String| {
            let from = shared(lua).loading.last().cloned().or_else(|| {
                crate::bind::caller(lua).map(|at| at.split(':').next().unwrap_or("").to_string())
            });
            let path = if let Some(rel) = name.strip_prefix("./") {
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
            if shared(lua).loading.contains(&path) {
                return Err(mlua::Error::runtime(format!(
                    "require cycle: {path} is already loading ({})",
                    shared(lua).loading.join(" -> ")
                )));
            }
            if let Some(by) = &from {
                let mut s = shared(lua);
                let pair = (path.clone(), by.clone());
                if !s.required_by.contains(&pair) {
                    s.required_by.push(pair);
                }
            }
            let watchdog = Watchdog::new();
            load_file(lua, &watchdog, &path, FileKind::Module)?;
            let files = lua.app_data_ref::<Files>().expect("files");
            match files.map.get(&path).and_then(|f| f.value.as_ref()) {
                Some(k) => lua.registry_value::<Value>(k),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    Ok(())
}

/// Keeps `errors`/`log` bounded (they are trimmed on push; this is for tests).
#[allow(dead_code)]
pub(crate) const LINES: usize = KEEP_LINES;
