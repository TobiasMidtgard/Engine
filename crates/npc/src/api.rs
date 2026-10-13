//! `npc.*` API (JSON in/out), so AI agents and tools can spawn, tune, steer and look inside
//! NPCs and animals. Entities are referred to by `id` (`Entity::to_bits`) or by `name`.

use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_world::NoiseEvent;
use serde_json::{Map, Value, json};

use crate::agent::{Npc, NpcBrain, NpcCombat, NpcOrder, NpcVitals, NpcWeapon};
use crate::faction::{Faction, Squad};
use crate::perception::NpcMemory;
use crate::skills::{NpcSkills, Perception};
use crate::visuals::StandIn;
use crate::wildlife::{Animal, AnimalMind, Herd, Species};

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "npc.list",
        "NPCs and animals with id, kind, faction, position, what they're doing and health. \
         Optional: near [x,y,z] with radius (m), kind \"npc\" | \"animal\".",
        r#"{"near": [0, 0, 0], "radius": 50, "kind": "npc"}"#,
        list,
    )
    .register_api(
        "npc.spawn",
        "Spawn an AI fighter. skills: preset name (rookie, soldier, veteran, elite, master), a \
         level 0..1, or an object of skill fields; weapon: sword, sword_and_shield, rifle, bow, \
         claws; faction: player, enemy, rival or a number; squad: number; name. Returns {id}.",
        r#"{"position": [0, 0, -20], "skills": "veteran", "weapon": "rifle", "faction": "enemy", "squad": 1, "face": [0, 0, 0]}"#,
        spawn,
    )
    .register_api(
        "npc.spawn_animal",
        "Spawn an animal: species deer, rabbit, wolf, boar or bird; herd groups them; range (m) \
         they roam from home; male (a stag's antlers, a boar's tusks); young (a boar's striped \
         piglet); name. Returns {id}.",
        r#"{"species": "deer", "position": [10, 0, 5], "herd": 1, "male": false}"#,
        spawn_animal,
    )
    .register_api(
        "npc.set_skill",
        "Change an NPC's skills: a preset name, a level 0..1, or any of reaction_time (s), \
         accuracy, logic, aggression, courage, defense, awareness, teamwork, learning, agility. \
         Returns the new skills.",
        r#"{"id": 4294967301, "skills": {"reaction_time": 0.15, "logic": 1.0}}"#,
        set_skill,
    )
    .register_api(
        "npc.inspect",
        "Everything about one NPC or animal: skills, senses, weapon, health, current action and \
         why, utility scores, target, chosen spot and the best candidates, what it knows about \
         others, combat state and stats. Animals: state, danger, needs.",
        r#"{"id": 4294967301}"#,
        inspect,
    )
    .register_api(
        "npc.order",
        "Give NPCs a direct order until done: {\"hold\": [x,y,z]}, {\"move\": [x,y,z]}, \
         {\"attack\": id or name} or \"clear\". ids: one id or name, or a list.",
        r#"{"ids": [4294967301], "order": {"move": [5, 0, 10]}}"#,
        order,
    )
    .register_api(
        "npc.alert",
        "Make a noise NPCs and animals hear (they investigate, animals may flee). radius in m.",
        r#"{"position": [0, 1, 0], "radius": 40}"#,
        alert,
    )
    .register_api(
        "npc.despawn",
        "Remove an NPC or animal (and its stand-in body).",
        r#"{"id": 4294967301}"#,
        despawn,
    );
}

fn vec3(v: &Value, what: &str) -> Result<DVec3, String> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or_else(|| format!("{what} must be [x, y, z]"))?;
    let f = |i: usize| {
        a[i].as_f64()
            .ok_or_else(|| format!("{what} must be numbers"))
    };
    Ok(DVec3::new(f(0)?, f(1)?, f(2)?))
}

fn arr(p: DVec3) -> Value {
    json!([round(p.x), round(p.y), round(p.z)])
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn name_arg(world: &mut World, e: Entity, args: &Value) {
    if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
        world.entity_mut(e).insert(Name::new(name.to_string()));
    }
}

fn faction_arg(v: &Value) -> Result<Faction, String> {
    if let Some(n) = v.as_u64() {
        return Ok(Faction(n as u32));
    }
    Ok(match v.as_str().unwrap_or_default() {
        "player" => Faction::PLAYER,
        "enemy" => Faction::ENEMY,
        "rival" => Faction::RIVAL,
        "prey" => Faction::PREY,
        "predator" => Faction::PREDATOR,
        other => return Err(format!("unknown faction {other:?}")),
    })
}

