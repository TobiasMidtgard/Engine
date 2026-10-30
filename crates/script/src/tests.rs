//! The runtime end to end: scripts in a temporary folder, run by the plugin in a headless app.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use serde_json::{Value, json};

use crate::{Script, ScriptActor, ScriptPlugin, ScriptRuntime, ScriptSettings};

/// A scripts folder of its own for one test, removed when the test ends.
struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "engine_script_test_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, path: &str, source: &str) {
        let full = self.0.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, source).unwrap();
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app at 30 frames a second that runs scripts from `folder`, checking files every frame.
fn app_in(folder: &Path) -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.insert_resource(ScriptSettings {
        folder: folder.display().to_string(),
        hot_reload: true,
        poll_secs: 0.0,
        call_limit_ms: 250.0,
        memory_limit_mb: 64,
        debug_port: None,
        parallel_vms: None,
    });
    app.register_api(
        "test.add",
        "Adds a and b.",
        r#"{"a": 1, "b": 2}"#,
        |_, args| {
            Ok(json!(
                args["a"].as_f64().unwrap_or(0.0) + args["b"].as_f64().unwrap_or(0.0)
            ))
        },
    );
    app.add_plugins(ScriptPlugin);
    app
}

fn spawn_script(app: &mut App, name: &str, path: &str) -> Entity {
    app.world
        .spawn((
            Name::new(name.to_string()),
            Transform::default(),
            Script::new(path),
        ))
        .id()
}

fn eval(app: &mut App, code: &str) -> Value {
    call_api(&mut app.world, "script.eval", json!({ "code": code }))
        .unwrap_or_else(|e| panic!("{code}: {e}"))
}

fn errors(app: &mut App) -> Vec<String> {
    let all = call_api(&mut app.world, "script.errors", json!({})).unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap_or("").to_string())
        .collect()
}

/// What `script.list` says about one loaded file.
fn file_info(app: &mut App, path: &str) -> Value {
    let list = call_api(&mut app.world, "script.list", json!({})).unwrap();
    list["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path)
        .cloned()
        .unwrap_or_else(|| panic!("{path} isn't loaded: {list}"))
}

/// Rewrites a file so its stamp changes even within one clock tick (the length differs).
fn edit(folder: &Folder, path: &str, source: &str) {
    folder.write(path, source);
}

const MOVER: &str = r#"
local Mover = {}
Mover.props = { speed = 3 }

function Mover:start()
	self.sum = test.add({ a = 2, b = 3 })
	self.frames = 0
end

function Mover:update(dt)
	self.frames += 1
	self.entity.position += vec3(self.props.speed * dt, 0, 0)
end

return Mover
"#;

#[test]
fn a_script_calls_engine_functions_and_moves_its_entity() {
    let folder = Folder::new();
    folder.write("mover.luau", MOVER);
    let mut app = app_in(&folder.0);
    let e = spawn_script(&mut app, "Mover", "mover");
    app.update_n(31);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let x = app.world.get::<Transform>(e).unwrap().translation.x;
    assert!(
        (2.5..3.5).contains(&x),
        "moved {x} m in about a second at 3 m/s"
    );
    assert_eq!(eval(&mut app, "world.find('Mover').script.sum"), json!(5));
}

#[test]
fn props_on_the_entity_override_the_defaults() {
    let folder = Folder::new();
    folder.write("mover.luau", MOVER);
    let mut app = app_in(&folder.0);
    let e = app
        .world
        .spawn((
            Name::new("Fast"),
            Transform::default(),
            Script::new("mover.luau").with_props(json!({ "speed": 30 })),
        ))
        .id();
    app.update_n(31);
    let x = app.world.get::<Transform>(e).unwrap().translation.x;
    assert!(x > 25.0, "{x}");
    // Changing the component's props reaches the running script.
    app.world.get_mut::<Script>(e).unwrap().props = json!({ "speed": 0 });
    app.update_n(2);
    let before = app.world.get::<Transform>(e).unwrap().translation.x;
    app.update_n(10);
    assert_eq!(app.world.get::<Transform>(e).unwrap().translation.x, before);
}

const TUNED: &str = r#"
local wave = require("lib/wave")
local Tuned = {}
Tuned.props = {
	-- @group Light
	lumens = 1800, -- at full brightness @range 0 5000 @unit lm
	tint = vec3(1, 0.5, 0.25),
	-- @group Motion
	swing = math.pi / 4, -- how far it sways @angle
	wave = { speed = 2, height = 0.5 }, -- how it bobs
	target = "Door", -- who hears it @entity
}

function Tuned:start()
	self.t = 0
	self.lit = true
end

function Tuned:update(dt)
	self.t += dt
	self.height = self.props.wave.height
	self.speed = self.props.wave.speed
	self.bob = wave.bob(self.t, 1)
end

return Tuned
"#;

#[test]
fn the_editor_reads_props_with_their_hints_and_the_live_state() {
    use crate::props::{self, PropKind};
    let folder = Folder::new();
    folder.write(
        "lib/wave.luau",
        "return { bob = function(t, s) return math.sin(t * s) end }",
    );
    folder.write("tuned.luau", TUNED);
    let mut app = app_in(&folder.0);
    // Before anything runs: read in a VM of its own.
    let read = props::of(&mut app.world, "tuned").unwrap();
    let names: Vec<&str> = read.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["lumens", "tint", "swing", "wave", "target"]);
    assert_eq!(read[0].unit.as_deref(), Some("lm"));
    assert!(read[0].slider);
    assert_eq!(read[0].group.as_deref(), Some("Light"));
    assert_eq!(read[0].line, 6);
    assert_eq!(read[1].kind, PropKind::Color);
    assert_eq!(read[2].kind, PropKind::Angle);
    assert!((read[2].default.as_f64().unwrap() - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    assert_eq!(read[3].kind, PropKind::Table);
    assert_eq!(read[3].doc, "how it bobs");
    assert_eq!(read[4].kind, PropKind::Entity);
    assert!(
        !app.world.contains_resource::<ScriptRuntime>(),
        "reading runs nothing"
    );
    // Files to pick from: scripts, not the shared modules.
    assert_eq!(props::files(&app.world), ["tuned.luau"]);
    // A new version on disk is read again; a broken one says why.
    folder.write(
        "tuned.luau",
        &TUNED.replace("@range 0 5000", "@range 0 9000 50"),
    );
    let read = props::of(&mut app.world, "tuned").unwrap();
    assert_eq!((read[0].max, read[0].step), (Some(9000.0), Some(50.0)));
    folder.write(
        "tuned.luau",
        &TUNED.replace("swing = math.pi / 4", "swing = math.pi /"),
    );
    let err = props::of(&mut app.world, "tuned").unwrap_err();
    assert!(err.contains("tuned.luau:9"), "{err}");
    folder.write("tuned.luau", TUNED);

    // Playing: a table prop set in part keeps its other defaults, and the panel sees `self`.
    let e = app
        .world
        .spawn((
            Name::new("Tuned"),
            Transform::default(),
            Script::new("tuned").with_props(json!({ "wave": { "speed": 3 } })),
        ))
        .id();
    app.update_n(3);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let state = props::state(&app.world, e).unwrap();
    let names: Vec<&str> = state.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["bob", "height", "lit", "speed", "t"]);
    let value = |n: &str| state.iter().find(|v| v.name == n).unwrap().value.clone();
    assert_eq!(value("height"), "0.5");
    assert_eq!(value("speed"), "3");
    assert_eq!(value("lit"), "true");
    let plain = app.world.spawn(Name::new("No script")).id();
    assert!(props::state(&app.world, plain).is_none());
}

const COUNTER_V1: &str = r#"
local Counter = {}
Counter.props = { step = 1, label = "one" }

function Counter:start()
	self.count = 0
end

function Counter:update(dt)
	self.count += self.props.step
end

function Counter:name()
	return "first"
end

return Counter
"#;

const COUNTER_V2: &str = r#"
local Counter = {}
Counter.props = { step = 1, label = "two" }

function Counter:start()
	self.count = 0
end

function Counter:update(dt)
	self.count += self.props.step
end

function Counter:name()
	return "second"
end

function Counter:on_reload()
	self.reloaded = (self.reloaded or 0) + 1
end

return Counter
"#;

#[test]
fn hot_reload_keeps_state_and_swaps_in_the_new_methods() {
    let folder = Folder::new();
    folder.write("counter.luau", COUNTER_V1);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Counter", "counter.luau");
    app.update_n(10);
    let before = eval(&mut app, "world.find('Counter').script.count")
        .as_f64()
        .unwrap();
    assert!(before >= 9.0, "{before}");
    edit(&folder, "counter.luau", COUNTER_V2);
    app.update_n(3);
    let s = "world.find('Counter').script";
    assert_eq!(eval(&mut app, &format!("{s}:name()")), json!("second"));
    assert_eq!(eval(&mut app, &format!("{s}.reloaded")), json!(1));
    // The count kept going: the instance wasn't restarted.
    let after = eval(&mut app, &format!("{s}.count")).as_f64().unwrap();
    assert!(after > before, "{before} -> {after}");
    // A default the instance doesn't override follows the file.
    assert_eq!(eval(&mut app, &format!("{s}.props.label")), json!("two"));
    assert_eq!(file_info(&mut app, "counter.luau")["version"], json!(2));
}

#[test]
fn a_broken_edit_keeps_the_old_code_running_until_it_is_fixed() {
    let folder = Folder::new();
    folder.write("counter.luau", COUNTER_V1);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Counter", "counter.luau");
    app.update_n(5);
    edit(
        &folder,
        "counter.luau",
        "local Counter = {\nfunction Counter:update(\nreturn",
    );
    app.update_n(3);
    let errs = errors(&mut app);
    assert!(
        errs.iter().any(|e| e.contains("counter.luau")),
        "the compile error names the file: {errs:?}"
    );
    let s = "world.find('Counter').script";
    assert_eq!(eval(&mut app, &format!("{s}:name()")), json!("first"));
    let a = eval(&mut app, &format!("{s}.count")).as_f64().unwrap();
    app.update_n(3);
    let b = eval(&mut app, &format!("{s}.count")).as_f64().unwrap();
    assert!(b > a, "still counting on the old code");
    assert!(file_info(&mut app, "counter.luau")["error"].is_string());

    edit(&folder, "counter.luau", COUNTER_V2);
    app.update_n(2);
    assert_eq!(eval(&mut app, &format!("{s}:name()")), json!("second"));
    assert!(file_info(&mut app, "counter.luau")["error"].is_null());
}

#[test]
fn restart_on_reload_starts_instances_again() {
    let folder = Folder::new();
    let source = |n: u32| {
        format!(
            "local S = {{ reload = \"restart\" }}\nfunction S:start() self.v = {n} \
             self.started = (self.started or 0) + 1 end\nreturn S\n"
        )
    };
    folder.write("r.luau", &source(1));
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "R", "r");
    app.update_n(2);
    folder.write("r.luau", &source(22));
    app.update_n(2);
    assert_eq!(eval(&mut app, "world.find('R').script.v"), json!(22));
    assert_eq!(eval(&mut app, "world.find('R').script.started"), json!(1));
}

#[test]
fn waits_count_world_time_on_the_entitys_own_clock() {
    let folder = Folder::new();
    folder.write(
        "waiter.luau",
        "local W = {}\nfunction W:start()\n\ttask.wait(1)\n\tself.done = true\nend\nreturn W\n",
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Normal", "waiter");
    let slow = spawn_script(&mut app, "Slow", "waiter");
    app.world.entity_mut(slow).insert(LocalTimeScale {
        factor: 0.5,
        ..Default::default()
    });
    app.update_n(25);
    assert_eq!(
        eval(&mut app, "world.find('Normal').script.done"),
        Value::Null
    );
    app.update_n(10);
    assert_eq!(
        eval(&mut app, "world.find('Normal').script.done"),
        json!(true)
    );
    assert_eq!(
        eval(&mut app, "world.find('Slow').script.done"),
        Value::Null
    );
    app.update_n(30);
    assert_eq!(
        eval(&mut app, "world.find('Slow').script.done"),
        json!(true)
    );
}

#[test]
fn tasks_tweens_and_every_run_and_stop_with_their_entity() {
    let folder = Folder::new();
    folder.write(
        "ticker.luau",
        r#"
local T = {}
function T:start()
	self.ticks = 0
	self:every(0.1, "tick")
	task.spawn(function()
		task.tween(0.5, function(a) self.alpha = a end, ease.smooth)
		self.tweened = true
	end)
end
function T:tick()
	self.ticks += 1
end
return T
"#,
    );
    let mut app = app_in(&folder.0);
    let e = spawn_script(&mut app, "Ticker", "ticker");
    app.update_n(8);
    // A task that waits 0 seconds comes back next frame, so the tween takes its half second.
    let alpha = eval(&mut app, "world.find('Ticker').script.alpha")
        .as_f64()
        .unwrap();
    assert!(alpha > 0.0 && alpha < 1.0, "{alpha}");
    app.update_n(23);
    let ticks = eval(&mut app, "world.find('Ticker').script.ticks")
        .as_f64()
        .unwrap();
    assert!((8.0..=11.0).contains(&ticks), "{ticks} ticks in a second");
    assert_eq!(
        eval(&mut app, "world.find('Ticker').script.alpha"),
        json!(1)
    );
    assert_eq!(
        eval(&mut app, "world.find('Ticker').script.tweened"),
        json!(true)
    );
    let tasks = |app: &mut App| {
        call_api(&mut app.world, "script.list", json!({})).unwrap()["tasks"]
            .as_u64()
            .unwrap()
    };
    assert!(tasks(&mut app) >= 1);
    app.world.despawn(e);
    app.update_n(2);
    assert_eq!(tasks(&mut app), 0, "the entity's tasks went with it");
}

#[test]
fn an_endless_loop_is_stopped_instead_of_freezing_the_game() {
    let folder = Folder::new();
    folder.write(
        "stuck.luau",
        "local S = {}\nfunction S:update()\n\twhile true do\n\t\tpcall(function() while true do end end)\n\tend\nend\nreturn S\n",
    );
    folder.write("mover.luau", MOVER);
    let mut app = app_in(&folder.0);
    app.world.resource_mut::<ScriptSettings>().call_limit_ms = 40.0;
    spawn_script(&mut app, "Stuck", "stuck");
    let mover = spawn_script(&mut app, "Mover", "mover");
    let t0 = std::time::Instant::now();
    app.update_n(3);
    assert!(t0.elapsed().as_secs_f32() < 5.0);
    let errs = errors(&mut app);
    assert!(errs.iter().any(|e| e.contains("too long")), "{errs:?}");
    // The other script kept running.
    assert!(app.world.get::<Transform>(mover).unwrap().translation.x > 0.0);
}

#[test]
fn scripts_cannot_reach_each_others_globals_or_change_the_libraries() {
    let folder = Folder::new();
    folder.write(
        "a.luau",
        "local A = {}\nfunction A:start()\n\tleaked = 42\n\tself.ok = pcall(function() string.upper = nil end)\nend\nreturn A\n",
    );
    folder.write(
        "b.luau",
        "local B = {}\nfunction B:start()\n\tself.seen = leaked\nend\nreturn B\n",
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "A", "a");
    app.update_n(1);
    spawn_script(&mut app, "B", "b");
    app.update_n(2);
    assert_eq!(eval(&mut app, "world.find('A').script.ok"), json!(false));
    assert_eq!(eval(&mut app, "world.find('B').script.seen"), Value::Null);
    assert_eq!(eval(&mut app, "string.upper('x')"), json!("X"));
}

#[test]
fn editing_a_required_module_reloads_the_scripts_that_use_it() {
    let folder = Folder::new();
    folder.write(
        "lib/numbers.luau",
        "return { value = function() return 1 end }\n",
    );
    folder.write(
        "user.luau",
        "local numbers = require('lib/numbers')\nlocal U = {}\nfunction U:update()\n\tself.v = numbers.value()\nend\nreturn U\n",
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "User", "user");
    app.update_n(2);
    assert_eq!(eval(&mut app, "world.find('User').script.v"), json!(1));
    folder.write(
        "lib/numbers.luau",
        "return { value = function() return 2 end } -- edited\n",
    );
    app.update_n(3);
    assert_eq!(eval(&mut app, "world.find('User').script.v"), json!(2));
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

#[test]
fn the_console_runs_expressions_and_statements() {
    let folder = Folder::new();
    folder.write("mover.luau", MOVER);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Mover", "mover");
    app.update_n(1);
    assert_eq!(eval(&mut app, "1 + 2"), json!(3));
    assert_eq!(eval(&mut app, "vec3(1, 2, 3) * 2"), json!([2, 4, 6]));
    assert_eq!(eval(&mut app, "test.add { a = 1, b = 1 }"), json!(2));
    assert_eq!(
        eval(
            &mut app,
            "local e = world.find('Mover') e.position = vec3(0, 5, 0) return e.position.y"
        ),
        json!(5)
    );
    let bad = call_api(
        &mut app.world,
        "script.eval",
        json!({"code": "test.nope()"}),
    );
    assert!(
        bad.unwrap_err().contains("test.add"),
        "suggests a near name"
    );
}

#[test]
fn messages_and_interactions_reach_scripts() {
    let folder = Folder::new();
    folder.write(
        "door.luau",
        r#"
local Door = {}
function Door:open(data)
	task.wait(0.2) -- event handlers run as tasks, so they may wait
	self.opened = data.by
end
function Door:on_interact(actor)
	self.used_by = actor.name
end
return Door
"#,
    );
    let mut app = app_in(&folder.0);
    app.add_message::<engine_world::Interact>();
    let door = spawn_script(&mut app, "Door", "door");
    let player = app.world.spawn(Name::new("Player")).id();
    app.update_n(1);
    call_api(
        &mut app.world,
        "script.send",
        json!({"entity": {"name": "Door"}, "event": "open", "data": {"by": "lever"}}),
    )
    .unwrap();
    app.world.write_message(engine_world::Interact {
        actor: player,
        target: door,
        from: DVec3::ZERO,
    });
    app.update_n(3);
    assert_eq!(
        eval(&mut app, "world.find('Door').script.used_by"),
        json!("Player")
    );
    assert_eq!(
        eval(&mut app, "world.find('Door').script.opened"),
        Value::Null
    );
    app.update_n(6);
    assert_eq!(
        eval(&mut app, "world.find('Door').script.opened"),
        json!("lever")
    );
}

const BELL: &str = r#"
local Bell = {}
Bell.signals = {
	rang = "times: number, by: Entity", -- each ring
}
function Bell:ring(by)
	self.times = (self.times or 0) + 1
	self:emit("rang", self.times, by)
end
function Bell:typo()
	self:emit("rnag")
end
return Bell
"#;

const EAR: &str = r#"
local Ear = {}
function Ear:start()
	self.heard = {}
	self.fixed = {}
	-- Code listeners: every ring, and only the next one; and a task that waits for one.
	local bell = world.find("Bell")
	bell:on("rang", function(times, by)
		table.insert(self.heard, `{times} by {by.name}`)
	end)
	self.first = 0
	bell:once("rang", function(times)
		self.first += times
	end)
	task.spawn(function()
		local times = task.wait_for(bell, "rang")
		self.waited = times
	end)
end
function Ear:hear(times, by)
	self.wired = `{times} {by.name}`
end
function Ear:fixed_values(a, b)
	table.insert(self.fixed, `{a} {b}`)
end
return Ear
"#;

/// Signals: `self:emit` reaches the methods wired in the emitter's Script component (with the
/// signal's values or fixed ones, `once`), `entity:on` / `entity:once` listeners and
/// `task.wait_for`; an undeclared signal and a wire to a missing method are errors that say so.
#[test]
fn signals_reach_wired_methods_and_listeners() {
    let folder = Folder::new();
    folder.write("bell.luau", BELL);
    folder.write("ear.luau", EAR);
    let mut app = app_in(&folder.0);
    let bell = app
        .world
        .spawn((
            Name::new("Bell"),
            Transform::default(),
            Script::new("bell")
                .connect("rang", "Ear", "hear")
                .connect("rang", "Ear", "missing"),
        ))
        .id();
    {
        let mut s = app.world.get_mut::<Script>(bell).unwrap();
        s.connections.push(crate::Connection {
            signal: "rang".into(),
            target: "Ear".into(),
            method: "fixed_values".into(),
            args: vec![json!(2), json!("open")],
            once: true,
        });
    }
    spawn_script(&mut app, "Ear", "ear");
    app.update_n(2);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    eval(
        &mut app,
        "world.find('Bell').script:ring(world.find('Ear'))",
    );
    app.update_n(2);
    eval(
        &mut app,
        "world.find('Bell').script:ring(world.find('Bell'))",
    );
    app.update_n(2);
    let ear = |app: &mut App, field: &str| eval(app, &format!("world.find('Ear').script.{field}"));
    assert_eq!(ear(&mut app, "wired"), json!("2 Bell"));
    assert_eq!(ear(&mut app, "heard"), json!(["1 by Ear", "2 by Bell"]));
    assert_eq!(ear(&mut app, "first"), json!(1), "once hears one ring");
    assert_eq!(ear(&mut app, "waited"), json!(1));
    assert_eq!(
        ear(&mut app, "fixed"),
        json!(["2 open"]),
        "fixed values, once"
    );
    let s = app.world.get::<Script>(bell).unwrap();
    assert_eq!(s.connections.len(), 2, "the once wire is spent");
    let errs = errors(&mut app);
    assert!(
        errs.iter()
            .any(|e| e.contains("has no method or state event missing")),
        "{errs:?}"
    );
    // A signal the class doesn't declare: the error lists the ones it does.
    let typo = call_api(
        &mut app.world,
        "script.eval",
        json!({"code": "world.find('Bell').script:typo()"}),
    )
    .unwrap_err();
    assert!(
        typo.contains("no signal \"rnag\" in signals (rang)"),
        "{typo}"
    );
    // The editor's reading: the signal with its values and comment, the methods.
    let declared = crate::props::declared(&mut app.world, "ear.luau").unwrap();
    let names: Vec<&str> = declared.methods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["hear", "fixed_values", "start"], "hooks last");
    assert_eq!(declared.methods[0].params, "times, by");
    let declared = crate::props::declared(&mut app.world, "bell.luau").unwrap();
    assert_eq!(
        declared.signals,
        [crate::props::Signal {
            name: "rang".into(),
            params: "times: number, by: Entity".into(),
            doc: "each ring".into(),
            line: 4,
        }]
    );
}

/// A visual script in the same conversation: its Bind node hears the bell's `rang` and runs
/// its custom event `ring`, which emits the graph's own `chimed` (wired to a Luau method); the
/// bell's `rang` is also wired straight to the graph's `hush`, which has no node yet.
#[test]
fn graphs_bind_emit_and_take_wired_signals() {
    use crate::graph::{Graph, Link, Node};
    let node = |id: u32, kind: &str, of: &str, values: Value| Node {
        id,
        kind: kind.into(),
        of: of.into(),
        at: [0, id as i32 * 100],
        values: serde_json::from_value(values).unwrap(),
    };
    let link = |a: u32, ap: &str, b: u32, bp: &str| Link {
        from: (a, ap.into()),
        to: (b, bp.into()),
    };
    let chime = Graph {
        signals: [("chimed".to_string(), "times: number".to_string())].into(),
        events: [
            ("ring".to_string(), "times: number, by: Entity".to_string()),
            ("hush".to_string(), String::new()),
        ]
        .into(),
        vars: [("last".to_string(), json!(0))].into(),
        nodes: vec![
            node(1, "event.start", "", json!({})),
            node(
                2,
                "signal.bind",
                "ring",
                json!({"entity": "Bell", "signal": "rang"}),
            ),
            node(3, "event.custom", "ring", json!({})),
            node(4, "var.set", "last", json!({})),
            node(5, "signal.emit", "chimed", json!({})),
        ],
        links: vec![
            link(1, "then", 2, "in"),
            link(3, "then", 4, "in"),
            link(3, "times", 4, "value"),
            link(4, "then", 5, "in"),
            link(3, "times", 5, "times"),
        ],
        ..Default::default()
    };
    let folder = Folder::new();
    folder.write("bell.luau", BELL);
    folder.write("chime.graph", &chime.to_text());
    folder.write(
        "listener.luau",
        "local L = {}\nfunction L:start() self.got = {} end\n\
         function L:chimed(times: number) table.insert(self.got, times) end\nreturn L\n",
    );
    let mut app = app_in(&folder.0);
    app.world.spawn((
        Name::new("Bell"),
        Transform::default(),
        Script::new("bell").connect("rang", "Chime", "hush"),
    ));
    app.world.spawn((
        Name::new("Chime"),
        Transform::default(),
        Script::new("chime.graph").connect("chimed", "Listener", "chimed"),
    ));
    spawn_script(&mut app, "Listener", "listener");
    app.update_n(2);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    for _ in 0..2 {
        eval(
            &mut app,
            "world.find('Bell').script:ring(world.find('Listener'))",
        );
        app.update_n(3);
    }
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    assert_eq!(eval(&mut app, "world.find('Chime').script.last"), json!(2));
    assert_eq!(
        eval(&mut app, "world.find('Listener').script.got"),
        json!([1, 2])
    );
    // The Details panel sees the graph's signal and its custom events as wire targets.
    let declared = crate::props::declared(&mut app.world, "chime.graph").unwrap();
    assert_eq!(declared.signals.len(), 1);
    assert_eq!(declared.signals[0].params, "times: number");
    let methods: Vec<&str> = declared.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(
        methods.contains(&"ring") && methods.contains(&"hush"),
        "{methods:?}"
    );
}

/// Steps `app` until the script test run is over (at most `frames`); returns `script.tests`.
fn finish_tests(app: &mut App, frames: usize) -> Value {
    for _ in 0..frames {
        let state = call_api(&mut app.world, "script.tests", json!({})).unwrap();
        if state["state"] != "running" {
            return state;
        }
        app.update();
    }
    panic!(
        "the tests are still running: {}",
        call_api(&mut app.world, "script.tests", json!({})).unwrap()
    );
}

/// A run started before any script has run (the editor's empty scene: Play, then a test run)
/// times its tests from the world's clock, not from the new VM's zero: the first test used to
/// time out at once when the world had been running for longer than its limit.
#[test]
fn a_test_run_on_a_fresh_vm_times_from_the_world_clock() {
    let folder = Folder::new();
    folder.write(
        "quick.test.luau",
        "it('waits a frame', function(t)\n\tt:frames(1)\nend, 2)\n",
    );
    let mut app = app_in(&folder.0);
    // Twelve seconds of world time with nothing scripted: no VM yet.
    app.update_n(30 * 12);
    assert!(app.world.get_resource::<ScriptRuntime>().is_none());
    call_api(&mut app.world, "script.test", json!({})).unwrap();
    let done = finish_tests(&mut app, 300);
    assert_eq!(done["passed"], json!(1), "{done:#}");
    assert!(
        done["results"][0]["secs"].as_f64().unwrap() < 0.5,
        "{done:#}"
    );
}

const DOOR: &str = r#"
local Door = {}
Door.signals = { opened = "by: Entity" }
function Door:start() self.open = false end
function Door:on_interact(by)
	self.open = not self.open
	if self.open then self:emit("opened", by) end
end
return Door
"#;

#[test]
fn script_tests_run_in_the_game_and_report_each_test() {
    let folder = Folder::new();
    folder.write("door.luau", DOOR);
    folder.write(
        "door.test.luau",
        r#"local SPEC = { name = "Test door", components = { Transform = {}, Script = { path = "door.luau" } } }
local order = {}
describe("Door", function()
	before_each(function(t) table.insert(order, "before " .. t.name) end)
	it("opens when pressed", function(t)
		local door = t:spawn(SPEC)
		expect(door.script.open).to_be(false)
		t:press(door)
		expect(door.script.open).to_be(true)
	end)
	it("tells listeners who opened it", function(t)
		local door = t:spawn(SPEC)
		local heard = t:listen(door, "opened")
		t:press(door, t:find("Player"))
		t:frames(2)
		expect(heard.count).to_be(1)
		expect(heard.last[1]).to_be(t:find("Player"))
	end)
	it("waits world time", function(t)
		local before = world.time()
		t:wait(1)
		expect(world.time() - before).to_be_near(1, 0.05)
		expect({ 1, { a = vec3(1, 2, 3) } }).to_equal({ 1, { a = vec3(1, 2, 3) } })
		expect("lamp").never.to_contain("x")
		expect(function() error("boom") end).to_throw("boom")
	end)
	it("fails on the line of the expect", function(t)
		local n = 1 + 1
		expect(n).to_be(3)
	end)
	it("reports an error", function(t)
		local nothing: any = nil
		nothing.field = 1
	end)
	it("stops at its time limit", function(t)
		t:wait(5)
	end, 0.5)
	it.skip("is skipped", function(t) end)
	describe("after a test", function()
		it("left nothing behind", function(t)
			expect(world.find("Test door")).to_be_nil()
			expect(#order).to_be(7)
		end)
	end)
end)
"#,
    );
    folder.write(
        "broken.test.luau",
        "describe('x', function()\n\tit('y', function(t)\nend)\n",
    );
    let mut app = app_in(&folder.0);
    app.add_message::<engine_world::Interact>();
    app.world.spawn(Name::new("Player"));
    let started = call_api(&mut app.world, "script.test", json!({})).unwrap();
    assert_eq!(started["files"], json!(2), "{started}");
    assert_eq!(
        started["broken"][0]["file"], "broken.test.luau",
        "{started}"
    );
    let done = finish_tests(&mut app, 30 * 10);
    let results = done["results"].as_array().unwrap();
    let status = |title: &str| {
        results
            .iter()
            .find(|r| r["title"] == title)
            .unwrap_or_else(|| panic!("no {title} in {done}"))
    };
    for passing in [
        "Door › opens when pressed",
        "Door › tells listeners who opened it",
        "Door › waits world time",
        "Door › after a test › left nothing behind",
    ] {
        assert_eq!(status(passing)["status"], "passed", "{}", status(passing));
    }
    let failed = status("Door › fails on the line of the expect");
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["message"], "expected 3, got 2");
    assert_eq!(failed["failed_at"], json!(29), "{failed}");
    assert_eq!(failed["line"], json!(27), "{failed}");
    let error = status("Door › reports an error");
    assert_eq!(error["status"], "failed");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("attempt to index nil"),
        "{error}"
    );
    assert_eq!(error["failed_at"], json!(33), "{error}");
    let slow = status("Door › stops at its time limit");
    assert_eq!(slow["status"], "failed");
    assert!(
        slow["message"]
            .as_str()
            .unwrap()
            .starts_with("didn't finish within 0.5 s"),
        "{slow}"
    );
    assert_eq!(status("Door › is skipped")["status"], "skipped");
    assert_eq!(done["passed"], json!(4), "{done}");
    // Three tests and the broken file.
    assert_eq!(done["failed"], json!(4), "{done}");
    assert_eq!(done["skipped"], json!(1), "{done}");

    // One test, by the line of its `it`; `only` picks too.
    call_api(
        &mut app.world,
        "script.test",
        json!({"file": "door.test.luau", "line": 5}),
    )
    .unwrap();
    let one = finish_tests(&mut app, 30 * 5);
    assert_eq!(one["results"].as_array().unwrap().len(), 1, "{one}");
    assert_eq!(one["passed"], json!(1), "{one}");
    folder.write(
        "focus.test.luau",
        "it('a', function(t) end)\nit.only('b', function(t) end)\n",
    );
    call_api(
        &mut app.world,
        "script.test",
        json!({"files": ["focus.test.luau"]}),
    )
    .unwrap();
    let focus = finish_tests(&mut app, 30);
    assert_eq!(focus["passed"], json!(1), "{focus}");
    assert_eq!(focus["skipped"], json!(1), "{focus}");
}

