//! The runtime end to end: scripts in a temporary folder, run by the plugin in a headless app.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use serde_json::{Value, json};

use crate::{Script, ScriptPlugin, ScriptSettings};

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
    assert!(values[&31].starts_with("glow = "), "{values:?}");
    call_api(&mut app.world, "script.logpoint", json!({"clear": true})).unwrap();

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
    assert!(
        eval(&mut app, &format!("{director}.flare"))
            .as_f64()
            .unwrap()
            > 1.2
    );
    app.update_n(30 * 8);
    assert!(errors(&mut app).is_empty(), "{:?}", errors(&mut app));
    assert_eq!(eval(&mut app, &format!("{director}.busy")), json!(false));
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
    let mut frames = 0;
    while !client.is_finished() && frames < 600 {
        app.update();
        frames += 1;
    }
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
    let mut frames = 0;
    while !client.is_finished() && frames < 600 {
        app.update();
        frames += 1;
    }
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
