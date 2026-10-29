//! The script debugger: breakpoints (with conditions, hit counts and logpoints), stepping, the
//! call stack, variables and watches for a DAP client attached through [`crate::dap`] (VS Code
//! with `crates/script/vscode`, nvim-dap, Zed), plus logpoints set through the API
//! (`script.logpoint`) whose newest values code boards show beside their lines.
//!
//! How it works (engine-design/scripting-research.md, "Debugger" and "Round 8"): Luau patches a
//! `BREAK` opcode over the first instruction of a breakpoint's line (`lua_breakpoint`) and calls
//! the VM's `debugbreak` callback when it runs, so a line without a breakpoint costs nothing.
//! While a step or a pause is pending, scripts run in Luau's single-step mode and the
//! `debugstep` callback stops at the next line. Logpoints never stop.
//!
//! A stop doesn't freeze the program. While a client is attached the runtime runs every hook
//! and task in a coroutine of its own, and a stop inside one yields it from the VM callback
//! (Luau lets `debugbreak` and `debugstep` yield, and resuming carries on at the same
//! instruction). The script waits at its line, the other scripts wait for it, the world's time
//! stands still (a [`TimeDilation`] hold at 0), and everything else goes on: the editor, its
//! views and this debugger's client, as in Roblox Studio or Unreal's Blueprint debugger, not
//! as in Unity, whose editor freezes with its game. Each stop is a snapshot of the stack (every
//! frame's locals and upvalues), so the client reads it without the stopped code. A stop where
//! a coroutine can't yield (inside a metamethod or an engine function calling back) blocks the
//! scripts while a DAP client answers it, or, for the editor alone, is shown as it was while
//! the code goes on to the end of the frame.
//!
//! Two clients can drive a session: a DAP client ([`crate::dap`]) and the editor in the same
//! process ([`crate::debug`]). The VM callbacks only get a `lua_State`, so sessions are found by
//! the VM's main thread in [`SESSIONS`]; everything else holds a [`Debugger`].
//!
//! [`TimeDilation`]: engine_core::prelude::TimeDilation

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, c_char, c_int};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, MultiValue, Table, Thread, Value, WeakLua, ffi};
use serde_json::{Value as Json, json};

use crate::access::{Watchdog, shared};
use crate::dap::{Incoming, Link};
use crate::runtime::{Files, normalize};

/// Sessions by the address of their VM's main thread.
static SESSIONS: Mutex<Vec<(usize, Arc<Mutex<Session>>)>> = Mutex::new(Vec::new());

/// The session of the VM `l` belongs to.
fn session_of(l: *mut ffi::lua_State) -> Option<Arc<Mutex<Session>>> {
    // SAFETY: `l` is a live thread handed to a VM callback.
    let main = unsafe { ffi::lua_mainthread(l) } as usize;
    let sessions = SESSIONS.lock().ok()?;
    sessions
        .iter()
        .find(|(m, _)| *m == main)
        .map(|(_, s)| s.clone())
}

/// The runtime's handle on its debug session (one per VM). Unregisters it when dropped.
pub(crate) struct Debugger {
    main: usize,
    session: Arc<Mutex<Session>>,
    attached: Arc<AtomicBool>,
    /// The client's "Uncaught errors" filter is on.
    break_errors: Arc<AtomicBool>,
}

/// Lets `load_file` and `reload` (which only have the VM) tell the session about new code.
struct SessionRef(Arc<Mutex<Session>>);

/// Wraps a coroutine's function while the debugger stops on errors, so an error stops where
/// it is raised with the frames' values: a coroutine that fails keeps its frames, but the VM
/// writes the error over the failing frame's first registers (`self` reads nil). Inside a
/// `pcall` the VM calls `debugprotectederror` before anything unwinds; the error goes on
/// unchanged afterwards.
const CATCH: &str = r#"
local pcall, error, pack, unpack = pcall, error, table.pack, table.unpack
return function(f)
	return function(...)
		local r = pack(pcall(f, ...))
		if not r[1] then
			error(r[2], 0)
		end
		return unpack(r, 2, r.n)
	end
end
"#;

/// Where [`CATCH`] lives; its frames are left out of the stack a client sees.
const CATCH_SOURCE: &str = "engine/debugger.luau";

/// [`CATCH`] and when to use it, for [`catching`].
struct Catch {
    bind: Function,
    attached: Arc<AtomicBool>,
    break_errors: Arc<AtomicBool>,
}

/// `f`, made to stop the debugger where an error nothing catches is raised while it stops on
/// errors; `f` itself otherwise.
pub(crate) fn catching(lua: &Lua, f: Function) -> Function {
    let bind = match lua.app_data_ref::<Catch>() {
        Some(c) if c.attached.load(Ordering::Relaxed) && c.break_errors.load(Ordering::Relaxed) => {
            c.bind.clone()
        }
        _ => return f,
    };
    bind.call::<Function>(f.clone()).unwrap_or(f)
}

impl Drop for Debugger {
    fn drop(&mut self) {
        if let Ok(mut sessions) = SESSIONS.lock() {
            sessions.retain(|(m, _)| *m != self.main);
        }
    }
}

