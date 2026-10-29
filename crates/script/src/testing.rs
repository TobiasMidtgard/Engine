//! Script tests: gameplay checks written in Luau, run inside the live game.
//!
//! A test file is any `*.test.luau` under the scripts folder. It reads like Jest (Roblox's
//! Jest Lua and TestEZ have the same shape): `describe` groups, `it` tests, `expect(value)`
//! matchers. Each test runs as a task in the running game, the way Unreal's functional tests
//! and Unity's play mode tests run in a level: it gets a `t` that spawns entities (gone after
//! the test), waits world seconds or frames, presses E on things and listens to signals, so a
//! test can check what scripts actually do frame by frame, not only what a function returns.
//!
//! ```lua
//! describe("Lamp", function()
//!     it("goes out when pressed", function(t)
//!         local lamp = t:spawn({ name = "Test lamp", components = {
//!             Transform = {}, PointLight = {}, Script = { path = "examples/lamp.luau" } } })
//!         t:press(lamp)
//!         expect(lamp.script.lit).to_be(false)
//!     end)
//! end)
//! ```
//!
//! `script.test` loads the files (their `describe` bodies run then, collecting the tests) and
//! starts the run; `script.tests` reports it, test by test, with the line each failure stopped
//! on. The Luau half (`testing.luau`) collects and runs; this half loads files, keeps the
//! results ([`TestRun`]) and presses E for `t:press`. Research and sources:
//! `engine-design/scripting-research.md` §18.

use std::path::Path;

use bevy_ecs::message::Messages;
use bevy_ecs::prelude::*;
use engine_core::math::DVec3;
use engine_core::prelude::{Name, Transform};
use mlua::{Function, Lua, Table, Thread, Value};
use serde::Serialize;

use crate::access::{shared, with_world_as};
use crate::entity::EntityRef;

/// The registry slot of the loaded `testing.luau`.
const FRAMEWORK: &str = "engine_script.testing";

/// Test files end with this.
pub const TEST_SUFFIX: &str = ".test.luau";

/// Whether `path` is a test file.
pub fn is_test_file(path: &str) -> bool {
    path.ends_with(TEST_SUFFIX)
}

/// One test of a run, as `script.tests` reports it.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct TestResult {
    /// The test file, relative to the scripts folder.
    pub file: String,
    /// The names of the `describe`s around it and its own, joined with " › ".
    pub title: String,
    /// The line of its `it(` (1-based).
    pub line: u32,
    /// "queued", "running", "passed", "failed", "skipped" (`it.skip`, or another test has
    /// `only`) or "stopped" (the run was stopped while it ran).
    pub status: String,
    /// Why it failed: the expect's message, or the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The test file's line it failed on (the `expect` that didn't hold).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<u32>,
    /// World seconds and frames it took.
    pub secs: f64,
    pub frames: u32,
}

/// A test file that didn't load (a syntax error, or an error in a `describe` body).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BrokenFile {
    pub file: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// The current (or last) run, kept in the VM.
#[derive(Default)]
pub(crate) struct TestRun {
    /// Counts runs, so a poller can tell a new run from the last one.
    pub id: u64,
    pub running: bool,
    pub files: Vec<String>,
    pub results: Vec<TestResult>,
    pub broken: Vec<BrokenFile>,
    /// World seconds when it started, and how long it took once done.
    pub started: f64,
    pub took: f64,
    /// The runner task and the test it runs now (to stop them).
    pub runner: Option<Thread>,
    pub worker: Option<Thread>,
}

impl TestRun {
    /// `script.tests`: the run so far.
    pub(crate) fn describe(&self, now: f64) -> serde_json::Value {
        let count = |s: &str| self.results.iter().filter(|r| r.status == s).count();
        serde_json::json!({
            "run": self.id,
            "state": if self.running { "running" } else if self.id == 0 { "idle" } else { "done" },
            "files": self.files,
            "passed": count("passed"),
            "failed": count("failed") + self.broken.len(),
            "skipped": count("skipped"),
            "queued": count("queued"),
            "secs": if self.running { now - self.started } else { self.took },
            "results": self.results,
            "broken": self.broken,
        })
    }
}

pub(crate) fn test_run(lua: &Lua) -> mlua::AppDataRefMut<'_, TestRun> {
    lua.app_data_mut::<TestRun>()
        .expect("the script runtime installs the test run with its shared state")
}

