//! How scripts reach the world, and the bookkeeping shared between Lua callbacks and the runner.
//!
//! The runner is an exclusive system: it holds `&mut World` for as long as scripts run, and lends
//! it to the Lua VM as a pointer in the VM's app data ([`lend_world`]). Callbacks
//! ([`with_world`]) borrow it for the length of one engine call. Outside the runner the pointer
//! is absent and callbacks fail with a plain error instead of touching freed memory.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use bevy_ecs::prelude::*;
use mlua::{Lua, RegistryKey, Thread};

/// The world, lent to the VM while scripts run.
struct WorldPtr(*mut World);

// SAFETY: the pointer is only set while the runner holds `&mut World` on this thread and is
// removed before it returns ([`LentWorld`]'s drop); the VM itself never leaves the runner.
unsafe impl Send for WorldPtr {}

/// Lends `world` to the VM until the guard drops.
pub(crate) fn lend_world(lua: &Lua, world: *mut World) -> LentWorld {
    lua.set_app_data(WorldPtr(world));
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

/// Runs `f` with the world the runner lent. Fails outside the runner (a callback kept and called
/// later from somewhere else).
pub(crate) fn with_world<R>(
    lua: &Lua,
    f: impl FnOnce(&mut World) -> mlua::Result<R>,
) -> mlua::Result<R> {
    let ptr = lua.app_data_ref::<WorldPtr>().map(|p| p.0).ok_or_else(|| {
        mlua::Error::runtime(
            "the world can only be used while scripts run (in start, update, events and tasks)",
        )
    })?;
    // SAFETY: see `WorldPtr`. The runner doesn't touch the world while the VM runs, and every
    // callback's borrow ends before it returns to Lua, so no two borrows overlap.
    f(unsafe { &mut *ptr })
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
    pub new_tasks: Vec<NewTask>,
    pub cancelled: Vec<Thread>,
    pub outbox: Vec<Outgoing>,
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

/// Stops a script that runs too long in one go (an endless loop) instead of freezing the game.
/// Luau calls the interrupt at loop back-edges and calls, so it only reads the clock every
/// 1024th time.
#[derive(Clone)]
pub(crate) struct Watchdog {
    epoch: Instant,
    /// Nanoseconds since `epoch` after which the running script is stopped (0 = no limit).
    deadline: Arc<AtomicU64>,
    ticks: Arc<AtomicU32>,
    /// Set when it stops a script, until the next call is armed: the prelude's `pcall`,
    /// `xpcall` and `coroutine.resume` pass the stop on instead of swallowing it.
    fired: Arc<AtomicBool>,
}

impl Watchdog {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            deadline: Arc::new(AtomicU64::new(0)),
            ticks: Arc::new(AtomicU32::new(0)),
            fired: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn install(&self, lua: &Lua) -> mlua::Result<()> {
        let fired = self.fired.clone();
        lua.globals().set(
            "__script_stopped",
            lua.create_function(move |_, ()| Ok(fired.load(Ordering::Relaxed)))?,
        )?;
        let dog = self.clone();
        lua.set_interrupt(move |_| {
            if dog.ticks.fetch_add(1, Ordering::Relaxed) & 1023 != 0 {
                return Ok(mlua::VmState::Continue);
            }
            let deadline = dog.deadline.load(Ordering::Relaxed);
            if deadline != 0 && dog.epoch.elapsed().as_nanos() as u64 > deadline {
                // Stays armed, and `pcall` rethrows it (prelude), so catching it doesn't help.
                dog.fired.store(true, Ordering::Relaxed);
                return Err(mlua::Error::runtime(STOPPED));
            }
            Ok(mlua::VmState::Continue)
        });
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