impl Debugger {
    /// Sets up debugging for `lua` (before any script loads): full debug info (local names) and
    /// the VM's break callback. Costs nothing until a breakpoint or logpoint is set.
    pub fn install(lua: &Lua, watchdog: Watchdog) -> mlua::Result<Self> {
        lua.set_compiler(mlua::chunk::Compiler::new().set_debug_level(2));
        let mut main = 0usize;
        // SAFETY: only reads the main thread and sets a callback no one else uses (mlua uses
        // `interrupt`, `userthread` and `userdata`).
        unsafe {
            lua.exec_raw::<()>((), |state| {
                let m = ffi::lua_mainthread(state);
                (*ffi::lua_callbacks(m)).debugbreak = Some(debugbreak);
                (*ffi::lua_callbacks(m)).debugprotectederror = Some(protected_error);
                main = m as usize;
            })?;
        }
        let attached = Arc::new(AtomicBool::new(false));
        let break_errors = Arc::new(AtomicBool::new(false));
        let session = Arc::new(Mutex::new(Session {
            lua: lua.weak(),
            main,
            watchdog,
            link: None,
            attached: attached.clone(),
            break_errors: break_errors.clone(),
            breakpoints: BTreeMap::new(),
            next_id: 1,
            dap: false,
            local: false,
            step: None,
            pause_requested: false,
            pause_is_step: false,
            stop: None,
            stops: 0,
            go: false,
            resuming: 0,
            skip_break: None,
            inline: BTreeMap::new(),
            inline_version: 0,
            log_seen: 0,
            errors_seen: 0,
            compiled: HashMap::new(),
        }));
        lua.set_app_data(SessionRef(session.clone()));
        let bind: Function = lua
            .load(CATCH)
            .set_name(format!("@{CATCH_SOURCE}"))
            .eval()?;
        lua.set_app_data(Catch {
            bind,
            attached: attached.clone(),
            break_errors: break_errors.clone(),
        });
        SESSIONS
            .lock()
            .map_err(|_| mlua::Error::runtime("debug sessions poisoned"))?
            .push((main, session.clone()));
        Ok(Self {
            main,
            session,
            attached,
            break_errors,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Starts the DAP server on `port` (0: any free port). Returns the port.
    pub fn listen(&self, port: u16) -> std::io::Result<u16> {
        let mut s = self.lock();
        if let Some(link) = &s.link {
            return Ok(link.port);
        }
        let link = crate::dap::listen(port)?;
        let port = link.port;
        tracing::info!(target: "script", "script debugger listening on 127.0.0.1:{port}");
        s.link = Some(link);
        Ok(port)
    }

    /// The DAP server is up (its requests are handled every frame).
    pub fn listening(&self) -> bool {
        self.lock().link.is_some()
    }

    /// A client is attached (scripts run in single-step mode).
    pub fn attached(&self) -> bool {
        self.attached.load(Ordering::Relaxed)
    }

    /// The client wants to stop on uncaught errors.
    pub fn breaks_on_errors(&self) -> bool {
        self.attached() && self.break_errors.load(Ordering::Relaxed)
    }

    /// A coroutine failed: stops on its stack, which a coroutine keeps after an error.
    pub fn stop_on_task_error(&self, lua: &Lua, thread: &Thread, err: &mlua::Error) {
        let t = state_of(lua, thread);
        if t == 0 {
            return;
        }
        self.lock().stop_here(
            lua,
            t as *mut ffi::lua_State,
            false,
            "exception",
            None,
            Some(err.to_string()),
        );
    }

    /// Before the runtime resumes one of its coroutines: puts it in single-step mode while a
    /// step or pause is pending, and lets a stop inside it wait in place (one in a coroutine
    /// the script resumes itself can't: its `coroutine.resume` would return early).
    pub fn resuming(&self, lua: &Lua, thread: &Thread) {
        let t = self.prepare(lua, thread);
        self.lock().resuming = t;
    }

    /// Puts a coroutine in single-step mode while a step or pause is pending (and out of it
    /// otherwise), so stepping follows it. Returns its `lua_State`.
    pub fn prepare(&self, lua: &Lua, thread: &Thread) -> usize {
        let t = state_of(lua, thread);
        let s = self.lock();
        let stepping = s.step.is_some() || s.pause_requested;
        if t != 0 {
            // SAFETY: a live coroutine of this VM, not running.
            unsafe { ffi::lua_singlestep(t as *mut ffi::lua_State, stepping as c_int) };
        }
        t
    }

    /// After the runtime's resume returned (a breakpoint's second run, on resuming from it,
    /// has happened by then).
    pub fn resumed(&self) {
        let mut s = self.lock();
        s.resuming = 0;
        // A coroutine waiting at a breakpoint runs its BREAK again when it goes on: that one
        // is skipped once (`on_break`).
        if s.stop.is_none() {
            s.skip_break = None;
        }
    }

    /// `thread` yielded because it stopped: it waits at its line for the client.
    pub fn holds(&self, lua: &Lua, thread: &Thread) -> bool {
        let t = state_of(lua, thread);
        self.lock()
            .stop
            .as_ref()
            .is_some_and(|s| s.kind == StopKind::Held && s.thread == t)
    }

    /// Stopped: the scripts wait until the client says go on.
    pub fn stopped(&self) -> bool {
        self.lock().stop.is_some()
    }

    /// The client said go on (continue or a step): ends the stop. Returns whether it did.
    pub fn take_go(&self) -> bool {
        let mut s = self.lock();
        if !std::mem::take(&mut s.go) {
            return false;
        }
        s.stop = None;
        if s.step.is_some() || s.pause_requested {
            s.set_stepping(true);
        }
        true
    }

    /// The stopped coroutine finished (or failed) while a step was under way: the step goes
    /// on to the next line any script runs, as a native debugger steps out of a frame.
    pub fn step_left(&self, lua: &Lua, thread: &Thread) {
        let t = state_of(lua, thread);
        let mut s = self.lock();
        if s.step.as_ref().is_some_and(|st| st.thread == t) {
            s.step = None;
            s.pause_requested = true;
            s.pause_is_step = true;
            s.set_stepping(true);
        }
    }

    /// Handles what the client sent since the last frame. Returns the `evaluate` requests to run
    /// as console code (the scripts aren't stopped, so they need the world).
    pub fn poll(&self, lua: &Lua) -> Vec<Json> {
        self.lock().poll(lua)
    }

    /// Sends script output and errors to the client.
    pub fn flush(&self, lua: &Lua) {
        self.lock().flush_output(lua);
    }

    pub fn respond(&self, request: &Json, result: Result<Json, String>) {
        if let Some(link) = &self.lock().link {
            link.respond(request, result);
        }
    }

    /// `script.debug`'s answer.
    pub fn status(&self) -> Json {
        let s = self.lock();
        json!({
            "port": s.link.as_ref().map(|l| l.port),
            "connected": s.link.as_ref().is_some_and(|l| l.connected()),
            "attached": s.attached.load(Ordering::Relaxed),
            "breakpoints": s.breakpoints.iter().map(|(path, bps)| {
                (path.clone(), Json::Array(bps.iter().map(|b| json!({
                    "line": b.line, "at": b.actual, "hits": b.hits,
                    "log": b.log, "condition": b.condition, "from_api": b.from_api,
                })).collect()))
            }).collect::<serde_json::Map<_, _>>(),
        })
    }

    /// Adds an API logpoint (kept when a client replaces the file's breakpoints).
    pub fn add_logpoint(
        &self,
        lua: &Lua,
        path: &str,
        line: u32,
        message: String,
        log: bool,
    ) -> Json {
        let mut s = self.lock();
        let id = s.next_id;
        s.next_id += 1;
        s.breakpoints
            .entry(path.to_string())
            .or_default()
            .push(Breakpoint {
                id,
                line,
                actual: None,
                condition: None,
                hit_condition: None,
                log: Some(message),
                from_api: true,
                from_editor: false,
                to_log: log,
                hits: 0,
                last: None,
            });
        s.apply(lua, path);
        let at = s.breakpoints[path].last().and_then(|b| b.actual);
        json!({"path": path, "line": line, "at": at, "loaded": at.is_some()})
    }

    /// Removes API logpoints (of one file, or all) and their values on boards.
    pub fn clear_logpoints(&self, lua: &Lua, path: Option<&str>) -> usize {
        let mut s = self.lock();
        let paths: Vec<String> = match path {
            Some(p) => vec![p.to_string()],
            None => s.breakpoints.keys().cloned().collect(),
        };
        let mut removed = 0;
        for p in paths {
            s.unapply(lua, &p);
            if let Some(bps) = s.breakpoints.get_mut(&p) {
                let before = bps.len();
                bps.retain(|b| !b.from_api);
                removed += before - bps.len();
            }
            s.apply(lua, &p);
            s.inline.remove(&p);
        }
        s.inline_version += 1;
        removed
    }

    /// How many times each line of `path` with an API logpoint has run (the graph editor
    /// counts its nodes' runs this way).
    pub fn line_hits(&self, path: &str) -> BTreeMap<u32, u64> {
        let s = self.lock();
        let mut out = BTreeMap::new();
        for b in s.breakpoints.get(path).into_iter().flatten() {
            if b.from_api {
                *out.entry(b.line).or_insert(0) += b.hits;
            }
        }
        out
    }

    /// The newest logpoint values of `path`, by line, and a number that changes with them.
    pub fn inline_values(&self, path: &str) -> (u64, BTreeMap<u32, String>) {
        let s = self.lock();
        (
            s.inline_version,
            s.inline.get(path).cloned().unwrap_or_default(),
        )
    }
}

/// A stop, for the editor ([`crate::debug`]).
#[derive(Clone, Debug, PartialEq)]
pub struct StopView {
    /// Counts stops: a new one, a new number.
    pub id: u64,
    /// "breakpoint", "step", "pause" or "exception".
    pub reason: String,
    /// The error it stopped on.
    pub exception: Option<String>,
    /// The script waits at its line and can step; otherwise the stop is only to look at (its
    /// code went on, or failed).
    pub live: bool,
    /// The Luau frames, innermost first.
    pub frames: Vec<FrameView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameView {
    /// The function's name ("update", "(function)").
    pub name: String,
    /// Its file, relative to the scripts folder.
    pub path: String,
    pub line: u32,
}

/// Where to go from a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Go {
    Continue,
    /// The next line in this function (calls run through).
    StepOver,
    /// The next line that runs, inside a call if there is one.
    StepIn,
    /// Back in the caller.
    StepOut,
}

/// The editor's client of a session ([`crate::debug`] wraps these with the world).
impl Debugger {
    pub fn attach_local(&self) {
        let mut s = self.lock();
        s.local = true;
        s.attach();
    }

    pub fn detach_local(&self, lua: &Lua) {
        let mut s = self.lock();
        s.local = false;
        if s.dap {
            s.drop_breakpoints(lua, |b| !b.from_editor);
        } else {
            s.detach(lua);
        }
    }

    pub fn local(&self) -> bool {
        self.lock().local
    }

    pub fn set_break_on_errors(&self, on: bool) {
        self.break_errors.store(on, Ordering::Relaxed);
    }

    /// Replaces `path`'s editor breakpoints (line, condition). Returns where each landed.
    pub fn set_editor_breakpoints(
        &self,
        lua: &Lua,
        path: &str,
        lines: &[(u32, Option<String>)],
    ) -> Vec<Option<u32>> {
        let mut s = self.lock();
        s.unapply(lua, path);
        let (mut old, mut kept): (Vec<Breakpoint>, Vec<Breakpoint>) = s
            .breakpoints
            .remove(path)
            .unwrap_or_default()
            .into_iter()
            .partition(|b| b.from_editor);
        let first = kept.len();
        for (line, condition) in lines {
            let condition = condition.clone().filter(|c| !c.trim().is_empty());
            // The same breakpoint again keeps its hits.
            if let Some(i) = old
                .iter()
                .position(|b| b.line == *line && b.condition == condition)
            {
                kept.push(old.swap_remove(i));
                continue;
            }
            let id = s.next_id;
            s.next_id += 1;
            kept.push(Breakpoint {
                id,
                line: *line,
                actual: None,
                condition,
                hit_condition: None,
                log: None,
                from_api: false,
                from_editor: true,
                to_log: false,
                hits: 0,
                last: None,
            });
        }
        s.breakpoints.insert(path.to_string(), kept);
        s.apply(lua, path);
        s.breakpoints[path][first..]
            .iter()
            .map(|b| b.actual)
            .collect()
    }

    /// `path`'s editor breakpoints: (line asked for, where it landed, hits).
    pub fn editor_breakpoints(&self, path: &str) -> Vec<(u32, Option<u32>, u64)> {
        self.lock()
            .breakpoints
            .get(path)
            .into_iter()
            .flatten()
            .filter(|b| b.from_editor)
            .map(|b| (b.line, b.actual, b.hits))
            .collect()
    }

    pub fn stop_view(&self) -> Option<StopView> {
        let s = self.lock();
        let stop = s.stop.as_ref()?;
        Some(StopView {
            id: s.stops,
            reason: stop.reason.clone(),
            exception: stop.exception.clone(),
            live: stop.kind != StopKind::Look && stop.exception.is_none(),
            frames: stop
                .frames
                .iter()
                .map(|f| FrameView {
                    name: f.name.clone(),
                    path: f.source.clone(),
                    line: f.line.max(0) as u32,
                })
                .collect(),
        })
    }

    /// Frame `frame`'s scopes, as DAP has them: `{"scopes": [{name, variablesReference}]}`.
    pub fn stop_scopes(&self, frame: usize) -> Result<Json, String> {
        self.lock().scopes(frame)
    }

    /// A scope's or a table's entries: `{"variables": [{name, value, type, variablesReference}]}`.
    pub fn stop_variables(&self, reference: usize) -> Result<Json, String> {
        self.lock().variables(reference)
    }

    /// Runs a watch in frame `frame` of the stop: `{result, variablesReference}`.
    pub fn stop_evaluate(&self, lua: &Lua, frame: usize, expr: &str) -> Result<Json, String> {
        let mut s = self.lock();
        let values = s.evaluate_in(lua, frame, expr)?;
        Ok(s.result_json(values))
    }

    /// Goes on from the stop. Returns false when there's none.
    pub fn go(&self, go: Go) -> bool {
        let mut s = self.lock();
        let step = match go {
            Go::Continue => None,
            Go::StepOver => Some(StepKind::Over),
            Go::StepIn => Some(StepKind::In),
            Go::StepOut => Some(StepKind::Out),
        };
        if s.stop
            .as_ref()
            .is_some_and(|st| st.kind == StopKind::Blocking)
        {
            // Only the DAP client can answer a stop that holds the scripts' thread.
            return false;
        }
        let going = s.proceed(step);
        if going {
            s.go = true;
            if let Some(link) = s.link.as_ref().filter(|l| l.connected()) {
                link.event(
                    "continued",
                    json!({"threadId": 1, "allThreadsContinued": true}),
                );
            }
        }
        going
    }

    /// Stops at the next line any script runs.
    pub fn pause(&self) {
        let mut s = self.lock();
        if s.stop.is_none() {
            s.pause_requested = true;
            s.set_stepping(true);
        }
    }
}

/// Tells the session `path` has new code (loaded or hot reloaded), so its breakpoints move
/// onto it.
pub(crate) fn loaded(lua: &Lua, path: &str) {
    let Some(session) = lua.app_data_ref::<SessionRef>().map(|s| s.0.clone()) else {
        return;
    };
    let Ok(mut s) = session.try_lock() else {
        return;
    };
    if s.breakpoints.contains_key(path) {
        s.apply(lua, path);
        s.announce(path);
    }
}

struct Breakpoint {
    id: i64,
    /// Line asked for (1-based).
    line: u32,
    /// Where Luau put it (the next line with code), once the file is loaded.
    actual: Option<u32>,
    condition: Option<String>,
    hit_condition: Option<String>,
    /// A logpoint's message: `{expressions}` in it are filled in, and it never stops.
    log: Option<String>,
    /// Set through `script.logpoint`; kept when a client replaces the file's breakpoints.
    from_api: bool,
    /// Set in the editor ([`crate::debug`]); kept when a DAP client replaces the file's.
    from_editor: bool,
    /// An API logpoint also writes to the script log (DAP logpoints go to the client).
    to_log: bool,
    hits: u64,
    /// When it last filled in its message (board-only logpoints do so 20 times a second at
    /// most, however often the line runs).
    last: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StepKind {
    In,
    Over,
    Out,
}

struct Step {
    thread: usize,
    depth: c_int,
    line: c_int,
    kind: StepKind,
}

/// Something the client can expand while stopped (frames by their index in the stop).
enum Var {
    Locals(usize),
    Upvalues(usize),
    Value(Value),
}

/// How a stop holds the scripts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StopKind {
    /// The runtime's coroutine yielded at the line and waits there: it can step.
    Held,
    /// The scripts' thread waits in the VM callback while a DAP client answers (a stop where
    /// no coroutine could yield).
    Blocking,
    /// Only to look at: the code went on (or its coroutine failed); the scripts wait after it.
    Look,
}

/// One frame of a stop, as it was when it stopped.
struct FrameSnap {
    name: String,
    source: String,
    line: c_int,
    locals: Vec<(String, Value)>,
    upvalues: Vec<(String, Value)>,
    /// The script's globals, for watches.
    globals: Option<Table>,
}

struct Stop {
    kind: StopKind,
    reason: String,
    /// The stopped thread's `lua_State`.
    thread: usize,
    /// The Luau frames, innermost first.
    frames: Vec<FrameSnap>,
    vars: Vec<Var>,
    /// The error it stopped on.
    exception: Option<String>,
}

pub(crate) struct Session {
    lua: WeakLua,
    main: usize,
    watchdog: Watchdog,
    link: Option<Link>,
    attached: Arc<AtomicBool>,
    break_errors: Arc<AtomicBool>,
    /// A DAP client attached.
    dap: bool,
    /// The editor attached ([`crate::debug`]).
    local: bool,
    breakpoints: BTreeMap<String, Vec<Breakpoint>>,
    next_id: i64,
    step: Option<Step>,
    pause_requested: bool,
    /// The pending pause ends a step that left its frame (it stops as a step).
    pause_is_step: bool,
    stop: Option<Stop>,
    /// Stops so far.
    stops: u64,
    /// The client said go on from a stop the runtime holds ([`Debugger::take_go`]).
    go: bool,
    /// The coroutine the runtime is resuming: a stop in it can wait in place.
    resuming: usize,
    /// A breakpoint stopped at in place runs again when its coroutine resumes: not a new hit.
    skip_break: Option<(usize, c_int)>,
    inline: BTreeMap<String, BTreeMap<u32, String>>,
    inline_version: u64,
    log_seen: u64,
    errors_seen: u64,
    /// Watch, condition and logpoint expressions, compiled once (each run gets the frame's
    /// scope as its environment).
    compiled: HashMap<String, Function>,
}

unsafe extern "C-unwind" fn debugbreak(l: *mut ffi::lua_State, ar: *mut ffi::lua_Debug) {
    // SAFETY: Luau passes a valid record.
    let line = unsafe { (*ar).currentline };
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(session) = session_of(l) else { return };
        // Taken while stopped or evaluating: a breakpoint hit by the debugger's own code.
        let Ok(mut s) = session.try_lock() else {
            return;
        };
        s.on_break(l, line);
    }));
}

