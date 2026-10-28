//! The script profiler (`script.profile`): where the scripts' time goes, by function and by
//! line, sampled while the game runs. Code boards draw each line's share beside it, and
//! [`crate::board::ProfileBoard`] draws the flame graph.
//!
//! It samples the way Luau's own `--profile` and Roblox's Script Profiler do: a timer thread
//! marks a sample due every 1/hz seconds while a script runs (the [`Watchdog`] knows when one
//! does), and the VM's interrupt, which Luau calls at loop back-edges, calls and returns, takes
//! it: it walks the running thread's stack and adds the script time since the last sample to
//! that stack and to every line on it. Time inside an engine call (`entity:set`, `fire.ignite`)
//! is taken when the call returns and charged to the call, so a slow engine function shows as
//! itself rather than as the Luau line after it. The flame graph's first level says what the
//! runtime was running (`update`, `tasks`, `events`, ...); garbage collection is charged to the
//! code that runs after it. Built-ins that Luau runs on its fast path (`math.sin`, `vector`
//! math) never reach the interrupt, so time in a tight loop lands on the loop's back edge: the
//! `for` or `while` line, not its body.
//!
//! When not recording it costs one atomic load per interrupt and per engine call.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, c_char, c_int};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mlua::{Lua, ffi};
use serde_json::{Value as Json, json};

use crate::access::Watchdog;
use crate::runtime::Files;

/// Recordings running in any VM: the engine-call check's quick way out.
static RECORDING: AtomicUsize = AtomicUsize::new(0);
/// Some VM has a sample due: the interrupt's quick check (each VM's [`Clock`] says which).
static DUE: AtomicBool = AtomicBool::new(false);

/// Stack levels walked per sample at most (deeper frames, nearest the root, are left out).
const MAX_DEPTH: c_int = 96;

/// What the runtime was running when a sample was taken: the flame graph's first level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Update,
    FixedUpdate,
    Start,
    Events,
    Tasks,
    Reload,
    Console,
    Parallel,
}

const PHASES: [&str; 8] = [
    "update",
    "fixed_update",
    "start",
    "events",
    "tasks",
    "reload",
    "console",
    "parallel_update",
];

/// Shared by the timer thread, the VM's interrupt and the engine calls.
#[derive(Default)]
pub(crate) struct Clock {
    /// A sample is due: set by the timer, cleared by whoever takes it.
    pub due: AtomicBool,
    /// Script time since the last sample (nanoseconds): the next sample's weight.
    pending_ns: AtomicU64,
    phase: AtomicU8,
    stop: AtomicBool,
}

impl Clock {
    /// What the runtime runs next (the samples' first level).
    pub fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }
}

/// What a frame of a sampled stack is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Phase,
    Lua,
    Engine,
}

/// A function (or engine call, or phase) seen in a sample.
pub(crate) struct Frame {
    pub name: String,
    /// Index into [`Recording::sources`]; `None` for engine calls and phases.
    pub source: Option<u32>,
    /// Where the function starts.
    pub line: u32,
    pub kind: Kind,
}

/// The current (or last) recording; Lua app data, so the interrupt can reach it.
#[derive(Default)]
pub(crate) struct Recording {
    clock: Arc<Clock>,
    hz: u32,
    started: Option<Instant>,
    ended: Option<Instant>,
    until: Option<Instant>,
    /// Script frames run while recording.
    frames: u64,
    /// Started by a live [`crate::board::ProfileBoard`] (boards show such a recording once it
    /// is over) rather than by `script.profile` (shown as it grows).
    pub(crate) auto: bool,
    /// Bumps when boards should redraw: when a recording ends, and every second while one
    /// started with `script.profile` grows.
    pub(crate) shown: u64,
    shown_at: Option<Instant>,
    pub(crate) frames_seen: Vec<Frame>,
    by_key: HashMap<(usize, c_int, usize), u32>,
    by_label: HashMap<String, u32>,
    pub(crate) sources: Vec<String>,
    by_source: HashMap<usize, u32>,
    /// Weight (nanoseconds) of each distinct stack, root first.
    stacks: HashMap<Vec<u32>, u64>,
    /// Weight of each (source, line) a sample's stack stood on.
    lines: HashMap<(u32, u32), u64>,
    samples: u64,
    total_ns: u64,
    /// Scratch for one sample: its frames (innermost first) and its lines.
    stack: Vec<u32>,
    seen: Vec<(u32, u32)>,
}

