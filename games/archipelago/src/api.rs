//! `isle.*`: the game for scripts, tests and AI agents (the same calls the HUD makes).

use engine::prelude::*;
use serde_json::{Value, json};

use crate::build;
use crate::camera::StrategyCam;
use crate::defs::{DEFS, Good, Kind};
use crate::fleet::{Roam, Route, Routes, Stop, Vessel, mast_point, mooring, spawn_vessel};
use crate::grid::{self, Grid};
use crate::island::islands;
use crate::orders;
use crate::sim::{Building, Economy, House, Producer, population};
use crate::vessels::Craft;

fn cell(args: &Value, key: &str) -> Result<IVec2, String> {
    let v = &args[key];
    let x = v[0]
        .as_i64()
        .ok_or(format!("{key} wants [x, z] in cells"))?;
    let z = v[1]
        .as_i64()
        .ok_or(format!("{key} wants [x, z] in cells"))?;
    Ok(IVec2::new(x as i32, z as i32))
}

fn index(args: &Value, key: &str) -> Result<usize, String> {
    args[key]
        .as_u64()
        .map(|i| i as usize)
        .ok_or(format!("{key} wants an index"))
}

fn goods(v: &Value) -> Result<Vec<Good>, String> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            g.as_str()
                .and_then(Good::from_name)
                .ok_or(format!("unknown good {g}"))
        })
        .collect()
}

fn vessel(world: &mut World, args: &Value) -> Result<Entity, String> {
    let key = match &args["vessel"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("vessel wants a name or an id".into()),
    };
    orders::find_vessel(world, &key).ok_or(format!("no vessel {key}"))
}

