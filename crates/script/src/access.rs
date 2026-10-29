//! How scripts reach the world, and the bookkeeping shared between Lua callbacks and the runner.
//!
//! The runner is an exclusive system: it holds `&mut World` for as long as scripts run, and lends
//! it to the Lua VM as a pointer in the VM's app data ([`lend_world`]). Callbacks
//! ([`with_world`]) borrow it for the length of one engine call. Outside the runner the pointer
//! is absent and callbacks fail with a plain error instead of touching freed memory.
//!
//! In the parallel phase (`parallel_update`, several VMs at once) each VM gets the world shared
//! and read-only ([`lend_world_parallel`]): reads ([`read_world_as`]) work as always, changes
//! ([`write_world_as`], [`edit_transform_as`]) are queued and applied right after the phase, in
//! a fixed order, and anything else ([`with_world_as`]) fails with a hint to call
//! `task.synchronize()` first.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use bevy_ecs::prelude::*;
use engine_core::prelude::Transform;
use mlua::{Lua, RegistryKey, Thread, ffi};

/// The world, lent to the VM while scripts run.
struct WorldPtr {
    ptr: *mut World,
    /// Lent shared, for the parallel phase: reads only, changes are queued.
    parallel: bool,
}

// SAFETY: the pointer is only set while the runner holds the world (`&mut` in the serial
// phases on the runner's thread; `&` shared by every VM in the parallel phase, when nothing
// writes to it) and is removed before the runner gives it back ([`LentWorld`]'s drop).
unsafe impl Send for WorldPtr {}

/// Lends `world` to the VM until the guard drops.
pub(crate) fn lend_world(lua: &Lua, world: *mut World) -> LentWorld {
    lua.set_app_data(WorldPtr {
        ptr: world,
        parallel: false,
    });
    LentWorld { lua: lua.clone() }
}

/// Lends `world` for the parallel phase: read-only and shared with the other VMs running at the
/// same time; changes go to the VM's [`Deferred`] queue.
pub(crate) fn lend_world_parallel(lua: &Lua, world: &World) -> LentWorld {
    lua.set_app_data(WorldPtr {
        ptr: world as *const World as *mut World,
        parallel: true,
    });
    LentWorld { lua: lua.clone() }
}

/// Takes the world back from the VM when dropped. Holds its own handle to the VM, so the
/// runtime stays free to borrow while the world is lent.
pub(crate) struct LentWorld {
    lua: Lua,
}

impl Drop for LentWorld {
    fn drop(&mut self) {
        self.lua.remove_app_data::<WorldPtr>();
    }
}

/// The VM runs `parallel_update` now (the world is shared and read-only).
pub(crate) fn in_parallel(lua: &Lua) -> bool {
    lua.app_data_ref::<WorldPtr>().is_some_and(|p| p.parallel)
}

fn lent(lua: &Lua) -> mlua::Result<(*mut World, bool)> {
    lua.app_data_ref::<WorldPtr>()
        .map(|p| (p.ptr, p.parallel))
        .ok_or_else(|| {
            mlua::Error::runtime(
                "the world can only be used while scripts run (in start, update, events and \
                 tasks)",
            )
        })
}

/// Runs `f` with the world the runner lent. Fails outside the runner (a callback kept and called
/// later from somewhere else).
pub(crate) fn with_world<R>(
    lua: &Lua,
    f: impl FnOnce(&mut World) -> mlua::Result<R>,
) -> mlua::Result<R> {
    with_world_as(lua, "engine", f)
}

