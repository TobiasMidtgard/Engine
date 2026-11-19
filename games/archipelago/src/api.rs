//! `isle.*`: the game for scripts, tests and AI agents (the same calls the HUD makes).

use engine::prelude::*;
use serde_json::{Value, json};

use crate::build;
use crate::camera::StrategyCam;
use crate::defs::{DEFS, Good, Kind, Tier};
use crate::fleet::{Roam, Route, Routes, Stop, Vessel, mast_point, mooring, spawn_vessel};
use crate::globe;
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
                        "burghers": b.population[2],
                        "harbours": b.harbours,
                        "stock": stock,
                        "workforce": b.workforce,
                    })
                })
                .collect();
            let mut houses = [0usize; Tier::COUNT];
            let mut house_income = 0.0f32;
            let mut q = world.query::<&House>();
            for h in q.iter(world) {
                houses[h.tier.index()] += 1;
                house_income += h.income;
            }
            // What every need is worth, tier by tier: people into a full house and coins a
            // minute. Plain values; nothing in the game is a percentage of taxes.
            let needs: Vec<Value> = Tier::ALL
                .iter()
                .map(|t| {
                    let wants: Vec<Value> = t
                        .wants()
                        .iter()
                        .map(|w| {
                            let (what, name) = match w.need {
                                crate::defs::Need::Good(g) => ("good", g.name()),
                                crate::defs::Need::Service(k) => ("service", k.def().name),
                            };
                            json!({"need": name, "kind": what, "people": w.people, "coins": w.coins})
                        })
                        .collect();
                    json!({"tier": t.name(), "house": t.capacity(), "full_income": t.full_income(), "wants": wants})
                })
                .collect();
            let (roads, paved) = {
                let g = world.resource::<crate::grid::Grid>();
                (g.roads.len(), g.paved.len())
            };
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
                "houses": {"fisherfolk": houses[0], "labourers": houses[1], "burghers": houses[2]},
                "house_income_per_min": house_income,
                "needs": needs,
                "roads": {"cells": roads, "paved": paved},
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
                            "input": d.input.map(Good::name), "input2": d.input2.map(Good::name), "output": d.output.map(Good::name), "cycle_s": d.cycle_s,
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
        "Lay road from one cell to another (x first, then z); paved lays setts, over dirt too.",
        r#"{"from": [10, 4], "to": [18, 9], "paved": false}"#,
        |world, args| {
            let paved = args["paved"].as_bool().unwrap_or(false);
            let n = build::place_road_line(world, cell(&args, "from")?, cell(&args, "to")?, paved, !args["free"].as_bool().unwrap_or(false))?;
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
                        "income": h.map(|h| h.income), "paved": h.map(|h| h.paved),
                    })
                })
                .collect();
            Ok(Value::Array(v))
        },
    )
    .register_api(
        "isle.fog",
        "The fog of war: show or hide it, clear it round a map point, or ask whether a map point has been seen (at_seen). Returns the share of the planet seen.",
        r#"{"on": true, "reveal": [0, 0, 1500], "at": [0, 0]}"#,
        |world, args| {
            let mut fog = world.resource_mut::<crate::fog::FogOfWar>();
            if let Some(on) = args.get("on").and_then(Value::as_bool) {
                fog.shown = on;
            }
            if let Some(r) = args.get("reveal").and_then(Value::as_array) {
                let n = |i: usize| r.get(i).and_then(Value::as_f64);
                let (Some(x), Some(z)) = (n(0), n(1)) else {
                    return Err("reveal takes [x, z, radius]".into());
                };
                fog.reveal_map(x, z, n(2).unwrap_or(1_000.0));
            }
            // "at": [x, z] asks whether that map point has been seen.
            let at = args.get("at").and_then(Value::as_array).and_then(|a| {
                Some(fog.is_seen(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
            });
            Ok(json!({"on": fog.shown, "seen": fog.share_seen(), "at_seen": at}))
        },
    )
    .register_api(
        "isle.regions",
        "The planet's regions: name, climate, where on the globe, the map point of each chart's middle, and its islands.",
        "{}",
        |_, _| {
            let out: Vec<Value> = globe::regions()
                .iter()
                .enumerate()
                .map(|(k, r)| {
                    let o = globe::sheet_origin(k);
                    json!({
                        "name": r.name, "about": r.blurb, "climate": format!("{:?}", r.biome),
                        "lat": r.lat_deg, "lon": r.lon_deg, "reach_m": r.reach_m,
                        "map": [o.x, o.y],
                        "islands": islands().iter().enumerate()
                            .filter(|(_, i)| i.region() == k)
                            .map(|(n, i)| json!({"index": n, "name": i.name, "at": [i.center.x, i.center.y], "radius": i.radius}))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(Value::Array(out))
        },
    )
    .register_api("isle.fleet", "Routes and every ship and aircraft: where, doing what, carrying what.", "{}", |world, _| {
        let routes = serde_json::to_value(&world.resource::<Routes>().0).map_err(|e| e.to_string())?;
        let mut q = world.query::<(Entity, &Vessel)>();
        let vessels: Vec<Value> = q
            .iter(world)
            .map(|(e, v)| {
                json!({
                    "id": e.to_bits(), "name": v.name, "craft": v.craft.name(), "route": v.route, "doing": v.doing,
                    "at": [v.at.x.round(), v.at.y.round(), v.at.z.round()],
                    "region": globe::region_at(v.at.x, v.at.z).name,
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
            // A new game: the route takes a craft the company has, off duty (built at a yard).
            if *world.resource::<crate::settle::StartMode>() == crate::settle::StartMode::Fresh {
                let named = args["vessel"].as_str().and_then(|n| orders::find_vessel(world, n));
                let e = match named {
                    Some(e) => e,
                    None => {
                        let mut q = world.query::<(Entity, &Vessel)>();
                        q.iter(world)
                            .find(|(_, v)| v.craft == craft && v.route.is_none() && v.owned() && !matches!(v.roam, Roam::Fishing { .. } | Roam::Found { .. }))
                            .map(|(e, _)| e)
                            .ok_or(format!("No {} of yours is off duty: build one at a {}.", craft.name().to_lowercase(), if craft.flies() { "airship mast" } else { "shipyard" }))?
                    }
                };
                let r = {
                    let mut routes = world.resource_mut::<Routes>();
                    routes.0.push(Route { name, stops, air: craft.flies() });
                    routes.0.len() - 1
                };
                orders::assign(world, e, r)?;
                return Ok(json!({"route": r, "vessel": e.to_bits()}));
            }
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
    .register_api("isle.items", "Items: renown and how many of each item building it allows, every Trading Union, Town Hall and Harbour Office with its slots and what each item is doing now, the shelf, and the pedlar's stock while he is moored.", "{}", |world, _| {
        Ok(crate::items::state(world))
    })
    .register_api("isle.items.table", "Every item there is: where it goes, its rank, guild and what it does; and what three of a guild add.", "{}", |_, _| {
        Ok(crate::items::catalogue())
    })
    .register_api("isle.item.socket", "Put a shelf item (uid from isle.items) into a slot of a Trading Union, Town Hall or Harbour Office; what was there goes back on the shelf.", r#"{"building": 41, "slot": 0, "uid": 7}"#, |world, args| {
        let b = index(&args, "building")? as u32;
        crate::items::socket(world, b, index(&args, "slot")?, index(&args, "uid")? as u32)?;
        build::refresh(world);
        Ok(crate::items::state(world))
    })
    .register_api("isle.item.unsocket", "Take the item out of a slot, back to the shelf.", r#"{"building": 41, "slot": 0}"#, |world, args| {
        let b = index(&args, "building")? as u32;
        crate::items::unsocket(world, b, index(&args, "slot")?)?;
        build::refresh(world);
        Ok(crate::items::state(world))
    })
    .register_api("isle.item.buy", "Buy one of the pedlar's offers (index from isle.items) while he is at the quay.", r#"{"offer": 0}"#, |world, args| {
        let name = crate::items::buy(world, index(&args, "offer")?)?;
        Ok(json!({"bought": name}))
    })
    .register_api("isle.item.sell", "Sell a shelf item for two fifths of its worth.", r#"{"uid": 7}"#, |world, args| {
        let coins = crate::items::sell(world, index(&args, "uid")? as u32)?;
        Ok(json!({"coins": coins}))
    })
    .register_api("isle.item.use", "Press the order or use the consumable in a slot of a Trading Union, Town Hall or Harbour Office.", r#"{"building": 41, "slot": 2}"#, |world, args| {
        let b = index(&args, "building")? as u32;
        let name = crate::items::use_item(world, b, index(&args, "slot")?)?;
        Ok(json!({"started": name}))
    })
    .register_api("isle.gear", "Every vessel's captain and gear slots, what each item aboard is doing, and what the gear adds up to.", "{}", |world, _| {
        Ok(serde_json::to_value(crate::gear::fleet(world)).unwrap_or_default())
    })
    .register_api("isle.gear.socket", "Put a shelf item aboard a vessel (vessel from isle.gear; slot 0 is the captain's).", r#"{"vessel": 120, "slot": 1, "uid": 7}"#, |world, args| {
        let v = index(&args, "vessel")? as u32;
        crate::gear::socket(world, v, index(&args, "slot")?, index(&args, "uid")? as u32)?;
        Ok(json!({"ok": true}))
    })
    .register_api("isle.gear.unsocket", "Take an item from aboard a vessel, back to the shelf.", r#"{"vessel": 120, "slot": 1}"#, |world, args| {
        let v = index(&args, "vessel")? as u32;
        crate::gear::unsocket(world, v, index(&args, "slot")?)?;
        Ok(json!({"ok": true}))
    })
    .register_api("isle.gear.use", "Press an order or use a consumable aboard a vessel.", r#"{"vessel": 120, "slot": 1}"#, |world, args| {
        let v = index(&args, "vessel")? as u32;
        let name = crate::gear::use_item(world, v, index(&args, "slot")?)?;
        Ok(json!({"started": name}))
    })
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