impl Recording {
    fn label(&mut self, name: &str, kind: Kind) -> u32 {
        if let Some(&id) = self.by_label.get(name) {
            return id;
        }
        let id = self.frames_seen.len() as u32;
        self.frames_seen.push(Frame {
            name: name.to_string(),
            source: None,
            line: 0,
            kind,
        });
        self.by_label.insert(name.to_string(), id);
        id
    }

    fn source(&mut self, ptr: *const c_char) -> u32 {
        if let Some(&id) = self.by_source.get(&(ptr as usize)) {
            return id;
        }
        let text = text(ptr).unwrap_or_default();
        let text = text.strip_prefix('@').unwrap_or(&text).to_string();
        // Interned by Luau, so one pointer per file; a second pointer to the same text (the
        // first string was freed) maps to the same index.
        let id = match self.sources.iter().position(|s| *s == text) {
            Some(i) => i as u32,
            None => {
                self.sources.push(text);
                self.sources.len() as u32 - 1
            }
        };
        self.by_source.insert(ptr as usize, id);
        id
    }

    /// The id of the Luau function described by `ar` (from `lua_getinfo` "sn").
    fn function(&mut self, ar: &ffi::lua_Debug) -> u32 {
        // Luau interns strings, so the source and name pointers identify the text; a function
        // keeps its id across hot reloads while its name and first line stay.
        let key = (ar.source as usize, ar.linedefined, ar.name as usize);
        if let Some(&id) = self.by_key.get(&key) {
            return id;
        }
        let source = self.source(ar.source);
        let what = text(ar.what).unwrap_or_default();
        let name = text(ar.name).filter(|n| !n.is_empty()).unwrap_or_else(|| {
            if what == "main" {
                "(file)".into()
            } else {
                "(function)".into()
            }
        });
        let id = self.frames_seen.len() as u32;
        self.frames_seen.push(Frame {
            name,
            source: Some(source),
            line: ar.linedefined.max(0) as u32,
            kind: Kind::Lua,
        });
        self.by_key.insert(key, id);
        id
    }

    /// Adds one sample: `self.stack` (innermost first, phase last) and `self.seen` get `weight`.
    fn add(&mut self, weight: u64) {
        self.stack.reverse();
        match self.stacks.get_mut(self.stack.as_slice()) {
            Some(w) => *w += weight,
            None => {
                self.stacks.insert(self.stack.clone(), weight);
            }
        }
        for key in &self.seen {
            *self.lines.entry(*key).or_default() += weight;
        }
        self.samples += 1;
        self.total_ns += weight;
    }