/// [`with_world`] for an engine call scripts make, named `label` in the profiler
/// (`"e:set"`, `"fire.ignite"`). Not in the parallel phase: it fails there, pointing to
/// `task.synchronize()`.
pub(crate) fn with_world_as<R>(
    lua: &Lua,
    label: &str,
    f: impl FnOnce(&mut World) -> mlua::Result<R>,
) -> mlua::Result<R> {
    let (ptr, parallel) = lent(lua)?;
    if parallel {
        return Err(mlua::Error::runtime(format!(
            "{label} can't run in parallel_update, where scripts run side by side and only read \
             the world; call task.synchronize() first (the rest of parallel_update then runs \
             after the parallel part), or do it in update"
        )));
    }
    // SAFETY: see `WorldPtr`. The runner doesn't touch the world while the VM runs, and every
    // callback's borrow ends before it returns to Lua, so no two borrows overlap.
    let out = f(unsafe { &mut *ptr });
    crate::profiler::after_engine_call(lua, label);
    out
}

/// Reads the world: in every phase, the parallel one included.
pub(crate) fn read_world_as<R>(
    lua: &Lua,
    label: &str,
    f: impl FnOnce(&World) -> mlua::Result<R>,
) -> mlua::Result<R> {
    let (ptr, _) = lent(lua)?;
    // SAFETY: see `WorldPtr`: shared reads only, while nothing writes (in the parallel phase
    // all changes are queued; in the serial phases only this thread uses the world).
    let out = f(unsafe { &*ptr });
    crate::profiler::after_engine_call(lua, label);
    out
}

/// Changes the world: now in the serial phases; in the parallel phase queued and applied right
/// after it (the VM's changes in the order made, VM after VM).
pub(crate) fn write_world_as(
    lua: &Lua,
    label: &'static str,
    f: impl FnOnce(&mut World) -> mlua::Result<()> + Send + 'static,
) -> mlua::Result<()> {
    let (_, parallel) = lent(lua)?;
    if !parallel {
        return with_world_as(lua, label, f);
    }
    let owner = shared(lua).owner;
    deferred(lua).writes.push(DeferredWrite {
        owner,
        label,
        run: Box::new(f),
    });
    Ok(())
}

/// `e`'s own `Transform`, as the running script sees it: in the parallel phase, with the
/// changes this script made to it, and otherwise as the phase began (so what a script sees
/// doesn't depend on which VM ran which script first).
pub(crate) fn read_transform_as(lua: &Lua, label: &str, e: Entity) -> mlua::Result<Transform> {
    let owner = lua.app_data_ref::<Shared>().and_then(|s| s.owner);
    if let Some(t) = lua.app_data_ref::<Deferred>().and_then(|d| {
        d.index
            .get(&e)
            .map(|&i| &d.transforms[i])
            .filter(|(_, _, by)| *by == owner)
            .map(|(_, t, _)| *t)
    }) {
        return Ok(t);
    }
    read_world_as(lua, label, |w| transform_of(w, e))
}

/// Edits `e`'s `Transform` (adding one if it has none): now, or in the parallel phase on this
/// VM's copy, which later reads see and which is written back after the phase.
pub(crate) fn edit_transform_as(
    lua: &Lua,
    label: &str,
    e: Entity,
    f: impl FnOnce(&mut Transform),
) -> mlua::Result<()> {
    let (_, parallel) = lent(lua)?;
    if !parallel {
        return with_world_as(lua, label, |w| edit_transform(w, e, f));
    }
    let mut t = read_transform_as(lua, label, e)?;
    f(&mut t);
    let owner = shared(lua).owner;
    let mut d = deferred(lua);
    match d.index.get(&e) {
        Some(&i) => d.transforms[i] = (e, t, owner),
        None => {
            let i = d.transforms.len();
            d.transforms.push((e, t, owner));
            d.index.insert(e, i);
        }
    }
    Ok(())
}

pub(crate) fn missing(e: Entity) -> mlua::Error {
    mlua::Error::runtime(format!(
        "entity {e} no longer exists (check entity:exists() before using a kept handle)"
    ))
}

pub(crate) fn transform_of(world: &World, e: Entity) -> mlua::Result<Transform> {
    if world.get_entity(e).is_err() {
        return Err(missing(e));
    }
    Ok(world.get::<Transform>(e).copied().unwrap_or_default())
}