/// Every test file under `root`, relative to it, sorted.
pub fn test_files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if is_test_file(&rel) {
                    out.push(rel);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// A new session of the framework (`testing.luau`, loaded once per VM), reporting into the
/// VM's [`TestRun`].
pub(crate) fn session(lua: &Lua, env: Table) -> mlua::Result<Table> {
    let framework: Table = match lua.named_registry_value::<Value>(FRAMEWORK)? {
        Value::Table(t) => t,
        _ => {
            let t: Table = lua
                .load(include_str!("testing.luau"))
                .set_name("@engine/testing.luau")
                .set_environment(env)
                .call(())?;
            lua.set_named_registry_value(FRAMEWORK, t.clone())?;
            t
        }
    };
    let report = lua.create_function(report)?;
    let press = lua.create_function(|lua, (target, by): (EntityRef, Option<EntityRef>)| {
        press(lua, target.0, by.map(|b| b.0))
    })?;
    framework.get::<Function>("session")?.call((report, press))
}

/// The framework's `report(kind, ...)`: the plan, each test's start and end, the end of the run.
fn report(lua: &Lua, args: mlua::MultiValue) -> mlua::Result<()> {
    let mut args = args.into_iter();
    let kind = match args.next() {
        Some(Value::String(s)) => s.to_str()?.to_string(),
        _ => return Ok(()),
    };
    let mut next = || args.next().unwrap_or(Value::Nil);
    let now = shared(lua).world_secs;
    let mut run = test_run(lua);
    match kind.as_str() {
        "plan" => {
            let Value::Table(rows) = next() else {
                return Ok(());
            };
            run.results.clear();
            for row in rows.sequence_values::<Table>() {
                let row = row?;
                let skip: bool = row.get("skip").unwrap_or(false);
                run.results.push(TestResult {
                    file: row.get("file")?,
                    title: row.get("title")?,
                    line: row.get("line").unwrap_or(0),
                    status: if skip { "skipped" } else { "queued" }.into(),
                    ..TestResult::default()
                });
            }
        }
        "start" => {
            let i = index(next());
            if let Some(r) = i.and_then(|i| run.results.get_mut(i)) {
                r.status = "running".into();
            }
            run.worker = match next() {
                Value::Thread(t) => Some(t),
                _ => None,
            };
        }
        "end" => {
            let i = index(next());
            let passed = matches!(next(), Value::Boolean(true));
            let message = match next() {
                Value::String(s) => Some(s.to_string_lossy().to_string()),
                _ => None,
            };
            let failed_at = match next() {
                Value::Integer(n) => Some(n as u32),
                Value::Number(n) => Some(n as u32),
                _ => None,
            };
            let secs = match next() {
                Value::Number(n) => n,
                Value::Integer(n) => n as f64,
                _ => 0.0,
            };
            let frames = match next() {
                Value::Integer(n) => n as u32,
                Value::Number(n) => n as u32,
                _ => 0,
            };
            run.worker = None;
            if let Some(r) = i.and_then(|i| run.results.get_mut(i)) {
                r.status = if passed { "passed" } else { "failed" }.into();
                r.message = message;
                r.failed_at = failed_at;
                r.secs = secs;
                r.frames = frames;
            }
        }
        "done" => {
            run.running = false;
            run.took = now - run.started;
            run.runner = None;
            let (passed, failed) = (
                run.results.iter().filter(|r| r.status == "passed").count(),
                run.results.iter().filter(|r| r.status == "failed").count() + run.broken.len(),
            );
            tracing::info!(target: "script", "tests: {passed} passed, {failed} failed");
        }
        _ => {}
    }
    Ok(())
}

/// A 1-based Lua index as a 0-based one.
fn index(v: Value) -> Option<usize> {
    match v {
        Value::Integer(i) if i >= 1 => Some(i as usize - 1),
        Value::Number(n) if n >= 1.0 => Some(n as usize - 1),
        _ => None,
    }
}

/// `t:press(target, by)`: the player (or `by`) presses E on `target`, as the interaction system
/// would; its script's `on_interact` runs next frame.
fn press(lua: &Lua, target: Entity, by: Option<Entity>) -> mlua::Result<()> {
    with_world_as(lua, "t:press", |w| {
        if !w.contains_resource::<Messages<engine_world::Interact>>() {
            return Err(mlua::Error::runtime(
                "t:press: this game has no interaction system (no Interact messages)",
            ));
        }
        let actor = by
            .or_else(|| {
                let mut q = w.try_query::<(Entity, &Name)>()?;
                q.iter(w)
                    .find(|(_, n)| n.as_str() == "Player")
                    .map(|(e, _)| e)
            })
            .unwrap_or(target);
        let from = w
            .get::<Transform>(actor)
            .map(|t| t.translation)
            .unwrap_or(DVec3::ZERO);
        w.write_message(engine_world::Interact {
            actor,
            target,
            from,
        });
        Ok(())
    })
}