    fn elapsed(&self) -> f64 {
        match (self.started, self.ended) {
            (Some(a), Some(b)) => (b - a).as_secs_f64(),
            (Some(a), None) => a.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }
}

fn text(p: *const c_char) -> Option<String> {
    // SAFETY: Luau's debug strings are NUL-terminated and live while the function does.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn recording_anywhere() -> bool {
    RECORDING.load(Ordering::Relaxed) != 0
}

/// Whether some VM has a sample due (the interrupt's quick check).
pub(crate) fn due_anywhere() -> bool {
    DUE.load(Ordering::Relaxed)
}

/// Takes a due sample: the running thread's stack, with `leaf` (an engine call that just
/// returned) on top. Called from the VM's interrupt and after engine calls.
pub(crate) fn sample(lua: &Lua, leaf: Option<&str>) {
    let Ok(Some(mut rec)) = lua.try_app_data_mut::<Recording>() else {
        return;
    };
    if !rec.clock.due.swap(false, Ordering::Relaxed) {
        return;
    }
    // Another VM's due sample waits for its timer's next tick (its weight keeps growing).
    DUE.store(false, Ordering::Relaxed);
    let weight = rec.clock.pending_ns.swap(0, Ordering::Relaxed).max(1);
    let phase = rec.clock.phase.load(Ordering::Relaxed) as usize;
    rec.stack.clear();
    rec.seen.clear();
    if let Some(label) = leaf {
        let id = rec.label(label, Kind::Engine);
        rec.stack.push(id);
    }
    let rec = &mut *rec;
    // SAFETY: `state` is the thread that is running (the VM hands it to the interrupt and to
    // engine calls); lua_getinfo only reads its frames and returns 0 past the last one.
    let _ = unsafe {
        lua.exec_raw::<()>((), |state| {
            for level in 0..MAX_DEPTH {
                let mut ar: ffi::lua_Debug = std::mem::zeroed();
                if ffi::lua_getinfo(state, level, c"sln".as_ptr(), &mut ar) == 0 {
                    break;
                }
                // Engine functions (and this call's own wrapper) have no source of their own.
                if !ar.what.is_null() && *ar.what == b'C' as c_char {
                    continue;
                }
                let id = rec.function(&ar);
                rec.stack.push(id);
                if ar.currentline > 0 {
                    let key = (
                        rec.frames_seen[id as usize].source.unwrap_or(0),
                        ar.currentline as u32,
                    );
                    if !rec.seen.contains(&key) {
                        rec.seen.push(key);
                    }
                }
            }
        })
    };
    let root = rec.label(PHASES[phase.min(PHASES.len() - 1)], Kind::Phase);
    rec.stack.push(root);
    rec.add(weight);
}

/// After an engine call returns: takes the sample that came due during it, charged to `label`.
pub(crate) fn after_engine_call(lua: &Lua, label: &str) {
    if !recording_anywhere() {
        return;
    }
    let due = lua
        .try_app_data_ref::<Recording>()
        .ok()
        .flatten()
        .is_some_and(|r| r.clock.due.load(Ordering::Relaxed));
    if due {
        sample(lua, Some(label));
    }
}

/// The runtime's handle on the profiler: starts and stops the timer.
pub(crate) struct Profiler {
    clock: Arc<Clock>,
    dog: Watchdog,
    timer: Option<JoinHandle<()>>,
}

impl Drop for Profiler {
    fn drop(&mut self) {
        self.halt();
    }
}

impl Profiler {
    pub fn new(lua: &Lua, dog: &Watchdog) -> Self {
        let clock = dog.clock.clone();
        lua.set_app_data(Recording {
            clock: clock.clone(),
            ..Default::default()
        });
        Self {
            clock,
            dog: dog.clone(),
            timer: None,
        }
    }

    pub fn recording(&self) -> bool {
        self.timer.is_some()
    }