pub(crate) fn edit_transform(
    world: &mut World,
    e: Entity,
    f: impl FnOnce(&mut Transform),
) -> mlua::Result<()> {
    let mut ent = world.get_entity_mut(e).map_err(|_| missing(e))?;
    match ent.get_mut::<Transform>() {
        Some(mut t) => f(&mut t),
        None => {
            let mut t = Transform::default();
            f(&mut t);
            ent.insert(t);
        }
    }
    Ok(())
}

/// A world change queued in the parallel phase.
pub(crate) type WorldWrite = Box<dyn FnOnce(&mut World) -> mlua::Result<()> + Send>;

/// A change a script made in the parallel phase, applied after it.
pub(crate) struct DeferredWrite {
    /// The script instance that made it (its errors are reported as its own).
    pub owner: Option<Entity>,
    pub label: &'static str,
    pub run: WorldWrite,
}

/// What one VM changed in the parallel phase: `Transform`s (with the script that changed each
/// last, which alone sees the change before it is applied) and other changes in the order
/// made.
#[derive(Default)]
pub(crate) struct Deferred {
    pub transforms: Vec<(Entity, Transform, Option<Entity>)>,
    index: HashMap<Entity, usize>,
    pub writes: Vec<DeferredWrite>,
}

fn deferred(lua: &Lua) -> mlua::AppDataRefMut<'_, Deferred> {
    if lua.app_data_ref::<Deferred>().is_none() {
        lua.set_app_data(Deferred::default());
    }
    lua.app_data_mut::<Deferred>()
        .expect("the deferred queue was just made")
}

/// Applies what the VM queued in the parallel phase: its `Transform`s first, then its other
/// changes in order. Returns the failures, by script instance.
pub(crate) fn apply_deferred(lua: &Lua, world: &mut World) -> Vec<(Option<Entity>, String)> {
    let Some(d) = lua.remove_app_data::<Deferred>() else {
        return Vec::new();
    };
    let mut failed = Vec::new();
    for (e, t, by) in d.transforms {
        if let Err(err) = edit_transform(world, e, |old| *old = t) {
            failed.push((by, format!("moving {e} (after parallel_update): {err}")));
        }
    }
    for w in d.writes {
        if let Err(err) = (w.run)(world) {
            failed.push((
                w.owner,
                format!("{} (after parallel_update): {err}", w.label),
            ));
        }
    }
    failed
}

/// A coroutine started by `task.spawn`/`task.delay` (or a script's `start`) and not yet
/// scheduled by the runner.
pub(crate) struct NewTask {
    pub thread: Thread,
    pub owner: Option<Entity>,
    /// Seconds on the owner's clock before it first runs; `None` once it ran and yielded
    /// (then `yielded` says for how long).
    pub delay: Option<f64>,
    pub yielded: Option<f64>,
    pub args: Option<RegistryKey>,
}

/// A message from one script to another (`entity:send`, `script.send`).
pub(crate) struct Outgoing {
    pub to: Entity,
    pub event: String,
    pub data: serde_json::Value,
}

/// A signal a script emitted (`self:emit`), delivered later in the frame.
pub(crate) struct Emitted {
    pub from: Entity,
    pub signal: String,
    /// The values, packed (`table.pack`).
    pub args: RegistryKey,
}

/// A function listening to an entity's signal (`entity:on`, `entity:once`).
pub(crate) struct Listener {
    pub id: u64,
    pub from: Entity,
    pub signal: String,
    /// The script that listens (its tasks run the function); none from the console.
    pub owner: Option<Entity>,
    pub func: RegistryKey,
    pub once: bool,
}

/// One script error, kept for `script.errors` and shown on screen.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScriptError {
    /// Script file (relative to the scripts folder), when known.
    pub path: Option<String>,
    /// Entity the script runs on (its id), when it has one.
    pub entity: Option<u64>,
    pub message: String,
    /// World seconds when it happened.
    pub at_secs: f64,
}