/// Skills from a preset name, a level, or an object merged over `base`.
fn skills_arg(v: &Value, base: NpcSkills) -> Result<NpcSkills, String> {
    match v {
        Value::String(name) => {
            NpcSkills::preset(name).ok_or_else(|| format!("unknown preset {name:?}"))
        }
        Value::Number(n) => Ok(NpcSkills::at_level(n.as_f64().unwrap_or(0.5) as f32)),
        Value::Object(fields) => {
            let mut current = serde_json::to_value(base).map_err(|e| e.to_string())?;
            let obj = current.as_object_mut().ok_or("skills are not an object")?;
            for (k, val) in fields {
                if !obj.contains_key(k) {
                    return Err(format!("unknown skill {k:?}"));
                }
                obj.insert(k.clone(), val.clone());
            }
            let s: NpcSkills = serde_json::from_value(current).map_err(|e| e.to_string())?;
            Ok(s.clamped())
        }
        _ => Err("skills must be a preset name, a level or an object".into()),
    }
}

fn list(world: &mut World, args: Value) -> Result<Value, String> {
    let near = args.get("near").map(|v| vec3(v, "near")).transpose()?;
    let radius = args
        .get("radius")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::INFINITY);
    let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("all");
    let mut out = Vec::new();
    if kind != "animal" {
        let mut q = world.query::<(
            Entity,
            &Transform,
            &Faction,
            Option<&Squad>,
            &NpcBrain,
            &NpcVitals,
            &NpcWeapon,
            &NpcSkills,
        )>();
        for (e, t, f, squad, brain, vitals, weapon, skills) in q.iter(world) {
            if near.is_some_and(|n| n.distance(t.translation) > radius) {
                continue;
            }
            out.push(json!({
                "id": e.to_bits(),
                "kind": "npc",
                "weapon": weapon_name(weapon),
                "skill_level": round(skills.level() as f64),
                "faction": f.0,
                "squad": squad.map(|s| s.0),
                "position": arr(t.translation),
                "action": brain.action.name(),
                "alert": format!("{:?}", brain.alert).to_lowercase(),
                "role": brain.role,
                "target": brain.target.map(|t| t.to_bits()),
                "health": round(vitals.health as f64),
                "downed": vitals.downed,
            }));
        }
    }
    if kind != "npc" {
        let mut q = world.query::<(
            Entity,
            &Transform,
            &Animal,
            Option<&Herd>,
            &AnimalMind,
            &NpcVitals,
        )>();
        for (e, t, a, herd, mind, vitals) in q.iter(world) {
            if near.is_some_and(|n| n.distance(t.translation) > radius) {
                continue;
            }
            out.push(json!({
                "id": e.to_bits(),
                "kind": "animal",
                "species": a.species.name(),
                "herd": herd.map(|h| h.0),
                "position": arr(t.translation),
                "state": mind.state.name(),
                "health": round(vitals.health as f64),
            }));
        }
    }
    Ok(json!({ "count": out.len(), "entities": out }))
}

fn weapon_name(w: &NpcWeapon) -> &'static str {
    ["sword", "sword_and_shield", "rifle", "bow", "claws"]
        .into_iter()
        .find(|n| NpcWeapon::preset(n).as_ref() == Some(w))
        .unwrap_or("custom")
}