    /// Starts a new recording at `hz` samples a second, for `seconds` (0: until stopped);
    /// `auto`: for a live profile board.
    pub fn start(&mut self, lua: &Lua, hz: u32, seconds: f64, auto: bool) {
        self.stop(lua);
        let hz = hz.clamp(10, 20_000);
        if let Some(mut rec) = lua.app_data_mut::<Recording>() {
            let (clock, shown, first) = (rec.clock.clone(), rec.shown, rec.started.is_none());
            *rec = Recording {
                clock,
                hz,
                started: Some(Instant::now()),
                until: (seconds > 0.0).then(|| Instant::now() + Duration::from_secs_f64(seconds)),
                auto,
                // Live boards keep showing the last recording until this one is over.
                shown: if auto && !first { shown } else { shown + 1 },
                ..Default::default()
            };
        }
        self.clock.stop.store(false, Ordering::Relaxed);
        self.clock.pending_ns.store(0, Ordering::Relaxed);
        self.clock.due.store(false, Ordering::Relaxed);
        let (clock, dog) = (self.clock.clone(), self.dog.clone());
        let period = Duration::from_secs_f64(1.0 / hz as f64);
        let spawned = std::thread::Builder::new()
            .name("script-profiler".into())
            .spawn(move || {
                let mut last = Instant::now();
                while !clock.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(period);
                    let now = Instant::now();
                    let dt = (now - last).as_nanos() as u64;
                    last = now;
                    // Only script time counts: between frames, and while the debugger holds
                    // the scripts, the watchdog is disarmed.
                    if dog.running() {
                        clock.pending_ns.fetch_add(dt, Ordering::Relaxed);
                        clock.due.store(true, Ordering::Relaxed);
                        DUE.store(true, Ordering::Relaxed);
                    }
                }
            });
        match spawned {
            Ok(handle) => {
                RECORDING.fetch_add(1, Ordering::Relaxed);
                self.timer = Some(handle);
            }
            Err(err) => tracing::warn!(target: "script", "the script profiler can't start: {err}"),
        }
    }

    fn halt(&mut self) -> bool {
        let Some(timer) = self.timer.take() else {
            return false;
        };
        self.clock.stop.store(true, Ordering::Relaxed);
        let _ = timer.join();
        self.clock.due.store(false, Ordering::Relaxed);
        RECORDING.fetch_sub(1, Ordering::Relaxed);
        true
    }

    /// Ends the recording (its results stay until the next one starts).
    pub fn stop(&mut self, lua: &Lua) {
        if self.halt()
            && let Some(mut rec) = lua.app_data_mut::<Recording>()
        {
            rec.ended = Some(Instant::now());
            rec.shown += 1;
        }
    }

    /// Drops the results (live profile boards start recording again).
    pub fn reset(&mut self, lua: &Lua) {
        self.halt();
        if let Some(mut rec) = lua.app_data_mut::<Recording>() {
            let (clock, shown) = (rec.clock.clone(), rec.shown + 1);
            *rec = Recording {
                clock,
                shown,
                ..Default::default()
            };
        }
    }

    /// Whether a live profile board may start a recording: none runs, and the last one (if
    /// any) was a live board's too.
    pub fn idle_for_boards(&self, lua: &Lua) -> bool {
        !self.recording()
            && lua
                .app_data_ref::<Recording>()
                .is_none_or(|r| r.started.is_none() || r.auto)
    }

    /// Once per script frame: counts it, ends the recording when its time is up, and tells
    /// boards to redraw a growing `script.profile` recording once a second.
    pub fn frame(&mut self, lua: &Lua) {
        if !self.recording() {
            return;
        }
        let over = lua.app_data_mut::<Recording>().is_some_and(|mut rec| {
            rec.frames += 1;
            if !rec.auto
                && rec
                    .shown_at
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
            {
                rec.shown += 1;
                rec.shown_at = Some(Instant::now());
            }
            rec.until.is_some_and(|t| Instant::now() >= t)
        });
        if over {
            self.stop(lua);
        }
    }

    /// What the runtime runs next (the samples' first level).
    pub fn phase(&self, phase: Phase) {
        self.clock.set_phase(phase);
    }

    /// After a call into the VM returned: a sample that came due too late to see its stack is
    /// charged to the phase itself (the call's setup and teardown).
    pub fn settle(&self, lua: &Lua) {
        if !self.clock.due.load(Ordering::Relaxed) {
            return;
        }
        let Ok(Some(mut rec)) = lua.try_app_data_mut::<Recording>() else {
            return;
        };
        if !rec.clock.due.swap(false, Ordering::Relaxed) {
            return;
        }
        let weight = rec.clock.pending_ns.swap(0, Ordering::Relaxed).max(1);
        let phase = rec.clock.phase.load(Ordering::Relaxed) as usize;
        rec.stack.clear();
        rec.seen.clear();
        let root = rec.label(PHASES[phase.min(PHASES.len() - 1)], Kind::Phase);
        rec.stack.push(root);
        rec.add(weight);
    }
}