/// What callbacks and the runner share. Borrowed only for short stretches, never across a call
/// into Lua.
#[derive(Default)]
pub(crate) struct Shared {
    /// State breakpoints reached this frame: (entity, enter or exit, state, why).
    pub state_hits: Vec<(Entity, String, String, String)>,
    pub new_tasks: Vec<NewTask>,
    pub cancelled: Vec<Thread>,
    pub outbox: Vec<Outgoing>,
    pub emitted: Vec<Emitted>,
    pub listeners: Vec<Listener>,
    pub next_listener: u64,
    pub errors: VecDeque<ScriptError>,
    pub log: VecDeque<String>,
    /// The script instance whose code runs now (tasks it starts belong to it).
    pub owner: Option<Entity>,
    /// The file being loaded now (for `require` dependencies).
    pub loading: Vec<String>,
    /// `module path → files that required it`.
    pub required_by: Vec<(String, String)>,
    pub world_secs: f64,
    /// Log lines and errors ever pushed (the debugger forwards the new ones).
    pub log_total: u64,
    pub errors_total: u64,
}

/// Errors and log lines kept for the API.
pub(crate) const KEEP_LINES: usize = 200;

pub(crate) fn shared(lua: &Lua) -> mlua::AppDataRefMut<'_, Shared> {
    lua.app_data_mut::<Shared>()
        .expect("the script runtime installs its shared state first")
}

pub(crate) fn push_error(lua: &Lua, path: Option<&str>, entity: Option<Entity>, message: &str) {
    let mut s = shared(lua);
    let at_secs = s.world_secs;
    tracing::warn!(target: "script", "{message}");
    s.errors.push_back(ScriptError {
        path: path.map(str::to_string),
        entity: entity.map(|e| e.to_bits()),
        message: message.to_string(),
        at_secs,
    });
    s.errors_total += 1;
    while s.errors.len() > KEEP_LINES {
        s.errors.pop_front();
    }
}

pub(crate) fn push_log(lua: &Lua, line: String) {
    tracing::info!(target: "script", "{line}");
    let mut s = shared(lua);
    s.log.push_back(line);
    s.log_total += 1;
    while s.log.len() > KEEP_LINES {
        s.log.pop_front();
    }
}

/// What a script stopped by the [`Watchdog`] fails with.
pub(crate) const STOPPED: &str = "the script ran too long without yielding and was stopped (an \
     endless loop? use task.wait() inside long loops)";

type InterruptFn = unsafe extern "C-unwind" fn(*mut ffi::lua_State, c_int);

/// mlua's interrupt function (one for every VM), which runs [`Watchdog::install`]'s callback.
static SLOW_INTERRUPT: OnceLock<InterruptFn> = OnceLock::new();

thread_local! {
    static INTERRUPTS: Cell<u32> = const { Cell::new(0) };
}

/// Luau calls the interrupt at every loop back-edge, call and return, and mlua's interrupt goes
/// through a full Rust callback each time (tens of nanoseconds; a tight loop's iteration takes a
/// few). This one counts in a thread-local and passes every 1024th call on to mlua's (for the
/// watchdog's clock check), and the ones that find a profiler sample due.
unsafe extern "C-unwind" fn fast_interrupt(l: *mut ffi::lua_State, gc: c_int) {
    if gc >= 0 {
        return; // mlua ignores the collector's interrupts too
    }
    let n = INTERRUPTS.with(|c| {
        let n = c.get().wrapping_add(1);
        c.set(n);
        n
    });
    if n & 1023 != 0 && !crate::profiler::due_anywhere() {
        return;
    }
    if let Some(slow) = SLOW_INTERRUPT.get() {
        // SAFETY: mlua's own interrupt, called as Luau would have called it. Nothing of this
        // frame needs dropping if it raises an error.
        unsafe { slow(l, gc) }
    }
}

