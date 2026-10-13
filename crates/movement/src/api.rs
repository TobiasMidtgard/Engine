//! `movement.*` API functions: JSON in, JSON out, for the console, scripts and AI agents.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_world::AttackKind;
use serde_json::{Value, json};

use crate::combat::{CombatLog, CombatState, Fighter};
use crate::demo::{DuelDirector, Round};
use crate::lockon::{self, LockOn};
use crate::mover::{DashRequest, Mover};
use crate::play::{self, Play};
use crate::profile::MovementProfile;
use crate::tempo::{PerceptionStatus, Tempo, TimePerception};

fn entity_arg(world: &mut World, args: &Value) -> Result<Entity, String> {
    let sel = args
        .get("entity")
        .ok_or("missing \"entity\" (an id or a name)")?;
    resolve_entity(world, sel)
}

fn vec3_arg(args: &Value, key: &str) -> Result<DVec3, String> {
    let a = args
        .get(key)
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 3)
        .ok_or_else(|| format!("\"{key}\" must be [x, y, z] in meters"))?;
    let f = |i: usize| a[i].as_f64().ok_or(format!("\"{key}\" must hold numbers"));
    Ok(DVec3::new(f(0)?, f(1)?, f(2)?))
}

fn mover_json(world: &World, e: Entity) -> Value {
    let m = world.get::<Mover>(e);
    let t = world.get::<Transform>(e);
    let cs = world.get::<CombatState>(e);
    let f = world.get::<Fighter>(e);
    json!({
        "id": e.to_bits(),
        "position": t.map(|t| t.translation.to_array()),
        "mode": m.map(|m| m.mode),
        "velocity": m.map(|m| m.velocity.to_array()),
        "grounded": m.map(|m| m.grounded),
        "dash_charges": m.map(|m| m.dash_charges),
        "tempo": world.get::<Tempo>(e),
        "health": f.map(|f| f.health),
        "attack": cs.and_then(|c| c.attack).map(|a| json!({"kind": a.kind, "phase": a.phase})),
        "defeated": cs.map(|c| c.defeated),
    })
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "movement.status",
        "Every moving character (mode, velocity, dash charges, tempo, health, attack) and the \
         reflex slow-motion state. Pass \"entity\" for one character.",
        r#"{"entity": "Player"}"#,
        |world, args| {
            let perception = world.resource::<PerceptionStatus>().clone();
            if args.get("entity").is_some() {
                let e = entity_arg(world, &args)?;
                return Ok(json!({"character": mover_json(world, e), "perception": perception}));
            }
            let mut q = world.query_filtered::<Entity, With<Mover>>();
            let ids: Vec<Entity> = q.iter(world).collect();
            Ok(json!({
                "characters": ids.iter().map(|e| mover_json(world, *e)).collect::<Vec<_>>(),
                "perception": perception,
                "time_scale": world.resource::<Time>().scale(),
            }))
        },
    )
    .register_api(
        "movement.set_tempo",
        "Set how fast a character acts (speed) and perceives (reflexes), or both from a level \
         (1 = untrained; level 10 = speed 1.45, reflexes 5.5).",
        r#"{"entity": "Player", "level": 10}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let mut tempo = match args.get("level").and_then(|v| v.as_u64()) {
                Some(level) => Tempo::level(level as u32),
                None => world.get::<Tempo>(e).copied().unwrap_or_default(),
            };
            if let Some(s) = args.get("speed").and_then(|v| v.as_f64()) {
                tempo.speed = s as f32;
            }
            if let Some(r) = args.get("reflexes").and_then(|v| v.as_f64()) {
                tempo.reflexes = r as f32;
            }
            world.entity_mut(e).insert(tempo);
            Ok(json!({"tempo": tempo}))
        },
    )
    .register_api(
        "movement.preset",
        "Give a character a movement preset: pilot (wall-running, double jump), human \
         (grounded), elite (long dashes).",
        r#"{"entity": "Player", "preset": "pilot"}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let name = args.get("preset").and_then(|v| v.as_str()).unwrap_or("");
            let p = MovementProfile::preset(name)
                .ok_or_else(|| format!("Unknown preset \"{name}\". Use pilot, human or elite."))?;
            world.entity_mut(e).insert(p);
            Ok(json!({"profile": p}))
        },
    )
    .register_api(
        "movement.dash",
        "Dash a character toward a point (up to its dash distance).",
        r#"{"entity": "Player", "to": [10.0, 0.0, 0.0]}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let to = vec3_arg(&args, "to")?;
            let mut m = world
                .get_mut::<Mover>(e)
                .ok_or("that entity has no MovementProfile")?;
            m.pending_dash = Some(DashRequest {
                to,
                strike: false,
                max_distance: None,
            });
            Ok(json!({"ok": true}))
        },
    )
    .register_api(
        "movement.attack",
        "Start an attack: kind \"melee\" or \"dash\" (dash-strike toward the nearest enemy in \
         front).",
        r#"{"entity": "Player", "kind": "dash"}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let kind = match args.get("kind").and_then(|v| v.as_str()).unwrap_or("melee") {
                "melee" => AttackKind::Melee,
                "dash" => AttackKind::Dash,
                other => return Err(format!("Unknown kind \"{other}\". Use melee or dash.")),
            };
            let mut cs = world
                .get_mut::<CombatState>(e)
                .ok_or("that entity has no Fighter")?;
            cs.request_attack(kind);
            Ok(json!({"ok": true}))
        },
    )
    .register_api(
        "movement.revive",
        "Bring a defeated fighter back with full health.",
        r#"{"entity": "Elite"}"#,
        |world, args| {
            let e = entity_arg(world, &args)?;
            let mut em = world.entity_mut(e);
            let (Some(mut f), Some(mut cs), Some(mut m)) = (
                em.get::<Fighter>().copied(),
                em.get::<CombatState>().cloned(),
                em.get::<Mover>().cloned(),
            ) else {
                return Err("that entity is not a Fighter with a MovementProfile".into());
            };
            cs.revive(&mut f, &mut m);
            em.insert((f, cs, m));
            if let Some(mut r) = em.get_mut::<engine_world::animation::Ragdoll>() {
                r.weight = 0.0;
            }
            Ok(json!({"ok": true}))
        },
    )
    .register_api(
        "movement.combat_log",
        "The most recent strikes: who hit whom, when (world and real seconds), outcome (Hit, \
         Counter, Blocked, Dodged, Whiff) and damage.",
        r#"{"limit": 10}"#,
        |world, args| {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let log = world.resource::<CombatLog>();
            let n = log.records.len();
            let recent: Vec<_> = log.records.iter().skip(n.saturating_sub(limit)).collect();
            Ok(json!({"records": recent}))
        },
    )
    .register_api(
        "movement.perception",
        "Reflex slow-motion settings and state. Pass any TimePerception fields to change them \
         (enabled, min_scale, engage_below, blend_in_secs, linger_secs, threat_radius, cinematic).",
        r#"{"min_scale": 0.15}"#,
        |world, args| {
            if args.as_object().is_some_and(|o| !o.is_empty()) {
                let mut settings = serde_json::to_value(world.resource::<TimePerception>().clone())
                    .map_err(|e| e.to_string())?;
                merge(&mut settings, &args);
                let parsed: TimePerception = serde_json::from_value(settings)
                    .map_err(|e| format!("Invalid perception settings: {e}"))?;
                world.insert_resource(parsed);
            }
            Ok(json!({
                "settings": world.resource::<TimePerception>(),
                "status": world.resource::<PerceptionStatus>(),
            }))
        },
    )
    .register_api(
        "movement.duel",
        "Restart the movement demo's duel: \"round\" Blink (an elite cuts down an untrained \
         player), Focus (a trained player sees the attack in slow motion and counters) or Pack \
         (four grunts fall in slow motion) plays that round on a loop; null cycles all three.",
        r#"{"round": "Focus"}"#,
        |world, args| {
            let round: Option<Round> = match args.get("round") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    serde_json::from_value(v.clone())
                        .map_err(|_| "\"round\" must be Blink, Focus, Pack or null".to_string())?,
                ),
            };
            let mut q = world.query::<&mut DuelDirector>();
            let mut out = Vec::new();
            for mut d in q.iter_mut(world) {
                d.restart(round);
                out.push(json!({"round": d.round, "only": d.only}));
            }
            if out.is_empty() {
                return Err("no duel in this world (spawn it with demo::spawn_demo)".into());
            }
            Ok(json!({ "duels": out }))
        },
    );
    app.register_api(
        "movement.climber",
        "Restart the movement demo's ledge climber from the start of its loop: it jumps at the \
         ledge block, catches the top, shimmies along it and around the corner at its end, \
         climbs up and walks off.",
        "{}",
        |world, _args| {
            let n = crate::demo::restart_climbers(world);
            if n == 0 {
                return Err("no climber in this world (spawn it with demo::spawn_demo)".into());
            }
            Ok(json!({ "climbers": n }))
        },
    );
    app.register_api(
        "movement.grappler",
        "Restart the movement demo's grappler from the start of its loop: it sprints at the \
         grapple tower, jumps, hooks its top edge, is reeled up the wall, climbs over, then runs \
         and jumps into the zip line on the roof and rides it down to the landing gate.",
        "{}",
        |world, _args| {
            let n = crate::demo::restart_grapplers(world);
            if n == 0 {
                return Err("no grappler in this world (spawn it with demo::spawn_demo)".into());
            }
            Ok(json!({ "grapplers": n }))
        },
    );
    app.register_api(
        "movement.rider",
        "Restart the movement demo's freight rider from the start of its loop, with the freight \
         car about to pull out: the rider sprints along the platform, leaps onto the car's side, \
         wall-runs along it, climbs onto its roof and rides it to the end of the track.",
        "{}",
        |world, _args| {
            let n = crate::demo::restart_riders(world);
            if n == 0 {
                return Err("no rider in this world (spawn it with demo::spawn_demo)".into());
            }
            Ok(json!({ "riders": n }))
        },
    );
    app.register_api(
        "movement.play",
        "Play mode: \"on\" true puts a level 10 pilot on the ground in front of the active \
         camera (or with its feet at \"at\") and a third-person camera behind it; false ends \
         it. Leave \"on\" out to switch. Controls: the default action map, lock_on, Escape \
         frees the mouse.",
        r#"{"on": true, "at": [0.0, 0.0, -10.0]}"#,
        |world, args| {
            let on = match args.get("on") {
                None | Some(Value::Null) => world.resource::<Play>().pilot.is_none(),
                Some(v) => v.as_bool().ok_or("\"on\" must be true or false")?,
            };
            if !on {
                let was_on = play::stop(world);
                return Ok(json!({ "playing": false, "was_playing": was_on }));
            }
            let at = match args.get("at") {
                None | Some(Value::Null) => None,
                Some(_) => Some(vec3_arg(&args, "at")?),
            };
            let pilot = play::start(world, at)?;
            Ok(json!({ "playing": true, "pilot": mover_json(world, pilot) }))
        },
    );
    app.register_api(
        "movement.lock_on",
        "Lock a character with LockOn (default: the play-mode pilot) onto \"target\" (an id or \
         a name), onto the best enemy in view with \"target\": \"auto\", or let go with null.",
        r#"{"target": "Elite"}"#,
        |world, args| {
            let me = match args.get("entity") {
                Some(_) => entity_arg(world, &args)?,
                None => world
                    .resource::<Play>()
                    .pilot
                    .ok_or("no pilot: pass \"entity\" or start play mode")?,
            };
            if world.get::<LockOn>(me).is_none() {
                return Err("that character has no LockOn".into());
            }
            let target = match args.get("target") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s == "auto" => lockon::best_target(world, me),
                Some(sel) => Some(resolve_entity(world, sel)?),
            };
            world.get_mut::<LockOn>(me).unwrap().target = target;
            Ok(json!({ "entity": me.to_bits(), "target": target.map(Entity::to_bits) }))
        },
    );
}

fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p.clone(),
    }
}