pub fn register(app: &mut App) {
    app.register_api(
        "isle.state",
        "Coins, balance, and per island: people, warehouse stock, made and used per minute.",
        "{}",
        |world, _| {
            let econ = world.resource::<Economy>().clone();
            let isles: Vec<Value> = econ
                .islands
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let stock: serde_json::Map<String, Value> =
                        Good::ALL.iter().map(|g| (g.name().to_string(), json!((b.get(*g) * 10.0).round() / 10.0))).collect();
                    json!({
                        "name": islands()[i].name,
                        "people": population(b),
                        "fisherfolk": b.population[0],
                        "labourers": b.population[1],
                        "harbours": b.harbours,
                        "stock": stock,
                        "workforce": b.workforce,
                    })
                })
                .collect();
            let mut houses = [0usize; 2];
            let mut q = world.query::<&House>();
            for h in q.iter(world) {
                houses[h.tier.index()] += 1;
            }
            let mut q = world.query::<&Producer>();
            let working = q.iter(world).filter(|p| p.efficiency > 0.5).count();
            let carts = world.query::<&crate::logistics::Cart>().iter(world).count();
            let vessels = world.query::<&Vessel>().iter(world).count();
            let rows = world.resource::<crate::draw::Drawn>().rows;
            let walkers = crate::walkers::count(world);
            let art_triangles: usize = world.get_resource::<crate::draw::Art>().map_or(0, |a| a.triangles.values().sum());
            Ok(json!({
                "coins": econ.coins.round(),
                "balance_per_min": econ.balance_per_min,
                "clock_s": econ.clock_s,
                "speed": econ.speed,
                "houses": {"fisherfolk": houses[0], "labourers": houses[1]},
                "workshops_working": working,
                "carts": carts,
                "walkers": walkers,
                "vessels": vessels,
                "instanced_rows": rows,
                "art_triangles": art_triangles,
                "islands": isles,
            }))
        },
    )
    .register_api(
        "isle.buildings",
        "Building kinds: size in cells, costs, workers, input and output.",
        "{}",
        |_, _| {
            Ok(Value::Array(
                DEFS.iter()
                    .map(|d| {
                        json!({
                            "kind": d.kind, "name": d.name, "size": [d.size.0, d.size.1], "coins": d.coins,
                            "cost": d.cost.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>(),
                            "input": d.input.map(Good::name), "output": d.output.map(Good::name), "cycle_s": d.cycle_s,
                        })
                    })
                    .collect(),
            ))
        },
    )
    .register_api(
        "isle.build",
        "Build a kind at a cell (its footprint's low corner), turned 0-3 quarter turns; free: true skips the cost.",
        r#"{"kind": "fishery", "at": [40, -12], "rot": 0, "free": false}"#,
        |world, args| {
            let kind = args["kind"].as_str().and_then(Kind::from_name).ok_or("unknown kind (see isle.buildings)")?;
            let at = cell(&args, "at")?;
            let rot = args["rot"].as_u64().unwrap_or(0) as u8;
            let free = args["free"].as_bool().unwrap_or(false);
            let e = build::place(world, kind, at, rot, !free)?;
            let id = world.get::<Building>(e).map(|b| b.id);
            Ok(json!({"id": id}))
        },
    )
    .register_api(
        "isle.road",
        "Lay road from one cell to another (x first, then z).",
        r#"{"from": [10, 4], "to": [18, 9]}"#,
        |world, args| {
            let n = build::place_road_line(world, cell(&args, "from")?, cell(&args, "to")?, !args["free"].as_bool().unwrap_or(false))?;
            Ok(json!({"cells": n}))
        },
    )
    .register_api("isle.demolish", "Remove a building by id, or a road at a cell.", r#"{"id": 12}"#, |world, args| {
        if let Some(id) = args["id"].as_u64() {
            let e = build::by_id(world, id as u32).ok_or("no building with that id")?;
            build::demolish(world, e);
        } else {
            build::remove_road(world, cell(&args, "at")?);
        }
        Ok(json!({}))
    })
    .register_api(
        "isle.list",
        "Every building: id, kind, island, cell, status and output.",
        r#"{"island": 0}"#,
        |world, args| {
            let only = args["island"].as_u64().map(|i| i as usize);
            let mut q = world.query::<(&Building, Option<&Producer>, Option<&House>)>();
            let v: Vec<Value> = q
                .iter(world)
                .filter(|(b, _, _)| only.is_none_or(|i| i == b.island))
                .map(|(b, p, h)| {
                    json!({
                        "id": b.id, "kind": b.kind, "island": b.island, "at": [b.origin.x, b.origin.y], "rot": b.rot,
                        "status": p.map(|p| p.status.text()), "efficiency": p.map(|p| p.efficiency),
                        "tier": h.map(|h| h.tier.name()), "residents": h.map(|h| h.residents),
                    })
                })
                .collect();
            Ok(Value::Array(v))
        },
    )
    .register_api("isle.fleet", "Routes and every ship and aircraft: where, doing what, carrying what.", "{}", |world, _| {
        let routes = serde_json::to_value(&world.resource::<Routes>().0).map_err(|e| e.to_string())?;
        let mut q = world.query::<(Entity, &Vessel, &Transform)>();
        let vessels: Vec<Value> = q
            .iter(world)
            .map(|(e, v, t)| {
                json!({
                    "id": e.to_bits(), "name": v.name, "craft": v.craft.name(), "route": v.route, "doing": v.doing,
                    "at": [t.translation.x.round(), t.translation.y.round(), t.translation.z.round()],
                    "cargo": v.cargo.iter().map(|(g, n)| json!([g.name(), n])).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({"routes": routes, "vessels": vessels}))
    })
    .register_api(
        "isle.route",
        "Open a trade route between harbours (or airship masts, for aircraft) and put a ship or aircraft on it.",
        r#"{"name": "Wool run", "craft": "schooner", "vessel": "Sea Lark", "stops": [{"at": 1, "load": ["wool"], "unload": []}, {"at": 7, "load": [], "unload": ["wool"]}]}"#,
        |world, args| {
            let craft = args["craft"].as_str().and_then(Craft::from_name).ok_or("craft: schooner, steamer, smack, barque, clipper, paddle_steamer, airship, biplane or seaplane")?;
            let mut stops = Vec::new();
            for s in args["stops"].as_array().ok_or("stops wants a list")? {
                let at = s["at"].as_u64().ok_or("each stop wants a building id in at")? as u32;
                let e = build::by_id(world, at).ok_or(format!("no building {at}"))?;
                let kind = world.get::<Building>(e).map(|b| b.kind);
                let want = if craft.flies() { Kind::AirshipMast } else { Kind::Harbour };
                if kind != Some(want) {
                    return Err(format!("stop {at} is not a {}", want.def().name));
                }
                stops.push(Stop { at, load: goods(&s["load"])?, unload: goods(&s["unload"])? });
            }
            if stops.len() < 2 {
                return Err("a route needs two stops".into());
            }
            let first = build::by_id(world, stops[0].at).and_then(|e| world.get::<Building>(e).cloned()).ok_or("no first stop")?;
            let name = args["name"].as_str().unwrap_or("New route").to_string();
            let vessel = args["vessel"].as_str().unwrap_or(craft.name()).to_string();
            let r = {
                let mut routes = world.resource_mut::<Routes>();
                routes.0.push(Route { name, stops, air: craft.flies() });
                routes.0.len() - 1
            };
            let (at, heading) = if craft.flies() {
                (mast_point(&first) + DVec3::new(-48.0, 4.0, 0.0), 0.0)
            } else {
                let (p, h) = mooring(&first);
                (DVec3::new(p.x, 0.0, p.y), h)
            };
            let seed = 900 + r as u64;
            spawn_vessel(world, craft, &vessel, at, heading, Some(r), Roam::None, seed);
            Ok(json!({"route": r}))
        },
    )
    .register_api("isle.route.open", "Open an empty trade route; air for airship masts. Returns its index.", r#"{"name": "Brick run", "air": false}"#, |world, args| {
        let r = orders::open_route(world, args["name"].as_str().unwrap_or(""), args["air"].as_bool().unwrap_or(false));
        Ok(json!({"route": r}))
    })
    .register_api("isle.route.close", "Close a route; its vessels wait at the nearest harbour or mast, and later routes move down by one.", r#"{"route": 2}"#, |world, args| {
        orders::close_route(world, index(&args, "route")?)?;
        Ok(json!({}))
    })
    .register_api("isle.route.rename", "Rename a route.", r#"{"route": 0, "name": "Wool run"}"#, |world, args| {
        orders::rename_route(world, index(&args, "route")?, args["name"].as_str().unwrap_or(""))?;
        Ok(json!({}))
    })
    .register_api("isle.route.add_stop", "Add a harbour (or mast) to a route, at index or at the end.", r#"{"route": 0, "at": 7, "index": 1, "load": ["wool"], "unload": []}"#, |world, args| {
        let at = args["at"].as_u64().ok_or("at wants a building id")? as u32;
        let i = args["index"].as_u64().map(|i| i as usize);
        let i = orders::add_stop(world, index(&args, "route")?, at, i, goods(&args["load"])?, goods(&args["unload"])?)?;
        Ok(json!({"stop": i}))
    })
    .register_api("isle.route.remove_stop", "Take a stop off a route.", r#"{"route": 0, "stop": 1}"#, |world, args| {
        orders::remove_stop(world, index(&args, "route")?, index(&args, "stop")?)?;
        Ok(json!({}))
    })
    .register_api("isle.route.move_stop", "Reorder a route: stop from becomes stop to.", r#"{"route": 0, "from": 2, "to": 0}"#, |world, args| {
        orders::move_stop(world, index(&args, "route")?, index(&args, "from")?, index(&args, "to")?)?;
        Ok(json!({}))
    })
    .register_api("isle.route.set_goods", "What a stop loads and unloads.", r#"{"route": 0, "stop": 1, "load": [], "unload": ["wool"]}"#, |world, args| {
        orders::set_goods(world, index(&args, "route")?, index(&args, "stop")?, goods(&args["load"])?, goods(&args["unload"])?)?;
        Ok(json!({}))
    })
    .register_api("isle.route.assign", "Put a ship or aircraft (by name or id from isle.fleet) on a route.", r#"{"route": 0, "vessel": "Sea Lark"}"#, |world, args| {
        let e = vessel(world, &args)?;
        orders::assign(world, e, index(&args, "route")?)?;
        Ok(json!({}))
    })
    .register_api("isle.route.unassign", "Take a ship or aircraft off its route; it waits at the nearest harbour or mast.", r#"{"vessel": "Sea Lark"}"#, |world, args| {
        let e = vessel(world, &args)?;
        orders::unassign(world, e)?;
        Ok(json!({}))
    })
    .register_api("isle.speed", "Game speed: 0 pauses, 1 normal, up to 8.", r#"{"speed": 2}"#, |world, args| {
        let s = args["speed"].as_f64().ok_or("speed wants a number")? as f32;
        world.resource_mut::<Economy>().speed = s.clamp(0.0, 8.0);
        Ok(json!({}))
    })
    .register_api(
        "isle.camera",
        "Move the view: focus [x, z] in meters (or island by name), yaw radians, dist meters; snap skips the glide.",
        r#"{"island": "Brannsey", "dist": 600, "yaw": 0.5, "snap": true}"#,
        |world, args| {
            let target = if let Some(name) = args["island"].as_str() {
                let i = islands().iter().find(|i| i.name.eq_ignore_ascii_case(name)).ok_or("no such island")?;
                Some(DVec3::new(i.center.x, 0.0, i.center.y))
            } else {
                args["focus"].as_array().map(|f| DVec3::new(f[0].as_f64().unwrap_or(0.0), 0.0, f[1].as_f64().unwrap_or(0.0)))
            };
            let mut q = world.query::<&mut StrategyCam>();
            let mut cam = q.iter_mut(world).next().ok_or("no camera")?;
            if let Some(t) = target {
                cam.goal_focus = t;
            }
            if let Some(y) = args["yaw"].as_f64() {
                cam.goal_yaw = y as f32;
            }
            if let Some(d) = args["dist"].as_f64() {
                cam.goal_dist = d as f32;
            }
            if args["snap"].as_bool().unwrap_or(false) {
                cam.snap();
            }
            Ok(json!({"focus": [cam.goal_focus.x, cam.goal_focus.z], "yaw": cam.goal_yaw, "dist": cam.goal_dist}))
        },
    )
    .register_api("isle.check", "Whether a kind fits at a cell, and why not.", r#"{"kind": "house", "at": [3, 7], "rot": 0}"#, |world, args| {
        let kind = args["kind"].as_str().and_then(Kind::from_name).ok_or("unknown kind")?;
        let at = cell(&args, "at")?;
        let rot = args["rot"].as_u64().unwrap_or(0) as u8;
        Ok(match grid::check(world.resource::<Grid>(), kind, at, rot) {
            Ok(i) => json!({"ok": true, "island": islands()[i].name}),
            Err(b) => json!({"ok": false, "why": b.message()}),
        })
    });
}