fn spawn(world: &mut World, args: Value) -> Result<Value, String> {
    let position = vec3(args.get("position").ok_or("missing position")?, "position")?;
    let skills = args
        .get("skills")
        .map(|v| skills_arg(v, NpcSkills::soldier()))
        .transpose()?
        .unwrap_or_else(NpcSkills::soldier);
    let weapon = match args.get("weapon").and_then(|v| v.as_str()) {
        Some(name) => NpcWeapon::preset(name).ok_or_else(|| format!("unknown weapon {name:?}"))?,
        None => NpcWeapon::rifle(),
    };
    let faction = args
        .get("faction")
        .map(faction_arg)
        .transpose()?
        .unwrap_or(Faction::ENEMY);
    let mut t = Transform::from_translation(position);
    if let Some(face) = args.get("face").map(|v| vec3(v, "face")).transpose()? {
        let d = (face - position).as_vec3();
        t.rotation = Quat::from_rotation_y((-d.x).atan2(-d.z));
    }
    let npc = Npc {
        home: Some(position),
        patrol_radius: args
            .get("patrol_radius")
            .and_then(|v| v.as_f64())
            .unwrap_or(8.0) as f32,
        ..Default::default()
    };
    let mut e = world.spawn((t, npc, skills, weapon, faction));
    if let Some(s) = args.get("squad").and_then(|v| v.as_u64()) {
        e.insert(Squad(s as u32));
    }
    if args
        .get("stand_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
    {
        e.insert(StandIn::default());
    }
    let id = e.id();
    name_arg(world, id, &args);
    Ok(json!({ "id": id.to_bits() }))
}

fn spawn_animal(world: &mut World, args: Value) -> Result<Value, String> {
    let position = vec3(args.get("position").ok_or("missing position")?, "position")?;
    let name = args
        .get("species")
        .and_then(|v| v.as_str())
        .ok_or("missing species")?;
    let species = Species::parse(name).ok_or_else(|| format!("unknown species {name:?}"))?;
    let mut animal = Animal::new(species);
    animal.male = args.get("male").and_then(|v| v.as_bool()).unwrap_or(false);
    animal.young = args.get("young").and_then(|v| v.as_bool()).unwrap_or(false);
    if let Some(r) = args.get("range").and_then(|v| v.as_f64()) {
        animal.range = r as f32;
    }
    let mut e = world.spawn((
        Transform::from_translation(position),
        animal,
        StandIn::default(),
    ));
    if let Some(h) = args.get("herd").and_then(|v| v.as_u64()) {
        e.insert(Herd(h as u32));
    }
    let id = e.id();
    name_arg(world, id, &args);
    Ok(json!({ "id": id.to_bits() }))
}

fn set_skill(world: &mut World, args: Value) -> Result<Value, String> {
    let e = resolve_entity(world, &args)?;
    let base = *world.get::<NpcSkills>(e).ok_or("not an NPC")?;
    let skills = skills_arg(args.get("skills").ok_or("missing skills")?, base)?;
    *world.get_mut::<NpcSkills>(e).unwrap() = skills;
    serde_json::to_value(skills).map_err(|e| e.to_string())
}

fn inspect(world: &mut World, args: Value) -> Result<Value, String> {
    let e = resolve_entity(world, &args)?;
    let pos = world
        .get::<Transform>(e)
        .map(|t| t.translation)
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert("id".into(), json!(e.to_bits()));
    out.insert("position".into(), arr(pos));
    if let Some(v) = world.get::<NpcVitals>(e) {
        out.insert("health".into(), json!(round(v.health as f64)));
        out.insert("max_health".into(), json!(v.max_health));
        out.insert("poise".into(), json!(round(v.poise as f64)));
        out.insert("downed".into(), json!(v.downed));
    }
    if let Some(f) = world.get::<Faction>(e) {
        out.insert("faction".into(), json!(f.0));
    }
    if let (Some(a), Some(m)) = (world.get::<Animal>(e), world.get::<AnimalMind>(e)) {
        out.insert("kind".into(), json!("animal"));
        out.insert("species".into(), json!(a.species.name()));
        out.insert("state".into(), json!(m.state.name()));
        out.insert("danger".into(), json!(round(m.danger as f64)));
        out.insert("threat".into(), json!(m.threat_pos.map(arr)));
        out.insert("prey".into(), json!(m.prey.map(|p| p.to_bits())));
        out.insert("destination".into(), json!(m.destination.map(arr)));
        out.insert(
            "needs".into(),
            json!({"hunger": round(m.hunger as f64), "thirst": round(m.thirst as f64), "fatigue": round(m.fatigue as f64), "stamina": round(m.stamina as f64)}),
        );
        out.insert("water".into(), json!(m.water.flatten().map(arr)));
        return Ok(Value::Object(out));
    }
    let Some(brain) = world.get::<NpcBrain>(e) else {
        return Err("not an NPC or animal".into());
    };
    out.insert("kind".into(), json!("npc"));
    out.insert("action".into(), json!(brain.action.name()));
    out.insert(
        "alert".into(),
        json!(format!("{:?}", brain.alert).to_lowercase()),
    );
    out.insert("role".into(), json!(brain.role));
    out.insert("target".into(), json!(brain.target.map(|t| t.to_bits())));
    out.insert(
        "destination".into(),
        json!(
            brain
                .destination
                .map(|d| json!({"position": arr(d), "reason": brain.destination_reason}))
        ),
    );
    out.insert("in_cover".into(), json!(brain.in_cover));
    out.insert(
        "revive".into(),
        json!(brain.revive.map(|a| json!({
            "ally": a.to_bits(),
            "kneeling_secs": round(brain.revive_progress as f64),
        }))),
    );
    let mut scores: Vec<_> = brain.scores.clone();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.insert(
        "scores".into(),
        Value::Object(
            scores
                .iter()
                .map(|(a, s)| (a.name().to_string(), json!(round(*s as f64))))
                .collect(),
        ),
    );
    out.insert(
        "best_spots".into(),
        json!(brain.spots.iter().take(5).map(|s| json!({"position": arr(s.position), "score": round(s.score as f64), "reason": s.reason, "cover": s.covered})).collect::<Vec<_>>()),
    );
    out.insert("order".into(), json!(brain.order.map(|o| format!("{o:?}"))));
    if let Some(s) = world.get::<NpcSkills>(e) {
        out.insert("skills".into(), serde_json::to_value(s).unwrap_or_default());
    }
    if let Some(p) = world.get::<Perception>(e) {
        out.insert(
            "perception".into(),
            serde_json::to_value(p).unwrap_or_default(),
        );
    }
    if let Some(w) = world.get::<NpcWeapon>(e) {
        out.insert("weapon".into(), serde_json::to_value(w).unwrap_or_default());
    }
    if let Some(c) = world.get::<NpcCombat>(e) {
        out.insert(
            "combat".into(),
            json!({
                "phase": format!("{:?}", c.phase),
                "defense": format!("{:?}", c.defense),
                "suppression": round(c.suppression as f64),
                "stats": serde_json::to_value(c.stats).unwrap_or_default(),
            }),
        );
    }
    if let Some(m) = world.get::<NpcMemory>(e) {
        let known: Vec<Value> = m
            .known
            .iter()
            .map(|k| {
                json!({
                    "id": k.entity.to_bits(),
                    "faction": k.faction.0,
                    "awareness": round(k.awareness as f64),
                    "visible": k.visible,
                    "last_position": arr(k.last_position),
                    "last_seen": round(k.last_seen),
                })
            })
            .collect();
        out.insert("knows".into(), json!(known));
    }
    Ok(Value::Object(out))
}

fn order(world: &mut World, args: Value) -> Result<Value, String> {
    let ids: Vec<Entity> = match args.get("ids").or(args.get("id")).or(args.get("name")) {
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| resolve_entity(world, v))
            .collect::<Result<_, _>>()?,
        Some(v) => vec![resolve_entity(world, v)?],
        None => return Err("missing ids".into()),
    };
    let o = args.get("order").ok_or("missing order")?;
    let order = if o.as_str() == Some("clear") {
        None
    } else if let Some(p) = o.get("hold") {
        Some(NpcOrder::Hold {
            at: vec3(p, "hold")?,
        })
    } else if let Some(p) = o.get("move") {
        Some(NpcOrder::MoveTo {
            at: vec3(p, "move")?,
        })
    } else if let Some(t) = o.get("attack") {
        Some(NpcOrder::Attack {
            target: resolve_entity(world, t)?.to_bits(),
        })
    } else {
        return Err(
            "order must be {\"hold\": [x,y,z]}, {\"move\": [x,y,z]}, {\"attack\": id} or \"clear\""
                .into(),
        );
    };
    let mut given = 0;
    for e in &ids {
        if let Some(mut b) = world.get_mut::<NpcBrain>(*e) {
            b.order = order;
            b.next_think = 0.0;
            given += 1;
        }
    }
    Ok(json!({ "ordered": given }))
}

fn alert(world: &mut World, args: Value) -> Result<Value, String> {
    let position = vec3(args.get("position").ok_or("missing position")?, "position")?;
    let radius = args.get("radius").and_then(|v| v.as_f64()).unwrap_or(40.0) as f32;
    world.write_message(NoiseEvent {
        position,
        radius,
        source: None,
    });
    Ok(json!({ "ok": true }))
}

fn despawn(world: &mut World, args: Value) -> Result<Value, String> {
    let e = resolve_entity(world, &args)?;
    let is_ours = world.get::<Npc>(e).is_some() || world.get::<Animal>(e).is_some();
    if !is_ours {
        return Err("not an NPC or animal".into());
    }
    world.despawn(e);
    Ok(json!({ "despawned": e.to_bits() }))
}