/// An error inside a `pcall` in a coroutine, before anything unwinds: when it is one nothing
/// in the script catches (the `pcall` is [`CATCH`]'s), stops on it with the frames' values.
unsafe extern "C-unwind" fn protected_error(l: *mut ffi::lua_State) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(session) = session_of(l) else { return };
        let Ok(mut s) = session.try_lock() else {
            return;
        };
        if !(s.attached.load(Ordering::Relaxed) && s.break_errors.load(Ordering::Relaxed))
            || !uncaught(l)
        {
            return;
        }
        let Some(lua) = s.lua.try_upgrade() else {
            return;
        };
        // SAFETY: the error object is on top of the stack.
        let text = unsafe { cstr(ffi::lua_tostring(l, -1)) }.unwrap_or_else(|| "error".into());
        s.stop_here(&lua, l, false, "exception", None, Some(text));
    }));
}

/// Whether the `pcall` that will catch the error in `l` is [`CATCH`]'s (not the script's own).
fn uncaught(l: *mut ffi::lua_State) -> bool {
    // SAFETY: lua_getinfo returns 0 past the last level.
    unsafe {
        let mut pcall_seen = false;
        for level in 0..200 {
            let mut ar: ffi::lua_Debug = std::mem::zeroed();
            if ffi::lua_getinfo(l, level, c"sn".as_ptr(), &mut ar) == 0 {
                return false;
            }
            if pcall_seen {
                let source = cstr(ar.source).unwrap_or_default();
                return source.strip_prefix('@').unwrap_or(&source) == CATCH_SOURCE;
            }
            let c = cstr(ar.what).as_deref() == Some("C");
            let name = cstr(ar.name).unwrap_or_default();
            pcall_seen = c && (name == "pcall" || name == "xpcall");
        }
    }
    false
}