/// The recording's headline numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Summary {
    pub recording: bool,
    pub hz: u32,
    pub seconds: f64,
    pub samples: u64,
    pub script_ms: f64,
    pub frames: u64,
    /// VMs whose samples are in it (the main one and the actor VMs that recorded).
    pub vms: usize,
}

impl Summary {
    /// Script time per frame while recording, milliseconds.
    pub fn ms_per_frame(&self) -> f64 {
        self.script_ms / self.frames.max(1) as f64
    }
}

/// A node of the flame graph: a function as called from the path above it.
#[derive(Debug)]
pub(crate) struct FlameNode {
    pub name: String,
    pub kind: Kind,
    /// File and line, for Luau functions.
    pub at: Option<(String, u32)>,
    pub ns: u64,
    pub children: Vec<FlameNode>,
}

impl FlameNode {
    fn child(&mut self, m: &Merged, id: u32) -> &mut FlameNode {
        let f = &m.frames[id as usize];
        let at = f.source.clone().map(|s| (s, f.line));
        let found = self
            .children
            .iter()
            .position(|c| c.name == f.name && c.kind == f.kind && c.at == at);
        let i = match found {
            Some(i) => i,
            None => {
                self.children.push(FlameNode {
                    name: f.name.clone(),
                    kind: f.kind,
                    at,
                    ns: 0,
                    children: Vec::new(),
                });
                self.children.len() - 1
            }
        };
        &mut self.children[i]
    }

    fn sort(&mut self) {
        self.children.sort_by_key(|c| std::cmp::Reverse(c.ns));
        for c in &mut self.children {
            c.sort();
        }
    }
}

/// A frame of [`Merged`]: like [`Frame`], with its source's text.
struct MergedFrame {
    name: String,
    source: Option<String>,
    line: u32,
    kind: Kind,
}

/// The recordings of every VM (the main one and the actor VMs) as one: functions with the
/// same name, file and line are one function.
#[derive(Default)]
struct Merged {
    frames: Vec<MergedFrame>,
    by_key: HashMap<(String, Option<String>, u32, u8), u32>,
    stacks: HashMap<Vec<u32>, u64>,
    lines: HashMap<(String, u32), u64>,
    samples: u64,
    total_ns: u64,
    frames_run: u64,
    hz: u32,
    seconds: f64,
    started: bool,
    shown: u64,
    vms: usize,
}

impl Merged {
    fn of(luas: &[&Lua]) -> Self {
        let mut m = Merged::default();
        for (vm, lua) in luas.iter().enumerate() {
            let Some(rec) = lua.app_data_ref::<Recording>() else {
                continue;
            };
            m.shown += rec.shown;
            if rec.started.is_none() {
                continue;
            }
            m.started = true;
            m.vms += 1;
            if vm == 0 || m.hz == 0 {
                m.hz = rec.hz;
            }
            m.seconds = m.seconds.max(rec.elapsed());
            m.frames_run = m.frames_run.max(rec.frames);
            m.samples += rec.samples;
            m.total_ns += rec.total_ns;
            let ids: Vec<u32> = rec
                .frames_seen
                .iter()
                .map(|f| {
                    let source = f.source.map(|s| rec.sources[s as usize].clone());
                    let key = (f.name.clone(), source.clone(), f.line, f.kind as u8);
                    if let Some(&id) = m.by_key.get(&key) {
                        return id;
                    }
                    let id = m.frames.len() as u32;
                    m.frames.push(MergedFrame {
                        name: f.name.clone(),
                        source,
                        line: f.line,
                        kind: f.kind,
                    });
                    m.by_key.insert(key, id);
                    id
                })
                .collect();
            for (stack, &w) in &rec.stacks {
                let mapped: Vec<u32> = stack.iter().map(|&id| ids[id as usize]).collect();
                *m.stacks.entry(mapped).or_default() += w;
            }
            for (&(source, line), &w) in &rec.lines {
                *m.lines
                    .entry((rec.sources[source as usize].clone(), line))
                    .or_default() += w;
            }
        }
        m
    }

