//! The script debugger: breakpoints (with conditions, hit counts and logpoints), stepping, the
//! call stack, variables and watches for a DAP client attached through [`crate::dap`] (VS Code
//! with `crates/script/vscode`, nvim-dap, Zed), plus logpoints set through the API
//! (`script.logpoint`) whose newest values code boards show beside their lines.
//!
//! How it works (engine-design/scripting-research.md, "Debugger"): Luau patches a `BREAK` opcode
//! over the first instruction of a breakpoint's line (`lua_breakpoint`) and calls the VM's
//! `debugbreak` callback when it runs, so a line without a breakpoint costs nothing. While a
//! client is attached, scripts run in Luau's single-step mode and the `debugstep` callback, set
//! only while stepping or pausing, stops at the next line. A stop blocks the thread that runs
//! the scripts and answers the client until it says continue: the game stands still while
//! stopped, as with a native debugger in Unity or Unreal. Logpoints never stop.
//!
//! The VM callbacks only get a `lua_State`, so sessions are found by the VM's main thread in
//! [`SESSIONS`]; everything else holds a [`Debugger`].

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
    /// `catch(handler, f, ...)`: calls `f` under `xpcall` with `handler`, then raises the error
    /// again (`CATCH`).
    catch: Function,
    /// Stops at an error where it was raised (the handler `catch` gives `xpcall`).
    handler: Function,
}

/// Runs a hook so an error stops the debugger before the stack unwinds: `xpcall` calls its
/// handler where the error was raised. The error goes on unchanged afterwards.
const CATCH: &str = r#"
local xpcall, error = xpcall, error
return function(handler, f, ...)
	local ok, err = xpcall(f, handler, ...)
	if not ok then
		error(err, 0)
	end
end
"#;

/// Where [`CATCH`] lives; its frames are left out of the stack a client sees.
const CATCH_SOURCE: &str = "engine/debugger.luau";