unsafe extern "C-unwind" fn debugstep(l: *mut ffi::lua_State, ar: *mut ffi::lua_Debug) {
    // SAFETY: Luau passes a valid record.
    let line = unsafe { (*ar).currentline };
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let Some(session) = session_of(l) else { return };
        let Ok(mut s) = session.try_lock() else {
            return;
        };
        s.on_step(l, line);
    }));
}

impl Session {
    fn main(&self) -> *mut ffi::lua_State {
        self.main as *mut ffi::lua_State
    }

    fn root(lua: &Lua) -> PathBuf {
        lua.app_data_ref::<Files>()
            .map(|f| f.root.clone())
            .unwrap_or_default()
    }

    fn connected(&self) -> bool {
        self.link.as_ref().is_some_and(|l| l.connected())
    }

    // ---- breakpoints in the VM ----

    /// Sets `path`'s breakpoints on its current code.
    fn apply(&mut self, lua: &Lua, path: &str) {
        let chunk = chunk_of(lua, path);
        let Some(bps) = self.breakpoints.get_mut(path) else {
            return;
        };
        for bp in bps {
            bp.actual = chunk
                .as_ref()
                .and_then(|f| set_breakpoint(lua, f, bp.line, true));
        }
    }

    /// Takes `path`'s breakpoints off its current code.
    fn unapply(&mut self, lua: &Lua, path: &str) {
        let Some(chunk) = chunk_of(lua, path) else {
            return;
        };
        for bp in self.breakpoints.get(path).into_iter().flatten() {
            if let Some(at) = bp.actual {
                set_breakpoint(lua, &chunk, at, false);
            }
        }
    }

    /// Tells the client where `path`'s breakpoints ended up.
    fn announce(&self, path: &str) {
        let Some(link) = self.link.as_ref().filter(|l| l.connected()) else {
            return;
        };
        for bp in self.breakpoints.get(path).into_iter().flatten() {
            if !bp.from_api && !bp.from_editor {
                link.event(
                    "breakpoint",
                    json!({"reason": "changed", "breakpoint": bp_json(bp, path)}),
                );
            }
        }
    }

    /// The `debugstep` callback and the main thread's single-step mode (coroutines are
    /// switched as the runtime resumes them, [`Debugger::resuming`]).
    fn set_stepping(&self, on: bool) {
        // SAFETY: the main thread lives as long as the session is registered; this only writes
        // a callback pointer and a flag in the VM's state.
        unsafe {
            (*ffi::lua_callbacks(self.main())).debugstep = if on { Some(debugstep) } else { None };
            ffi::lua_singlestep(self.main(), on as c_int);
        }
    }

    fn attach(&mut self) {
        self.attached.store(true, Ordering::Relaxed);
    }

    /// A client left. With none left, its breakpoints go and the scripts run at full speed
    /// again; the other client's stay.
    fn detach(&mut self, lua: &Lua) {
        self.dap = false;
        let keep_editor = self.local;
        if !keep_editor {
            self.attached.store(false, Ordering::Relaxed);
            self.break_errors.store(false, Ordering::Relaxed);
            self.step = None;
            self.pause_requested = false;
            self.set_stepping(false);
            // A stop nobody can answer goes on.
            if self
                .stop
                .as_ref()
                .is_some_and(|s| s.kind != StopKind::Blocking)
            {
                self.go = true;
            }
        }
        self.drop_breakpoints(lua, |b| b.from_api || (keep_editor && b.from_editor));
    }

    /// Keeps only the breakpoints `keep` says to.
    fn drop_breakpoints(&mut self, lua: &Lua, keep: impl Fn(&Breakpoint) -> bool) {
        let paths: Vec<String> = self.breakpoints.keys().cloned().collect();
        for path in paths {
            self.unapply(lua, &path);
            if let Some(bps) = self.breakpoints.get_mut(&path) {
                bps.retain(&keep);
            }
            self.apply(lua, &path);
        }
        self.breakpoints.retain(|_, b| !b.is_empty());
    }

    // ---- VM callbacks ----

    fn on_break(&mut self, l: *mut ffi::lua_State, line: c_int) {
        let Some(lua) = self.lua.try_upgrade() else {
            return;
        };
        if self.skip_break == Some((l as usize, line)) {
            self.skip_break = None;
            return;
        }
        let path = source_at(l, 0).unwrap_or_default();
        // A client's breakpoint and an API logpoint may share a line.
        let ids: Vec<i64> = self
            .breakpoints
            .get(&path)
            .into_iter()
            .flatten()
            .filter(|b| b.actual == Some(line as u32))
            .map(|b| b.id)
            .collect();
        // None: left behind in code that has since been reloaded, or removed.
        let mut stop = None;
        for id in ids {
            if self.hit(&lua, l, &path, line, id) {
                stop = Some(id);
            }
        }
        if let Some(id) = stop
            && self.attached.load(Ordering::Relaxed)
        {
            self.stop_here(&lua, l, true, "breakpoint", Some(id), None);
        }
    }

    /// Counts a hit of breakpoint `id`; logs a logpoint. Returns whether it should stop.
    fn hit(&mut self, lua: &Lua, l: *mut ffi::lua_State, path: &str, line: c_int, id: i64) -> bool {
        let Some(bp) = self
            .breakpoints
            .get_mut(path)
            .and_then(|bps| bps.iter_mut().find(|b| b.id == id))
        else {
            return false;
        };
        bp.hits += 1;
        if bp.from_api && !bp.to_log {
            if bp
                .last
                .is_some_and(|t| t.elapsed() < Duration::from_millis(50))
            {
                return false;
            }
            bp.last = Some(Instant::now());
        }
        let hits = bp.hits;
        let condition = bp.condition.clone();
        let hit_condition = bp.hit_condition.clone();
        let log = bp.log.clone().map(|m| (m, bp.from_api, bp.to_log));
        if let Some(h) = hit_condition
            && !hit_matches(&h, hits)
        {
            return false;
        }
        if let Some(c) = condition {
            match self.evaluate_at(lua, l, 0, &c) {
                Ok(v) if truthy(&v) => {}
                Ok(_) => return false,
                Err(err) => self.output(
                    "stderr",
                    &format!("breakpoint condition `{c}` at {path}:{line} failed: {err}\n"),
                ),
            }
        }
        let Some((message, from_api, to_log)) = log else {
            return true;
        };
        let text = self.interpolate(lua, l, &message);
        if from_api {
            self.inline
                .entry(path.to_string())
                .or_default()
                .insert(line as u32, text.clone());
            self.inline_version += 1;
            if to_log {
                crate::access::push_log(lua, format!("{path}:{line}: {text}"));
            }
        } else {
            self.output_at("console", &format!("{text}\n"), lua, path, line);
        }
        false
    }

    fn on_step(&mut self, l: *mut ffi::lua_State, line: c_int) {
        let Some(lua) = self.lua.try_upgrade() else {
            return;
        };
        if self.pause_requested {
            let reason = if self.pause_is_step { "step" } else { "pause" };
            self.stop_here(&lua, l, true, reason, None, None);
            return;
        }
        let Some(step) = &self.step else {
            return;
        };
        if l as usize != step.thread {
            return;
        }
        // SAFETY: `l` is the running thread.
        let depth = unsafe { ffi::lua_stackdepth(l) };
        let stop = match step.kind {
            StepKind::In => depth != step.depth || line != step.line,
            StepKind::Over => depth < step.depth || (depth == step.depth && line != step.line),
            StepKind::Out => depth < step.depth,
        };
        if stop {
            self.stop_here(&lua, l, true, "step", None, None);
        }
    }

