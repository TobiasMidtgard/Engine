//! The `npc.*` JSON API: an agent can spawn fighters and animals, tune skills, give orders,
//! make noise and look inside a brain, all without touching Rust.

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::WorldPlugin;
use serde_json::{Value, json};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    // Running each system in turn on the test's own thread is far quicker than handing them
    // about a thread pool, and changes nothing (the order is fixed: `tests/schedule_order.rs`).
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn call(app: &mut App, name: &str, args: Value) -> Value {
    call_api(&mut app.world, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn id(v: &Value) -> Entity {
    Entity::try_from_bits(v["id"].as_u64().unwrap()).unwrap()
}

#[test]
fn spawn_tune_and_inspect() {
    let mut app = app();
    let red = call(
        &mut app,
        "npc.spawn",
        json!({"position": [0, 0, 0], "skills": "rookie", "weapon": "rifle", "faction": "enemy",
               "squad": 1, "face": [0, 0, -10], "name": "Red"}),
    );
    let blue = call(
        &mut app,
        "npc.spawn",
        json!({"position": [0, 0, -14], "skills": 0.9, "weapon": "sword", "faction": "rival"}),
    );
    let red = id(&red);
    let blue = id(&blue);
    assert_eq!(app.world.get::<Faction>(red), Some(&Faction::ENEMY));
    assert_eq!(app.world.get::<Squad>(red).map(|s| s.0), Some(1));
    assert_eq!(app.world.get::<Name>(red).map(|n| n.as_str()), Some("Red"));
    assert_eq!(
        app.world.get::<NpcWeapon>(blue),
        NpcWeapon::preset("sword").as_ref()
    );

    // Merge single skills over the current ones; everything else stays.
    let before = *app.world.get::<NpcSkills>(red).unwrap();
    let skills = call(
        &mut app,
        "npc.set_skill",
        json!({"name": "Red", "skills": {"reaction_time": 0.12, "logic": 1.0}}),
    );
    assert!((skills["reaction_time"].as_f64().unwrap() - 0.12).abs() < 1e-6);
    let after = *app.world.get::<NpcSkills>(red).unwrap();
    assert_eq!(after.logic, 1.0);
    assert_eq!(after.accuracy, before.accuracy);
    // Out-of-range values are clamped, unknown skills refused.
    call(
        &mut app,
        "npc.set_skill",
        json!({"id": red.to_bits(), "skills": {"accuracy": 7.0}}),
    );
    assert!(app.world.get::<NpcSkills>(red).unwrap().accuracy <= 1.0);
    assert!(
        call_api(
            &mut app.world,
            "npc.set_skill",
            json!({"id": red.to_bits(), "skills": {"luck": 1.0}})
        )
        .is_err()
    );

    // Let them see each other and fight a little.
    app.update_n(60 * 4);
    let info = call(&mut app, "npc.inspect", json!({"id": red.to_bits()}));
    assert_eq!(info["kind"], "npc");
    assert_eq!(info["target"].as_u64(), Some(blue.to_bits()), "{info:#}");
    assert!(
        info["knows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["id"].as_u64() == Some(blue.to_bits()))
    );
    assert!(info["scores"].as_object().is_some_and(|s| !s.is_empty()));
    assert!(info["skills"]["logic"].as_f64() == Some(1.0));

    let list = call(
        &mut app,
        "npc.list",
        json!({"near": [0, 0, 0], "radius": 30, "kind": "npc"}),
    );
    assert_eq!(list["count"], 2, "{list:#}");
}

#[test]
fn orders_move_and_hold() {
    let mut app = app();
    let a = call(
        &mut app,
        "npc.spawn",
        json!({"position": [0, 0, 0], "faction": 7}),
    );
    let a = id(&a);
    let r = call(
        &mut app,
        "npc.order",
        json!({"ids": [a.to_bits()], "order": {"move": [12, 0, 0]}}),
    );
    assert_eq!(r["ordered"], 1);
    app.update_n(60 * 6);
    let p = app.world.get::<Transform>(a).unwrap().translation;
    assert!(p.distance(DVec3::new(12.0, 0.0, 0.0)) < 2.0, "went to {p}");
    call(
        &mut app,
        "npc.order",
        json!({"id": a.to_bits(), "order": "clear"}),
    );
    assert!(app.world.get::<NpcBrain>(a).unwrap().order.is_none());
}

#[test]
fn animals_hear_an_alert() {
    let mut app = app();
    let mut deer = Vec::new();
    for k in 0..3 {
        let r = call(
            &mut app,
            "npc.spawn_animal",
            json!({"species": "deer", "position": [k as f64 * 2.0, 0, 0], "herd": 4}),
        );
        deer.push(id(&r));
    }
    app.update_n(30);
    call(
        &mut app,
        "npc.alert",
        json!({"position": [4, 1, 4], "radius": 40}),
    );
    let mut fled = false;
    for _ in 0..120 {
        app.update();
        fled |= deer.iter().any(|&d| {
            matches!(
                app.world.get::<AnimalMind>(d).unwrap().state,
                AnimalState::Alert | AnimalState::Flee
            )
        });
    }
    assert!(fled, "no deer reacted to the noise");
    let info = call(&mut app, "npc.inspect", json!({"id": deer[0].to_bits()}));
    assert_eq!(info["kind"], "animal");
    assert_eq!(info["species"], "deer");
    let list = call(&mut app, "npc.list", json!({"kind": "animal"}));
    assert_eq!(list["count"], 3);
    call(&mut app, "npc.despawn", json!({"id": deer[2].to_bits()}));
    app.update();
    let list = call(&mut app, "npc.list", json!({"kind": "animal"}));
    assert_eq!(list["count"], 2);
}

#[test]
fn bad_arguments_are_explained() {
    let mut app = app();
    for (name, args) in [
        ("npc.spawn", json!({})),
        (
            "npc.spawn",
            json!({"position": [0, 0, 0], "weapon": "banana"}),
        ),
        (
            "npc.spawn",
            json!({"position": [0, 0, 0], "skills": "godlike"}),
        ),
        (
            "npc.spawn_animal",
            json!({"species": "dragon", "position": [0, 0, 0]}),
        ),
        ("npc.inspect", json!({"id": 999999})),
        ("npc.order", json!({"ids": [], "order": {"dance": true}})),
    ] {
        let err =
            call_api(&mut app.world, name, args.clone()).expect_err(&format!("{name} {args}"));
        assert!(!err.is_empty());
    }
}