/// The test files that ship with the engine pass, in a world with lights and materials.
#[test]
fn the_shipped_script_tests_pass() {
    shipped_tests_pass(1.0 / 30.0);
}

/// At 4 frames a second too, the world stepping a quarter second a frame (its longest step),
/// as in the editor's headless screenshot runs: a test that counts on a short state or a
/// threshold crossing being seen fails there.
#[test]
fn the_shipped_script_tests_pass_at_four_frames_a_second() {
    shipped_tests_pass(0.25);
}

fn shipped_tests_pass(step_secs: f64) {
    use engine_assets::Assets;
    use engine_render::{MeshRenderer, PointLight, StandardMaterial};

    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scripts");
    let files = crate::testing::test_files(&shipped);
    assert!(files.len() >= 2, "{files:?}");
    let mut app = app_in(&shipped);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs });
    app.world.resource_mut::<ScriptSettings>().hot_reload = false;
    app.insert_resource(Assets::<StandardMaterial>::default());
    app.register_component::<MeshRenderer>("A mesh.")
        .register_component::<PointLight>("A light.")
        .register_component::<engine_world::Interactable>("Something the player can use.")
        .add_message::<engine_world::Interact>();
    app.world.spawn(Name::new("Player"));
    call_api(&mut app.world, "script.test", json!({})).unwrap();
    let done = finish_tests(&mut app, 30 * 60);
    assert_eq!(done["failed"], json!(0), "{done:#}");
    assert!(done["passed"].as_u64().unwrap() >= 8, "{done:#}");
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

#[test]
fn errors_name_the_file_and_line_and_dont_repeat() {
    let folder = Folder::new();
    folder.write(
        "bad.luau",
        "local Bad = {}\nfunction Bad:update()\n\tlocal x = nil\n\tx.y = 1\nend\nreturn Bad\n",
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Bad", "bad");
    app.update_n(20);
    let errs = errors(&mut app);
    assert_eq!(errs.len(), 1, "logged once while it repeats: {errs:?}");
    assert!(errs[0].contains("bad.luau:4"), "{}", errs[0]);
}

#[test]
fn the_api_writes_and_edits_script_files() {
    let folder = Folder::new();
    let mut app = app_in(&folder.0);
    call_api(
        &mut app.world,
        "script.write",
        json!({"path": "made/hello.luau", "source": "local H = {}\nfunction H:start() self.n = 10 end\nreturn H\n"}),
    )
    .unwrap();
    spawn_script(&mut app, "Hello", "made/hello");
    app.update_n(2);
    assert_eq!(eval(&mut app, "world.find('Hello').script.n"), json!(10));
    call_api(
        &mut app.world,
        "script.edit",
        json!({"path": "made/hello.luau", "find": "self.n = 10", "replace": "self.n = 11"}),
    )
    .unwrap();
    // A restart takes the file as it is now, even before the poller has seen the edit.
    call_api(
        &mut app.world,
        "script.restart",
        json!({"entity": {"name": "Hello"}}),
    )
    .unwrap();
    app.update_n(1);
    assert_eq!(eval(&mut app, "world.find('Hello').script.n"), json!(11));
    assert!(
        call_api(
            &mut app.world,
            "script.write",
            json!({"path": "../escape.luau", "source": ""})
        )
        .is_err()
    );
    let missing = call_api(
        &mut app.world,
        "script.edit",
        json!({"path": "made/hello.luau", "find": "nothing like this", "replace": ""}),
    );
    assert!(missing.is_err());
    let types = call_api(&mut app.world, "script.types", json!({})).unwrap();
    let text = std::fs::read_to_string(types["file"].as_str().unwrap()).unwrap();
    assert!(
        text.contains("declare test: {"),
        "registered functions are typed"
    );
    assert!(text.contains("add: (args:"), "{text}");
}

#[test]
fn animated_materials_edit_one_material_of_their_own() {
    use engine_assets::Assets;
    use engine_render::{MeshRenderer, Shape, StandardMaterial};

    let folder = Folder::new();
    folder.write(
        "glow.luau",
        "local G = {}\nfunction G:update(dt)\n\tself.t = (self.t or 0) + dt\n\tself.entity:material({ emissive_strength = 10 * self.t })\nend\nreturn G\n",
    );
    let mut app = app_in(&folder.0);
    app.insert_resource(Assets::<StandardMaterial>::default());
    let e = app
        .world
        .spawn((
            Name::new("Glow"),
            Transform::default(),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 1.0,
                    segments: 8,
                },
                StandardMaterial::color(1.0, 1.0, 1.0),
            ),
            Script::new("glow"),
        ))
        .id();
    app.update_n(20);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let own = app.world.get::<crate::OwnMaterial>(e).unwrap().0;
    let materials = app.world.resource::<Assets<StandardMaterial>>();
    assert_eq!(materials.len(), 1, "one material, edited in place");
    assert!(materials.get(own).unwrap().emissive_strength > 5.0);
    app.world.despawn(e);
    app.update_n(2);
    assert_eq!(
        app.world.resource::<Assets<StandardMaterial>>().len(),
        0,
        "freed with it"
    );
}

/// The showcase's courtyard and lever, as shipped, run without errors: the director builds its
/// lanterns and orrery, the lever's pull plays the ritual through.
#[test]
fn the_showcase_courtyard_builds_and_plays_its_ritual() {
    use engine_assets::Assets;
    use engine_render::{MeshRenderer, PointLight, StandardMaterial};

    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scripts");
    let mut app = app_in(&shipped);
    app.world.resource_mut::<ScriptSettings>().hot_reload = false;
    app.insert_resource(Assets::<StandardMaterial>::default());
    app.register_component::<MeshRenderer>("A mesh.")
        .register_component::<PointLight>("A light.")
        .register_component::<engine_world::Interactable>("Usable.")
        .add_message::<engine_world::Interact>();
    for name in ["vfx.spawn", "vfx.beam"] {
        app.register_api(name, "Stub.", "{}", |_, _| Ok(Value::Null));
    }
    let mut commands_queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut commands_queue, &app.world);
    crate::demo::spawn_demo(&mut commands, DVec3::new(70.0, 0.0, -105.0));
    commands_queue.apply(&mut app.world);
    app.update_n(5);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let lanterns = eval(&mut app, "#world.find_all('Lantern ')");
    assert_eq!(lanterns, json!(12));
    // The portcullis's state machine runs; its tour stops' press raises it.
    let gate = "world.find('Courtyard gate').script.state";
    assert_eq!(eval(&mut app, gate), json!("closed"));
    let (name, args) = crate::demo::GATE_PRESS;
    call_api(&mut app.world, name, serde_json::from_str(args).unwrap()).unwrap();
    assert_eq!(eval(&mut app, gate), json!("opening"));
    // The rune seal's: two runes leave it sealed, the third opens it.
    let seal = "local s = world.find('Rune seal').script return";
    assert_eq!(
        eval(&mut app, &format!("{seal} s:in_state('sealed')")),
        json!(true)
    );
    assert_eq!(eval(&mut app, "#world.find_all(' rune')"), json!(3));
    let (name, args) = crate::demo::SEAL_TWO;
    call_api(&mut app.world, name, serde_json::from_str(args).unwrap()).unwrap();
    let e = eval(&mut app, "return world.find('Rune seal').id")
        .as_u64()
        .unwrap();
    let live = crate::states::live(&app.world, Entity::from_bits(e)).unwrap();
    assert_eq!(live.current, ["left_lit", "middle_dark", "right_lit"]);
    assert_eq!(live.done.iter().collect::<Vec<_>>(), ["left", "right"]);
    let (name, args) = crate::demo::SEAL_ALL;
    call_api(&mut app.world, name, serde_json::from_str(args).unwrap()).unwrap();
    assert_eq!(eval(&mut app, &format!("{seal} s.state")), json!("opening"));
    // The swarm: wisps on the actor VMs.
    let wisps = eval(&mut app, "#world.find_all('Wisp ')");
    assert_eq!(wisps, json!(320));
    let list = call_api(&mut app.world, "script.list", json!({})).unwrap();
    let on_actors = list["instances"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["path"] == "showcase/wisp.luau" && i["vm"].as_u64() > Some(0))
        .count();
    assert_eq!(on_actors, 320, "{}", list["vms"]);
    const WISP_HEIGHT: &str =
        "local s = 0 for _, w in world.find_all('Wisp ') do s += w.position.y end return s / 320";
    let resting = eval(&mut app, WISP_HEIGHT).as_f64().unwrap();
    // No tour stop's camera stands in the swarm: a wisp passing a hand's width from the lens
    // fills the frame (one hid the crystal stop). Orbits swing 0.5 m and lag, so keep a meter.
    let widest = eval(
        &mut app,
        "local m = 0 for _, w in world.find_all('Wisp ') do \
         m = math.max(m, math.sqrt(w.position.x ^ 2 + w.position.z ^ 2)) end return m",
    )
    .as_f64()
    .unwrap();
    for view in crate::demo::VIEWS {
        let out = view.eye[0].hypot(view.eye[2]);
        assert!(
            out > widest + 1.0,
            "{}: camera {out:.2} m out, wisps to {widest:.2} m",
            view.key
        );
    }
    assert!(
        eval(&mut app, "world.find('Ring 3') ~= nil")
            .as_bool()
            .unwrap()
    );

    // The live-values stop: its logpoints sit on lines with code, and fill in.
    for (name, args) in crate::demo::LIVE_VALUES_ON {
        let out = call_api(&mut app.world, name, serde_json::from_str(args).unwrap()).unwrap();
        if *name == "script.logpoint" {
            assert_eq!(out["at"], out["line"], "{args} moved: {out}");
        }
    }
    app.update_n(2);
    let (_, values) = app
        .world
        .resource::<crate::ScriptRuntime>()
        .debugger
        .inline_values("showcase/courtyard.luau");
    assert_eq!(values.len(), 5, "{values:?}");
    assert!(values[&32].starts_with("glow = "), "{values:?}");
    call_api(&mut app.world, "script.logpoint", json!({"clear": true})).unwrap();

    // The profiler stops' recording, over the ritual.
    call_api(
        &mut app.world,
        "script.profile",
        json!({"seconds": 0, "hz": 4000}),
    )
    .unwrap();
    let lever = eval(&mut app, "world.find('Ritual lever').id")
        .as_u64()
        .unwrap();
    let player = app.world.spawn(Name::new("Player")).id();
    app.world.write_message(engine_world::Interact {
        actor: player,
        target: Entity::from_bits(lever),
        from: DVec3::ZERO,
    });
    // The ritual peaks about 6 s after the pull (the tour's ritual stop counts on it) and is
    // over by 13 s.
    app.update_n(30 * 6);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let director = "world.find('Courtyard director').script";
    assert_eq!(eval(&mut app, &format!("{director}.busy")), json!(true));
    let risen = eval(&mut app, WISP_HEIGHT).as_f64().unwrap();
    assert!(
        risen > resting + 3.0,
        "the swarm rises: {resting} -> {risen}"
    );
    assert!(
        eval(&mut app, &format!("{director}.flare"))
            .as_f64()
            .unwrap()
            > 1.2
    );
    app.update_n(30 * 8);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    assert_eq!(eval(&mut app, &format!("{director}.busy")), json!(false));
    // The pull is wired to the crystal graph's `flare` custom event too.
    assert_eq!(
        eval(&mut app, "world.find('Graph crystal').script.flares"),
        json!(1)
    );
    let profile = call_api(&mut app.world, "script.profile", json!({"stop": true})).unwrap();
    assert!(profile["samples"].as_u64().unwrap() > 10, "{profile}");
    let functions = profile["functions"].as_array().unwrap();
    assert!(
        functions
            .iter()
            .any(|f| f["name"] == "update" && f["source"] == "showcase/courtyard.luau"),
        "{profile}"
    );
    assert!(
        functions.iter().any(|f| f["engine"] == true),
        "engine calls are entries of their own: {profile}"
    );
    assert!(
        functions
            .iter()
            .any(|f| f["name"] == "parallel_update" && f["source"] == "showcase/wisp.luau"),
        "the actor VMs' samples are in it: {profile}"
    );
    // The error stop's typo is on a line the board shows once scrolled to `update`.
    let source = std::fs::read_to_string(shipped.join(crate::demo::COURTYARD_SCRIPT)).unwrap();
    let typo_line = source
        .lines()
        .position(|l| l.contains("local glow = lamp.dim * ("))
        .expect("the error stop's line is in the director script")
        + 1;
    assert!((17..17 + 28).contains(&typo_line), "{typo_line}");
    let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
    assert!(log.as_array().unwrap().is_empty(), "{log}");
}

#[test]
fn positions_a_script_keeps_follow_a_world_shift() {
    let folder = Folder::new();
    folder.write(
        "keeper.luau",
        r#"
local K = {}
function K:start()
	self.home = vec3(1, 2, 3)
end
function K:on_world_shift(shift)
	self.home = shift:point(self.home)
end
return K
"#,
    );
    let mut app = app_in(&folder.0);
    app.add_message::<engine_world::WorldShift>();
    spawn_script(&mut app, "Keeper", "keeper");
    app.update_n(2);
    app.world
        .write_message(engine_world::WorldShift::translate(DVec3::new(
            100.0, 0.0, -50.0,
        )));
    app.update_n(1);
    assert_eq!(
        eval(&mut app, "world.find('Keeper').script.home"),
        json!([101, 2, -47])
    );
}

const COUNTER: &str = "local Counter = {}
function Counter:start()
	self.count = 0
end
function Counter:update(dt)
	local step = 2
	self.count += step
	local double = self.count * 2
end
return Counter
";

/// A small Debug Adapter Protocol client, as VS Code talks to the debugger.
struct Client {
    stream: std::net::TcpStream,
    reader: std::io::BufReader<std::net::TcpStream>,
    seq: i64,
    events: Vec<Value>,
}

impl Client {
    fn connect(port: u16) -> Self {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(20)))
            .unwrap();
        let reader = std::io::BufReader::new(stream.try_clone().unwrap());
        Self {
            stream,
            reader,
            seq: 0,
            events: Vec::new(),
        }
    }

    fn read(&mut self) -> Value {
        crate::dap::read_message(&mut self.reader).expect("the debugger went away")
    }

    /// Sends a request and returns its response's body (panics when it failed).
    fn request(&mut self, command: &str, arguments: Value) -> Value {
        use std::io::Write;
        self.seq += 1;
        let message = json!({"seq": self.seq, "type": "request", "command": command,
            "arguments": arguments});
        self.stream.write_all(&crate::dap::frame(&message)).unwrap();
        loop {
            let m = self.read();
            if m["type"] == "response" && m["request_seq"] == self.seq {
                assert_eq!(m["success"], json!(true), "{command}: {m}");
                return m["body"].clone();
            }
            self.events.push(m);
        }
    }

    fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.events.iter().position(|e| e["event"] == name) {
            return self.events.remove(i)["body"].clone();
        }
        loop {
            let m = self.read();
            if m["event"] == name {
                return m["body"].clone();
            }
            self.events.push(m);
        }
    }

    fn variable(&mut self, reference: &Value, name: &str) -> Value {
        let vars = self.request("variables", json!({"variablesReference": reference}));
        vars["variables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name} in {vars}"))
    }
}