    fn summary(&self, recording: bool) -> Summary {
        Summary {
            recording,
            hz: self.hz,
            seconds: self.seconds,
            samples: self.samples,
            script_ms: self.total_ns as f64 / 1e6,
            frames: self.frames_run,
            vms: self.vms,
        }
    }
}

/// The flame graph of the current (or last) recording of every VM in `luas`; `None` before
/// the first one.
pub(crate) fn flame(luas: &[&Lua], recording: bool) -> Option<(Summary, FlameNode)> {
    let m = Merged::of(luas);
    if !m.started {
        return None;
    }
    let mut root = FlameNode {
        name: "all scripts".into(),
        kind: Kind::Phase,
        at: None,
        ns: m.total_ns,
        children: Vec::new(),
    };
    for (stack, &w) in &m.stacks {
        let mut node = &mut root;
        for &id in stack {
            node = node.child(&m, id);
            node.ns += w;
        }
    }
    root.sort();
    Some((m.summary(recording), root))
}

/// Each line's share of the recorded script time (percent), for `path`; and a number that
/// changes when boards should redraw.
pub(crate) fn line_heat(luas: &[&Lua], path: &str) -> (u64, BTreeMap<u32, f32>) {
    let m = Merged::of(luas);
    let total = m.total_ns.max(1) as f64;
    let heat = m
        .lines
        .iter()
        .filter(|((s, _), _)| s == path)
        .map(|(&(_, line), &w)| (line, (w as f64 / total * 100.0) as f32))
        .collect();
    (m.shown, heat)
}

/// The number [`line_heat`] returns first: changes when boards should redraw.
pub(crate) fn shown(luas: &[&Lua]) -> u64 {
    luas.iter()
        .map(|lua| lua.app_data_ref::<Recording>().map_or(0, |r| r.shown))
        .sum()
}

/// A line's text from a file one of the VMs loaded, trimmed.
fn code_line(luas: &[&Lua], path: &str, line: u32) -> Option<String> {
    luas.iter().find_map(|lua| {
        let files = lua.app_data_ref::<Files>()?;
        let source = &files.map.get(path)?.source;
        let text = source.lines().nth(line.checked_sub(1)? as usize)?;
        Some(text.trim().to_string())
    })
}

fn hottest(m: &Merged, luas: &[&Lua], top: usize) -> Vec<(String, u32, f64, String)> {
    let total = m.total_ns.max(1) as f64;
    let mut lines: Vec<(&(String, u32), &u64)> = m.lines.iter().collect();
    lines.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    lines
        .into_iter()
        .filter(|((s, _), _)| !s.starts_with("engine/"))
        .take(top)
        .map(|((path, line), &w)| {
            let code = code_line(luas, path, *line).unwrap_or_default();
            (path.clone(), *line, w as f64 / total * 100.0, code)
        })
        .collect()
}

/// The hottest lines: (file, line, percent, code).
pub(crate) fn hot_lines(luas: &[&Lua], top: usize) -> Vec<(String, u32, f64, String)> {
    hottest(&Merged::of(luas), luas, top)
}