/// Stops a script that runs too long in one go (an endless loop) instead of freezing the game.
/// Luau calls the interrupt at loop back-edges and calls; [`fast_interrupt`] lets every 1024th
/// through, and only those read the clock.
#[derive(Clone)]
pub(crate) struct Watchdog {
    epoch: Instant,
    /// Nanoseconds since `epoch` after which the running script is stopped (0 = no limit).
    deadline: Arc<AtomicU64>,
    /// Set when it stops a script, until the next call is armed: the prelude's `pcall`,
    /// `xpcall` and `coroutine.resume` pass the stop on instead of swallowing it.
    fired: Arc<AtomicBool>,
    /// The profiler's sample clock: the interrupt takes a sample when one is due.
    pub(crate) clock: Arc<crate::profiler::Clock>,
}

impl Watchdog {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            deadline: Arc::new(AtomicU64::new(0)),
            fired: Arc::new(AtomicBool::new(false)),
            clock: Arc::default(),
        }
    }

    pub fn install(&self, lua: &Lua) -> mlua::Result<()> {
        let fired = self.fired.clone();
        lua.globals().set(
            "__script_stopped",
            lua.create_function(move |_, ()| Ok(fired.load(Ordering::Relaxed)))?,
        )?;
        let dog = self.clone();
        lua.set_interrupt(move |lua| {
            if dog.clock.due.load(Ordering::Relaxed) {
                crate::profiler::sample(lua, None);
            }
            let deadline = dog.deadline.load(Ordering::Relaxed);
            if deadline != 0 && dog.epoch.elapsed().as_nanos() as u64 > deadline {
                // Stays armed, and `pcall` rethrows it (prelude), so catching it doesn't help.
                dog.fired.store(true, Ordering::Relaxed);
                return Err(mlua::Error::runtime(STOPPED));
            }
            Ok(mlua::VmState::Continue)
        });
        // SAFETY: only swaps the callback pointer mlua just set for the filter in front of it.
        unsafe {
            lua.exec_raw::<()>((), |state| {
                let callbacks = ffi::lua_callbacks(ffi::lua_mainthread(state));
                if let Some(slow) = (*callbacks).interrupt {
                    let _ = SLOW_INTERRUPT.set(slow);
                    (*callbacks).interrupt = Some(fast_interrupt);
                }
            })?;
        }
        Ok(())
    }

    /// Arms the limit for the next call into Lua.
    pub fn arm(&self, limit_ms: f32) {
        self.fired.store(false, Ordering::Relaxed);
        let now = self.epoch.elapsed().as_nanos() as u64;
        self.deadline.store(
            now + (limit_ms.max(1.0) as f64 * 1.0e6) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn disarm(&self) {
        self.deadline.store(0, Ordering::Relaxed);
    }

    /// A call into Lua is running (armed and not held by the debugger).
    pub fn running(&self) -> bool {
        self.deadline.load(Ordering::Relaxed) != 0
    }

    /// Takes the running call's limit away (while the debugger has the scripts stopped, or runs
    /// its own code); [`Watchdog::resume`] gives it back, moved on by the time held.
    pub fn suspend(&self) -> Held {
        Held {
            deadline: self.deadline.swap(0, Ordering::Relaxed),
            at: self.epoch.elapsed().as_nanos() as u64,
            fired: self.fired.load(Ordering::Relaxed),
        }
    }

    pub fn resume(&self, held: Held) {
        let deadline = match held.deadline {
            0 => 0,
            d => d + (self.epoch.elapsed().as_nanos() as u64).saturating_sub(held.at),
        };
        self.deadline.store(deadline, Ordering::Relaxed);
        self.fired.store(held.fired, Ordering::Relaxed);
    }
}

/// A call's limit while [`Watchdog::suspend`]ed.
pub(crate) struct Held {
    deadline: u64,
    at: u64,
    fired: bool,
}