/// Runs frames while a client thread drives the debugger: a stop no longer holds a frame, so
/// the frames go on (the stopped script and the world's time wait) until it is done.
fn run_while_client(app: &mut App, client: &std::thread::JoinHandle<()>) {
    let start = std::time::Instant::now();
    while !client.is_finished() && start.elapsed().as_secs() < 60 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn the_debugger_stops_at_a_breakpoint_shows_variables_and_steps() {
    let folder = Folder::new();
    folder.write("debug/counter.luau", COUNTER);
    let mut app = app_in(&folder.0);
    let counter = spawn_script(&mut app, "Counter", "debug/counter");
    app.update_n(2);
    let port = call_api(&mut app.world, "script.debug", json!({"port": 0})).unwrap()["port"]
        .as_u64()
        .unwrap() as u16;
    let file = std::path::absolute(folder.0.join("debug/counter.luau"))
        .unwrap()
        .display()
        .to_string();

    // The client drives the session from its own thread while the game runs frames here; a
    // stop holds `app.update()` until the client continues.
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        let caps = c.request("initialize", json!({"adapterID": "engine-luau"}));
        assert_eq!(caps["supportsLogPoints"], json!(true));
        c.event("initialized");
        c.request("attach", json!({}));
        let set = c.request(
            "setBreakpoints",
            json!({"source": {"path": file}, "breakpoints": [
                {"line": 7, "condition": "self.count >= 4"}
            ]}),
        );
        assert_eq!(set["breakpoints"][0]["verified"], json!(true), "{set}");
        assert_eq!(set["breakpoints"][0]["line"], json!(7));
        c.request("configurationDone", json!({}));

        let stopped = c.event("stopped");
        assert_eq!(stopped["reason"], json!("breakpoint"));
        let stack = c.request("stackTrace", json!({"threadId": 1}));
        let top = &stack["stackFrames"][0];
        assert_eq!(top["name"], json!("update"), "{stack}");
        assert_eq!(top["line"], json!(7));
        assert!(
            top["source"]["path"]
                .as_str()
                .unwrap()
                .ends_with("counter.luau")
        );
        let scopes = c.request("scopes", json!({"frameId": top["id"]}));
        let locals = scopes["scopes"][0]["variablesReference"].clone();
        assert_eq!(c.variable(&locals, "step")["value"], json!("2"));
        let this = c.variable(&locals, "self");
        let count: f64 = c.variable(&this["variablesReference"], "count")["value"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(count >= 4.0, "the condition held: {count}");
        let sum = c.request(
            "evaluate",
            json!({"expression": "self.count + step", "frameId": top["id"], "context": "watch"}),
        );
        assert_eq!(sum["result"], json!((count + 2.0).to_string()));

        c.request("next", json!({"threadId": 1}));
        assert_eq!(c.event("stopped")["reason"], json!("step"));
        let stack = c.request("stackTrace", json!({"threadId": 1}));
        assert_eq!(stack["stackFrames"][0]["line"], json!(8), "{stack}");
        let now = c.request(
            "evaluate",
            json!({"expression": "self.count", "frameId": 0}),
        );
        assert_eq!(now["result"], json!((count + 2.0).to_string()));

        c.request(
            "setBreakpoints",
            json!({"source": {"path": file}, "breakpoints": []}),
        );
        c.request("continue", json!({"threadId": 1}));
        c.request("disconnect", json!({}));
    });
    run_while_client(&mut app, &client);
    client.join().expect("the debug session failed");
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let before = eval(&mut app, "world.find('Counter').script.count")
        .as_f64()
        .unwrap();
    app.update_n(3);
    let after = eval(&mut app, "world.find('Counter').script.count")
        .as_f64()
        .unwrap();
    assert_eq!(
        after,
        before + 6.0,
        "the script runs on after the client left"
    );
    assert!(app.world.get::<Script>(counter).is_some());
}

#[test]
fn logpoints_print_values_without_stopping_and_survive_a_reload() {
    let folder = Folder::new();
    folder.write("debug/counter.luau", COUNTER);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Counter", "debug/counter");
    app.update_n(2);
    let set = call_api(
        &mut app.world,
        "script.logpoint",
        json!({"path": "debug/counter.luau", "line": 8, "message": "count = {self.count}", "log": true}),
    )
    .unwrap();
    assert_eq!(set["at"], json!(8), "{set}");
    app.update_n(3);
    let count = eval(&mut app, "world.find('Counter').script.count")
        .as_f64()
        .unwrap();
    let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
    let lines: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .collect();
    assert_eq!(lines.len(), 3, "once a frame: {log}");
    assert_eq!(lines[2], format!("debug/counter.luau:8: count = {count}"));
    let inline = |app: &mut App| {
        app.world
            .resource::<crate::ScriptRuntime>()
            .debugger
            .inline_values("debug/counter.luau")
    };
    let (v1, values) = inline(&mut app);
    assert_eq!(values.get(&8).cloned(), Some(format!("count = {count}")));

    // A hot reload moves the logpoint onto the new code.
    edit(
        &folder,
        "debug/counter.luau",
        &COUNTER.replace("local step = 2", "local step = 3"),
    );
    app.update_n(2);
    let (v2, values) = inline(&mut app);
    assert!(v2 > v1);
    let count = eval(&mut app, "world.find('Counter').script.count")
        .as_f64()
        .unwrap();
    assert_eq!(values.get(&8).cloned(), Some(format!("count = {count}")));

    let removed = call_api(&mut app.world, "script.logpoint", json!({"clear": true})).unwrap();
    assert_eq!(removed["removed"], json!(1));
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

#[test]
fn the_debugger_steps_over_waits_in_tasks_and_pauses_on_request() {
    let folder = Folder::new();
    folder.write(
        "debug/waiter.luau",
        "local Waiter = {}
function Waiter:start()
	self.n = 0
	while true do
		self.n += 1
		task.wait(0.1)
	end
end
return Waiter
",
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Waiter", "debug/waiter");
    app.update_n(2);
    let port = call_api(&mut app.world, "script.debug", json!({"port": 0})).unwrap()["port"]
        .as_u64()
        .unwrap() as u16;
    let file = std::path::absolute(folder.0.join("debug/waiter.luau"))
        .unwrap()
        .display()
        .to_string();
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        c.request("initialize", json!({}));
        c.request("attach", json!({}));
        c.request(
            "setBreakpoints",
            json!({"source": {"path": file}, "breakpoints": [{"line": 6}]}),
        );
        c.request("configurationDone", json!({}));
        assert_eq!(c.event("stopped")["reason"], json!("breakpoint"));
        let n =
            c.request("evaluate", json!({"expression": "self.n", "frameId": 0}))["result"].clone();
        c.request(
            "setBreakpoints",
            json!({"source": {"path": file}, "breakpoints": []}),
        );
        // Over the wait: the task yields, and the step ends when it runs again, a few frames
        // later, still in start().
        c.request("next", json!({"threadId": 1}));
        assert_eq!(c.event("stopped")["reason"], json!("step"));
        let top = c.request("stackTrace", json!({"threadId": 1}))["stackFrames"][0].clone();
        assert_eq!(top["name"], json!("start"), "{top}");
        assert_eq!(
            c.request("evaluate", json!({"expression": "self.n", "frameId": 0}))["result"],
            n,
            "stopped before the loop counted again"
        );
        c.request("continue", json!({"threadId": 1}));
        c.request("pause", json!({"threadId": 1}));
        assert_eq!(c.event("stopped")["reason"], json!("pause"));
        c.request("continue", json!({"threadId": 1}));
        c.request("disconnect", json!({}));
    });
    run_while_client(&mut app, &client);
    client.join().expect("the debug session failed");
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let before = eval(&mut app, "world.find('Waiter').script.n")
        .as_f64()
        .unwrap();
    app.update_n(10);
    let after = eval(&mut app, "world.find('Waiter').script.n")
        .as_f64()
        .unwrap();
    assert!(after > before, "the task runs on after the client left");
}

#[test]
fn error_locations_and_gists_are_read_from_messages() {
    use crate::runtime::{error_gist, error_location};
    let runtime = "runtime error: showcase/courtyard.luau:31: attempt to perform arithmetic (mul) \
                   on nil and number\nstack traceback:\n\tshowcase/courtyard.luau:31: in function 'update'";
    assert_eq!(
        error_location(runtime),
        Some(("showcase/courtyard.luau".into(), 31))
    );
    assert_eq!(
        error_gist(runtime),
        "attempt to perform arithmetic (mul) on nil and number"
    );
    // An engine call's error names the call; the traceback names the line.
    let callback = "runtime error: test.fail: no such thing\nstack traceback:\n\t[C]: in ?\n\t\
                    lib/util.lua:7: in function 'go'";
    assert_eq!(error_location(callback), Some(("lib/util.lua".into(), 7)));
    assert_eq!(error_gist(callback), "test.fail: no such thing");
    assert_eq!(error_location("no place at all: 3"), None);
}

#[test]
fn runtime_errors_mark_their_line_until_the_file_is_fixed() {
    let folder = Folder::new();
    folder.write(
        "faulty.luau",
        "local F = {}\nfunction F:update()\n\tlocal x = nil\n\tx.y = 1\nend\nreturn F\n",
    );
    folder.write(
        "calls.luau",
        "local C = {}\nfunction C:update()\n\ttest.fail({})\nend\nreturn C\n",
    );
    let mut app = app_in(&folder.0);
    app.register_api("test.fail", "Fails.", "{}", |_, _| {
        Err("no such thing".to_string())
    });
    spawn_script(&mut app, "Faulty", "faulty");
    spawn_script(&mut app, "Calls", "calls");
    app.update_n(10);
    let fault = &file_info(&mut app, "faulty.luau")["fault"];
    assert_eq!(fault["line"], json!(4), "{fault}");
    assert!(
        fault["message"]
            .as_str()
            .unwrap()
            .contains("attempt to index nil"),
        "{fault}"
    );
    assert!(
        fault["count"].as_u64().unwrap() >= 9,
        "counted every time: {fault}"
    );
    let fault = &file_info(&mut app, "calls.luau")["fault"];
    assert_eq!(
        fault["line"],
        json!(3),
        "an engine call's error marks the call: {fault}"
    );
    assert!(
        fault["message"].as_str().unwrap().contains("test.fail"),
        "{fault}"
    );

    edit(
        &folder,
        "faulty.luau",
        "local F = {}\nfunction F:update()\n\tlocal x = {}\n\tx.y = 1\nend\nreturn F\n",
    );
    app.update_n(3);
    assert_eq!(file_info(&mut app, "faulty.luau")["fault"], Value::Null);
}

const BUSY: &str = "local Busy = {}
local function hot(n)
\tlocal s = 0
\tfor i = 1, n do
\t\ts += math.sin(i) * math.cos(i)
\tend
\treturn s
end
local function cold(n)
\tlocal s = 0
\tfor i = 1, n do s += i end
\treturn s
end
function Busy:update(dt)
\tself.a = hot(20000)
\tself.b = cold(500)
\ttest.slow({ ms = 1 })
end
return Busy
";

#[test]
fn the_profiler_finds_the_hot_function_its_lines_and_slow_engine_calls() {
    let folder = Folder::new();
    folder.write("busy.luau", BUSY);
    let mut app = app_in(&folder.0);
    app.register_api("test.slow", "Busy-waits ms.", r#"{"ms": 1}"#, |_, args| {
        let until = std::time::Instant::now()
            + std::time::Duration::from_secs_f64(args["ms"].as_f64().unwrap_or(1.0) / 1000.0);
        while std::time::Instant::now() < until {
            std::hint::spin_loop();
        }
        Ok(Value::Null)
    });
    spawn_script(&mut app, "Busy", "busy");
    app.update_n(2);
    let started = call_api(
        &mut app.world,
        "script.profile",
        json!({"seconds": 0, "hz": 5000}),
    )
    .unwrap();
    assert_eq!(started["recording"], json!(true));
    let t0 = std::time::Instant::now();
    while t0.elapsed() < std::time::Duration::from_millis(700) {
        app.update();
    }
    let report = call_api(
        &mut app.world,
        "script.profile",
        json!({"stop": true, "folded": true}),
    )
    .unwrap();
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    assert_eq!(report["recording"], json!(false));
    assert!(report["samples"].as_u64().unwrap() > 40, "{report}");
    let functions = report["functions"].as_array().unwrap();
    let find = |name: &str| {
        functions
            .iter()
            .find(|f| f["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {report}"))
    };
    let pct = |f: &Value, key: &str| f[key].as_f64().unwrap();
    let (update, hot, slow) = (find("update"), find("hot"), find("test.slow"));
    assert_eq!(update["source"], json!("busy.luau"));
    assert_eq!(hot["line"], json!(2), "where hot starts");
    assert_eq!(slow["engine"], json!(true));
    assert!(pct(update, "total_pct") > 80.0, "{report}");
    assert!(pct(hot, "self_pct") > 15.0, "{report}");
    // test.slow busy-waits 1 ms a frame (however loaded the machine is, which makes Luau's
    // share vary): the samples that come due during it are charged to it.
    let frames = report["frames"].as_f64().unwrap();
    assert!(
        slow["self_ms"].as_f64().unwrap() > 0.6 * frames,
        "charged to the engine call: {report}"
    );
    let cold = functions
        .iter()
        .find(|f| f["name"] == "cold")
        .map_or(0.0, |f| pct(f, "total_pct"));
    assert!(pct(hot, "total_pct") > cold * 3.0, "{report}");
    // Lines: the call to hot and the engine call take their callees' time; the loop body
    // is the hottest line inside hot.
    let line = |n: u64| {
        report["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["line"] == n && l["source"] == "busy.luau")
            .map_or(0.0, |l| l["pct"].as_f64().unwrap())
    };
    assert!(line(15) >= pct(hot, "total_pct"), "{report}");
    assert!(line(17) >= pct(slow, "total_pct"), "{report}");
    // Built-ins like math.sin run on Luau's fast path, without the interrupt the samples come
    // from, so a tight loop's samples land on its back edge: the `for` line.
    assert!(line(4) + line(5) > line(11), "{report}");
    let lines = report["lines"].as_array().unwrap();
    let head = lines.iter().find(|l| l["line"] == 4).unwrap();
    assert_eq!(head["code"], json!("for i = 1, n do"));
    let folded = report["folded"].as_str().unwrap();
    assert!(
        folded.contains("update;update (busy.luau:14);test.slow "),
        "{folded}"
    );
    // Not recording, the scripts don't pay for it: reset drops the results.
    call_api(&mut app.world, "script.profile", json!({"reset": true})).unwrap();
    let empty = call_api(&mut app.world, "script.profile", json!({})).unwrap();
    assert_eq!(empty["samples"], json!(0), "{empty}");
}

const FAULTS: &str = r#"
local Faults = {}

function Faults:start()
	self.n = 0
	task.wait(0.5)
	local in_task = "yes"
	error("the task failed: " .. in_task)
end

function Faults:update(dt)
	self.n += 1
	local limit = 30
	if self.n > limit then
		local missing = nil
		missing.field = self.n
	end
end

return Faults
"#;

#[test]
fn the_debugger_stops_where_an_uncaught_error_is_raised() {
    let folder = Folder::new();
    folder.write("debug/faults.luau", FAULTS);
    let mut app = app_in(&folder.0);
    app.update_n(2);
    let port = call_api(&mut app.world, "script.debug", json!({"port": 0})).unwrap()["port"]
        .as_u64()
        .unwrap() as u16;
    // The script starts once the client is set up, so its errors come at known frames.
    let ready = Arc::new(AtomicBool::new(false));
    let configured = ready.clone();
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        let caps = c.request("initialize", json!({"adapterID": "engine-luau"}));
        assert_eq!(
            caps["exceptionBreakpointFilters"][0]["filter"],
            json!("uncaught")
        );
        assert_eq!(caps["supportsExceptionInfoRequest"], json!(true));
        c.request("attach", json!({}));
        c.request("setExceptionBreakpoints", json!({"filters": ["uncaught"]}));
        c.request("configurationDone", json!({}));
        configured.store(true, Ordering::Relaxed);

        // The task's error: its coroutine keeps its frames, so the stop shows them.
        let stopped = c.event("stopped");
        assert_eq!(stopped["reason"], json!("exception"), "{stopped}");
        assert!(
            stopped["text"]
                .as_str()
                .unwrap()
                .contains("the task failed: yes")
        );
        let stack = c.request("stackTrace", json!({"threadId": 1}));
        let top = stack["stackFrames"][0].clone();
        assert_eq!(top["line"], json!(8), "{stack}");
        assert_eq!(top["name"], json!("start"), "{stack}");
        let scopes = c.request("scopes", json!({"frameId": top["id"]}));
        let locals = scopes["scopes"][0]["variablesReference"].clone();
        assert!(
            c.variable(&locals, "in_task")["value"]
                .as_str()
                .unwrap()
                .contains("yes")
        );
        c.request("continue", json!({"threadId": 1}));

        // The update hook's error: stopped before the stack unwinds.
        let stopped = c.event("stopped");
        assert_eq!(stopped["reason"], json!("exception"), "{stopped}");
        let info = c.request("exceptionInfo", json!({"threadId": 1}));
        assert!(
            info["description"]
                .as_str()
                .unwrap()
                .contains("attempt to index nil"),
            "{info}"
        );
        // Only the script's own lines under it, not the debugger's wrapper.
        let trace = info["details"]["stackTrace"].as_str().unwrap();
        assert!(
            !trace.contains("engine/") && !trace.contains("[C]"),
            "{trace}"
        );
        let stack = c.request("stackTrace", json!({"threadId": 1}));
        let top = stack["stackFrames"][0].clone();
        assert_eq!(top["line"], json!(16), "{stack}");
        assert_eq!(top["name"], json!("update"), "{stack}");
        assert!(
            stack["stackFrames"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| !f["source"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("debugger")),
            "the catching helper stays out of sight: {stack}"
        );
        let scopes = c.request("scopes", json!({"frameId": top["id"]}));
        let locals = scopes["scopes"][0]["variablesReference"].clone();
        assert_eq!(c.variable(&locals, "limit")["value"], json!("30"));
        assert_eq!(c.variable(&locals, "missing")["value"], json!("nil"));
        let n = c.request(
            "evaluate",
            json!({"expression": "self.n", "frameId": top["id"], "context": "watch"}),
        );
        assert_eq!(n["result"], json!("31"));
        // Stepping from an error goes on (the stack unwinds).
        c.request("next", json!({"threadId": 1}));
        // Filter off: the error repeats every frame but no longer stops.
        let stopped = c.event("stopped");
        assert_eq!(stopped["reason"], json!("exception"));
        c.request("setExceptionBreakpoints", json!({"filters": []}));
        c.request("continue", json!({"threadId": 1}));
        c.request("disconnect", json!({}));
    });
    let t0 = std::time::Instant::now();
    while !ready.load(Ordering::Relaxed) && !client.is_finished() && t0.elapsed().as_secs() < 30 {
        app.update();
    }
    spawn_script(&mut app, "Faults", "debug/faults");
    run_while_client(&mut app, &client);
    client.join().expect("the debug session failed");
    app.update_n(3);
    let errs = errors(&mut app);
    assert!(
        errs.iter().any(|e| e.contains("the task failed: yes")),
        "the errors are still reported: {errs:?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("attempt to index nil")),
        "{errs:?}"
    );
    assert_eq!(
        file_info(&mut app, "debug/faults.luau")["fault"]["line"],
        json!(16)
    );
}

const PAIR: &str = r#"
local Pair = {}
Pair.props = { other = "" }

function Pair:start()
	self.other = world.find(self.props.other)
end

function Pair:parallel_update(dt)
	local x = self.other.position.x
	self.entity.position = vec3(x + 1, 0, 0)
	print(self.entity.name, "sees itself at", self.entity.position.x)
end

return Pair
"#;

/// An app whose scripts may use `vms` actor VMs.
fn app_with_vms(folder: &Path, vms: u32) -> App {
    let mut app = app_in(folder);
    app.world.resource_mut::<ScriptSettings>().parallel_vms = Some(vms);
    app
}

fn x_of(app: &App, e: Entity) -> f64 {
    app.world.get::<Transform>(e).unwrap().translation.x
}

#[test]
fn parallel_scripts_read_the_frame_start_and_their_changes_apply_after() {
    for vms in [0, 2] {
        let folder = Folder::new();
        folder.write("pair.luau", PAIR);
        let mut app = app_with_vms(&folder.0, vms);
        let a = spawn_script(&mut app, "A", "pair");
        let b = spawn_script(&mut app, "B", "pair");
        for (e, other, x) in [(a, "B", 0.0), (b, "A", 10.0)] {
            app.world.get_mut::<Script>(e).unwrap().props = json!({ "other": other });
            app.world.get_mut::<Transform>(e).unwrap().translation.x = x;
        }
        app.update();
        assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
        // Both read the other where the frame began (one after the other, B would see 11).
        assert_eq!(
            (x_of(&app, a), x_of(&app, b)),
            (11.0, 1.0),
            "with {vms} actor VMs"
        );
        let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
        let log = log.to_string();
        assert!(
            log.contains("A\\tsees itself at\\t11"),
            "reads its own change: {log}"
        );
        let actors: Vec<Option<u32>> = [a, b]
            .iter()
            .map(|&e| app.world.get::<ScriptActor>(e).map(|a| a.0))
            .collect();
        if vms == 0 {
            assert_eq!(actors, [None, None]);
        } else {
            assert_eq!(actors, [Some(1), Some(2)], "spread over the actor VMs");
            let list = call_api(&mut app.world, "script.list", json!({})).unwrap();
            assert_eq!(list["vms"], json!(3), "{list}");
        }
    }
}

const SWARM: &str = r#"
local Mote = {}
Mote.props = { seed = 0, leader = "" }

function Mote:start()
	self.leader = world.find(self.props.leader)
	self.phase = self.props.seed * 0.37
end

function Mote:parallel_update(dt)
	self.phase += dt
	local p = self.entity.position
	local to = self.leader.position - p
	local swirl = vec3(math.cos(self.phase), math.sin(self.phase * 1.3), math.sin(self.phase))
	self.entity.position = p + to * (0.05 * dt) + swirl * dt
	self.entity:rotate_y(dt * 0.5)
end

return Mote
"#;

/// Positions of a swarm of `n` motes, each following the one before it, after `frames`.
fn swarm_positions(vms: u32, n: usize, frames: usize) -> Vec<[f64; 3]> {
    let folder = Folder::new();
    folder.write("mote.luau", SWARM);
    let mut app = app_with_vms(&folder.0, vms);
    let motes: Vec<Entity> = (0..n)
        .map(|i| {
            let e = spawn_script(&mut app, &format!("Mote {i}"), "mote");
            app.world.get_mut::<Script>(e).unwrap().props =
                json!({ "seed": i, "leader": format!("Mote {}", (i + 1) % n) });
            app.world.get_mut::<Transform>(e).unwrap().translation =
                engine_core::math::DVec3::new(i as f64, (i % 7) as f64, -(i as f64) * 0.5);
            e
        })
        .collect();
    app.update_n(frames);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    if vms > 0 {
        let used: std::collections::BTreeSet<u32> = motes
            .iter()
            .filter_map(|&e| app.world.get::<ScriptActor>(e).map(|a| a.0))
            .collect();
        assert_eq!(used.len(), vms as usize, "every actor VM gets motes");
    }
    motes
        .iter()
        .map(|&e| {
            app.world
                .get::<Transform>(e)
                .unwrap()
                .translation
                .to_array()
        })
        .collect()
}

#[test]
fn parallel_scripts_give_the_same_result_on_any_number_of_vms() {
    let one = swarm_positions(0, 48, 20);
    let three = swarm_positions(3, 48, 20);
    assert_eq!(
        one, three,
        "parallel_update reads the frame's start, so no order shows"
    );
    assert_ne!(one[0], [0.0, 0.0, 0.0]);
}

const SYNCS: &str = r#"
local Syncs = {}

function Syncs:parallel_update(dt)
	if self.done then
		return
	end
	self.done = true
	local ok, err = pcall(test.add, { a = 1, b = 2 })
	print("parallel", ok, string.find(tostring(err), "task.synchronize", 1, true) ~= nil)
	self.entity.name = "Renamed"
	print("name until applied", self.entity.name)
	task.synchronize()
	print("serial", test.add({ a = 1, b = 2 }), self.entity.name)
	test.add({})
end

return Syncs
"#;

#[test]
fn engine_calls_wait_for_task_synchronize_and_changes_apply_after_the_phase() {
    let folder = Folder::new();
    folder.write("syncs.luau", SYNCS);
    let mut app = app_with_vms(&folder.0, 1);
    let e = spawn_script(&mut app, "Syncs", "syncs");
    app.update_n(2);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
    let log: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    let line = |start: &str| {
        log.iter()
            .find(|l| l.contains(start))
            .unwrap_or_else(|| panic!("no {start} in {log:?}"))
            .to_string()
    };
    assert!(
        line("parallel").ends_with("parallel\tfalse\ttrue"),
        "{log:?}"
    );
    assert!(
        line("name until").ends_with("Syncs"),
        "a queued change shows after: {log:?}"
    );
    assert!(line("serial").ends_with("serial\t3\tRenamed"), "{log:?}");
    assert_eq!(app.world.get::<Name>(e).unwrap().as_str(), "Renamed");
    assert_eq!(app.world.get::<ScriptActor>(e), Some(&ScriptActor(1)));
}

const PINGER: &str = r#"
local Pinger = {}

function Pinger:parallel_update(dt)
	if not self.sent then
		self.sent = true
		world.find("Ponger"):send("ping", { from = self.entity.id })
	end
end

function Pinger:on_message(event, data)
	print("pinger got", event, data.n)
end

return Pinger
"#;

const PONGER: &str = r#"
local Ponger = {}

function Ponger:on_message(event, data)
	print("ponger got", event)
	world.entity(data.from):send("pong", { n = 7 })