/// Lets `load_file` and `reload` (which only have the VM) tell the session about new code.
struct SessionRef(Arc<Mutex<Session>>);

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
            step: None,
            pause_requested: false,
            stop: None,
            inline: BTreeMap::new(),
            inline_version: 0,
            log_seen: 0,
            errors_seen: 0,
            compiled: HashMap::new(),
        }));
        lua.set_app_data(SessionRef(session.clone()));
        SESSIONS
            .lock()
            .map_err(|_| mlua::Error::runtime("debug sessions poisoned"))?
            .push((main, session.clone()));
        let catch: Function = lua
            .load(CATCH)
            .set_name(format!("@{CATCH_SOURCE}"))
            .eval()?;
        let weak = Arc::downgrade(&session);
        let handler = lua.create_function(move |lua, error: Value| {
            let text = match &error {
                Value::String(s) => s.to_string_lossy(),
                Value::Error(err) => err.to_string(),
                other => display(other, false),
            };
            let mut thread = std::ptr::null_mut();
            // SAFETY: only reads which thread is running (the one that raised the error).
            unsafe { lua.exec_raw::<()>((), |state| thread = state)? };
            if let Some(session) = weak.upgrade()
                && let Ok(mut s) = session.try_lock()
                && s.attached.load(Ordering::Relaxed)
            {
                s.stop_here(lua, thread, "exception", None, Some(text));
            }
            Ok(error)
        })?;
        Ok(Self {
            main,
            session,
            attached,
            break_errors,
            catch,
            handler,
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

    /// When stopping on errors: [`CATCH`] and its handler, to call a hook through.
    pub fn catcher(&self) -> Option<(&Function, &Function)> {
        self.breaks_on_errors()
            .then_some((&self.catch, &self.handler))
    }

    /// A task failed: stops on its stack, which a coroutine keeps after an error.
    pub fn stop_on_task_error(&self, lua: &Lua, thread: &Thread, err: &mlua::Error) {
        let mut t = std::ptr::null_mut();
        // SAFETY: the thread is a coroutine of this VM, pushed as the only argument.
        let _ = unsafe {
            lua.exec_raw::<()>(thread.clone(), |state| {
                t = ffi::lua_tothread(state, -1);
                ffi::lua_pop(state, 1);
            })
        };
        if t.is_null() {
            return;
        }
        self.lock()
            .stop_here(lua, t, "exception", None, Some(err.to_string()));
    }

    /// Puts a task thread in single-step mode before it resumes, so stepping follows it.
    pub fn prepare(&self, lua: &Lua, thread: &Thread) {
        // SAFETY: the thread is a live coroutine of this VM, pushed as the only argument.
        let _ = unsafe {
            lua.exec_raw::<()>(thread.clone(), |state| {
                let t = ffi::lua_tothread(state, -1);
                if !t.is_null() {
                    ffi::lua_singlestep(t, 1);
                }
                ffi::lua_pop(state, 1);
            })
        };
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

    /// The newest logpoint values of `path`, by line, and a number that changes with them.
    pub fn inline_values(&self, path: &str) -> (u64, BTreeMap<u32, String>) {
        let s = self.lock();
        (
            s.inline_version,
            s.inline.get(path).cloned().unwrap_or_default(),
        )
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

/// Something the client can expand while stopped.
enum Var {
    Locals(c_int),
    Upvalues(c_int),
    Value(Value),
}

struct Stop {
    /// Stack levels of the Luau frames, innermost first.
    frames: Vec<c_int>,
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
    breakpoints: BTreeMap<String, Vec<Breakpoint>>,
    next_id: i64,
    step: Option<Step>,
    pause_requested: bool,
    stop: Option<Stop>,
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
            if !bp.from_api {
                link.event(
                    "breakpoint",
                    json!({"reason": "changed", "breakpoint": bp_json(bp, path)}),
                );
            }
        }
    }

    fn set_stepping(&self, on: bool) {
        // SAFETY: the main thread lives as long as the session is registered; this only writes
        // a callback pointer in the VM's global state.
        unsafe {
            (*ffi::lua_callbacks(self.main())).debugstep = if on { Some(debugstep) } else { None };
        }
    }

    fn attach(&mut self) {
        self.attached.store(true, Ordering::Relaxed);
        // SAFETY: as in `set_stepping`. Threads made from it inherit single-step mode; tasks
        // are also switched as they resume (`Debugger::prepare`).
        unsafe { ffi::lua_singlestep(self.main(), 1) };
    }

    /// The client left: its breakpoints go, the scripts run at full speed again.
    fn detach(&mut self, lua: &Lua) {
        self.attached.store(false, Ordering::Relaxed);
        self.break_errors.store(false, Ordering::Relaxed);
        self.step = None;
        self.pause_requested = false;
        self.set_stepping(false);
        // SAFETY: as in `set_stepping`.
        unsafe { ffi::lua_singlestep(self.main(), 0) };
        let paths: Vec<String> = self.breakpoints.keys().cloned().collect();
        for path in paths {
            self.unapply(lua, &path);
            if let Some(bps) = self.breakpoints.get_mut(&path) {
                bps.retain(|b| b.from_api);
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
            self.stop_here(&lua, l, "breakpoint", Some(id), None);
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
            self.stop_here(&lua, l, "pause", None, None);
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
            self.stop_here(&lua, l, "step", None, None);
        }
    }

    /// Stops the scripts at `l`'s current line and serves the client until it continues
    /// (`exception`: the error it stopped on).
    fn stop_here(
        &mut self,
        lua: &Lua,
        l: *mut ffi::lua_State,
        reason: &str,
        bp: Option<i64>,
        exception: Option<String>,
    ) {
        if !self.connected() {
            self.detach(lua);
            return;
        }
        let held = self.watchdog.suspend();
        self.set_stepping(false);
        self.step = None;
        self.pause_requested = false;
        let frames = lua_frames(l)
            .into_iter()
            .filter(|&level| source_at(l, level).as_deref() != Some(CATCH_SOURCE))
            .collect();
        let mut body = json!({"reason": reason, "threadId": 1, "allThreadsStopped": true});
        if let Some(id) = bp {
            body["hitBreakpointIds"] = json!([id]);
        }
        if let Some(text) = &exception {
            body["description"] = json!("Paused on an error");
            body["text"] = json!(text.lines().next().unwrap_or_default());
        }
        self.stop = Some(Stop {
            frames,
            vars: Vec::new(),
            exception,
        });
        self.flush_output(lua);
        if let Some(link) = &self.link {
            link.event("stopped", body);
        }
        tracing::info!(target: "script", "script debugger: stopped ({reason}) at {}:{}",
            source_at(l, 0).unwrap_or_default(), current_line(l, 0));
        while let Some(message) = self
            .link
            .as_ref()
            .map(|l| l.recv(Duration::from_millis(200)))
        {
            match message {
                Some(Incoming::Request(request)) => {
                    if self.handle_stopped(lua, l, &request) {
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

    /// While the scripts run (once a frame).
    fn poll(&mut self, lua: &Lua) -> Vec<Json> {
        let mut evals = Vec::new();
        while let Some(message) = self.link.as_ref().and_then(|l| l.recv(Duration::ZERO)) {
            match message {
                Incoming::Connected => {
                    self.log_seen = shared(lua).log_total;
                    self.errors_seen = shared(lua).errors_total;
                }
                Incoming::Closed => self.detach(lua),
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
        self.flush_output(lua);
        evals
    }

    /// While stopped. Returns true when the scripts should go on.
    fn handle_stopped(&mut self, lua: &Lua, l: *mut ffi::lua_State, request: &Json) -> bool {
        let command = request["command"].as_str().unwrap_or_default();
        let args = &request["arguments"];
        match command {
            "continue" => {
                self.reply(request, Ok(json!({"allThreadsContinued": true})));
                true
            }
            // An error unwinds the stack on continuing: there is no next line to step to.
            "next" | "stepIn" | "stepOut"
                if self.stop.as_ref().is_some_and(|s| s.exception.is_some()) =>
            {
                self.reply(request, Ok(Json::Null));
                true
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
                        "details": {"message": first, "typeName": "error", "stackTrace": text},
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
                self.step = Some(Step {
                    thread: l as usize,
                    // SAFETY: `l` is the stopped thread.
                    depth: unsafe { ffi::lua_stackdepth(l) },
                    line: current_line(l, 0),
                    kind,
                });
                self.reply(request, Ok(Json::Null));
                true
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
                let frames = self.stack_trace(lua, l);
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
                let result = self.variables(lua, l, reference);
                self.reply(request, result);
                false
            }
            "evaluate" => {
                let expr = args["expression"].as_str().unwrap_or_default().to_string();
                let level = args["frameId"]
                    .as_u64()
                    .and_then(|f| self.stop.as_ref()?.frames.get(f as usize).copied())
                    .unwrap_or(0);
                let result = self
                    .evaluate_at(lua, l, level, &expr)
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
                self.attach();
                self.reply(request, Ok(Json::Null));
                let root = Self::root(lua);
                self.output(
                    "console",
                    &format!(
                        "Attached to the game's scripts ({}). Breakpoints, logpoints and \
                         stepping work in .luau files under that folder; the game stands \
                         still while stopped.\n",
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
            .filter(|b| b.from_api)
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

    fn stack_trace(&self, lua: &Lua, l: *mut ffi::lua_State) -> Vec<Json> {
        let root = std::path::absolute(Self::root(lua)).unwrap_or_default();
        let Some(stop) = &self.stop else {
            return Vec::new();
        };
        stop.frames
            .iter()
            .enumerate()
            .map(|(id, &level)| {
                let info = frame_info(l, level);
                let mut source = json!({"name": info.source});
                if info.source.ends_with(".luau") || info.source.ends_with(".lua") {
                    if info.source.starts_with("engine/") {
                        source["presentationHint"] = json!("deemphasize");
                    } else {
                        source["path"] = json!(root.join(&info.source).display().to_string());
                    }
                }
                json!({
                    "id": id,
                    "name": info.name,
                    "source": source,
                    "line": info.line.max(1),
                    "column": 1,
                })
            })
            .collect()
    }

    fn keep(&mut self, var: Var) -> usize {
        let Some(stop) = &mut self.stop else { return 0 };
        stop.vars.push(var);
        stop.vars.len()
    }

    fn scopes(&mut self, frame: usize) -> Result<Json, String> {
        let level = self
            .stop
            .as_ref()
            .and_then(|s| s.frames.get(frame).copied())
            .ok_or("no such frame")?;
        let locals = self.keep(Var::Locals(level));
        let upvalues = self.keep(Var::Upvalues(level));
        Ok(json!({"scopes": [
            {"name": "Locals", "presentationHint": "locals", "variablesReference": locals, "expensive": false},
            {"name": "Upvalues", "variablesReference": upvalues, "expensive": false},
        ]}))
    }

    fn variables(
        &mut self,
        lua: &Lua,
        l: *mut ffi::lua_State,
        reference: usize,
    ) -> Result<Json, String> {
        let pairs: Vec<(String, Value)> = {
            let stop = self.stop.as_ref().ok_or("not stopped")?;
            match stop
                .vars
                .get(reference.wrapping_sub(1))
                .ok_or("no such variable")?
            {
                Var::Locals(level) => locals(lua, l, *level),
                Var::Upvalues(level) => upvalues(lua, l, *level),
                Var::Value(v) => children(v),
            }
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
        let reference = if values.len() == 1 && expandable(&first) {
            self.keep(Var::Value(first))
        } else {
            0
        };
        json!({"result": text, "variablesReference": reference})
    }

    /// Runs `expr` (an expression, or statements) where frame `level` of `l` stands: its
    /// locals and upvalues are in scope, then its script's globals.
    fn evaluate_at(
        &mut self,
        lua: &Lua,
        l: *mut ffi::lua_State,
        level: c_int,
        expr: &str,
    ) -> Result<MultiValue, String> {
        let env = frame_env(lua, l, level).map_err(|e| e.to_string())?;
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
            let Some(close) = rest[open..].find('}') else {
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
            if cstr(ar.what).as_deref() != Some("C") {
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
fn frame_env(lua: &Lua, l: *mut ffi::lua_State, level: c_int) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    for (name, v) in upvalues(lua, l, level) {
        env.raw_set(name, v)?;
    }
    for (name, v) in locals(lua, l, level) {
        env.raw_set(name, v)?;
    }
    let meta = lua.create_table()?;
    let globals = function_env(lua, l, level).unwrap_or_else(|| lua.globals());
    meta.raw_set("__index", globals)?;
    env.set_metatable(Some(meta))?;
    Ok(env)
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
fn display(v: &Value, quoted: bool) -> String {
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