    /// Stops at `l`'s current line (`exception`: the error it stopped on). `inside`: called
    /// from a VM callback while `l` runs. Inside the coroutine the runtime is resuming, it
    /// yields and waits there; where it can't, a DAP client is served here until it says go
    /// on; otherwise the stop is a snapshot to look at and the code goes on.
    fn stop_here(
        &mut self,
        lua: &Lua,
        l: *mut ffi::lua_State,
        inside: bool,
        reason: &str,
        bp: Option<i64>,
        exception: Option<String>,
    ) {
        if self.dap && !self.connected() {
            self.detach(lua);
        }
        if !self.attached.load(Ordering::Relaxed) || self.stop.is_some() {
            return;
        }
        self.set_stepping(false);
        self.step = None;
        self.pause_requested = false;
        self.pause_is_step = false;
        // SAFETY: `l` is the running thread (inside) or a stopped coroutine.
        let yieldable = inside && unsafe { ffi::lua_isyieldable(l) } != 0;
        let kind = if inside && l as usize == self.resuming && yieldable {
            StopKind::Held
        } else if inside && self.connected() {
            StopKind::Blocking
        } else {
            StopKind::Look
        };
        let frames = snapshot(lua, l);
        let (path, line) = frames
            .first()
            .map(|f| (f.source.clone(), f.line))
            .unwrap_or_default();
        let mut body = json!({"reason": reason, "threadId": 1, "allThreadsStopped": true});
        if let Some(id) = bp {
            body["hitBreakpointIds"] = json!([id]);
        }
        if let Some(text) = &exception {
            body["description"] = json!("Paused on an error");
            body["text"] = json!(text.lines().next().unwrap_or_default());
        }
        self.stops += 1;
        self.stop = Some(Stop {
            kind,
            reason: reason.to_string(),
            thread: l as usize,
            frames,
            vars: Vec::new(),
            exception,
        });
        self.go = false;
        self.flush_output(lua);
        if let Some(link) = self.link.as_ref().filter(|l| l.connected()) {
            link.event("stopped", body);
        }
        tracing::info!(target: "script", "script debugger: stopped ({reason}, {kind:?}) at {path}:{line}");
        match kind {
            StopKind::Held => {
                // Resuming runs this instruction again: its breakpoint isn't a new hit.
                self.skip_break = Some((l as usize, current_line(l, 0)));
                // SAFETY: inside a debugbreak or debugstep callback of `l`, which Luau lets
                // yield; nothing was pushed.
                unsafe { ffi::lua_yield(l, 0) };
            }
            StopKind::Look => {}
            StopKind::Blocking => self.serve_blocking(lua),
        }
    }

    /// Answers the client while the scripts' thread waits in a VM callback.
    fn serve_blocking(&mut self, lua: &Lua) {
        let held = self.watchdog.suspend();
        while let Some(message) = self
            .link
            .as_ref()
            .map(|l| l.recv(Duration::from_millis(200)))
        {
            match message {
                Some(Incoming::Request(request)) => {
                    if self.handle_stopped(lua, &request) {
                        break;
                    }
                }
                Some(Incoming::Closed) => {
                    self.detach(lua);
                    break;
                }
                Some(Incoming::Connected) => {}
                None if !self.connected() => {
                    self.detach(lua);
                    break;
                }
                None => {}
            }
        }
        self.stop = None;
        self.go = false;
        self.watchdog.resume(held);
        if self.step.is_some() || self.pause_requested {
            self.set_stepping(true);
        }
    }

    // ---- requests ----

    fn reply(&self, request: &Json, result: Result<Json, String>) {
        if let Some(link) = &self.link {
            link.respond(request, result);
        }
    }

    /// Once a frame: requests while the scripts run, or while a stop the runtime holds waits.
    fn poll(&mut self, lua: &Lua) -> Vec<Json> {
        let mut evals = Vec::new();
        while let Some(message) = self.link.as_ref().and_then(|l| l.recv(Duration::ZERO)) {
            match message {
                Incoming::Connected => {
                    self.log_seen = shared(lua).log_total;
                    self.errors_seen = shared(lua).errors_total;
                }
                Incoming::Closed => self.detach(lua),
                Incoming::Request(request) if self.stop.is_some() => {
                    if self.handle_stopped(lua, &request) {
                        self.go = true;
                    }
                }
                Incoming::Request(request) => {
                    let command = request["command"].as_str().unwrap_or_default();
                    match command {
                        "evaluate" => evals.push(request),
                        "pause" => {
                            self.pause_requested = true;
                            self.set_stepping(true);
                            self.reply(&request, Ok(Json::Null));
                        }
                        "continue" => {
                            self.reply(&request, Ok(json!({"allThreadsContinued": true})));
                        }
                        "next" | "stepIn" | "stepOut" => self.reply(&request, Ok(Json::Null)),
                        "stackTrace" => {
                            self.reply(&request, Ok(json!({"stackFrames": [], "totalFrames": 0})))
                        }
                        "scopes" | "variables" => {
                            self.reply(&request, Err("the scripts are running".into()))
                        }
                        "disconnect" => {
                            self.detach(lua);
                            self.reply(&request, Ok(Json::Null));
                        }
                        _ => self.handle_common(lua, &request),
                    }
                }
            }
        }
        if self.dap && !self.connected() {
            self.detach(lua);
        }
        self.flush_output(lua);
        evals
    }

    /// Go on from the stop: `step` from where it stands. Returns false when there's no stop.
    fn proceed(&mut self, step: Option<StepKind>) -> bool {
        let Some(stop) = &self.stop else {
            return false;
        };
        // An error unwinds the stack, and a stop to look at has gone on already: neither has a
        // next line to step to.
        if let Some(kind) = step
            && stop.exception.is_none()
            && stop.kind != StopKind::Look
        {
            let thread = stop.thread;
            let line = stop.frames.first().map_or(0, |f| f.line);
            self.step = Some(Step {
                thread,
                // SAFETY: the stopped thread (waiting in a callback, or suspended in place).
                depth: unsafe { ffi::lua_stackdepth(thread as *mut ffi::lua_State) },
                line,
                kind,
            });
        }
        true
    }

    /// While stopped. Returns true when the scripts should go on.
    fn handle_stopped(&mut self, lua: &Lua, request: &Json) -> bool {
        let command = request["command"].as_str().unwrap_or_default();
        let args = &request["arguments"];
        match command {
            "continue" => {
                self.reply(request, Ok(json!({"allThreadsContinued": true})));
                self.proceed(None)
            }
            "exceptionInfo" => {
                let text = self
                    .stop
                    .as_ref()
                    .and_then(|s| s.exception.clone())
                    .unwrap_or_default();
                let first = text.lines().next().unwrap_or_default().to_string();
                self.reply(
                    request,
                    Ok(json!({
                        "exceptionId": "error",
                        "description": first,
                        "breakMode": "unhandled",
                        "details": {"message": first, "typeName": "error",
                            "stackTrace": script_traceback(&text)},
                    })),
                );
                false
            }
            "next" | "stepIn" | "stepOut" => {
                let kind = match command {
                    "next" => StepKind::Over,
                    "stepIn" => StepKind::In,
                    _ => StepKind::Out,
                };
                self.reply(request, Ok(Json::Null));
                self.proceed(Some(kind))
            }
            "pause" => {
                self.reply(request, Ok(Json::Null));
                false
            }
            "disconnect" => {
                self.detach(lua);
                self.reply(request, Ok(Json::Null));
                true
            }
            "stackTrace" => {
                let frames = self.stack_trace(lua);
                let total = frames.len();
                let start = args["startFrame"].as_u64().unwrap_or(0) as usize;
                let levels = args["levels"].as_u64().filter(|n| *n > 0).unwrap_or(1000) as usize;
                let frames: Vec<Json> = frames.into_iter().skip(start).take(levels).collect();
                self.reply(
                    request,
                    Ok(json!({"stackFrames": frames, "totalFrames": total})),
                );
                false
            }
            "scopes" => {
                let result = self.scopes(args["frameId"].as_u64().unwrap_or(0) as usize);
                self.reply(request, result);
                false
            }
            "variables" => {
                let reference = args["variablesReference"].as_u64().unwrap_or(0) as usize;
                let result = self.variables(reference);
                self.reply(request, result);
                false
            }
            "evaluate" => {
                let expr = args["expression"].as_str().unwrap_or_default();
                let frame = args["frameId"].as_u64().unwrap_or(0) as usize;
                let result = self
                    .evaluate_in(lua, frame, expr)
                    .map(|values| self.result_json(values));
                self.reply(request, result);
                self.flush_output(lua);
                false
            }
            _ => {
                self.handle_common(lua, request);
                false
            }
        }
    }