end

return Ponger
"#;

#[test]
fn messages_reach_scripts_in_other_vms() {
    let folder = Folder::new();
    folder.write("pinger.luau", PINGER);
    folder.write("ponger.luau", PONGER);
    let mut app = app_with_vms(&folder.0, 2);
    let pinger = spawn_script(&mut app, "Pinger", "pinger");
    spawn_script(&mut app, "Ponger", "ponger");
    app.update_n(4);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    assert!(app.world.get::<ScriptActor>(pinger).is_some());
    let log = call_api(&mut app.world, "script.log", json!({}))
        .unwrap()
        .to_string();
    assert!(log.contains("ponger got\\tping"), "{log}");
    assert!(log.contains("pinger got\\tpong\\t7"), "{log}");
}

#[test]
fn parallel_errors_are_reported_marked_and_fixed_by_hot_reload() {
    let folder = Folder::new();
    folder.write(
        "faulty_mote.luau",
        "local M = {}\nfunction M:parallel_update(dt)\n\tlocal t = nil\n\tself.entity.position = t.x\nend\nreturn M\n",
    );
    let mut app = app_with_vms(&folder.0, 2);
    let e = spawn_script(&mut app, "Faulty", "faulty_mote");
    app.update_n(3);
    let errs = errors(&mut app);
    assert!(
        errs.iter().any(|m| m.contains("faulty_mote.luau:4")),
        "{errs:?}"
    );
    assert_eq!(
        file_info(&mut app, "faulty_mote.luau")["fault"]["line"],
        json!(4)
    );
    edit(
        &folder,
        "faulty_mote.luau",
        "local M = {}\nfunction M:parallel_update(dt)\n\tself.entity.position = vec3(5, 0, 0)\nend\nreturn M\n",
    );
    app.update_n(3);
    assert_eq!(x_of(&app, e), 5.0, "the actor VM reloaded the file");
    assert_eq!(
        file_info(&mut app, "faulty_mote.luau")["fault"],
        Value::Null
    );
}

/// How much faster `parallel_update` runs on several VMs:
/// `cargo test -p engine_script --release -- --ignored --nocapture parallel_speed`.
#[test]
#[ignore]
fn parallel_speed() {
    const BUSY_MOTE: &str = "local M = {}
function M:parallel_update(dt)
	local p = self.entity.position
	local s = 0
	for i = 1, 300 do s += math.sin(p.x + i) * math.cos(p.z + i) end
	self.entity.position = p + vec3(s * 1e-6, 0, 0)
end
return M
";
    for vms in [0, 1, 2, 3, 7] {
        let folder = Folder::new();
        folder.write("busy_mote.luau", BUSY_MOTE);
        let mut app = app_with_vms(&folder.0, vms);
        for i in 0..2000 {
            spawn_script(&mut app, &format!("Mote {i}"), "busy_mote");
        }
        app.update_n(5);
        let t0 = std::time::Instant::now();
        app.update_n(30);
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / 30.0;
        let parallel_ms = app.world.resource::<ScriptRuntime>().parallel_ms();
        println!("{vms} actor VMs: {ms:.2} ms a frame (parallel phase {parallel_ms:.2} ms)");
    }
}

/// How fast a tight Luau loop runs, for comparing interrupt costs:
/// `cargo test -p engine_script --release -- --ignored --nocapture tight_loop`.
#[test]
#[ignore]
fn tight_loop_speed() {
    let folder = Folder::new();
    let mut app = app_in(&folder.0);
    app.world.resource_mut::<ScriptSettings>().call_limit_ms = 60_000.0;
    eval(&mut app, "return 1");
    for _ in 0..3 {
        let t0 = std::time::Instant::now();
        eval(
            &mut app,
            "local s = 0 for i = 1, 20000000 do s += i % 7 end return s",
        );
        let loop_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t0 = std::time::Instant::now();
        eval(
            &mut app,
            "local function f(x) return x + 1 end local s = 0 for i = 1, 5000000 do s = f(s) end return s",
        );
        let calls_ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("20M loop iterations: {loop_ms:.0} ms; 5M calls: {calls_ms:.0} ms");
    }
}

/// What common script operations cost, a million times each:
/// `cargo test -p engine_script --release -- --ignored --nocapture math_speed`.
#[test]
#[ignore]
fn math_speed() {
    let folder = Folder::new();
    let mut app = app_in(&folder.0);
    app.world.resource_mut::<ScriptSettings>().call_limit_ms = 60_000.0;
    eval(&mut app, "return 1");
    for (label, code) in [
        (
            "1M plain adds",
            "local x = 1.5 local s = 0 for i = 1, 1000000 do s += x end return s",
        ),
        (
            "1M math.sin",
            "local s = 0 for i = 1, 1000000 do s += math.sin(i) end return s",
        ),
        (
            "1M local sin",
            "local sin = math.sin local s = 0 for i = 1, 1000000 do s += sin(i) end return s",
        ),
        (
            "1M p.x",
            "local p = vec3(1, 2, 3) local s = 0 for i = 1, 1000000 do s += p.x end return s",
        ),
        (
            "1M p + q",
            "local p = vec3(1, 2, 3) local q = p for i = 1, 1000000 do q = q + p end return q.x",
        ),
        (
            "1M p * 2",
            "local p = vec3(1, 2, 3) local q = p for i = 1, 1000000 do q = p * 2 end return q.x",
        ),
        (
            "1M vec3(x, y, z)",
            "local q for i = 1, 1000000 do q = vec3(i, 2, 3) end return q.x",
        ),
        (
            "1M v:length()",
            "local p = vec3(1, 2, 3) local s = 0 for i = 1, 1000000 do s += p:length() end return s",
        ),
        (
            "1M q * v",
            "local q = quat.axis_angle(vec3.up, 0.5) local v = vec3(1, 2, 3) local r for i = 1, 1000000 do r = q * v end return r.x",
        ),
        (
            "1M vector.x",
            "local p = vector.create(1, 2, 3) local s = 0 for i = 1, 1000000 do s += p.x end return s",
        ),
        (
            "1M vector +",
            "local p = vector.create(1, 2, 3) local q = p for i = 1, 1000000 do q = q + p end return q.x",
        ),
    ] {
        let t0 = std::time::Instant::now();
        let r = eval(&mut app, code);
        println!(
            "{label}: {:.1} ms ({r})",
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// What the editor's Scripts tab reads: output since a cursor (with file and line), a file's
/// marks (instances, the last runtime error's line and count, logpoint values), and scripts
/// held still while `ScriptsHeld` is there.
#[test]
fn an_editor_sees_output_marks_and_can_hold_scripts() {
    use crate::editing::{OutputCursor, ScriptsHeld, file_marks};

    let folder = Folder::new();
    folder.write(
        "lamp.luau",
        "local Lamp = {}\nfunction Lamp:start()\n\tself.t = 0\n\tprint('lit')\nend\n\
         function Lamp:update(dt)\n\tself.t += dt\n\tlocal glow = self.t * 2\n\tif self.t > 0.5 then\n\
         \t\terror('too bright')\n\tend\nend\nreturn Lamp\n",
    );
    let mut app = app_in(&folder.0);
    app.insert_resource(ScriptsHeld);
    let e = spawn_script(&mut app, "Lamp", "lamp");
    app.update_n(10);
    // Held: the editor's VM loads the file (for `construct` and gizmos), but nothing runs.
    let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
    assert!(!log.to_string().contains("lit"), "held: nothing ran: {log}");
    app.world.remove_resource::<ScriptsHeld>();
    call_api(
        &mut app.world,
        "script.logpoint",
        json!({"path": "lamp.luau", "line": 9, "message": "glow = {glow}"}),
    )
    .unwrap();
    app.update_n(30);
    let mut cursor = OutputCursor::default();
    let out = app
        .world
        .resource::<ScriptRuntime>()
        .output_since(&mut cursor);
    assert!(
        out.iter()
            .any(|l| !l.error && l.text.ends_with("lit") && l.at == Some(("lamp.luau".into(), 4))),
        "{out:?}"
    );
    let error = out.iter().find(|l| l.error).expect("the error");
    assert_eq!(error.at, Some(("lamp.luau".into(), 10)), "{error:?}");
    assert!(error.text.contains("too bright"), "{error:?}");
    // Only what is new comes next time.
    app.update_n(2);
    let again = app
        .world
        .resource::<ScriptRuntime>()
        .output_since(&mut cursor);
    assert!(again.iter().all(|l| l.error), "{again:?}");
    assert!(again.len() <= 2, "{again:?}");

    let marks = file_marks(&app.world, "lamp.luau");
    assert!(
        marks.loaded && marks.version == 1 && marks.instances == 1,
        "{marks:?}"
    );
    let fault = marks.fault.expect("a fault");
    assert_eq!(fault.line, Some(10));
    assert!(fault.count > 5, "{}", fault.count);
    assert!(
        marks
            .values
            .get(&9)
            .is_some_and(|v| v.starts_with("glow = ")),
        "{:?}",
        marks.values
    );
    let api = call_api(&mut app.world, "script.marks", json!({"path": "lamp.luau"})).unwrap();
    assert_eq!(api["fault"]["line"], json!(10));
    let check = call_api(
        &mut app.world,
        "script.check",
        json!({"source": "local x = (1 +"}),
    )
    .unwrap();
    assert_eq!(check["ok"], json!(false));
    // Luau's type checker, with every API this app registers in its definitions.
    let types = call_api(
        &mut app.world,
        "script.typecheck",
        json!({"path": "lamp.luau", "text": "--!strict\nlocal x: number = \"a\"\nreturn x\n"}),
    )
    .unwrap();
    assert_eq!(types["files"][0]["errors"], json!(1), "{types}");
    assert_eq!(
        types["files"][0]["problems"][0]["line"],
        json!(2),
        "{types}"
    );
    let _ = e;
}

/// A visual script: a `.graph` file runs like a `.luau` one, hot reloads keeping its state,
/// keeps its old version while an edit has mistakes, and can be watched (each node's runs and
/// each wire's value) by the editor and graph boards.
#[test]
fn a_graph_runs_reloads_and_is_watched() {
    use crate::editing::file_marks;
    use crate::graph::{self, Graph, Link, Node};

    let node = |id: u32, kind: &str, of: &str, values: Value| Node {
        id,
        kind: kind.into(),
        of: of.into(),
        at: [0, 0],
        values: serde_json::from_value(values).unwrap_or_default(),
    };
    let link = |a: u32, ap: &str, b: u32, bp: &str| Link {
        from: (a, ap.into()),
        to: (b, bp.into()),
    };
    // Every frame: count = count + dt × speed.
    let counter = |speed: f64| Graph {
        props: [("speed".to_string(), json!(speed))].into(),
        vars: [("count".to_string(), json!(0))].into(),
        nodes: vec![
            node(1, "event.update", "", json!({})),
            node(2, "var.set", "count", json!({})),
            node(3, "math.add", "", json!({})),
            node(4, "var.get", "count", json!({})),
            node(5, "math.multiply", "", json!({})),
            node(6, "prop.get", "speed", json!({})),
        ],
        links: vec![
            link(1, "then", 2, "in"),
            link(4, "value", 3, "a"),
            link(5, "value", 3, "b"),
            link(1, "dt", 5, "a"),
            link(6, "value", 5, "b"),
            link(3, "value", 2, "value"),
        ],
        ..Default::default()
    };
    let folder = Folder::new();
    folder.write("counter.graph", &counter(3.0).to_text());
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Counter", "counter.graph");
    app.update_n(10);
    let s = "world.find('Counter').script";
    let count = |app: &mut App| eval(app, &format!("return {s}.count")).as_f64().unwrap();
    let first = count(&mut app);
    assert!((0.8..=1.2).contains(&first), "{first}");

    // Faster, and the count goes on from where it was.
    edit(&folder, "counter.graph", &counter(30.0).to_text());
    app.update_n(3);
    assert_eq!(
        eval(&mut app, &format!("return {s}.props.speed")),
        json!(30)
    );
    let second = count(&mut app);
    assert!(second > first + 1.5, "{first} -> {second}");
    assert_eq!(file_info(&mut app, "counter.graph")["version"], json!(2));

    // A mistake (a variable that doesn't exist) keeps the last good version running.
    let mut broken = counter(30.0);
    broken.nodes[3].of = "nope".into();
    edit(&folder, "counter.graph", &broken.to_text());
    app.update_n(3);
    let errs = errors(&mut app);
    assert!(
        errs.iter()
            .any(|e| e.contains("counter.graph") && e.contains("no variable named 'nope'")),
        "{errs:?}"
    );
    assert!(count(&mut app) > second, "still counting");
    assert_eq!(file_info(&mut app, "counter.graph")["version"], json!(2));
    edit(&folder, "counter.graph", &counter(30.0).to_text());
    app.update_n(2);
    assert!(file_info(&mut app, "counter.graph")["error"].is_null());

    // Watched: node 2 runs every frame, and the wires carry their values.
    let compiled = graph::compile(&counter(30.0), "counter.graph");
    assert!(graph::watch(&app.world, "counter.graph", &compiled));
    app.update_n(5);
    let marks = file_marks(&app.world, "counter.graph");
    let live = graph::live(&compiled, &marks.hits, &marks.values);
    assert!(live.runs.get(&2).is_some_and(|n| *n >= 4), "{live:?}");
    let dt = live.values.get(&3).expect("the dt wire's value");
    assert!(
        dt.parse::<f64>()
            .is_ok_and(|v| (v - 1.0 / 30.0).abs() < 1e-3),
        "{dt}"
    );
    assert!(
        live.values
            .get(&4)
            .and_then(|v| v.parse::<f64>().ok())
            .is_some(),
        "{live:?}"
    );

    // The API compiles graphs and lists the nodes there are.
    let api = call_api(
        &mut app.world,
        "script.graph",
        json!({"path": "counter.graph"}),
    )
    .unwrap();
    assert_eq!(api["ok"], json!(true));
    assert!(
        api["luau"]
            .as_str()
            .unwrap()
            .contains("function Counter:update(dt: number)")
    );
    let nodes = call_api(&mut app.world, "script.graph_nodes", json!({})).unwrap();
    assert!(
        nodes
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["kind"] == "api.call" && n["of"] == "test.add"),
        "engine functions are nodes"
    );

    // Every timers follow the graph across reloads: one whose node is deleted stops.
    let ticker = |every: bool| Graph {
        vars: [("ticks".to_string(), json!(0))].into(),
        nodes: if every {
            vec![
                node(1, "event.every", "", json!({"seconds": 0.1})),
                node(2, "var.set", "ticks", json!({})),
                node(3, "math.add", "", json!({"b": 1})),
                node(4, "var.get", "ticks", json!({})),
            ]
        } else {
            vec![node(5, "event.update", "", json!({}))]
        },
        links: if every {
            vec![
                link(1, "then", 2, "in"),
                link(4, "value", 3, "a"),
                link(3, "value", 2, "value"),
            ]
        } else {
            Vec::new()
        },
        ..Default::default()
    };
    folder.write("ticker.graph", &ticker(true).to_text());
    spawn_script(&mut app, "Ticker", "ticker.graph");
    app.update_n(31);
    let ticks = |app: &mut App| {
        eval(app, "return world.find('Ticker').script.ticks")
            .as_f64()
            .unwrap()
    };
    assert!(ticks(&mut app) >= 8.0, "{}", ticks(&mut app));
    edit(&folder, "ticker.graph", &ticker(false).to_text());
    app.update_n(3);
    let stopped = ticks(&mut app);
    app.update_n(30);
    assert_eq!(ticks(&mut app), stopped, "the deleted node's timer stopped");
    let errs = errors(&mut app);
    assert!(!errs.iter().any(|e| e.contains("ticker")), "{errs:?}");
}

/// The courtyard's crystal graph runs as it says: it bobs, changes color every 1.5 s and
/// flares when sent "flare", counting its flares.
#[test]
fn the_showcase_crystal_graph_runs() {
    use engine_render::{MeshRenderer, PointLight, Shape, StandardMaterial};

    let folder = Folder::new();
    let graph = include_str!("../../../assets/scripts/showcase/crystal.graph");
    folder.write("showcase/crystal.graph", graph);
    let mut app = app_in(&folder.0);
    app.insert_resource(engine_assets::Assets::<StandardMaterial>::default());
    app.register_component::<MeshRenderer>("A mesh.")
        .register_component::<PointLight>("A light.");
    app.world.spawn((
        Name::new("Graph crystal"),
        Transform::from_translation(DVec3::new(0.0, 1.0, 0.0)),
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.2,
                segments: 6,
            },
            StandardMaterial::default(),
        ),
        PointLight::default(),
        Script::new("showcase/crystal.graph"),
    ));
    app.update_n(50);
    assert_eq!(errors(&mut app), Vec::<String>::new());
    let s = "world.find('Graph crystal')";
    let y = eval(&mut app, &format!("return {s}.position.y"))
        .as_f64()
        .unwrap();
    assert!(
        (y - 1.0).abs() > 1e-4 && (y - 1.0).abs() < 0.13,
        "bobbing: {y}"
    );
    // Every 1.5 s the other color: A first (as it started), then B.
    let red = |app: &mut App| {
        eval(app, &format!("return {s}:get('PointLight').color"))[0]
            .as_f64()
            .unwrap()
    };
    assert!(red(&mut app) < 0.3);
    app.update_n(45);
    assert!(red(&mut app) > 0.9);
    call_api(
        &mut app.world,
        "script.send",
        json!({"entity": {"name": "Graph crystal"}, "event": "flare"}),
    )
    .unwrap();
    app.update_n(10);
    let lm = eval(
        &mut app,
        &format!("return {s}:get('PointLight').intensity_lm"),
    )
    .as_f64()
    .unwrap();
    assert!(lm > 2000.0, "flaring: {lm}");
    app.update_n(45);
    assert_eq!(
        eval(&mut app, &format!("return {s}.script.flares")),
        json!(1)
    );
    assert_eq!(errors(&mut app), Vec::<String>::new());
}

/// The example graphs the editor's Graphs tab opens: the lamp breathes between its low and
/// high lumens, the stone floats above where it started.
#[test]
fn the_example_graphs_run() {
    use engine_render::{MeshRenderer, PointLight, Shape, StandardMaterial};

    let folder = Folder::new();
    for (path, text) in [
        (
            "examples/pulse.graph",
            include_str!("../../../assets/scripts/examples/pulse.graph"),
        ),
        (
            "examples/hover.graph",
            include_str!("../../../assets/scripts/examples/hover.graph"),
        ),
    ] {
        let compiled = crate::graph::compile_text(text, path).unwrap();
        assert!(
            compiled.problems.is_empty(),
            "{path}: {:?}",
            compiled.problems
        );
        folder.write(path, text);
    }
    let mut app = app_in(&folder.0);
    app.insert_resource(engine_assets::Assets::<StandardMaterial>::default());
    app.register_component::<MeshRenderer>("A mesh.")
        .register_component::<PointLight>("A light.");
    let sphere = || {
        MeshRenderer::new(
            Shape::Sphere {
                radius: 0.2,
                segments: 6,
            },
            StandardMaterial::default(),
        )
    };
    app.world.spawn((
        Name::new("Lamp"),
        Transform::default(),
        sphere(),
        PointLight::default(),
        Script::new("examples/pulse.graph"),
    ));
    app.world.spawn((
        Name::new("Stone"),
        Transform::from_translation(DVec3::new(0.0, 1.0, 0.0)),
        sphere(),
        Script::new("examples/hover.graph"),
    ));
    let mut seen = Vec::new();
    for _ in 0..8 {
        app.update_n(10);
        let lm = eval(
            &mut app,
            "return world.find('Lamp'):get('PointLight').intensity_lm",
        );
        seen.push(lm.as_f64().unwrap());
    }
    assert!(
        seen.iter().all(|lm| (250.0..=1800.0).contains(lm)),
        "{seen:?}"
    );
    assert!(
        seen.iter().any(|lm| (lm - seen[0]).abs() > 50.0),
        "breathing: {seen:?}"
    );
    let y = eval(&mut app, "return world.find('Stone').position.y")
        .as_f64()
        .unwrap();
    assert!(y > 1.5 && y < 2.2, "floating: {y}");
    assert_eq!(errors(&mut app), Vec::<String>::new());
}

const LANTERN: &str = "local Lantern = {}
function Lantern:start()
	self.t = 0
	self.lit = 0
end
function Lantern:light(k)
	local glow = k * 2
	return glow
end
function Lantern:update(dt)
	self.t += dt
	local k = self.t * 10
	local glow = self:light(k)
	self.lit += 1
end
return Lantern
";

const OTHER: &str = "local Other = {}
function Other:start()
	self.n = 0
end
function Other:update()
	self.n += 1
end
return Other
";

fn local_names(world: &World, frame: usize) -> Vec<String> {
    let scopes = crate::debug::scopes(world, frame);
    let locals = scopes
        .iter()
        .find(|s| s.name == "Locals")
        .expect("a Locals scope");
    crate::debug::children(world, locals.reference)
        .into_iter()
        .map(|v| v.name)
        .collect()
}

fn script_number(app: &mut App, entity: &str, field: &str) -> f64 {
    eval(
        app,
        &format!("return world.find('{entity}').script.{field}"),
    )
    .as_f64()
    .unwrap()
}

#[test]
fn the_editor_debugs_in_place_while_the_frames_go_on() {
    use crate::debug::{self, Go};
    let folder = Folder::new();
    folder.write("debug/lantern.luau", LANTERN);
    folder.write("debug/other.luau", OTHER);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Lantern", "debug/lantern");
    spawn_script(&mut app, "Other", "debug/other");
    app.update_n(3);
    assert_eq!(
        debug::set_breakpoints(&mut app.world, "debug/lantern.luau", &[13]),
        vec![Some(13)]
    );
    assert!(debug::attached(&app.world));
    app.update();
    let stop = debug::stopped(&app.world).expect("stopped at the breakpoint");
    assert_eq!(stop.reason, "breakpoint");
    assert!(stop.live && stop.exception.is_none());
    assert_eq!(stop.frames[0].name, "update");
    assert_eq!(stop.frames[0].path, "debug/lantern.luau");
    assert_eq!(stop.frames[0].line, 13);
    let names = local_names(&app.world, 0);
    for name in ["self", "dt", "k"] {
        assert!(names.contains(&name.to_string()), "{name} in {names:?}");
    }
    let k = debug::evaluate(&mut app.world, 0, "k").unwrap();
    let k: f64 = k.value.parse().unwrap();
    let twice = debug::evaluate(&mut app.world, 0, "k * 2").unwrap();
    let twice: f64 = twice.value.parse().unwrap();
    assert!(
        (twice - k * 2.0).abs() <= 0.01 * k.abs().max(1.0),
        "{twice} vs {k}"
    );
    assert_eq!(
        debug::breakpoints(&app.world, "debug/lantern.luau")[0].hits,
        1
    );

    // The frames go on, and everything waits: the stopped script, the others, the world's
    // time.
    let (lit, n) = (
        script_number(&mut app, "Lantern", "lit"),
        script_number(&mut app, "Other", "n"),
    );
    app.update_n(5);
    assert_eq!(debug::stopped(&app.world).map(|s| s.id), Some(stop.id));
    assert_eq!(script_number(&mut app, "Lantern", "lit"), lit);
    assert_eq!(script_number(&mut app, "Other", "n"), n);
    assert!(
        app.world
            .resource::<TimeDilation>()
            .is_active("script-debugger")
    );

    // In: the arrow in light(), its caller under it; the file's values by their lines.
    assert!(debug::go(&mut app.world, Go::StepIn));
    app.update();
    let stop = debug::stopped(&app.world).expect("stepped in");
    assert_eq!(stop.reason, "step");
    assert_eq!(
        (stop.frames[0].name.as_str(), stop.frames[0].line),
        ("light", 7)
    );
    assert_eq!(
        (stop.frames[1].name.as_str(), stop.frames[1].line),
        ("update", 13)
    );
    let here = debug::stop_in(&app.world, "debug/lantern.luau", LANTERN).unwrap();
    assert_eq!((here.frame, here.line), (0, 7));
    assert!(here.values[&6].contains("k = "), "{:?}", here.values);
    let caller = debug::stop_at(&app.world, "debug/lantern.luau", LANTERN, Some(1)).unwrap();
    assert_eq!((caller.frame, caller.line), (1, 13));
    assert!(caller.values[&10].contains("dt = "), "{:?}", caller.values);
    assert!(caller.values[&12].contains("k = "), "{:?}", caller.values);

    // Out, back in update; over, to the next line.
    assert!(debug::go(&mut app.world, Go::StepOut));
    app.update();
    let stop = debug::stopped(&app.world).expect("stepped out");
    assert_eq!(stop.frames[0].name, "update");
    let line = stop.frames[0].line;
    assert!((13..=14).contains(&line), "{line}");
    assert!(debug::go(&mut app.world, Go::StepOver));
    app.update();
    let stop = debug::stopped(&app.world).expect("stepped over");
    assert!(stop.frames[0].line > line, "{:?}", stop.frames[0]);

    // Breakpoint off, Continue: the scripts and the time go on.
    debug::set_breakpoints(&mut app.world, "debug/lantern.luau", &[]);
    assert!(debug::go(&mut app.world, Go::Continue));
    app.update_n(3);
    assert!(debug::stopped(&app.world).is_none());
    assert!(script_number(&mut app, "Lantern", "lit") > lit);
    assert!(script_number(&mut app, "Other", "n") > n);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));

    // A conditional breakpoint stops only when its condition holds.
    debug::set_breakpoints_with(
        &mut app.world,
        "debug/lantern.luau",
        &[(14, Some("self.lit % 4 == 3".into()))],
    );
    app.update();
    for _ in 0..4 {
        if debug::stopped(&app.world).is_some() {
            break;
        }
        app.update();
    }
    debug::stopped(&app.world).expect("stopped when the condition held");
    let lit = debug::evaluate(&mut app.world, 0, "self.lit").unwrap();
    assert_eq!(lit.value.parse::<f64>().unwrap() % 4.0, 3.0);
    debug::detach(&mut app.world);
    app.update_n(2);
    assert!(debug::stopped(&app.world).is_none(), "detaching goes on");
    assert!(!debug::attached(&app.world));
}

