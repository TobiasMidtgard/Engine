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