    /// Requests answered the same way running or stopped.
    fn handle_common(&mut self, lua: &Lua, request: &Json) {
        let command = request["command"].as_str().unwrap_or_default();
        match command {
            "initialize" => {
                self.reply(
                    request,
                    Ok(json!({
                        "supportsConfigurationDoneRequest": true,
                        "supportsConditionalBreakpoints": true,
                        "supportsHitConditionalBreakpoints": true,
                        "supportsLogPoints": true,
                        "supportsEvaluateForHovers": true,
                        "supportsExceptionInfoRequest": true,
                        "exceptionBreakpointFilters": [{
                            "filter": "uncaught",
                            "label": "Uncaught errors",
                            "description": "Stop where a script error nothing catches is \
                                raised, with its stack and locals, before the script's call \
                                fails.",
                            "default": true,
                        }],
                        "supportTerminateDebuggee": false,
                    })),
                );
                if let Some(link) = &self.link {
                    link.event("initialized", Json::Null);
                }
            }
            "attach" | "launch" => {
                self.dap = true;
                self.attach();
                self.reply(request, Ok(Json::Null));
                let root = Self::root(lua);
                self.output(
                    "console",
                    &format!(
                        "Attached to the game's scripts ({}). Breakpoints, logpoints and \
                         stepping work in .luau files under that folder; while stopped, the \
                         scripts wait and the world's time stands still.\n",
                        std::path::absolute(&root).unwrap_or(root).display()
                    ),
                );
            }
            "setBreakpoints" => {
                let result = self.set_breakpoints(lua, &request["arguments"]);
                self.reply(request, result);
            }
            "setExceptionBreakpoints" => {
                let filters: Vec<&str> = request["arguments"]["filters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str())
                    .collect();
                self.break_errors
                    .store(filters.contains(&"uncaught"), Ordering::Relaxed);
                let list: Vec<Json> = filters.iter().map(|_| json!({"verified": true})).collect();
                self.reply(request, Ok(json!({"breakpoints": list})));
            }
            "configurationDone" => self.reply(request, Ok(Json::Null)),
            "threads" => self.reply(
                request,
                Ok(json!({"threads": [{"id": 1, "name": "Scripts"}]})),
            ),
            _ => self.reply(
                request,
                Err(format!("the script debugger doesn't support '{command}'")),
            ),
        }
    }

    fn set_breakpoints(&mut self, lua: &Lua, args: &Json) -> Result<Json, String> {
        let root = Self::root(lua);
        let given = args["source"]["path"]
            .as_str()
            .ok_or("setBreakpoints needs source.path")?;
        let Some(path) = relative(&root, given) else {
            let list: Vec<Json> = args["breakpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|b| {
                    json!({"verified": false, "line": b["line"], "message":
                        format!("not a script: scripts live under {}", root.display())})
                })
                .collect();
            return Ok(json!({"breakpoints": list}));
        };
        self.unapply(lua, &path);
        let mut kept: Vec<Breakpoint> = self
            .breakpoints
            .remove(&path)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.from_api || b.from_editor)
            .collect();
        let text = |b: &Json, key: &str| {
            b[key]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let mut ids = Vec::new();
        for b in args["breakpoints"].as_array().into_iter().flatten() {
            let id = self.next_id;
            self.next_id += 1;
            ids.push(id);
            kept.push(Breakpoint {
                id,
                line: b["line"].as_u64().unwrap_or(1) as u32,
                actual: None,
                condition: text(b, "condition"),
                hit_condition: text(b, "hitCondition"),
                log: text(b, "logMessage"),
                from_api: false,
                from_editor: false,
                to_log: false,
                hits: 0,
                last: None,
            });
        }
        self.breakpoints.insert(path.clone(), kept);
        self.apply(lua, &path);
        let list: Vec<Json> = self.breakpoints[&path]
            .iter()
            .filter(|b| ids.contains(&b.id))
            .map(|b| bp_json(b, &path))
            .collect();
        Ok(json!({"breakpoints": list}))
    }

    fn stack_trace(&self, lua: &Lua) -> Vec<Json> {
        let root = std::path::absolute(Self::root(lua)).unwrap_or_default();
        let Some(stop) = &self.stop else {
            return Vec::new();
        };
        stop.frames
            .iter()
            .enumerate()
            .map(|(id, f)| {
                let mut source = json!({"name": f.source});
                if f.source.ends_with(".luau") || f.source.ends_with(".lua") {
                    if f.source.starts_with("engine/") {
                        source["presentationHint"] = json!("deemphasize");
                    } else {
                        source["path"] = json!(root.join(&f.source).display().to_string());
                    }
                }
                json!({
                    "id": id,
                    "name": f.name,
                    "source": source,
                    "line": f.line.max(1),
                    "column": 1,
                })
            })
            .collect()
    }

    /// A reference to `var` for the client (the same one each time it's asked for).
    fn keep(&mut self, var: Var) -> usize {
        let Some(stop) = &mut self.stop else { return 0 };
        let same = |a: &Var| match (a, &var) {
            (Var::Locals(x), Var::Locals(y)) | (Var::Upvalues(x), Var::Upvalues(y)) => x == y,
            (Var::Value(x), Var::Value(y)) => x.to_pointer() == y.to_pointer(),
            _ => false,
        };
        if let Some(i) = stop.vars.iter().position(same) {
            return i + 1;
        }
        stop.vars.push(var);
        stop.vars.len()
    }

    fn scopes(&mut self, frame: usize) -> Result<Json, String> {
        if !self.stop.as_ref().is_some_and(|s| frame < s.frames.len()) {
            return Err("no such frame".into());
        }
        let locals = self.keep(Var::Locals(frame));
        let upvalues = self.keep(Var::Upvalues(frame));
        Ok(json!({"scopes": [
            {"name": "Locals", "presentationHint": "locals", "variablesReference": locals, "expensive": false},
            {"name": "Upvalues", "variablesReference": upvalues, "expensive": false},
        ]}))
    }

    fn variables(&mut self, reference: usize) -> Result<Json, String> {
        let pairs: Vec<(String, Value)> = {
            let stop = self.stop.as_ref().ok_or("not stopped")?;
            match stop
                .vars
                .get(reference.wrapping_sub(1))
                .ok_or("no such variable")?
            {
                Var::Locals(f) => stop.frames.get(*f).map(|f| f.locals.clone()),
                Var::Upvalues(f) => stop.frames.get(*f).map(|f| f.upvalues.clone()),
                Var::Value(v) => Some(children(v)),
            }
            .unwrap_or_default()
        };
        let list: Vec<Json> = pairs
            .into_iter()
            .map(|(name, v)| {
                let reference = if expandable(&v) {
                    self.keep(Var::Value(v.clone()))
                } else {
                    0
                };
                json!({
                    "name": name,
                    "value": display(&v, true),
                    "type": crate::math::type_name(&v),
                    "variablesReference": reference,
                })
            })
            .collect();
        Ok(json!({"variables": list}))
    }

    fn result_json(&mut self, values: MultiValue) -> Json {
        let first = values.iter().next().cloned().unwrap_or(Value::Nil);
        let text = if values.len() > 1 {
            values
                .iter()
                .map(|v| display(v, true))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            display(&first, true)
        };
        let kind = crate::math::type_name(&first);
        let reference = if values.len() == 1 && expandable(&first) {
            self.keep(Var::Value(first))
        } else {
            0
        };
        json!({"result": text, "type": kind, "variablesReference": reference})
    }

    /// Runs `expr` (an expression, or statements) where frame `level` of the running `l`
    /// stands: its locals and upvalues are in scope, then its script's globals.
    fn evaluate_at(
        &mut self,
        lua: &Lua,
        l: *mut ffi::lua_State,
        level: c_int,
        expr: &str,
    ) -> Result<MultiValue, String> {
        let env = scope_env(
            lua,
            upvalues(lua, l, level),
            locals(lua, l, level),
            function_env(lua, l, level),
        )
        .map_err(|e| e.to_string())?;
        self.run_expr(lua, env, expr)
    }

    /// Runs `expr` in frame `frame` of the stop, as it was.
    fn evaluate_in(&mut self, lua: &Lua, frame: usize, expr: &str) -> Result<MultiValue, String> {
        let f = self
            .stop
            .as_ref()
            .and_then(|s| s.frames.get(frame))
            .ok_or("not stopped")?;
        let env = scope_env(lua, f.upvalues.clone(), f.locals.clone(), f.globals.clone())
            .map_err(|e| e.to_string())?;
        self.run_expr(lua, env, expr)
    }

    fn run_expr(&mut self, lua: &Lua, env: Table, expr: &str) -> Result<MultiValue, String> {
        let f = match self.compiled.get(expr) {
            Some(f) => f.clone(),
            None => {
                let load = |code: &str| lua.load(code).set_name("=watch").into_function();
                let f = load(&format!("return {expr}"))
                    .or_else(|_| load(expr))
                    .map_err(|e| short_error(&e))?;
                if self.compiled.len() > 256 {
                    self.compiled.clear();
                }
                self.compiled.insert(expr.to_string(), f.clone());
                f
            }
        };
        f.set_environment(env).map_err(|e| e.to_string())?;
        let held = self.watchdog.suspend();
        self.watchdog.arm(500.0);
        let result = f.call::<MultiValue>(());
        self.watchdog.resume(held);
        result.map_err(|e| short_error(&e))
    }