#[test]
fn the_editor_stops_in_tasks_on_errors_and_where_it_cannot_wait() {
    use crate::debug::{self, Go};
    let folder = Folder::new();
    folder.write(
        "debug/waiter.luau",
        "local Waiter = {}
function Waiter:start()
	self.n = 0
	while true do
		self.n += 1
		task.wait(0.1)
	end
end
return Waiter
",
    );
    folder.write(
        "debug/sorter.luau",
        "local Sorter = {}
function Sorter:start()
	self.sorted = 0
end
function Sorter:update()
	local items = { 3, 1, 2 }
	table.sort(items, function(a, b)
		return a < b
	end)
	self.sorted += 1
end
return Sorter
",
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Waiter", "debug/waiter");
    spawn_script(&mut app, "Sorter", "debug/sorter");
    app.update_n(2);

    // In a task: it waits at its line; a step over its wait ends when it runs again.
    debug::set_breakpoints(&mut app.world, "debug/waiter.luau", &[6]);
    let mut frames = 0;
    while debug::stopped(&app.world).is_none() && frames < 20 {
        app.update();
        frames += 1;
    }
    let stop = debug::stopped(&app.world).expect("stopped in the task");
    assert_eq!(
        (stop.frames[0].name.as_str(), stop.frames[0].line),
        ("start", 6)
    );
    assert!(stop.live);
    let n = debug::evaluate(&mut app.world, 0, "self.n").unwrap().value;
    debug::set_breakpoints(&mut app.world, "debug/waiter.luau", &[]);
    assert!(debug::go(&mut app.world, Go::StepOver));
    let mut frames = 0;
    app.update();
    while debug::stopped(&app.world).is_none() && frames < 30 {
        app.update();
        frames += 1;
    }
    let stop = debug::stopped(&app.world).expect("the step ended after the wait");
    assert_eq!(stop.reason, "step");
    assert_eq!(stop.frames[0].name, "start");
    assert_eq!(
        debug::evaluate(&mut app.world, 0, "self.n").unwrap().value,
        n
    );
    assert!(debug::go(&mut app.world, Go::Continue));
    app.update_n(2);
    assert!(debug::stopped(&app.world).is_none());

    // Inside a sort's comparator a script can't wait (engine code is between it and the
    // frame): the stop shows the values there and the code goes on.
    debug::set_breakpoints(&mut app.world, "debug/sorter.luau", &[8]);
    app.update();
    let stop = debug::stopped(&app.world).expect("stopped in the comparator");
    assert_eq!(stop.reason, "breakpoint");
    assert!(!stop.live, "a look, not a wait");
    assert_eq!(stop.frames[0].line, 8);
    let names = local_names(&app.world, 0);
    assert!(names.contains(&"a".to_string()), "{names:?}");
    assert!(
        stop.frames.iter().any(|f| f.name == "update"),
        "{:?}",
        stop.frames
    );
    debug::set_breakpoints(&mut app.world, "debug/sorter.luau", &[]);
    assert!(debug::go(&mut app.world, Go::Continue));
    app.update_n(2);
    assert!(debug::stopped(&app.world).is_none());

    // On errors: where it was raised, with the locals there; going on, it stops again at the
    // next one until errors no longer stop.
    folder.write(
        "debug/sorter.luau",
        "local Sorter = {}
function Sorter:update(dt)
	self.n = (self.n or 0) + 1
	local limit = 30
	if self.n > 0 then
		local missing = nil
		missing.field = self.n
	end
end
return Sorter
",
    );
    debug::set_break_on_errors(&mut app.world, true);
    let mut frames = 0;
    while debug::stopped(&app.world).is_none() && frames < 10 {
        app.update();
        frames += 1;
    }
    let stop = debug::stopped(&app.world).expect("stopped on the error");
    let e = stop.exception.clone().expect("an exception stop");
    assert!(e.contains("attempt to index nil"), "{e}");
    assert!(!stop.live);
    assert_eq!(
        (stop.frames[0].name.as_str(), stop.frames[0].line),
        ("update", 7)
    );
    assert_eq!(
        debug::evaluate(&mut app.world, 0, "limit").unwrap().value,
        "30"
    );
    // `self` too: the VM writes an error over the frame's first registers once it unwinds.
    let n = debug::evaluate(&mut app.world, 0, "self.n").unwrap();
    assert!(n.kind == "integer" || n.kind == "number", "{n:?}");
    assert!(
        debug::go(&mut app.world, Go::StepOver),
        "goes on from an error"
    );
    app.update_n(2);
    let again = debug::stopped(&app.world).expect("the next error stops again");
    assert!(again.id > stop.id);
    debug::set_break_on_errors(&mut app.world, false);
    assert!(debug::go(&mut app.world, Go::Continue));
    app.update_n(3);
    assert!(debug::stopped(&app.world).is_none());
    assert!(
        errors(&mut app)
            .iter()
            .any(|e| e.contains("attempt to index nil")),
        "the errors are still reported"
    );
}

#[test]
fn script_break_debugs_from_the_api() {
    let folder = Folder::new();
    folder.write("debug/lantern.luau", LANTERN);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Lantern", "debug/lantern");
    app.update_n(2);
    let set = call_api(
        &mut app.world,
        "script.break",
        json!({"path": "debug/lantern.luau", "lines": [12]}),
    )
    .unwrap();
    assert_eq!(set["attached"], json!(true));
    assert_eq!(set["stopped"], Value::Null);
    app.update();
    let stop = call_api(&mut app.world, "script.break", json!({})).unwrap();
    assert_eq!(stop["stopped"]["reason"], json!("breakpoint"), "{stop}");
    assert_eq!(stop["stopped"]["frames"][0]["line"], json!(12));
    let locals = stop["stopped"]["locals"].as_array().unwrap();
    assert!(locals.iter().any(|l| l["name"] == "dt"), "{stop}");
    call_api(&mut app.world, "script.break", json!({"go": "over"})).unwrap();
    app.update();
    let stop = call_api(&mut app.world, "script.break", json!({})).unwrap();
    assert_eq!(stop["stopped"]["reason"], json!("step"), "{stop}");
    assert_eq!(stop["stopped"]["frames"][0]["line"], json!(13));
    assert!(call_api(&mut app.world, "script.break", json!({"go": "sideways"})).is_err());
    let lit = script_number(&mut app, "Lantern", "lit");
    call_api(&mut app.world, "script.break", json!({"detach": true})).unwrap();
    app.update_n(3);
    let done = call_api(&mut app.world, "script.break", json!({})).unwrap();
    assert_eq!(done["stopped"], Value::Null);
    assert!(script_number(&mut app, "Lantern", "lit") > lit);
    assert_eq!(done["attached"], json!(false));
}

const LIFT: &str = r#"
local Lift = {}
Lift.props = { wait = 0.5 }
Lift.states = {
	initial = "on",
	on = { reset = "on" },
	states = {
		on = {
			initial = "down",
			history = true,
			on = { cut = "off" },
			states = {
				down = { on = { call = { to = "rising", guard = "allowed" } } },
				rising = { done = "up" },
				up = { after = { "wait", "falling" }, when = { ["not allowed"] = "falling" } },
				falling = { done = "down" },
			},
		},
		off = { on = { cut = "on" } },
	},
}
function Lift:start()
	self.allowed = true
	self.trace = {}
end
function Lift:enter_rising()
	table.insert(self.trace, "rise")
	task.wait(0.2)
	table.insert(self.trace, "risen")
end
function Lift:exit_rising()
	table.insert(self.trace, "left rising")
end
function Lift:enter_falling()
	task.wait(0.1)
end
function Lift:update_up(dt)
	self.up_time = (self.up_time or 0) + dt
end
return Lift
"#;

fn lift_state(app: &mut App) -> String {
    eval(app, "return world.find('Lift').script.state")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Runs frames until the lift is in `want`; how many it took.
fn until_state(app: &mut App, want: &str, max: usize) -> usize {
    for k in 0..max {
        if lift_state(app) == want {
            return k;
        }
        app.update();
    }
    panic!(
        "not {want} after {max} frames but {}: {:?}",
        lift_state(app),
        errors(app)
    );
}

fn send(app: &mut App, to: &str, event: &str) {
    call_api(
        &mut app.world,
        "script.send",
        json!({"entity": {"name": to}, "event": event}),
    )
    .unwrap();
}

/// A class's `states` table runs as a hierarchical state machine: it starts in the first state
/// of the first group, messages are events (with guards), `done` follows an enter task that
/// waited, `after` counts a prop's seconds, `when` checks a condition every frame, a group with
/// history comes back to where it was, and leaving a state stops its enter task.
#[test]
fn a_states_table_runs_as_a_state_machine() {
    let folder = Folder::new();
    folder.write("lift.luau", LIFT);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Lift", "lift");
    app.update_n(2);
    assert_eq!(lift_state(&mut app), "down");
    assert_eq!(
        eval(&mut app, "return world.find('Lift').script:in_state('on')"),
        json!(true)
    );

    // A guard that doesn't hold: the event changes nothing.
    eval(&mut app, "world.find('Lift').script.allowed = false");
    send(&mut app, "Lift", "call");
    app.update_n(2);
    assert_eq!(lift_state(&mut app), "down");
    eval(&mut app, "world.find('Lift').script.allowed = true");

    // An event; `done` once the enter task has waited its 0.2 s (6 frames).
    send(&mut app, "Lift", "call");
    until_state(&mut app, "rising", 3);
    let rising = until_state(&mut app, "up", 12);
    assert!((5..=8).contains(&rising), "rising took {rising} frames");
    assert_eq!(
        eval(
            &mut app,
            "return table.concat(world.find('Lift').script.trace, ', ')"
        ),
        json!("rise, risen, left rising")
    );
    // `after` the `wait` prop's 0.5 s, falling, and down again when its wait is done.
    let up = until_state(&mut app, "falling", 30);
    assert!((14..=17).contains(&up), "up for {up} frames");
    let up_time = eval(&mut app, "return world.find('Lift').script.up_time")
        .as_f64()
        .unwrap();
    assert!((0.4..0.6).contains(&up_time), "update_up ran {up_time} s");
    until_state(&mut app, "down", 6);

    // History: cut the power while rising, back on goes on rising (its enter starts again);
    // leaving rising stopped the first enter task before it finished.
    eval(&mut app, "world.find('Lift').script.trace = {}");
    send(&mut app, "Lift", "call");
    until_state(&mut app, "rising", 3);
    send(&mut app, "Lift", "cut");
    until_state(&mut app, "off", 3);
    app.update_n(10);
    send(&mut app, "Lift", "cut");
    until_state(&mut app, "rising", 3);
    assert_eq!(
        eval(
            &mut app,
            "return table.concat(world.find('Lift').script.trace, ', ')"
        ),
        json!("rise, left rising, rise")
    );
    until_state(&mut app, "up", 12);

    // `when`: a condition that comes true moves it on the next frame.
    eval(&mut app, "world.find('Lift').script.allowed = false");
    app.update_n(1);
    assert_eq!(lift_state(&mut app), "falling");

    // The outer table's transitions work from any state; self:go and self:trigger from code.
    eval(&mut app, "world.find('Lift').script:go('off')");
    assert_eq!(lift_state(&mut app), "off");
    assert_eq!(
        eval(
            &mut app,
            "return world.find('Lift').script:trigger('nothing')"
        ),
        json!(false)
    );
    assert_eq!(
        eval(
            &mut app,
            "return world.find('Lift').script:trigger('reset')"
        ),
        json!(true)
    );
    // History again: `on` resumes in the state it was in, falling.
    assert_eq!(lift_state(&mut app), "falling");

    // What the editor reads: states, transitions with their lines, the active path and the log.
    let lift = app
        .world
        .query::<(Entity, &Name)>()
        .iter(&app.world)
        .find(|(_, n)| n.as_str() == "Lift")
        .map(|(e, _)| e)
        .unwrap();
    let d = call_api(
        &mut app.world,
        "script.states",
        json!({"entity": {"name": "Lift"}}),
    )
    .unwrap();
    assert_eq!(d["active"], json!(["on", "falling"]), "{d:#}");
    let names: Vec<&str> = d["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["off", "on", "down", "falling", "rising", "up"]);
    let after = d["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["trigger"] == "after wait (0.5 s)")
        .unwrap_or_else(|| panic!("{d:#}"));
    let line = LIFT.lines().position(|l| l.contains("after = {")).unwrap() as u64 + 1;
    assert_eq!(after["line"], json!(line));
    let live = crate::states::live(&app.world, lift).unwrap();
    let whys: Vec<&str> = live.log.iter().map(|c| c.why.as_str()).collect();
    assert!(whys.contains(&"call"), "{whys:?}");
    assert!(whys.contains(&"done"), "{whys:?}");
    assert!(whys.contains(&"after 0.5 s"), "{whys:?}");
    assert!(whys.contains(&"when not allowed"), "{whys:?}");
    assert_eq!(live.current(), Some("falling"));
    // The Live values don't show the machine's own bookkeeping.
    let values = crate::props::state(&app.world, lift).unwrap();
    assert!(values.iter().any(|v| v.name == "state"));
    assert!(values.iter().all(|v| !v.name.starts_with("__")));
}

/// Hot reload keeps the states that still exist (and their tasks); a state that went away
/// starts its group again. Wired signals and interactions are events too.
#[test]
fn state_machines_reload_and_hear_signals_and_interactions() {
    let folder = Folder::new();
    folder.write("lift.luau", LIFT);
    folder.write(
        "button.luau",
        r#"
local Button = {}
Button.signals = { pressed = "" }
function Button:on_interact(by)
	self:emit("pressed")
end
return Button
"#,
    );
    let mut app = app_in(&folder.0);
    app.add_message::<engine_world::Interact>();
    let lift = spawn_script(&mut app, "Lift", "lift");
    let button = app
        .world
        .spawn((
            Name::new("Button"),
            Transform::default(),
            Script::new("button").connect("pressed", "Lift", "call"),
        ))
        .id();
    let player = app.world.spawn(Name::new("Player")).id();
    app.update_n(2);
    app.world.write_message(engine_world::Interact {
        actor: player,
        target: button,
        from: DVec3::ZERO,
    });
    app.update_n(3);
    assert_eq!(lift_state(&mut app), "rising", "{:?}", errors(&mut app));
    app.update_n(8);
    assert_eq!(lift_state(&mut app), "up");

    // An edit that keeps `up`: still up, the same time in it.
    edit(
        &folder,
        "lift.luau",
        &LIFT.replace("wait = 0.5", "wait = 5"),
    );
    app.update_n(2);
    assert_eq!(lift_state(&mut app), "up");
    // An edit that renames `up`: the group starts again from its first state.
    let renamed = LIFT
        .replace("wait = 0.5", "wait = 5")
        .replace("done = \"up\"", "done = \"top\"")
        .replace("\t\t\t\tup = {", "\t\t\t\ttop = {")
        .replace("update_up", "update_top");
    edit(&folder, "lift.luau", &renamed);
    app.update_n(2);
    assert_eq!(lift_state(&mut app), "down", "{:?}", errors(&mut app));
    let live = crate::states::live(&app.world, lift).unwrap();
    assert_eq!(live.log.last().unwrap().why, "reload");

    // Interactions are the `interact` event.
    edit(
        &folder,
        "lift.luau",
        &renamed.replace(
            "down = { on = { call =",
            "down = { on = { interact = \"rising\", call =",
        ),
    );
    app.update_n(2);
    app.world.write_message(engine_world::Interact {
        actor: player,
        target: lift,
        from: DVec3::ZERO,
    });
    app.update_n(2);
    assert_eq!(lift_state(&mut app), "rising", "{:?}", errors(&mut app));
    let live = crate::states::live(&app.world, lift).unwrap();
    assert_eq!(live.log.last().unwrap().why, "interact by Player");
}

/// Mistakes in a states table are errors that name the state and what is wrong, and a loop of
/// transitions that never waits is stopped.
#[test]
fn state_machine_mistakes_say_what_is_wrong() {
    let folder = Folder::new();
    folder.write(
        "typo.luau",
        r#"
local Typo = {}
Typo.states = {
	initial = "a",
	states = {
		a = { on = { go = "bb" } },
		b = {},
	},
}
return Typo
"#,
    );
    folder.write(
        "spin.luau",
        r#"
local Spin = {}
Spin.states = {
	initial = "a",
	states = { a = { done = "b" }, b = { done = "a" } },
}
return Spin
"#,
    );
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Typo", "typo");
    spawn_script(&mut app, "Spin", "spin");
    app.update_n(2);
    let errs = errors(&mut app);
    assert!(
        errs.iter().any(
            |e| e.contains("states: state a: on go goes to \"bb\", which isn't a state (a, b)")
        ),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| e.contains("more than 32 changes in one frame")),
        "{errs:?}"
    );
    // The editor finds the line of the mistake.
    let declared = crate::props::declared(&mut app.world, "typo.luau").unwrap();
    let problem = declared.machine.unwrap().unwrap_err();
    assert_eq!(problem.line, 6, "{problem:?}");
}

const TURRET: &str = r#"
local Turret = {}
Turret.states = {
	initial = "active",
	states = {
		off = { on = { power = "active" } },
		active = {
			parallel = true,
			on = { power = "off" },
			states = {
				aim = {
					initial = "scanning",
					states = {
						scanning = { on = { seen = "tracking", alarm = "tracking" } },
						tracking = { on = { lost = "scanning" } },
					},
				},
				gun = {
					initial = "ready",
					states = {
						ready = { on = { fire = "reloading", alarm = "reloading" } },
						reloading = { after = { 0.2, "ready" } },
						jammed = { on = { fix = "ready" } },
					},
				},
			},
		},
	},
}
function Turret:start()
	self.trace = {}
end
function Turret:enter_active()
	table.insert(self.trace, "+active")
end
function Turret:exit_active()
	table.insert(self.trace, "-active")
end
function Turret:exit_gun()
	table.insert(self.trace, "-gun")
end
return Turret
"#;

fn turret(app: &mut App, code: &str) -> Value {
    eval(
        app,
        &format!("local t = world.find('Turret').script return {code}"),
    )
}

/// A `parallel = true` group runs all its states at once: each region keeps its own state, an
/// event reaches every region (one event can change both), `after` counts in each, leaving the
/// group leaves every region and coming back starts each from its first state; `go` changes
/// only the region of the state it goes to. The log keeps every state active after each
/// change, so the editor can show the machine as it was.
#[test]
fn parallel_states_run_side_by_side() {
    let folder = Folder::new();
    folder.write("turret.luau", TURRET);
    let mut app = app_in(&folder.0);
    let e = spawn_script(&mut app, "Turret", "turret");
    app.update_n(2);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(
        live.active,
        ["active", "aim", "scanning", "gun", "ready"],
        "document order: a group before its states, regions one after another"
    );
    assert_eq!(live.current, ["scanning", "ready"]);
    assert_eq!(turret(&mut app, "t.state"), json!("scanning"));
    assert_eq!(turret(&mut app, "t:in_state('ready')"), json!(true));

    // Each region on its own: firing reloads the gun, the aim keeps scanning.
    send(&mut app, "Turret", "fire");
    app.update();
    assert_eq!(turret(&mut app, "t:in_state('reloading')"), json!(true));
    assert_eq!(turret(&mut app, "t:in_state('scanning')"), json!(true));
    for _ in 0..12 {
        app.update();
    }
    assert_eq!(
        turret(&mut app, "t:in_state('ready')"),
        json!(true),
        "after 0.2 s"
    );

    // One event, both regions.
    send(&mut app, "Turret", "alarm");
    app.update();
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["tracking", "reloading"]);
    let alarms: Vec<&str> = live
        .log
        .iter()
        .filter(|c| c.why == "alarm")
        .map(|c| c.to.as_str())
        .collect();
    assert_eq!(alarms, ["tracking", "reloading"]);

    // `go` changes only its region.
    eval(&mut app, "world.find('Turret').script:go('jammed')");
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["tracking", "jammed"]);

    // Leaving the group leaves both regions; back, each starts from its first state.
    send(&mut app, "Turret", "power");
    app.update();
    assert_eq!(turret(&mut app, "t.state"), json!("off"));
    assert_eq!(turret(&mut app, "t:in_state('aim')"), json!(false));
    send(&mut app, "Turret", "power");
    app.update();
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["scanning", "ready"]);
    assert_eq!(
        turret(&mut app, "table.concat(t.trace, ' ')"),
        json!("+active -gun -active +active")
    );

    // The machine as it was after the alarm.
    let machine = crate::props::declared(&mut app.world, "turret.luau")
        .unwrap()
        .machine
        .unwrap()
        .unwrap();
    assert!(machine.state("active").unwrap().parallel);
    let k = live.log.iter().position(|c| c.why == "alarm").unwrap() + 1;
    let then = live.as_of(k, &machine);
    assert_eq!(then.current, ["tracking", "reloading"]);
    assert_eq!(then.clock, live.log[k].t);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

/// A breakpoint on a state's entry or exit is reached when the machine enters or leaves it: the
/// runtime hands the editor which entity, which state and why.
#[test]
fn state_breakpoints_are_reached() {
    let folder = Folder::new();
    folder.write("turret.luau", TURRET);
    let mut app = app_in(&folder.0);
    let e = spawn_script(&mut app, "Turret", "turret");
    app.update_n(2);
    {
        let mut b = app.world.resource_mut::<crate::states::StateBreakpoints>();
        b.set("turret.luau", "reloading", false, true);
        b.set("turret.luau", "tracking", true, true);
    }
    app.update();
    send(&mut app, "Turret", "fire");
    send(&mut app, "Turret", "seen");
    app.update();
    send(&mut app, "Turret", "lost");
    app.update();
    let hits = std::mem::take(
        &mut app
            .world
            .resource_mut::<crate::states::StateBreakpoints>()
            .hits,
    );
    let seen: Vec<(Entity, &str, &str, &str)> = hits
        .iter()
        .map(|h| (h.entity, h.kind.as_str(), h.state.as_str(), h.why.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            (e, "enter", "reloading", "fire"),
            (e, "exit", "tracking", "lost")
        ]
    );
    assert_eq!(hits[0].path, "turret.luau");
    // Cleared: no more.
    app.world
        .resource_mut::<crate::states::StateBreakpoints>()
        .set("turret.luau", "reloading", false, false);
    app.update_n(10);
    send(&mut app, "Turret", "fire");
    app.update();
    assert!(
        app.world
            .resource::<crate::states::StateBreakpoints>()
            .hits
            .is_empty()
    );

    // A parallel group can't say which state starts: they all do.
    folder.write(
        "bad.luau",
        "local Bad = {}\nBad.states = {\n\tstates = {\n\t\tboth = { parallel = true, initial = \"a\", states = { a = {}, b = {} } },\n\t},\n}\nreturn Bad\n",
    );
    spawn_script(&mut app, "Bad", "bad");
    app.update_n(2);
    let errs = errors(&mut app);
    assert!(
        errs.iter().any(|e| e.contains(
            "state both is parallel: all its states start together, so it has no initial"
        )),
        "{errs:?}"
    );
}

const SEAL: &str = r#"
local Seal = {}
Seal.props = { hold = 0.2 }
Seal.states = {
	initial = "sealed",
	states = {
		sealed = {
			parallel = true,
			done = "open",
			states = {
				a = {
					initial = "a_dark",
					states = { a_dark = { on = { touch_a = "a_lit" } }, a_lit = { final = true } },
				},
				b = {
					initial = "b_dark",
					states = { b_dark = { on = { touch_b = "b_lit" } }, b_lit = { final = true } },
				},
			},
		},
		open = { after = { "hold", "spent" } },
		spent = { final = true },
	},
}
function Seal:start()
	self.opened = 0
end
function Seal:enter_open()
	self.opened += 1
end
return Seal
"#;

/// `final = true` ends a group: a parallel group's `done` follows once every region is in a
/// final state (XState's onDone), and a final state at the outer level ends the machine:
/// events change nothing until a `go` starts it again. Each change keeps the script's values.
#[test]
fn final_states_finish_groups_and_end_the_machine() {
    let folder = Folder::new();
    folder.write("seal.luau", SEAL);
    let mut app = app_in(&folder.0);
    let e = spawn_script(&mut app, "Seal", "seal");
    app.update_n(2);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let seal = |app: &mut App, code: &str| {
        eval(
            app,
            &format!("local s = world.find('Seal').script return {code}"),
        )
    };
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["a_dark", "b_dark"]);
    assert!(live.done.is_empty() && !live.over);

    // One region done: the group waits for the other.
    send(&mut app, "Seal", "touch_a");
    app.update();
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["a_lit", "b_dark"]);
    assert_eq!(live.done.iter().collect::<Vec<_>>(), ["a"]);
    assert_eq!(seal(&mut app, "s:in_state('sealed')"), json!(true));

    // Both: the group is done and its `done` goes on, the same frame.
    send(&mut app, "Seal", "touch_b");
    app.update();
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["open"]);
    let last = live.log.last().unwrap();
    assert_eq!((last.to.as_str(), last.why.as_str()), ("open", "done"));
    let value = |c: &crate::states::Change, name: &str| {
        c.values
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(value(last, "opened").as_deref(), Some("1"), "{last:?}");
    assert_eq!(value(last, "props.hold").as_deref(), Some("0.2"));
    let before = &live.log[live.log.len() - 2];
    assert_eq!(value(before, "opened").as_deref(), Some("0"));

    // The machine as it was after the first rune: region a done.
    let machine = crate::props::declared(&mut app.world, "seal.luau")
        .unwrap()
        .machine
        .unwrap()
        .unwrap();
    assert!(machine.state("a_lit").unwrap().is_final);
    let k = live.log.iter().position(|c| c.to == "a_lit").unwrap();
    let then = live.as_of(k, &machine);
    assert_eq!(then.done.iter().collect::<Vec<_>>(), ["a"]);
    assert!(!then.over);

    // A final state at the outer level ends the machine.
    app.update_n(8);
    let live = crate::states::live(&app.world, e).unwrap();
    assert_eq!(live.current, ["spent"]);
    assert!(live.over);
    assert!(live.as_of(live.log.len() - 1, &machine).over);
    assert_eq!(seal(&mut app, "s:trigger('touch_a')"), json!(false));
    app.update_n(3);
    assert_eq!(seal(&mut app, "s.state"), json!("spent"));

    // `go` starts it again: each region from its first state.
    seal(&mut app, "s:go('sealed')");
    let live = crate::states::live(&app.world, e).unwrap();
    assert!(!live.over);
    assert_eq!(live.current, ["a_dark", "b_dark"]);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

const GUARD: &str = r#"
local Guard = {}
Guard.states = {
	initial = "patrol",
	states = {
		patrol = {
			initial = "route",
			history = "deep",
			on = { alarm = "alert", swap = "watch" },
			states = {
				route = {
					initial = "north",
					states = {
						north = { on = { next = "east" } },
						east = { on = { next = "south" } },
						south = {},
					},
				},
			},
		},
		alert = { on = { calm = "patrol" } },
		watch = {
			initial = "post",
			history = true,
			on = { swap = "patrol" },
			states = {
				post = {
					initial = "left",
					states = { left = { on = { turn = "right" } }, right = {} },
				},
			},
		},
	},
}
return Guard
"#;

/// `history = "deep"` brings a group back to where it was all the way in; `history = true`
/// only to the state directly inside it (the groups in that one start from their first).
#[test]
fn deep_history_resumes_inside_groups() {
    let folder = Folder::new();
    folder.write("guard.luau", GUARD);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Guard", "guard");
    app.update_n(2);
    let state = |app: &mut App| eval(app, "return world.find('Guard').script.state");
    assert_eq!(state(&mut app), json!("north"));
    for (event, want) in [
        ("next", "east"),
        ("alarm", "alert"),
        ("calm", "east"),
        ("swap", "left"),
        ("turn", "right"),
        ("swap", "east"),
        ("swap", "left"),
    ] {
        send(&mut app, "Guard", event);
        app.update();
        assert_eq!(state(&mut app), json!(want), "after {event}");
    }
    let machine = crate::props::declared(&mut app.world, "guard.luau")
        .unwrap()
        .machine
        .unwrap()
        .unwrap();
    let patrol = machine.state("patrol").unwrap();
    assert!(patrol.history && patrol.deep);
    let watch = machine.state("watch").unwrap();
    assert!(watch.history && !watch.deep);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

const CHAIN: &str = r#"
local Chain = {}
Chain.states = {
	initial = "a",
	states = {
		a = { on = { push = "b" } },
		b = { on = { next = "c" } },
		c = {},
		d = {},
	},
}
function Chain:start()
	self.trace = {}
end
function Chain:exit_a()
	table.insert(self.trace, "exit a")
end
function Chain:enter_b()
	table.insert(self.trace, "enter b")
	local took = self:trigger("next")
	table.insert(self.trace, `trigger {took} in {self.state}`)
end
function Chain:exit_b()
	table.insert(self.trace, "exit b")
end
function Chain:enter_c()
	table.insert(self.trace, "enter c")
	self:go("d")
	table.insert(self.trace, `go in {self.state}`)
end
function Chain:enter_d()
	table.insert(self.trace, "enter d")
end
return Chain
"#;

/// An event or a `go` sent from an enter or exit hook waits until the change it is part of has
/// settled (run to completion, as SCXML and XState), so every change finishes its exits and
/// enters first; a hook sees the state it is entering.
#[test]
fn events_from_hooks_wait_for_the_change() {
    let folder = Folder::new();
    folder.write("chain.luau", CHAIN);
    let mut app = app_in(&folder.0);
    let e = spawn_script(&mut app, "Chain", "chain");
    app.update_n(2);
    send(&mut app, "Chain", "push");
    app.update();
    assert_eq!(
        eval(
            &mut app,
            "return table.concat(world.find('Chain').script.trace, ', ')"
        ),
        json!("exit a, enter b, trigger true in b, exit b, enter c, go in c, enter d")
    );
    let live = crate::states::live(&app.world, e).unwrap();
    let path: Vec<&str> = live.log.iter().map(|c| c.to.as_str()).collect();
    assert_eq!(path, ["a", "b", "c", "d"]);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

/// Mistakes with final states and history say what is wrong.
#[test]
fn final_and_history_mistakes_say_what_is_wrong() {
    let folder = Folder::new();
    let cases = [
        (
            "out.luau",
            "a = { final = true, on = { poke = \"b\" } }, b = {}",
            "state a is final, so it has no way out (on): reaching it ends its group",
        ),
        (
            "never.luau",
            "a = { done = \"b\", states = { x = {} } }, b = {}",
            "state a: done follows when it reaches a final state, and none of its states is final",
        ),
        (
            "region.luau",
            "a = { parallel = true, done = \"b\", states = { x = { final = true }, y = { states = { y1 = {} } } } }, b = {}",
            "state a: done follows when every region is in a final state, and y never is",
        ),
        (
            "leaf.luau",
            "a = { history = true }, b = {}",
            "state a has history but no states inside",
        ),
        (
            "both.luau",
            "a = { parallel = true, history = true, states = { x = {}, y = {} } }, b = {}",
            "state a is parallel, so it enters all its states anyway: history = \"deep\"",
        ),
    ];
    for (file, states, _) in &cases {
        folder.write(
            file,
            &format!(
                "local M = {{}}\nM.states = {{\n\tinitial = \"a\",\n\tstates = {{ {states} }},\n}}\nreturn M\n"
            ),
        );
    }
    let mut app = app_in(&folder.0);
    for (file, _, _) in &cases {
        spawn_script(&mut app, file, file.trim_end_matches(".luau"));
    }
    app.update_n(2);
    let errs = errors(&mut app);
    for (_, _, want) in &cases {
        assert!(errs.iter().any(|e| e.contains(want)), "{want}: {errs:?}");
    }
}

const BUILDER: &str = r#"
local B = {}
B.props = {
	width = 2, -- where the part stands @handle x
}
function B:construct()
	self.part = self.entity:spawn({
		name = "Part",
		components = { Transform = { translation = vec3(self.props.width, 0, 0) } },
	})
	task.spawn(function() task.wait(1) print("never in edit mode") end)
end
function B:start()
	self.found = self.part ~= nil and self.part:exists()
end
function B:draw_gizmos()
	draw.sphere(vec3(1, 0, 0), 0.5, "orange")
end
function B:draw_gizmos_selected()
	draw.line(vec3.zero, vec3(0, 3, 0), { color = "cyan", on_top = true })
end
return B
"#;

/// The parts a script's `construct` built, with where they stand (their Transform).
fn parts_of(app: &mut App, by: Entity) -> Vec<(Entity, DVec3)> {
    let mut q = app
        .world
        .query::<(Entity, &crate::Constructed, &Transform)>();
    q.iter(&app.world)
        .filter(|(_, c, _)| c.by == by)
        .map(|(e, _, t)| (e, t.translation))
        .collect()
}

/// In an editor's edit mode (`ScriptsHeld`) a script's `construct` builds its parts on a
/// stand-in, again when its props change (the old parts go), and nothing else of it runs; once
/// playing, the stand-in's parts go and the script builds its own before `start`. Stopping the
/// script takes its parts with it.
#[test]
fn construct_builds_in_edit_mode_and_before_start() {
    let folder = Folder::new();
    folder.write("builder.luau", BUILDER);
    let mut app = app_in(&folder.0);
    app.insert_resource(crate::ScriptsHeld);
    let e = spawn_script(&mut app, "Builder", "builder");
    app.update_n(3);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let parts = parts_of(&mut app, e);
    assert_eq!(parts.len(), 1, "built once: {parts:?}");
    assert_eq!(parts[0].1, DVec3::new(2.0, 0.0, 0.0));
    assert_eq!(
        app.world.get::<ChildOf>(parts[0].0).map(|c| c.parent()),
        Some(e)
    );
    let first = parts[0].0;

    // A props change (a Details edit, a handle drag) builds it again.
    app.world.get_mut::<Script>(e).unwrap().props = json!({"width": 3.5});
    app.update_n(2);
    let parts = parts_of(&mut app, e);
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0].1, DVec3::new(3.5, 0.0, 0.0));
    assert!(app.world.get_entity(first).is_err(), "the old part went");
    // Its task was dropped: nothing runs in edit mode.
    app.update_n(40);
    let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
    assert!(!log.to_string().contains("never in edit mode"), "{log}");

    // Play: built anew by the script itself, before its `start`.
    app.world.remove_resource::<crate::ScriptsHeld>();
    app.update_n(2);
    let parts = parts_of(&mut app, e);
    assert_eq!(parts.len(), 1, "the stand-in's part went: {parts:?}");
    assert_eq!(
        eval(&mut app, "return world.find('Builder').script.found"),
        json!(true)
    );
    // The script goes, and its parts with it.
    app.world.entity_mut(e).remove::<Script>();
    app.update_n(2);
    assert!(parts_of(&mut app, e).is_empty());
}

/// `draw_gizmos` runs while gizmos show, `draw_gizmos_selected` for the selected entities,
/// both in the entity's own space; edit mode draws them from the stand-in. `draw` from any
/// code reaches the debug lines.
#[test]
fn gizmos_draw_in_the_entitys_space() {
    let folder = Folder::new();
    folder.write("builder.luau", BUILDER);
    let mut app = app_in(&folder.0);
    app.insert_resource(engine_debug::DebugDraw::default());
    app.insert_resource(crate::ScriptsHeld);
    let e = app
        .world
        .spawn((
            Name::new("Builder"),
            Transform::from_translation(DVec3::new(10.0, 0.0, 0.0))
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
            Script::new("builder"),
        ))
        .id();
    let lines = |app: &mut App| -> Vec<(DVec3, DVec3)> {
        let mut d = app.world.resource_mut::<engine_debug::DebugDraw>();
        let all: Vec<(DVec3, DVec3)> = [
            engine_debug::DepthMode::Tested,
            engine_debug::DepthMode::OnTop,
        ]
        .into_iter()
        .flat_map(|m| d.lines(m).map(|l| (l.a, l.b)).collect::<Vec<_>>())
        .collect();
        d.clear();
        all
    };
    app.update_n(2);
    assert!(lines(&mut app).is_empty(), "gizmos are off by default");

    app.world.resource_mut::<crate::ScriptGizmos>().show = true;
    app.update_n(1);
    let drawn = lines(&mut app);
    assert!(!drawn.is_empty());
    // The sphere at (1, 0, 0) in its space: turned a quarter, that is (10, 0, -1).
    let mid = drawn.iter().fold(DVec3::ZERO, |s, (a, b)| s + *a + *b) / (2 * drawn.len()) as f64;
    assert!((mid - DVec3::new(10.0, 0.0, -1.0)).length() < 0.05, "{mid}");

    app.world.resource_mut::<crate::ScriptGizmos>().selected = vec![e];
    app.update_n(1);
    let drawn = lines(&mut app);
    assert!(
        drawn
            .iter()
            .any(|(a, b)| (*a - DVec3::new(10.0, 0.0, 0.0)).length() < 1e-6
                && (*b - DVec3::new(10.0, 3.0, 0.0)).length() < 1e-6),
        "the selected line, up from the entity"
    );

    // Playing, and from the console.
    app.world.remove_resource::<crate::ScriptsHeld>();
    app.update_n(2);
    assert!(!lines(&mut app).is_empty(), "the running script draws too");
    eval(&mut app, "draw.line(vec3(0, 0, 0), vec3(0, 0, 7), 'red')");
    app.update_n(1);
    assert!(
        lines(&mut app)
            .iter()
            .any(|(a, b)| *a == DVec3::ZERO && *b == DVec3::new(0.0, 0.0, 7.0))
    );
    let err = call_api(
        &mut app.world,
        "script.eval",
        json!({"code": "draw.line(vec3.zero, vec3.one, 'chartreuse')"}),
    )
    .unwrap_err();
    assert!(err.contains("no color named"), "{err}");
}

const TOOLED: &str = r#"
local T = {}
T.props = {
	count = 2, -- parts @int
	baked = false, -- @hide
}
function T:construct()
	if self.props.baked then return end
	for i = 1, self.props.count do
		self.entity:spawn({ name = `Built {i}`, components = { Transform = {} } })
	end
end
-- Makes the parts for good. @button
function T:bake()
	for i = 1, self.props.count do
		self.entity:spawn({ name = `Kept {i}`, components = { Transform = {} } })
	end
	self.props.baked = true
end
-- One more. @button Add one
function T:add_one()
	self.props.count += 1
end
-- @button
function T:oops()
	error("no stones here")
end
function T:not_a_button() end
return T
"#;

/// `@button` methods: listed with their labels, run in edit mode on the stand-in (what they
/// set in `self.props` is kept in the Script, what they spawn stays), and while playing on the
/// running script; an error comes back to the caller.
#[test]
fn tool_buttons_run_before_play_and_keep_their_props() {
    let folder = Folder::new();
    folder.write("tooled.luau", TOOLED);
    let mut app = app_in(&folder.0);
    app.insert_resource(crate::ScriptsHeld);
    let e = spawn_script(&mut app, "Tooled", "tooled");
    app.update_n(2);
    let methods = crate::props::declared(&mut app.world, "tooled.luau")
        .unwrap()
        .methods;
    let buttons: Vec<(String, String, String)> = methods
        .iter()
        .filter_map(|m| m.button.clone().map(|b| (m.name.clone(), b, m.doc.clone())))
        .collect();
    assert_eq!(
        buttons,
        [
            (
                "bake".into(),
                "Bake".into(),
                "Makes the parts for good.".into()
            ),
            ("add_one".into(), "Add one".into(), "One more.".into()),
            ("oops".into(), "Oops".into(), String::new()),
        ]
    );
    assert_eq!(parts_of(&mut app, e).len(), 2);

    let tool = |app: &mut App, method: &str| {
        call_api(
            &mut app.world,
            "script.tool",
            json!({"entity": {"name": "Tooled"}, "method": method}),
        )
    };
    let out = tool(&mut app, "add_one").unwrap();
    assert_eq!(out["changed"], json!({"count": 3}));
    app.update_n(2);
    assert_eq!(parts_of(&mut app, e).len(), 3, "rebuilt with the new count");

    tool(&mut app, "bake").unwrap();
    app.update_n(2);
    let script = app.world.get::<Script>(e).unwrap().clone();
    assert_eq!(script.props["baked"], json!(true));
    assert!(
        parts_of(&mut app, e).is_empty(),
        "baked: construct builds nothing"
    );
    let kept = eval(&mut app, "return #world.find('Tooled'):children()");
    assert_eq!(kept, json!(3), "the baked parts stay");

    let err = tool(&mut app, "oops").unwrap_err();
    assert!(err.contains("no stones here"), "{err}");
    assert!(
        tool(&mut app, "missing")
            .unwrap_err()
            .contains("no method missing")
    );

    // Playing: on the running script.
    app.world.remove_resource::<crate::ScriptsHeld>();
    app.update_n(2);
    let out = tool(&mut app, "add_one").unwrap();
    assert_eq!(out["changed"], json!({"count": 4}));
    assert_eq!(
        eval(&mut app, "return world.find('Tooled').script.props.count"),
        json!(4)
    );
}

/// Tool files in `tools/` are listed with their titles and run on a selection.
#[test]
fn tool_files_run_on_the_selection() {
    let folder = Folder::new();
    folder.write(
        "tools/shout.luau",
        "--!strict\n-- Names the selection in capitals.\nlocal Tool = {}\nTool.title = \"Shout\"\n\
         function Tool.run(selection: { Entity })\n\tfor _, e in selection do\n\t\t\
         e.name = string.upper(e.name or \"\")\n\tend\nend\nreturn Tool\n",
    );
    folder.write("tools/quiet.luau", "return { run = function() end }\n");
    let mut app = app_in(&folder.0);
    app.insert_resource(crate::ScriptsHeld);
    app.world.spawn((Name::new("crate"), Transform::default()));
    app.world.spawn((Name::new("barrel"), Transform::default()));
    app.update_n(1);
    let list = call_api(&mut app.world, "script.tools", json!({})).unwrap();
    assert_eq!(
        list[0]["title"],
        json!("Quiet"),
        "titled from its name: {list}"
    );
    assert_eq!(list[1]["title"], json!("Shout"));
    assert_eq!(list[1]["doc"], json!("Names the selection in capitals."));
    call_api(
        &mut app.world,
        "script.tool",
        json!({"file": "tools/shout.luau", "selection": ["crate", "barrel"]}),
    )
    .unwrap();
    assert!(
        call_api(
            &mut app.world,
            "script.eval",
            json!({"code": "return world.find('CRATE') ~= nil and world.find('BARREL') ~= nil"})
        )
        .unwrap()
            == json!(true)
    );
}

const WATCHER: &str = r#"
local Watcher = {}
Watcher.props = { speed = 2 }
Watcher.signals = { spotted = "" }
Watcher.states = {
	initial = "idle",
	states = {
		idle = { after = { 0.5, "alert" } },
		alert = {},
	},
}

function Watcher:construct()
	self.eye = self.entity:spawn({ name = "Eye", components = { Transform = { translation = vec3(0, 1, 0) } } })
end

function Watcher:start()
	self.flown = 0
end

function Watcher:update(dt)
	self.flown += self.props.speed * dt
	self.eye.position = vec3(self.flown, 1, 0)
	draw.sphere(vec3(0, 0, 0), 1, "red")
	if self.flown > 1 and not self.said then
		self.said = true
		print("past one meter")
		world.find("Ear"):send("hello")
		self:emit("spotted")
	end
	if self.flown > 1.5 and not self.failed then
		self.failed = true
		error("boom")
	end
end
return Watcher
"#;

/// The rewind recorder keeps each script's values (when they change), its states, where it
/// and the parts it holds were, what happened (prints, messages both ways, signals, errors) and
/// what it drew, frame by frame; it answers what was when, and forgets what is too old.
#[test]
fn the_rewind_recorder_keeps_what_each_script_did_frame_by_frame() {
    let folder = Folder::new();
    folder.write("watcher.luau", WATCHER);
    folder.write(
        "ear.luau",
        "local Ear = {}\nfunction Ear:hello() self.heard = true end\nreturn Ear\n",
    );
    let mut app = app_in(&folder.0);
    app.insert_resource(crate::ScriptRewind::recording());
    let watcher = spawn_script(&mut app, "Watcher", "watcher");
    spawn_script(&mut app, "Ear", "ear");
    // 2 m/s at 30 frames a second: past 1 m after 15 frames, 1.5 m after 23.
    app.update_n(40);

    let rw = app.world.resource::<crate::ScriptRewind>();
    let (from, to) = rw.span().expect("frames were recorded");
    assert!(to - from > 1.2, "{from}..{to}");
    let track = rw.track(watcher).expect("the watcher has a track");
    assert_eq!(track.name, "Watcher");
    assert_eq!(track.path, "watcher.luau");
    // `flown` grows every frame; the prop never changes, so it is kept once.
    let flown = track.values.iter().find(|s| s.name == "flown").unwrap();
    assert!(flown.points.len() > 30, "{}", flown.points.len());
    let speed = track
        .values
        .iter()
        .find(|s| s.name == "props.speed")
        .unwrap();
    assert_eq!(speed.points.len(), 1);
    // Half a second in: idle; a second in: alert (the machine's `state` isn't a value).
    let half = rw.frame_at(from + 0.25).unwrap();
    let later = rw.frame_at(from + 1.0).unwrap();
    assert_eq!(track.states_at(half), ["idle"]);
    assert_eq!(track.states_at(later), ["alert"]);
    assert!(track.value_at("state", later).is_none());
    // The eye it holds moved with `flown`; the watcher itself stayed put.
    let eye = track.parts.iter().find(|p| p.field == "eye").unwrap();
    let at_later = eye.pose_at(later).unwrap().position.x;
    let flown_later = track
        .value_at("flown", later)
        .and_then(|v| v.number())
        .unwrap();
    assert!(
        (at_later - flown_later).abs() < 1e-6,
        "{at_later} vs {flown_later}"
    );
    assert_eq!(track.parts[0].poses.len(), 1);
    // What happened, each in its frame: the print, the message (both ends), the signal, the
    // error; and the sphere it drew every frame.
    let kinds: Vec<&str> = track.events.iter().map(|e| e.kind.name()).collect();
    for kind in ["print", "sent", "signal", "error"] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    let sent = track
        .events
        .iter()
        .find(|e| e.kind.name() == "sent")
        .unwrap();
    assert_eq!(sent.text, "hello to Ear");
    let printed = track
        .events
        .iter()
        .find(|e| e.kind.name() == "print")
        .unwrap();
    let said_at = track
        .values
        .iter()
        .find(|s| s.name == "said")
        .unwrap()
        .points[0]
        .0;
    assert_eq!(
        printed.frame, said_at,
        "the print is in the frame `said` was set"
    );
    let ear = rw.tracks().iter().find(|t| t.name == "Ear").unwrap();
    assert!(
        ear.events.iter().any(|e| e.text == "hello from Watcher"),
        "{:?}",
        ear.events
    );
    let last = rw.frame_span().unwrap().1;
    assert_eq!(track.shapes_at(rw, last, to).count(), 1);

    // The API: as it was, one value over time, and the errors.
    let then = call_api(&mut app.world, "script.rewind", json!({"at": from + 1.0})).unwrap();
    let w = then["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Watcher")
        .unwrap()
        .clone();
    assert_eq!(w["states"], json!(["alert"]));
    assert!(w["values"]["flown"].as_f64().unwrap() > 1.9, "{w}");
    let series = call_api(
        &mut app.world,
        "script.rewind",
        json!({"entity": {"name": "Watcher"}, "value": "flown"}),
    )
    .unwrap();
    assert!(series["points"].as_array().unwrap().len() > 30, "{series}");
    let errors = call_api(
        &mut app.world,
        "script.rewind",
        json!({"events": true, "kind": "error"}),
    )
    .unwrap();
    assert_eq!(errors.as_array().unwrap().len(), 1, "{errors}");
    assert!(
        errors[0]["text"].as_str().unwrap().contains("boom"),
        "{errors}"
    );

    // Kept for `keep_secs`: older frames go, and what was set before stays known.
    call_api(&mut app.world, "script.rewind", json!({"keep_secs": 1.0})).unwrap();
    app.update_n(60);
    let rw = app.world.resource::<crate::ScriptRewind>();
    let (first_before, _) = (from, to);
    let (from, to) = rw.span().unwrap();
    // It trims in steps: back to 1 s once over 1.1 s and a second.
    assert!(from > first_before + 1.0, "{first_before} then {from}");
    assert!(to - from <= 2.15, "{from}..{to}");
    let track = rw.track(watcher).unwrap();
    let first = rw.frame_span().unwrap().0;
    assert!(track.value_at("props.speed", first).is_some());
    assert_eq!(track.states_at(first), ["alert"]);

    // Stopped: nothing more is kept.
    call_api(&mut app.world, "script.rewind", json!({"record": false})).unwrap();
    let frames = app.world.resource::<crate::ScriptRewind>().frame_count();
    app.update_n(5);
    assert_eq!(
        app.world.resource::<crate::ScriptRewind>().frame_count(),
        frames
    );
}

const TWEENER: &str = r#"
local Tweener = {}

function Tweener:start()
	self.glow = 0
	self.peak = 0
	self.log = {}
	self.door = world.find("Door")
	self.lift = world.find("Lift")
	self.slow = world.find("Slow")
	-- An entity's position, linear over a second, then a callback.
	self.move = self:tween(self.door, { position = vec3(4, 0, 0) }, 1, {
		ease = "linear",
		done = function()
			table.insert(self.log, "moved")
		end,
	})
	-- A field of self, overshooting.
	self:tween(self, { glow = 1 }, 0.5, { ease = ease.out_back })
	-- A sequence: up, a call, then 2 m along x from wherever it is.
	self:sequence()
		:to(self.lift, { position = vec3(0, 1, 0) }, 0.5, { ease = "linear" })
		:call(function()
			table.insert(self.log, "up")
		end)
		:to(self.lift, { position = vec3(2, 0, 0) }, 0.5, { ease = "linear", relative = true })
	-- A component's field.
	self:tween(self.slow, { ["LocalTimeScale.factor"] = 0.25 }, 0.5, { ease = "linear" })
end

function Tweener:update()
	self.peak = math.max(self.peak, self.glow)
end

return Tweener
"#;

fn spawn_at(app: &mut App, name: &str, at: DVec3) -> Entity {
    app.world
        .spawn((Name::new(name.to_string()), Transform::from_translation(at)))
        .id()
}

/// Tweens move entities, a script's own fields and components' fields, through their eases;
/// sequences run their steps in order with calls between; a newer tween takes a value over.
#[test]
fn tweens_and_sequences_move_values_over_time() {
    let folder = Folder::new();
    folder.write("tweener.luau", TWEENER);
    let mut app = app_in(&folder.0);
    let door = spawn_at(&mut app, "Door", DVec3::ZERO);
    let lift = spawn_at(&mut app, "Lift", DVec3::ZERO);
    let slow = app
        .world
        .spawn((
            Name::new("Slow"),
            Transform::default(),
            LocalTimeScale {
                factor: 1.0,
                ignore_world_dilation: false,
            },
        ))
        .id();
    spawn_script(&mut app, "Tweener", "tweener.luau");
    let x = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    // The start frame, then half a second (the tween starts the frame after it's made).
    app.update_n(2 + 15);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let half = x(&app, door).x;
    assert!((half - 2.0).abs() < 0.2, "half way at half time: {half}");
    assert!((x(&app, lift).y - 1.0).abs() < 0.1, "{}", x(&app, lift));
    app.update_n(20);
    assert!((x(&app, door).x - 4.0).abs() < 1e-9, "{}", x(&app, door));
    let lifted = x(&app, lift);
    assert!(
        (lifted - DVec3::new(2.0, 1.0, 0.0)).length() < 1e-6,
        "up, then 2 m along: {lifted}"
    );
    let factor = app.world.get::<LocalTimeScale>(slow).unwrap().factor;
    assert!((factor - 0.25).abs() < 1e-6, "{factor}");
    assert_eq!(
        eval(&mut app, "return world.find('Tweener').script.log"),
        json!(["up", "moved"])
    );
    let peak = eval(&mut app, "return world.find('Tweener').script.peak");
    assert!(peak.as_f64().unwrap() > 1.05, "out_back overshoots: {peak}");
    assert_eq!(
        eval(&mut app, "return world.find('Tweener').script.glow"),
        json!(1)
    );
    assert!(eval(&mut app, "return world.find('Tweener').script.move.done") == json!(true));

    // A newer tween on the same value takes it over; the older one stops moving it.
    eval(
        &mut app,
        "local d = world.find('Door')
         tween.to(d, { position = vec3(0, 0, 0) }, 10, { ease = 'linear', label = 'slow' })
         tween.to(d, { position = vec3(0, 3, 0) }, 0.2, { ease = 'linear' })",
    );
    app.update_n(12);
    assert!(
        (x(&app, door) - DVec3::new(0.0, 3.0, 0.0)).length() < 1e-6,
        "{}",
        x(&app, door)
    );
    let rows = app.world.resource::<ScriptRuntime>().tweens();
    assert!(
        rows.is_empty(),
        "the slow one lost its only value: {rows:?}"
    );

    // Loops and yoyo: there and back, twice.
    eval(
        &mut app,
        "tween.to(world.find('Door'), { position = vec3(1, 3, 0) }, 0.2, { ease = 'linear', loops = 4, yoyo = true })",
    );
    // Its first frame starts it; three more are 0.1 s.
    app.update_n(1 + 3);
    assert!((x(&app, door).x - 0.5).abs() < 0.05, "{}", x(&app, door));
    app.update_n(7);
    assert!(x(&app, door).x < 0.4, "on its way back: {}", x(&app, door));
    app.update_n(30);
    assert!(
        x(&app, door).x.abs() < 1e-6,
        "back where it began: {}",
        x(&app, door)
    );

    // Mistakes are said where they're made.
    let err = eval(
        &mut app,
        "local ok, err = pcall(tween.to, world.find('Door'), { postion = vec3(1, 0, 0) }, 1)
         return tostring(err)",
    );
    assert!(
        err.as_str().unwrap().contains("can't tween \"postion\""),
        "{err}"
    );
    let err = eval(
        &mut app,
        "local ok, err = pcall(tween.to, world.find('Door'), { position = vec3(1, 0, 0) }, 1, { ease = 'out_wobble' })
         return tostring(err)",
    );
    assert!(
        err.as_str().unwrap().contains("no ease \"out_wobble\""),
        "{err}"
    );
}

/// A spring follows its goal without overshooting (critically damped), and calling it again
/// moves the goal; a bouncy one overshoots.
#[test]
fn springs_follow_their_goal() {
    let folder = Folder::new();
    let mut app = app_in(&folder.0);
    let ball = spawn_at(&mut app, "Ball", DVec3::ZERO);
    let bob = spawn_at(&mut app, "Bob", DVec3::ZERO);
    eval(
        &mut app,
        "tween.spring(world.find('Ball'), { position = vec3(3, 0, 0) }, { halflife = 0.1 })
         tween.spring(world.find('Bob'), { position = vec3(3, 0, 0) }, { halflife = 0.2, bounce = 0.6 })",
    );
    let mut ball_max = 0.0f64;
    let mut bob_max = 0.0f64;
    for _ in 0..30 {
        app.update();
        ball_max = ball_max.max(app.world.get::<Transform>(ball).unwrap().translation.x);
        bob_max = bob_max.max(app.world.get::<Transform>(bob).unwrap().translation.x);
    }
    assert!(
        ball_max <= 3.0 + 1e-6,
        "critical: no overshoot ({ball_max})"
    );
    assert!(bob_max > 3.05, "bouncy: past the goal ({bob_max})");
    let x = app.world.get::<Transform>(ball).unwrap().translation.x;
    assert!((x - 3.0).abs() < 0.01, "{x}");
    eval(
        &mut app,
        "tween.spring(world.find('Ball'), { position = vec3(-1, 0, 0) }, { halflife = 0.1 })",
    );
    app.update_n(45);
    let x = app.world.get::<Transform>(ball).unwrap().translation.x;
    assert!((x + 1.0).abs() < 1e-3, "{x}");
}

const TIMELINE_DOOR: &str = r#"
local Door = {}
Door.timelines = {
	open = {
		tracks = {
			swing = { { 0, 0, "linear" }, { 1, 1 } },
			glow = "curves/glow.curve",
		},
		events = { creak = 0.25, thud = 1 },
	},
}

function Door:start()
	self.events = {}
	self.swing = 0
	self.glow = 0
end

function Door:on_open(v)
	self.swing = v.swing
	self.glow = v.glow
end

function Door:creak()
	table.insert(self.events, "creak")
end

function Door:thud()
	table.insert(self.events, "thud")
end

function Door:on_open_done(tl)
	table.insert(self.events, "done " .. tl.direction)
end

return Door
"#;

/// A class's timelines play their tracks (keys in the script, or a .curve file that follows
/// edits), call their events' methods as they pass them either way, and say when they end.
#[test]
fn timelines_play_tracks_and_events_both_ways() {
    let folder = Folder::new();
    folder.write("door.luau", TIMELINE_DOOR);
    folder.write(
        "curves/glow.curve",
        r#"{ "keys": [ { "t": 0, "v": 0, "interp": "linear" }, { "t": 1, "v": 2 } ] }"#,
    );
    let mut app = app_in(&folder.0);
    let door = spawn_script(&mut app, "Door", "door.luau");
    app.update_n(2);
    eval(
        &mut app,
        "world.find('Door').script:timeline('open'):play()",
    );
    app.update_n(15);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    let get = |app: &mut App, k: &str| eval(app, &format!("return world.find('Door').script.{k}"));
    let swing = get(&mut app, "swing").as_f64().unwrap();
    assert!((swing - 0.5).abs() < 0.05, "{swing}");
    let glow = get(&mut app, "glow").as_f64().unwrap();
    assert!((glow - 1.0).abs() < 0.1, "{glow}");
    assert_eq!(get(&mut app, "events"), json!(["creak"]));
    app.update_n(20);
    assert_eq!(get(&mut app, "swing"), json!(1));
    assert_eq!(get(&mut app, "events"), json!(["creak", "thud", "done 1"]));

    // The editor sees where it is, its tracks and events.
    let tl = app
        .world
        .resource_scope(|world, rt: Mut<ScriptRuntime>| rt.timelines_of(world, door));
    assert_eq!(tl.len(), 1);
    assert_eq!(tl[0].name, "open");
    assert_eq!(tl[0].length, 1.0);
    assert_eq!(tl[0].tracks.len(), 2);
    assert_eq!(tl[0].events.len(), 2);

    // Back from the end: creak again, then done backward.
    eval(
        &mut app,
        "world.find('Door').script:timeline('open'):reverse()",
    );
    app.update_n(40);
    assert_eq!(get(&mut app, "swing"), json!(0));
    assert_eq!(
        get(&mut app, "events"),
        json!(["creak", "thud", "done 1", "creak", "done -1"])
    );

    // The curve file changes: the next play follows it.
    std::thread::sleep(std::time::Duration::from_millis(20));
    folder.write(
        "curves/glow.curve",
        r#"{ "keys": [ { "t": 0, "v": 5, "interp": "constant" }, { "t": 1, "v": 5 } ] }"#,
    );
    app.update_n(2);
    eval(
        &mut app,
        "world.find('Door').script:timeline('open'):play_from_start()",
    );
    app.update_n(3);
    assert_eq!(get(&mut app, "glow"), json!(5));
}

/// Eases by name, CSS beziers and curves made from keys.
#[test]
fn eases_and_curves_for_scripts() {
    let folder = Folder::new();
    let mut app = app_in(&folder.0);
    assert_eq!(eval(&mut app, "return ease.out_cubic(0.5)"), json!(0.875));
    assert_eq!(eval(&mut app, "return ease.in_out(0.25)"), json!(0.125));
    let b = eval(&mut app, "return ease.bezier(0.42, 0, 0.58, 1)(0.5)");
    assert!((b.as_f64().unwrap() - 0.5).abs() < 1e-5, "{b}");
    assert_eq!(
        eval(&mut app, "return curve({ {0, 0}, {1, 2} }):at(0.5)"),
        json!(1)
    );
    assert_eq!(
        eval(&mut app, "return curve({ {0, 0}, {2, 4} }).length"),
        json!(2)
    );
    assert_eq!(eval(&mut app, "return #ease.names()"), json!(44));
}

const STAGE_CUT: &str = r#"{
  "length": 4,
  "blend_in": 0,
  "marks": [ { "t": 3, "name": "late" } ],
  "tracks": [
    { "kind": "cuts", "cuts": [ { "t": 0, "camera": "A" }, { "t": 2, "camera": "B", "blend": 1 } ] },
    { "kind": "events", "bind": "Ear", "events": [
      { "t": 0, "event": "heard", "data": { "word": "start" } },
      { "t": 1, "event": "heard", "data": { "word": "one" } },
      { "t": 3.5, "event": "heard", "data": { "word": "late" }, "retro": true }
    ] },
    { "kind": "value", "bind": "Box", "prop": "position", "channels": {
      "x": { "keys": [ { "t": 0, "v": 0, "interp": "linear" }, { "t": 4, "v": 8 } ] } } },
    { "kind": "value", "bind": "Box", "prop": "scale", "finished": "restore", "channels": {
      "y": { "keys": [ { "t": 0, "v": 1, "interp": "linear" }, { "t": 4, "v": 3 } ] } } }
  ]
}"#;