/// `script.profile`'s answer: the headline numbers, the functions and lines that took the most
/// time, the time per phase, and (`folded`) every stack in the folded format flame graph
/// tools read (`a;b;c microseconds` per line). Over every VM in `luas`.
pub(crate) fn report(luas: &[&Lua], recording: bool, top: usize, folded: bool) -> Json {
    let m = Merged::of(luas);
    if !m.started {
        return json!({"recording": false, "samples": 0,
            "hint": "start one with {\"seconds\": 5}"});
    }
    let lines = hottest(&m, luas, top);
    let s = m.summary(recording);
    let total = m.total_ns.max(1) as f64;
    let ms = |ns: u64| (ns as f64 / 1e4).round() / 100.0;
    let pct = |ns: u64| (ns as f64 / total * 1000.0).round() / 10.0;
    let n = m.frames.len();
    let (mut own, mut all) = (vec![0u64; n], vec![0u64; n]);
    let mut seen = Vec::new();
    for (stack, &w) in &m.stacks {
        if let Some(&leaf) = stack.last() {
            own[leaf as usize] += w;
        }
        seen.clear();
        for &id in stack {
            if !seen.contains(&id) {
                seen.push(id);
                all[id as usize] += w;
            }
        }
    }
    let mut order: Vec<usize> = (0..n)
        .filter(|&i| m.frames[i].kind != Kind::Phase && all[i] > 0)
        .collect();
    order.sort_by(|&a, &b| all[b].cmp(&all[a]).then(own[b].cmp(&own[a])));
    let functions: Vec<Json> = order
        .into_iter()
        .take(top)
        .map(|i| {
            let f = &m.frames[i];
            json!({
                "name": f.name,
                "source": f.source,
                "line": (f.kind == Kind::Lua).then_some(f.line),
                "engine": f.kind == Kind::Engine,
                "self_ms": ms(own[i]),
                "total_ms": ms(all[i]),
                "self_pct": pct(own[i]),
                "total_pct": pct(all[i]),
            })
        })
        .collect();
    let phases: serde_json::Map<String, Json> = (0..n)
        .filter(|&i| m.frames[i].kind == Kind::Phase)
        .map(|i| (m.frames[i].name.clone(), json!(ms(all[i]))))
        .collect();
    let mut out = json!({
        "recording": s.recording,
        "hz": s.hz,
        "seconds": (s.seconds * 100.0).round() / 100.0,
        "samples": s.samples,
        "frames": s.frames,
        "vms": s.vms,
        "script_ms": (s.script_ms * 100.0).round() / 100.0,
        "ms_per_frame": (s.ms_per_frame() * 1000.0).round() / 1000.0,
        "phases": phases,
        "functions": functions,
        "lines": lines.iter().map(|(path, line, p, code)| json!({
            "source": path, "line": line, "pct": (p * 10.0).round() / 10.0, "code": code,
        })).collect::<Vec<_>>(),
    });
    if folded {
        let name = |id: u32| {
            let f = &m.frames[id as usize];
            match &f.source {
                Some(src) => format!("{} ({}:{})", f.name, src, f.line),
                None => f.name.clone(),
            }
        };
        let mut text: Vec<String> = m
            .stacks
            .iter()
            .map(|(stack, w)| {
                let path: Vec<String> = stack.iter().map(|&id| name(id)).collect();
                format!("{} {}", path.join(";"), w / 1000)
            })
            .collect();
        text.sort();
        out["folded"] = json!(text.join("\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_fold_into_a_flame_tree() {
        let mut rec = Recording::default();
        let update = rec.label("update", Kind::Phase);
        let set = rec.label("e:set", Kind::Engine);
        let tasks = rec.label("tasks", Kind::Phase);
        // Root first, as `add` leaves them.
        rec.stacks.insert(vec![update, set], 300);
        rec.stacks.insert(vec![update], 100);
        rec.stacks.insert(vec![tasks], 50);
        rec.total_ns = 450;
        rec.started = Some(Instant::now());
        let lua = Lua::new();
        lua.set_app_data(rec);
        let (summary, root) = flame(&[&lua], false).unwrap();
        assert_eq!(summary.script_ms, 450.0 / 1e6);
        assert_eq!(root.children[0].name, "update", "biggest first");
        assert_eq!(root.children[0].ns, 400);
        assert_eq!(root.children[0].children[0].name, "e:set");
        assert_eq!(root.children[1].ns, 50);
    }
}