    /// A logpoint's message with each `{expression}` filled in.
    fn interpolate(&mut self, lua: &Lua, l: *mut ffi::lua_State, message: &str) -> String {
        let mut out = String::new();
        let mut rest = message;
        while let Some(open) = rest.find('{') {
            // The matching brace, so an expression may hold a table: `{#{a, b}}`.
            let mut depth = 0;
            let Some(close) = rest[open..].find(|c| {
                depth += match c {
                    '{' => 1,
                    '}' => -1,
                    _ => 0,
                };
                depth == 0
            }) else {
                break;
            };
            out.push_str(&rest[..open]);
            let expr = &rest[open + 1..open + close];
            match self.evaluate_at(lua, l, 0, expr) {
                Ok(values) => out.push_str(
                    &values
                        .iter()
                        .map(|v| display(v, false))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                Err(err) => out.push_str(&format!("<{err}>")),
            }
            rest = &rest[open + close + 1..];
        }
        out.push_str(rest);
        out
    }

    // ---- output ----

    fn output(&self, category: &str, text: &str) {
        if let Some(link) = self.link.as_ref().filter(|l| l.connected()) {
            link.event("output", json!({"category": category, "output": text}));
        }
    }

    fn output_at(&self, category: &str, text: &str, lua: &Lua, path: &str, line: c_int) {
        if let Some(link) = self.link.as_ref().filter(|l| l.connected()) {
            let full = std::path::absolute(Self::root(lua).join(path)).unwrap_or_default();
            link.event(
                "output",
                json!({"category": category, "output": text, "line": line,
                    "source": {"name": path, "path": full.display().to_string()}}),
            );
        }
    }

    /// Script `print`s and errors since the last flush, to the client's console.
    fn flush_output(&mut self, lua: &Lua) {
        if !self.connected() {
            return;
        }
        let Ok(s) = lua.try_app_data_ref::<crate::access::Shared>() else {
            return;
        };
        let Some(s) = s else { return };
        let new_logs = (s.log_total - self.log_seen.min(s.log_total)).min(s.log.len() as u64);
        let logs: Vec<String> = s
            .log
            .iter()
            .skip(s.log.len() - new_logs as usize)
            .cloned()
            .collect();
        let new_errors =
            (s.errors_total - self.errors_seen.min(s.errors_total)).min(s.errors.len() as u64);
        let errors: Vec<String> = s
            .errors
            .iter()
            .skip(s.errors.len() - new_errors as usize)
            .map(|e| e.message.clone())
            .collect();
        self.log_seen = s.log_total;
        self.errors_seen = s.errors_total;
        drop(s);
        for line in logs {
            self.output("stdout", &format!("{line}\n"));
        }
        for error in errors {
            self.output("stderr", &format!("{error}\n"));
        }
    }
}

fn bp_json(bp: &Breakpoint, path: &str) -> Json {
    match bp.actual {
        Some(at) => json!({"id": bp.id, "verified": true, "line": at}),
        None => json!({"id": bp.id, "verified": false, "line": bp.line,
            "message": format!("{path} isn't loaded yet (or has no code at or after this line)")}),
    }
}

/// `hitCondition`: `5` or `== 5` (the 5th hit), `>= 5`, `> 5`, `< 5`, `% 5` (every 5th).
fn hit_matches(condition: &str, hits: u64) -> bool {
    let c = condition.trim();
    let (op, n) = ["==", ">=", "<=", ">", "<", "%", "="]
        .iter()
        .find_map(|op| c.strip_prefix(op).map(|n| (*op, n)))
        .unwrap_or(("==", c));
    let Ok(n) = n.trim().parse::<u64>() else {
        return true;
    };
    match op {
        ">=" => hits >= n,
        "<=" => hits <= n,
        ">" => hits > n,
        "<" => hits < n,
        "%" => n > 0 && hits.is_multiple_of(n),
        _ => hits == n,
    }
}

fn truthy(values: &MultiValue) -> bool {
    !matches!(
        values.iter().next(),
        None | Some(Value::Nil) | Some(Value::Boolean(false))
    )
}

fn short_error(err: &mlua::Error) -> String {
    let text = err.to_string();
    let first = text.lines().next().unwrap_or_default();
    first
        .strip_prefix("runtime error: ")
        .or_else(|| first.strip_prefix("syntax error: "))
        .unwrap_or(first)
        .replace("[string \"watch\"]:1: ", "")
        .replace("watch:1: ", "")
}

/// `path` (as a client names it: absolute, any slashes and drive-letter case) relative to the
/// scripts folder.
fn relative(root: &Path, path: &str) -> Option<String> {
    let clean = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("//?/")
            .trim_end_matches('/')
            .to_string()
    };
    let roots = [
        clean(&std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf())),
        clean(&root.canonicalize().unwrap_or_else(|_| root.to_path_buf())),
    ];
    let given = Path::new(path);
    let candidates = [
        clean(given),
        clean(&given.canonicalize().unwrap_or_else(|_| given.to_path_buf())),
    ];
    for full in &candidates {
        for r in &roots {
            if full.len() > r.len() + 1
                && full.is_char_boundary(r.len())
                && full[..r.len()].eq_ignore_ascii_case(r)
                && full.as_bytes()[r.len()] == b'/'
            {
                return Some(normalize(&full[r.len() + 1..]));
            }
        }
    }
    None
}

fn chunk_of(lua: &Lua, path: &str) -> Option<Function> {
    lua.app_data_ref::<Files>()?.map.get(path)?.chunk.clone()
}

/// Sets or clears a breakpoint on `line` of `chunk` and the functions inside it. Returns the
/// line it went to (the next with code).
fn set_breakpoint(lua: &Lua, chunk: &Function, line: u32, on: bool) -> Option<u32> {
    let mut target: c_int = -1;
    // SAFETY: the chunk (a Luau function) is the only argument on the stack.
    unsafe {
        lua.exec_raw::<()>(chunk.clone(), |state| {
            target = ffi::lua_breakpoint(state, -1, line as c_int, on as c_int);
            ffi::lua_pop(state, 1);
        })
        .ok()?;
    }
    (target > 0).then_some(target as u32)
}

/// Pops the value on top of `l`'s stack into a [`Value`] (through the registry, so it works
/// whichever thread `l` is).
///
/// SAFETY: `l` is a live thread of `lua`'s VM with a value on top.
unsafe fn pop_value(lua: &Lua, l: *mut ffi::lua_State) -> Value {
    unsafe {
        let r = ffi::lua_ref(l, -1);
        ffi::lua_pop(l, 1);
        if r == ffi::LUA_REFNIL || r == ffi::LUA_NOREF {
            return Value::Nil;
        }
        let v = lua
            .exec_raw::<Value>((), |state| {
                ffi::lua_rawgeti(state, ffi::LUA_REGISTRYINDEX, r as _);
            })
            .unwrap_or(Value::Nil);
        ffi::lua_unref(l, r);
        v
    }
}

/// The `lua_State` of a coroutine (0 if it isn't one).
fn state_of(lua: &Lua, thread: &Thread) -> usize {
    let mut t: *mut ffi::lua_State = std::ptr::null_mut();
    // SAFETY: the thread is pushed as the only argument and popped.
    let _ = unsafe {
        lua.exec_raw::<()>(thread.clone(), |state| {
            t = ffi::lua_tothread(state, -1);
            ffi::lua_pop(state, 1);
        })
    };
    t as usize
}