const CUT_EAR: &str = r#"
local Ear = {}
function Ear:start() self.words = {} end
function Ear:heard(data) table.insert(self.words, data.word) end
return Ear
"#;

/// The stage for the cutscene tests: a box, a script that hears events, two cameras.
fn cutscene_stage(folder: &Folder) -> (App, Entity, Entity, Entity) {
    folder.write("cutscenes/stage.cutscene", STAGE_CUT);
    folder.write("ear.luau", CUT_EAR);
    let mut app = app_in(&folder.0);
    let boxed = app
        .world
        .spawn((Name::new("Box"), Transform::from_xyz(0.0, 1.0, 5.0)))
        .id();
    spawn_script(&mut app, "Ear", "ear.luau");
    let off = engine_render::Camera {
        active: false,
        ..engine_render::Camera::perspective(40.0)
    };
    let a = app
        .world
        .spawn((Name::new("A"), Transform::from_xyz(10.0, 2.0, 0.0), off))
        .id();
    let b = app
        .world
        .spawn((
            Name::new("B"),
            Transform::from_xyz(-10.0, 2.0, 0.0),
            engine_render::Camera {
                active: false,
                ..engine_render::Camera::perspective(80.0)
            },
        ))
        .id();
    // The game's own camera, behind the cutscene's view.
    app.world
        .spawn((Transform::default(), engine_render::Camera::default()));
    app.update_n(2);
    let _ = a;
    (app, boxed, a, b)
}