fn cstr(p: *const c_char) -> Option<String> {
    // SAFETY: Luau's debug strings are NUL-terminated and live while the frame does.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

struct FrameInfo {
    name: String,
    source: String,
    line: c_int,
}

fn frame_info(l: *mut ffi::lua_State, level: c_int) -> FrameInfo {
    // SAFETY: a zeroed record is valid for lua_getinfo to fill; `l` is stopped.
    unsafe {
        let mut ar: ffi::lua_Debug = std::mem::zeroed();
        if ffi::lua_getinfo(l, level, c"sln".as_ptr(), &mut ar) == 0 {
            return FrameInfo {
                name: "?".into(),
                source: String::new(),
                line: 0,
            };
        }
        let source = cstr(ar.source).unwrap_or_default();
        let what = cstr(ar.what).unwrap_or_default();
        FrameInfo {
            name: cstr(ar.name).filter(|n| !n.is_empty()).unwrap_or_else(|| {
                if what == "main" {
                    "(file)".into()
                } else {
                    "(function)".into()
                }
            }),
            source: source.strip_prefix('@').unwrap_or(&source).to_string(),
            line: ar.currentline,
        }
    }
}

/// The script lines of an error's traceback: no message (shown above it), no engine functions,
/// no debugger wrapper.
fn script_traceback(text: &str) -> String {
    text.lines()
        .skip_while(|l| !l.trim_start().starts_with("stack traceback:"))
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("[C]") && !l.starts_with("engine/"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_at(l: *mut ffi::lua_State, level: c_int) -> Option<String> {
    let info = frame_info(l, level);
    (!info.source.is_empty()).then_some(info.source)
}

fn current_line(l: *mut ffi::lua_State, level: c_int) -> c_int {
    frame_info(l, level).line
}

/// The stack levels of `l` that run Luau code (not engine functions), innermost first.
fn lua_frames(l: *mut ffi::lua_State) -> Vec<c_int> {
    let mut frames = Vec::new();
    // SAFETY: lua_getinfo returns 0 past the last level.
    unsafe {
        for level in 0..200 {
            let mut ar: ffi::lua_Debug = std::mem::zeroed();
            if ffi::lua_getinfo(l, level, c"s".as_ptr(), &mut ar) == 0 {
                break;
            }
            let source = cstr(ar.source).unwrap_or_default();
            if cstr(ar.what).as_deref() != Some("C")
                && source.strip_prefix('@').unwrap_or(&source) != CATCH_SOURCE
            {
                frames.push(level);
            }
        }
    }
    frames
}

/// The locals in scope at frame `level` (temporaries like `(for state)` left out).
fn locals(lua: &Lua, l: *mut ffi::lua_State, level: c_int) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    // SAFETY: each lua_getlocal that returns a name pushed its value, which pop_value takes.
    unsafe {
        for n in 1..=255 {
            let Some(name) = cstr(ffi::lua_getlocal(l, level, n)) else {
                break;
            };
            let v = pop_value(lua, l);
            if !name.starts_with('(') {
                out.push((name, v));
            }
        }
    }
    out
}

/// The upvalues of the function at frame `level`.
fn upvalues(lua: &Lua, l: *mut ffi::lua_State, level: c_int) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    // SAFETY: "f" pushes the function; each named upvalue pushes its value; both are popped.
    unsafe {
        let mut ar: ffi::lua_Debug = std::mem::zeroed();
        if ffi::lua_getinfo(l, level, c"f".as_ptr(), &mut ar) == 0 {
            return out;
        }
        let f = ffi::lua_gettop(l);
        for n in 1..=255 {
            let Some(name) = cstr(ffi::lua_getupvalue(l, f, n)) else {
                break;
            };
            let v = pop_value(lua, l);
            out.push((
                if name.is_empty() {
                    format!("#{n}")
                } else {
                    name
                },
                v,
            ));
        }
        ffi::lua_pop(l, 1);
    }
    out
}

/// The environment the function at frame `level` sees (its script's globals).
fn function_env(lua: &Lua, l: *mut ffi::lua_State, level: c_int) -> Option<Table> {
    // SAFETY: "f" pushes the function, lua_getfenv its environment; both are popped.
    unsafe {
        let mut ar: ffi::lua_Debug = std::mem::zeroed();
        if ffi::lua_getinfo(l, level, c"f".as_ptr(), &mut ar) == 0 {
            return None;
        }
        ffi::lua_getfenv(l, -1);
        let env = pop_value(lua, l);
        ffi::lua_pop(l, 1);
        match env {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }
}

/// A table that reads a frame's locals and upvalues, then its script's globals.
fn scope_env(
    lua: &Lua,
    upvalues: Vec<(String, Value)>,
    locals: Vec<(String, Value)>,
    globals: Option<Table>,
) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    for (name, v) in upvalues.into_iter().chain(locals) {
        env.raw_set(name, v)?;
    }
    let meta = lua.create_table()?;
    meta.raw_set("__index", globals.unwrap_or_else(|| lua.globals()))?;
    env.set_metatable(Some(meta))?;
    Ok(env)
}

/// `l`'s Luau frames as they are now, innermost first.
fn snapshot(lua: &Lua, l: *mut ffi::lua_State) -> Vec<FrameSnap> {
    lua_frames(l)
        .into_iter()
        .map(|level| {
            let info = frame_info(l, level);
            FrameSnap {
                name: info.name,
                source: info.source,
                line: info.line,
                locals: locals(lua, l, level),
                upvalues: upvalues(lua, l, level),
                globals: function_env(lua, l, level),
            }
        })
        .collect()
}

fn expandable(v: &Value) -> bool {
    matches!(v, Value::Table(t) if crate::math::kind(t).is_none())
}

/// A table's entries (array part first, then the rest by name), and its class.
fn children(v: &Value) -> Vec<(String, Value)> {
    let Value::Table(t) = v else {
        return Vec::new();
    };
    let mut numbered = Vec::new();
    let mut named = Vec::new();
    for pair in t.clone().pairs::<Value, Value>().take(1000) {
        let Ok((k, v)) = pair else { continue };
        match &k {
            Value::Integer(i) => numbered.push((*i as f64, format!("[{i}]"), v)),
            Value::Number(n) => numbered.push((*n, format!("[{}]", number(*n)), v)),
            Value::String(s) => named.push((s.to_string_lossy().to_string(), v)),
            other => named.push((format!("[{}]", display(other, true)), v)),
        }
    }
    numbered.sort_by(|a, b| a.0.total_cmp(&b.0));
    named.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<(String, Value)> = numbered.into_iter().map(|(_, k, v)| (k, v)).collect();
    out.extend(named);
    if let Some(meta) = t.metatable()
        && let Ok(Value::Table(class)) = meta.raw_get::<Value>("__index")
    {
        out.push(("(class)".into(), Value::Table(class)));
    }
    out
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        let s = format!("{n:.4}");
        let s = s.trim_end_matches('0');
        s.strip_suffix('.').unwrap_or(s).to_string()
    }
}

/// How a value reads in the debugger (`quoted`: strings in quotes, as in the variables view).
pub(crate) fn display(v: &Value, quoted: bool) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => number(*n),
        Value::String(s) => {
            let s = s.to_string_lossy();
            let s: String = s.chars().take(300).collect();
            if quoted { format!("{s:?}") } else { s }
        }
        // As they're written: vec3(1, 2.5, -3).
        Value::Table(t) if crate::math::kind(t).is_some() => {
            let (name, keys): (&str, &[&str]) = match crate::math::kind(t) {
                Some(crate::math::Kind::Quat) => ("quat", &["x", "y", "z", "w"]),
                _ => ("vec3", &["x", "y", "z"]),
            };
            let parts: Vec<String> = keys
                .iter()
                .map(|k| number(t.raw_get::<f64>(*k).unwrap_or(f64::NAN)))
                .collect();
            format!("{name}({})", parts.join(", "))
        }
        Value::Table(t) => {
            let mut parts = Vec::new();
            let mut count = 0;
            for pair in t.clone().pairs::<Value, Value>() {
                let Ok((k, v)) = pair else { continue };
                count += 1;
                if parts.len() < 4 {
                    let key = match &k {
                        Value::String(s) => s.to_string_lossy().to_string(),
                        other => format!("[{}]", display(other, true)),
                    };
                    let value = match &v {
                        Value::Table(_) => "{…}".to_string(),
                        other => display(other, true),
                    };
                    parts.push(format!("{key} = {value}"));
                }
            }
            if count > parts.len() {
                parts.push(format!("… {} more", count - parts.len()));
            }
            format!("{{{}}}", parts.join(", "))
        }
        Value::Function(f) => {
            let info = f.info();
            match (info.short_src, info.line_defined) {
                (Some(src), Some(line)) => format!("function ({src}:{line})"),
                _ => "function (engine)".into(),
            }
        }
        other => other
            .to_string()
            .unwrap_or_else(|_| other.type_name().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_conditions() {
        assert!(hit_matches("3", 3) && !hit_matches("3", 4));
        assert!(hit_matches(">= 2", 2) && !hit_matches(">=2", 1));
        assert!(hit_matches("%3", 6) && !hit_matches("% 3", 7));
        assert!(hit_matches("nonsense", 1));
    }

    #[test]
    fn client_paths_map_into_the_scripts_folder() {
        let root = std::env::temp_dir().join("engine-debugger-paths");
        let full = std::path::absolute(root.join("showcase/a.luau")).unwrap();
        let windowsy = full.display().to_string().replace('/', "\\");
        assert_eq!(
            relative(&root, &full.display().to_string()).as_deref(),
            Some("showcase/a.luau")
        );
        assert_eq!(
            relative(&root, &windowsy).as_deref(),
            Some("showcase/a.luau")
        );
        assert_eq!(relative(&root, "/elsewhere/a.luau"), None);
    }

    #[test]
    fn numbers_read_like_luau() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(1.0 / 3.0), "0.3333");
    }
}