fn the_view(app: &mut App) -> Option<(Transform, engine_render::Camera)> {
    let mut q = app.world.query::<(
        &Transform,
        &engine_render::Camera,
        &crate::cutscene::CutsceneView,
    )>();
    q.iter(&app.world).next().map(|(t, c, _)| (*t, *c))
}

/// A cutscene moves its objects on their curves, sends its events in order, looks through its
/// cameras (blending between them), and at its end keeps or restores what each track says.
#[test]
fn cutscenes_move_objects_send_events_and_cut_cameras() {
    let folder = Folder::new();
    let (mut app, boxed, _, _) = cutscene_stage(&folder);
    let id = call_api(
        &mut app.world,
        "script.cutscene",
        json!({"path": "cutscenes/stage", "play": true}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap();
    // 1 s in (30 frames): the box a quarter of the way, its scale too; camera A shown.
    app.update_n(30);
    let t = *app.world.get::<Transform>(boxed).unwrap();
    assert!((t.translation.x - 2.0).abs() < 0.15, "{t:?}");
    assert_eq!(t.translation.y, 1.0, "axes without curves keep their value");
    assert!((t.scale.y - 1.5).abs() < 0.1, "{t:?}");
    let (view, lens) = the_view(&mut app).expect("a view camera while it plays");
    assert!((view.translation.x - 10.0).abs() < 1e-6, "{view:?}");
    assert_eq!(lens.order, crate::cutscene::VIEW_ORDER);
    assert!(lens.active);
    app.update_n(5);
    assert_eq!(
        eval(&mut app, "return world.find('Ear').script.words"),
        json!(["start", "one"])
    );
    // 2.5 s: half way through the blend from A to B, the lens between.
    app.update_n(40);
    let (view, lens) = the_view(&mut app).unwrap();
    assert!(view.translation.x.abs() < 1.5, "{view:?}");
    let engine_render::Projection::Perspective { fov_y_deg, .. } = lens.projection else {
        panic!()
    };
    assert!(fov_y_deg > 50.0 && fov_y_deg < 70.0, "{fov_y_deg}");
    // To the end: the box kept where it ended, its scale put back, the view gone.
    app.update_n(60);
    let t = *app.world.get::<Transform>(boxed).unwrap();
    assert!((t.translation.x - 8.0).abs() < 1e-6, "{t:?}");
    assert_eq!(t.scale.y, 1.0, "a restore track puts it back");
    assert!(the_view(&mut app).is_none());
    assert!(app.world.resource::<crate::Cutscenes>().get(id).is_none());
    app.update_n(2);
    assert_eq!(
        eval(&mut app, "return world.find('Ear').script.words"),
        json!(["start", "one", "late"])
    );
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

/// From a script: a handle to pause, seek to a mark, skip (its retro events still sent), and
/// `restore` putting back everything.
#[test]
fn scripts_play_pause_seek_and_skip_cutscenes() {
    let folder = Folder::new();
    let (mut app, boxed, _, _) = cutscene_stage(&folder);
    let ear = "world.find('Ear').script";
    eval(
        &mut app,
        &format!("{ear}.cs = cutscene.play('cutscenes/stage', {{ cameras = false }})"),
    );
    app.update_n(15);
    eval(&mut app, &format!("{ear}.cs:pause()"));
    let x = app.world.get::<Transform>(boxed).unwrap().translation.x;
    app.update_n(15);
    assert_eq!(app.world.get::<Transform>(boxed).unwrap().translation.x, x);
    assert_eq!(
        eval(&mut app, &format!("return {ear}.cs.playing")),
        json!(false)
    );
    assert!(the_view(&mut app).is_none(), "cameras = false: no view");
    // To the "late" mark: posed there at once.
    eval(&mut app, &format!("{ear}.cs:seek('late')"));
    let x = app.world.get::<Transform>(boxed).unwrap().translation.x;
    assert!((x - 6.0).abs() < 1e-6, "{x}");
    assert_eq!(
        eval(&mut app, &format!("return {ear}.cs.time")).as_f64(),
        Some(3.0)
    );
    eval(&mut app, &format!("{ear}.cs:skip()"));
    app.update_n(3);
    assert_eq!(
        eval(&mut app, &format!("return {ear}.cs.done")),
        json!(true)
    );
    assert_eq!(
        eval(&mut app, "return world.find('Ear').script.words"),
        json!(["start", "late"]),
        "the jump to 'late' sends nothing; the skip sends the retro event"
    );
    // restore = true: everything goes back when it ends.
    eval(
        &mut app,
        &format!("{ear}.cs = cutscene.play('cutscenes/stage', {{ restore = true, rate = 4 }})"),
    );
    app.update_n(10);
    let x = app.world.get::<Transform>(boxed).unwrap().translation.x;
    assert!(
        x > 1.0 && x < 6.0,
        "{x}: from the start again, four times as fast"
    );
    app.update_n(30);
    let t = *app.world.get::<Transform>(boxed).unwrap();
    assert!(
        (t.translation.x - 8.0).abs() < 1e-6,
        "{t:?}: kept from the first play"
    );
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

/// A `CutscenePlayer` plays by itself when the game runs, finding names below it first; a
/// looping one goes round, sending a `once` event only the first time.
#[test]
fn a_cutscene_player_autoplays_and_loops() {
    let folder = Folder::new();
    folder.write(
        "cutscenes/spin.cutscene",
        r#"{ "length": 1, "tracks": [
  { "kind": "value", "bind": "Wheel", "prop": "rotation", "channels": {
    "yaw": { "keys": [ { "t": 0, "v": 0, "interp": "linear" }, { "t": 1, "v": 90 } ] } } },
  { "kind": "events", "events": [ { "t": 0.5, "event": "half", "once": true }, { "t": 0.25, "event": "tick" } ] }
] }"#,
    );
    folder.write(
        "counter.luau",
        "local C = {}\nfunction C:start() self.halves = 0 self.ticks = 0 end\nfunction C:half() self.halves += 1 end\nfunction C:tick() self.ticks += 1 end\nreturn C\n",
    );
    let mut app = app_in(&folder.0);
    // Another Wheel elsewhere: the player's own child is the one that turns.
    let elsewhere = app
        .world
        .spawn((Name::new("Wheel"), Transform::default()))
        .id();
    let player = app
        .world
        .spawn((
            Name::new("Spinner"),
            Transform::default(),
            Script::new("counter.luau"),
            crate::CutscenePlayer {
                path: "cutscenes/spin".into(),
                looping: true,
                ..Default::default()
            },
        ))
        .id();
    let wheel = app
        .world
        .spawn((Name::new("Wheel"), Transform::default(), ChildOf(player)))
        .id();
    app.update_n(15);
    let yaw = |app: &App, e: Entity| {
        app.world
            .get::<Transform>(e)
            .unwrap()
            .rotation
            .to_euler(engine_core::math::EulerRot::YXZ)
            .0
            .to_degrees()
    };
    assert!(
        (yaw(&app, wheel) - 45.0).abs() < 4.0,
        "{}",
        yaw(&app, wheel)
    );
    assert_eq!(yaw(&app, elsewhere), 0.0);
    app.update_n(80);
    assert_eq!(
        eval(&mut app, "return world.find('Spinner').script.halves"),
        json!(1)
    );
    let ticks = eval(&mut app, "return world.find('Spinner').script.ticks")
        .as_f64()
        .unwrap();
    assert!(ticks >= 3.0, "{ticks}: once a loop");
    assert_eq!(app.world.resource::<crate::Cutscenes>().playing.len(), 1);
}

/// The shipped cutscenes read back as the editor writes them (a key a line), and the
/// courtyard's names are the showcase's.
#[test]
fn the_shipped_cutscenes_read_back_as_the_editor_writes_them() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scripts");
    let files = crate::cutscene::find_files(&root);
    assert!(files.len() >= 2, "{files:?}");
    for f in &files {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        let c = crate::cutscene::Cutscene::parse(&text).unwrap_or_else(|e| panic!("{f}: {e}"));
        assert_eq!(
            c.to_text(),
            text,
            "{f} isn't written as the editor writes it"
        );
    }
    let text = std::fs::read_to_string(root.join("cutscenes/courtyard.cutscene")).unwrap();
    let c = crate::cutscene::Cutscene::parse(&text).unwrap();
    for name in c.names() {
        assert!(
            [
                "Herald",
                "Cam wide",
                "Cam herald",
                "Cam bridge",
                crate::demo::BRIDGE_NAME
            ]
            .contains(&name.as_str()),
            "{name}"
        );
    }
}

// ------------------------------------------------------------------------------- story

const GUIDE_YARN: &str = r#"title: Start
when: once
---
<<declare $gold = 5>>
Guide: Hello, {$name}. You have {$gold} gold.
<<wave 2>>
-> Buy a map (3 gold) <<if $gold >= 3>>
    <<set $gold -= 3>>
    <<set $has_map to true>>
    Guide: Here you are.
-> Ask the way
    <<detour Way>>
    Guide: Anything else?
-> Rob me <<if $gold > 100>>
-> Leave
    <<stop>>
Guide: Safe travels.
<<shout "bye now" 2>>
===
title: Start
when: always
---
=> Guide: Back so soon?
=> Guide: Map treating you well? <<if $has_map>>
<<wait 0.5>>
Guide: Off you go.
===
title: Way
---
Guide: North, past the seal.
<<return>>
Guide: (never said)
===
title: Broken
---
Guide: Fine so far.
Guide: {10 / $zero}
===
"#;

const GUIDE_SCRIPT: &str = r#"local Guide = {}
function Guide:start()
	self.waves = 0
	dialogue.commands.shout = function(words, times)
		self.shouted = `{words} x{times}`
	end
end
function Guide:wave(n)
	self.waves += n
end
return Guide
"#;

fn story_now(app: &mut App) -> Value {
    call_api(&mut app.world, "story.state", json!({})).unwrap()["current"].clone()
}

fn story_last(app: &mut App, id: u64) -> Value {
    call_api(&mut app.world, "story.state", json!({ "id": id })).unwrap()
}

fn start_guide(app: &mut App, node: &str) -> u64 {
    call_api(
        &mut app.world,
        "story.start",
        json!({"path": "dialogue/guide", "node": node, "owner": "Guide", "headless": true}),
    )
    .unwrap_or_else(|e| panic!("{e}"))["id"]
        .as_u64()
        .unwrap()
}

/// A Yarn conversation: its lines with facts in them, a command on the script that started
/// it, options shown by their conditions, picks, `<<set>>`, a detour and its return, and a
/// command of `dialogue.commands`; then the node group's other node, its line group and a
/// wait.
#[test]
fn conversations_run_lines_options_and_commands() {
    let folder = Folder::new();
    folder.write("dialogue/guide.yarn", GUIDE_YARN);
    folder.write("guide.luau", GUIDE_SCRIPT);
    let mut app = app_in(&folder.0);
    spawn_script(&mut app, "Guide", "guide.luau");
    app.update_n(2);

    // The detour first.
    let id = start_guide(&mut app, "Start");
    let now = story_now(&mut app);
    assert_eq!(now["text"], "Hello, . You have 5 gold.", "{now}");
    assert_eq!(now["speaker"], "Guide");
    assert_eq!(now["waiting"], "options");
    assert_eq!(
        now["options"],
        json!(["Buy a map (3 gold)", "Ask the way", "Leave"]),
        "Rob me is hidden by its condition"
    );
    assert_eq!(
        eval(&mut app, "return world.find('Guide').script.waves"),
        json!(2)
    );
    call_api(&mut app.world, "story.choose", json!({"index": 2})).unwrap();
    app.update_n(2);
    let done = story_last(&mut app, id);
    assert_eq!(done["done"], true, "{done}");
    assert_eq!(done["text"], "Safe travels.");
    assert_eq!(done["taken"], json!(["Start", "Way"]));
    assert_eq!(
        eval(&mut app, "return world.find('Guide').script.shouted"),
        json!("bye now x2")
    );
    let facts = call_api(&mut app.world, "story.facts", json!({})).unwrap();
    assert_eq!(facts["facts"]["gold"], json!(5));
    assert_eq!(facts["visited"]["Start"], json!(1));
    assert_eq!(facts["visited"]["Way"], json!(1));

    // Again from the start of the story: the map bought this time.
    call_api(
        &mut app.world,
        "story.facts",
        json!({"clear": true, "set": {"name": "Ada"}}),
    )
    .unwrap();
    let id = start_guide(&mut app, "Start");
    assert_eq!(story_now(&mut app)["text"], "Hello, Ada. You have 5 gold.");
    call_api(
        &mut app.world,
        "story.choose",
        json!({"index": 1, "id": id}),
    )
    .unwrap();
    app.update_n(2);
    assert_eq!(story_last(&mut app, id)["done"], true);
    assert_eq!(
        eval(&mut app, "return { facts.gold, facts.has_map }"),
        json!([2, true]),
        "scripts read what the conversation set"
    );

    // The once node is spent: the other one, whose line group picks the line that tests more,
    // then waits half a second of game time.
    let id = start_guide(&mut app, "Start");
    let now = story_last(&mut app, id);
    assert_eq!(now["text"], "Map treating you well?", "{now}");
    assert_eq!(now["waiting"], "time");
    app.update_n(10);
    assert_eq!(story_last(&mut app, id)["done"], false, "still waiting");
    app.update_n(8);
    let end = story_last(&mut app, id);
    assert_eq!(end["done"], true, "{end}");
    assert_eq!(end["text"], "Off you go.");

    // A mistake at run time ends it with its .yarn line, in the script errors too.
    let id = start_guide(&mut app, "Broken");
    app.update_n(1);
    let broken = story_last(&mut app, id);
    assert_eq!(broken["done"], true);
    let error = broken["error"].as_str().unwrap_or("").to_string();
    assert!(
        error.contains("dialogue/guide.yarn:37") && error.contains("divided by zero"),
        "{error}"
    );
    assert!(errors(&mut app).iter().any(|e| e.contains("guide.yarn:37")));
    // And one in the file itself names its line before anything runs.
    folder.write("dialogue/bad.yarn", "title: A\n---\n<<if $x>>\nHi\n===\n");
    let e = call_api(
        &mut app.world,
        "story.start",
        json!({"path": "dialogue/bad", "node": "A", "headless": true}),
    )
    .unwrap_err();
    assert!(e.contains("dialogue/bad.yarn:3"), "{e}");
}

const GATE_YARN: &str = r#"title: Start
---
<<declare $skill_persuade = 1>>
<<bonus persuade 2 "Knows his name">>
Guard: Halt.
-> Talk him round #check:persuade:8
    <<if passed()>>
        Guard: Fine, go on.
    <<else>>
        Guard: Nice try.
    <<endif>>
-> Shove past #check:might:10 #red
    Guard: Oof. {passed()}
-> Leave
===
title: Chatter
when: $mood == "happy"
---
Guard: Lovely day.
Guard: Isn't it?
===
title: Chatter
when: $mood == "cross"
---
Guard: Hm.
===
"#;

fn close(a: &Value, b: f64) -> bool {
    a.as_f64().is_some_and(|a| (a - b).abs() < 1e-9)
}

/// The odds are counted over every way the dice fall, with double six and double one (or a
/// natural 20 and 1) deciding by themselves.
#[test]
fn check_odds_count_every_roll() {
    use crate::story::check_chance;
    assert_eq!(
        check_chance("2d6", 0.0, 13.0),
        1.0 / 36.0,
        "double six still passes"
    );
    assert_eq!(
        check_chance("2d6", 0.0, 2.0),
        35.0 / 36.0,
        "double one still fails"
    );
    assert_eq!(check_chance("2d6", 3.0, 8.0), 30.0 / 36.0);
    assert_eq!(check_chance("2d6", 0.0, 10.0), 6.0 / 36.0);
    assert_eq!(check_chance("d20", 0.0, 11.0), 0.5);
    assert_eq!(check_chance("d20", 0.0, 40.0), 0.05, "a natural 20");
    assert_eq!(check_chance("d20", 0.0, -5.0), 0.95, "a natural 1");
}

/// Skill checks on options: their odds with the skill and bonuses in them, the result in the
/// option's lines through passed(), a failed white check shut until the skill is higher, a
/// red check gone once rolled, luck after a failure, and the difficulty setting.
#[test]
fn checks_roll_remember_and_shut() {
    let folder = Folder::new();
    folder.write("dialogue/gate.yarn", GATE_YARN);
    let mut app = app_in(&folder.0);
    app.update_n(2);
    let start = |app: &mut App| -> u64 {
        call_api(
            &mut app.world,
            "story.start",
            json!({"path": "dialogue/gate", "headless": true}),
        )
        .unwrap_or_else(|e| panic!("{e}"))["id"]
            .as_u64()
            .unwrap()
    };

    let id = start(&mut app);
    let now = story_now(&mut app);
    assert_eq!(now["waiting"], "options", "{now}");
    let talk = &now["checks"][0];
    assert_eq!(talk["option"].as_f64(), Some(1.0), "{now}");
    assert_eq!(talk["skill"], "persuade");
    assert_eq!(talk["value"].as_f64(), Some(1.0));
    assert_eq!(talk["bonuses"], json!([[2.0, "Knows his name"]]));
    assert!(close(&talk["chance"], 30.0 / 36.0), "{talk}");
    let shove = &now["checks"][1];
    assert_eq!(shove["red"], true);
    assert!(close(&shove["chance"], 6.0 / 36.0), "{shove}");

    // The talk fails (decided here, not rolled): its failure lines, and it is shut.
    call_api(
        &mut app.world,
        "story.choose",
        json!({"index": 1, "pass": false}),
    )
    .unwrap();
    app.update_n(2);
    let done = story_last(&mut app, id);
    assert_eq!(done["done"], true, "{done}");
    assert_eq!(done["text"], "Nice try.", "{done}");
    assert_eq!(done["roll"]["passed"], false);
    assert_eq!(done["roll"]["how"], "forced");
    assert!(
        done["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["roll"] == true && l["text"] == "Persuade 8: failure (decided, not rolled)"),
        "{done}"
    );

    // Again: the talk is shut until Persuade is above 1, and luck helps the shove.
    let id = start(&mut app);
    let now = story_now(&mut app);
    assert_eq!(now["options"], json!(["Shove past", "Leave"]), "{now}");
    assert_eq!(
        now["shut"],
        json!([{"text": "Talk him round", "skill": "persuade", "above": 1.0}])
    );
    assert_eq!(now["checks"][0]["bonuses"], json!([[1.0, "Luck"]]));
    assert!(close(&now["checks"][0]["chance"], 10.0 / 36.0), "{now}");
    call_api(
        &mut app.world,
        "story.choose",
        json!({"index": 1, "pass": true}),
    )
    .unwrap();
    app.update_n(2);
    assert_eq!(story_last(&mut app, id)["text"], "Oof. true");

    // Better at persuading: the talk opens again; the shove, a red check, is gone for good,
    // and the pass took the luck away.
    call_api(
        &mut app.world,
        "story.facts",
        json!({"set": {"skill_persuade": 2}}),
    )
    .unwrap();
    call_api(
        &mut app.world,
        "story.settings",
        json!({"difficulty": "hard"}),
    )
    .unwrap();
    let id = start(&mut app);
    let now = story_now(&mut app);
    assert_eq!(now["options"], json!(["Talk him round", "Leave"]), "{now}");
    assert_eq!(now["checks"][0]["target"].as_f64(), Some(10.0), "hard: +2");
    assert_eq!(
        now["checks"][0]["bonuses"],
        json!([[2.0, "Knows his name"]])
    );
    // Rolled for real this time: the dice and the sum in the roll.
    call_api(&mut app.world, "story.choose", json!({"index": 1})).unwrap();
    app.update_n(2);
    let done = story_last(&mut app, id);
    let roll = &done["roll"];
    let dice: Vec<f64> = roll["dice"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_f64().unwrap())
        .collect();
    assert_eq!(dice.len(), 2, "{roll}");
    assert_eq!(roll["total"].as_f64(), Some(dice[0] + dice[1] + 4.0));
    let passed = roll["passed"].as_bool().unwrap();
    assert_eq!(
        done["text"],
        if passed { "Fine, go on." } else { "Nice try." },
        "{done}"
    );
}

/// Barks: a node group said over the speaker's head, the best fit for the facts, line after
/// line in real time; nothing when no node fits.
#[test]
fn barks_say_the_best_fit_over_the_speaker() {
    let folder = Folder::new();
    folder.write("dialogue/gate.yarn", GATE_YARN);
    let mut app = app_in(&folder.0);
    app.world.spawn((Name::new("Guard"), Transform::default()));
    app.update_n(2);
    let bark = |app: &mut App| {
        call_api(
            &mut app.world,
            "story.bark",
            json!({"path": "dialogue/gate", "node": "Chatter", "owner": "Guard"}),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    };
    let barks = |app: &mut App| {
        call_api(&mut app.world, "story.state", json!({})).unwrap()["barks"].clone()
    };
    assert_eq!(bark(&mut app)["id"], Value::Null, "no node fits yet");
    call_api(
        &mut app.world,
        "story.facts",
        json!({"set": {"mood": "happy"}}),
    )
    .unwrap();
    assert!(bark(&mut app)["id"].is_u64());
    app.update_n(1);
    let now = barks(&mut app);
    assert_eq!(now[0]["speaker"], "Guard", "{now}");
    assert_eq!(now[0]["text"], "Lovely day.");
    assert!(
        story_now(&mut app).is_null(),
        "a bark is not a conversation going on"
    );
    // 1.6 s up (1.1 s + 0.05 s a letter, at least 1.6), then the next line.
    app.update_n(52);
    let now = barks(&mut app);
    assert_eq!(now[0]["text"], "Isn't it?", "{now}");
    app.update_n(60);
    assert_eq!(barks(&mut app), json!([]));
}

const RELIC_QUEST: &str = r#"{
  "title": "The Relic",
  "start_when": "$met_priest",
  "facts": { "relics": 0 },
  "objectives": [
    { "text": "Find the relics", "progress": "$relics", "count": 3 },
    { "text": "Bring them back", "done_when": "$returned" },
    { "text": "Find the bonus", "done_when": "$bonus", "optional": true }
  ],
  "on_done": { "blessed": true }
}"#;

const ESCORT_QUEST: &str = r#"{
  "id": "escort",
  "title": "Escort",
  "fail_when": "$merchant_hp <= 0",
  "objectives": [{ "text": "Reach the gate", "done_when": "$at_gate" }],
  "on_fail": { "merchant_lost": true }
}"#;

fn quest(app: &mut App, id: &str) -> Value {
    let all = call_api(&mut app.world, "story.quests", json!({})).unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no quest {id} in {all}"))
}

/// Quests are data: one starts by itself once its condition holds, its objectives follow the
/// facts (a count, a condition; an optional one isn't needed), and finishing sets its facts.
/// Another fails by its condition.
#[test]
fn quests_follow_the_facts() {
    let folder = Folder::new();
    folder.write("quests/relic.quest", RELIC_QUEST);
    folder.write("quests/escort.quest", ESCORT_QUEST);
    let mut app = app_in(&folder.0);
    app.update_n(2);
    assert_eq!(quest(&mut app, "relic")["state"], "inactive");
    let set = |app: &mut App, facts: Value| {
        call_api(&mut app.world, "story.facts", json!({ "set": facts })).unwrap();
        app.update_n(1);
    };
    set(&mut app, json!({"met_priest": true}));
    let q = quest(&mut app, "relic");
    assert_eq!(q["state"], "active", "{q}");
    assert_eq!(
        eval(&mut app, "return facts.relics"),
        json!(0),
        "its starting facts"
    );
    set(&mut app, json!({"relics": 2}));
    let q = quest(&mut app, "relic");
    assert_eq!(q["objectives"][0]["progress"], 2);
    assert_eq!(q["objectives"][0]["done"], false);
    set(&mut app, json!({"relics": 3}));
    assert_eq!(quest(&mut app, "relic")["objectives"][0]["done"], true);
    // Done stays done.
    set(&mut app, json!({"relics": 1}));
    assert_eq!(quest(&mut app, "relic")["objectives"][0]["done"], true);
    set(&mut app, json!({"returned": true}));
    assert_eq!(quest(&mut app, "relic")["state"], "completed");
    assert_eq!(eval(&mut app, "return facts.blessed"), json!(true));
    assert_eq!(
        eval(&mut app, "return quests.state('relic')"),
        json!("completed")
    );

    // Started by hand (from a script here), failed by its condition.
    eval(&mut app, "quests.start('escort')");
    set(&mut app, json!({"merchant_hp": 5}));
    assert_eq!(quest(&mut app, "escort")["state"], "active");
    set(&mut app, json!({"merchant_hp": 0}));
    assert_eq!(quest(&mut app, "escort")["state"], "failed");
    assert_eq!(eval(&mut app, "return facts.merchant_lost"), json!(true));
    // A mistake in a condition is the quest's error, not a crash.
    folder.write(
        "quests/odd.quest",
        r#"{"title": "Odd", "start_when": "$a / 0 > 1", "objectives": []}"#,
    );
    app.update_n(20);
    let odd = quest(&mut app, "odd");
    assert_eq!(odd["state"], "inactive");
    assert!(
        odd["error"]
            .as_str()
            .unwrap_or("")
            .contains("divided by zero"),
        "{odd}"
    );
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
}

/// The shipped conversation and quest compile without problems.
#[test]
fn the_shipped_story_files_compile() {
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scripts");
    for f in crate::story::find_files(&shipped, crate::story::DIALOGUE_FOLDER, "yarn") {
        let text = std::fs::read_to_string(shipped.join(&f)).unwrap();
        let c = crate::story::compile_yarn(&text, &f);
        assert!(c.problems.is_empty(), "{f}: {:?}", c.problems);
    }
    let mut book = crate::story::QuestBook::default();
    book.reload(&shipped);
    assert!(book.problems.is_empty(), "{:?}", book.problems);
    assert!(book.def("sealed_bridge").is_some());
}

const YARD_YARN: &str = r#"title: Start
---
Guard: Where do you stand?
<<spots 2 0.5>>
-> Stay #spot:here #fallback
    Guard: As you were.
-> Go left #spot:left
    Guard: Left it is. <<if standing_at("left")>>
-> Go right #spot:right
    Guard: Right it is.
===

title: Lost
---
Guard: Nowhere to go.
-> Up there #spot:roof
    Guard: Up it is.
-> Stay
    Guard: Fine.
===
"#;

/// A yard with a walker standing in the spot "here", and spots "left" and "right" 3 m away.
fn yard() -> (Folder, App, Entity) {
    let folder = Folder::new();
    folder.write("dialogue/yard.yarn", YARD_YARN);
    let mut app = app_in(&folder.0);
    app.add_message::<engine_world::Interact>();
    let walker = app
        .world
        .spawn((Name::new("Walker"), Transform::from_xyz(0.0, 0.0, 0.0)))
        .id();
    for (name, x) in [("here", 0.0), ("left", -3.0), ("right", 3.0)] {
        app.world.spawn((
            Name::new(format!("Spot {name}")),
            Transform::from_xyz(x, 0.0, 0.0),
            crate::spots::StorySpot {
                name: name.into(),
                radius_m: 0.5,
            },
        ));
    }
    app.update_n(2);
    (folder, app, walker)
}

fn walk_in_yard(app: &mut App, node: &str) -> u64 {
    call_api(
        &mut app.world,
        "story.start",
        json!({"path": "dialogue/yard", "node": node, "headless": true, "chooser": "Walker"}),
    )
    .unwrap_or_else(|e| panic!("{e}"))["id"]
        .as_u64()
        .unwrap()
}

fn rings(app: &mut App) -> usize {
    let mut q = app.world.query::<&crate::spots::SpotRing>();
    q.iter(&app.world).count()
}

/// Options tagged `#spot:` are picked by walking: standing in a ring for a moment (not the one
/// you stood in when the choice opened), E on a ring, the fallback when time runs out, or
/// `story.choose`; the world slows meanwhile while the walker keeps its speed, and everything
/// the choice put up goes when it lands.
#[test]
fn choices_are_made_by_walking_to_spots() {
    let (_folder, mut app, walker) = yard();
    let id = walk_in_yard(&mut app, "Start");
    app.update_n(1);
    let now = story_last(&mut app, id);
    assert_eq!(now["waiting"], "spots", "{now}");
    let spots = call_api(&mut app.world, "story.state", json!({})).unwrap()["spots"].clone();
    assert_eq!(spots[0]["prompt"], "Where do you stand?", "{spots}");
    assert_eq!(spots[0]["options"][0]["spot"], "here");
    assert_eq!(spots[0]["options"][0]["inside"], true);
    assert_eq!(spots[0]["options"][0]["fallback"], true);
    assert!(
        rings(&mut app) >= 3 * 5,
        "a ring, its discs and light for each spot"
    );
    // Standing where it opened picks nothing; the world slows, the walker doesn't.
    app.update_n(12);
    assert_eq!(story_last(&mut app, id)["waiting"], "spots");
    assert!(app.world.resource::<TimeDilation>().factor() < 0.9);
    assert_eq!(
        app.world
            .get::<LocalTimeScale>(walker)
            .map(|l| l.ignore_world_dilation),
        Some(true)
    );
    // Over to the left: in its ring for 0.7 s.
    app.world.get_mut::<Transform>(walker).unwrap().translation = DVec3::new(-2.8, 0.0, 0.2);
    app.update_n(10);
    assert_eq!(
        story_last(&mut app, id)["waiting"],
        "spots",
        "still filling"
    );
    app.update_n(16);
    let done = story_last(&mut app, id);
    assert_eq!(done["done"], true, "{done}");
    assert_eq!(done["text"], "Left it is.", "standing_at sees it");
    // The rings, the view and the slow motion go; the walker's clock is its own again.
    app.update_n(30);
    assert_eq!(rings(&mut app), 0);
    assert!(app.world.get::<LocalTimeScale>(walker).is_none());
    let mut views = app.world.query::<&crate::spots::SpotView>();
    assert_eq!(views.iter(&app.world).count(), 0);
    app.update_n(15);
    assert!(app.world.resource::<TimeDilation>().factor() > 0.99);

    // Nothing chosen in two seconds: the fallback.
    app.world.get_mut::<Transform>(walker).unwrap().translation = DVec3::ZERO;
    let id = walk_in_yard(&mut app, "Start");
    app.update_n(70);
    let done = story_last(&mut app, id);
    assert_eq!(done["text"], "As you were.", "{done}");

    // E on the right ring.
    app.update_n(30);
    let id = walk_in_yard(&mut app, "Start");
    app.update_n(1);
    let ring = {
        let mut q = app
            .world
            .query::<(Entity, &crate::spots::SpotRing, &engine_world::Interactable)>();
        q.iter(&app.world)
            .find(|(_, r, _)| r.option == 2)
            .map(|(e, _, i)| (e, i.verb.clone()))
            .unwrap()
    };
    assert_eq!(ring.1, "Go right");
    app.world.write_message(engine_world::Interact {
        actor: walker,
        target: ring.0,
        from: DVec3::ZERO,
    });
    app.update_n(2);
    assert_eq!(story_last(&mut app, id)["text"], "Right it is.");

    // story.choose still picks.
    app.update_n(30);
    let id = walk_in_yard(&mut app, "Start");
    app.update_n(1);
    call_api(
        &mut app.world,
        "story.choose",
        json!({"index": 3, "id": id}),
    )
    .unwrap();
    app.update_n(2);
    assert_eq!(story_last(&mut app, id)["text"], "Right it is.");
    app.update_n(30);
    assert_eq!(rings(&mut app), 0);

    // A spot that isn't there: the options show as a menu instead.
    let id = walk_in_yard(&mut app, "Lost");
    app.update_n(1);
    assert_eq!(story_last(&mut app, id)["waiting"], "options");
    assert!(
        logs(&mut app)
            .iter()
            .any(|l| l.contains("no StorySpot named 'roof'")),
        "{:?}",
        logs(&mut app)
    );
}

/// What scripts printed (and warned), each entry as text.
fn logs(app: &mut App) -> Vec<String> {
    let log = call_api(&mut app.world, "script.log", json!({})).unwrap();
    log.as_array()
        .map(|a| a.iter().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}
