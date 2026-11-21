//! Cheat orders, to test a game faster: coins, goods, the whole chart, every building open,
//! ships finished, houses filled and moved up, standing with a house, and more speed. They
//! work only while the game's rules allow cheats ([`crate::rules::Rules::cheats`]), always
//! for the player's company, and the rival captains never use them.

use engine::prelude::*;
use serde_json::{Value, json};

use crate::defs::{Good, Tier};
use crate::sim::{Building, Economy, House, PLAYER};

fn allowed(world: &World) -> Result<(), String> {
    if world.resource::<Economy>().rules.cheats {
        Ok(())
    } else {
        Err("Cheats are off in this game's rules.".into())
    }
}

/// The islands an order names: `island` (a number), or every island of the player's.
fn isles(world: &World, args: &Value) -> Result<Vec<usize>, String> {
    let econ = world.resource::<Economy>();
    match args.get("island").and_then(Value::as_u64) {
        Some(i) if econ.owns(PLAYER, i as usize) => Ok(vec![i as usize]),
        Some(_) => Err("That island is not yours.".into()),
        None => Ok((0..econ.islands.len())
            .filter(|&i| econ.owns(PLAYER, i))
            .collect()),
    }
}

/// Add coins to the purse.
pub fn coins(world: &mut World, n: f64) -> Result<Value, String> {
    allowed(world)?;
    let mut econ = world.resource_mut::<Economy>();
    econ.coins += n;
    Ok(json!({ "coins": econ.coins.round() }))
}

/// Fill warehouses with `n` of `goods` (every good when it is empty), up to their room.
pub fn goods(world: &mut World, at: &[usize], goods: &[Good], n: f64) -> Result<Value, String> {
    allowed(world)?;
    let all: Vec<Good> = if goods.is_empty() {
        Good::ALL.to_vec()
    } else {
        goods.to_vec()
    };
    let mut econ = world.resource_mut::<Economy>();
    for &i in at {
        let b = &mut econ.islands[i];
        for &g in &all {
            let room = (b.capacity - b.get(g)).max(0.0);
            b.add(g, n.min(room));
        }
    }
    Ok(json!({ "islands": at, "goods": all.len() }))
}

/// Clear the fog from the whole map and chart every island.
pub fn chart(world: &mut World) -> Result<Value, String> {
    allowed(world)?;
    let n = crate::island::islands().len();
    for i in 0..n {
        crate::fog::reveal_island(world, i);
    }
    if let Some(mut ch) = world.get_resource_mut::<crate::brain::Charted>() {
        for i in 0..n {
            ch.mark(PLAYER, i);
        }
    }
    Ok(json!({ "islands": n }))
}

/// Open (or close again) every building to the player whatever its people.
pub fn unlock(world: &mut World, on: bool) -> Result<Value, String> {
    allowed(world)?;
    world.resource_mut::<Economy>().rules.unlocked = on;
    Ok(json!({ "unlocked": on }))
}

/// Finish what is on every slip of the player's yards now.
pub fn ships(world: &mut World) -> Result<Value, String> {
    allowed(world)?;
    let econ = world.resource::<Economy>().clone();
    let mut n = 0;
    let mut q = world.query::<(&Building, &mut crate::shipyard::Yard)>();
    for (b, mut y) in q.iter_mut(world) {
        if econ.owns(PLAYER, b.island) && !y.queue.is_empty() {
            y.progress_s = 1e6;
            n += 1;
        }
    }
    Ok(json!({ "finished": n }))
}

/// Fill every house of the player's (on `at`) to the brim and make it content, so it is
/// ready to move up; with `up`, move each up one tier at once for free (to `top` at most,
/// and no higher than the rules allow).
pub fn houses(world: &mut World, at: &[usize], up: bool, top: Tier) -> Result<Value, String> {
    allowed(world)?;
    let top = top.min(world.resource::<Economy>().rules.top_tier);
    let mut filled = 0;
    let mut moved = 0;
    let mut q = world.query::<(&Building, &mut House)>();
    let mut ups: Vec<(usize, House)> = Vec::new();
    for (b, mut h) in q.iter_mut(world) {
        if !at.contains(&b.island) {
            continue;
        }
        h.residents = h.tier.capacity();
        h.happiness = h.happiness.max(80.0);
        filled += 1;
        if up && h.tier.next().is_some_and(|(t, _)| t <= top) {
            ups.push((b.island, h.clone()));
        }
    }
    if up {
        let mut done: Vec<House> = Vec::new();
        {
            let mut econ = world.resource_mut::<Economy>();
            for (i, mut h) in ups {
                crate::sim::move_up(&mut h, &mut econ.islands[i], true);
                h.residents = h.tier.capacity();
                done.push(h);
            }
        }
        // (Write the moved houses back in the order they were read.)
        let mut it = done.into_iter();
        let mut q = world.query::<(&Building, &mut House)>();
        for (b, mut h) in q.iter_mut(world) {
            if !at.contains(&b.island) {
                continue;
            }
            if h.tier.next().is_some_and(|(t, _)| t <= top)
                && let Some(n) = it.next()
            {
                *h = n;
                moved += 1;
            }
        }
    }
    Ok(json!({ "filled": filled, "moved_up": moved }))
}

/// Set where rival house `id` stands with the player (and make peace if it is at war).
pub fn standing(world: &mut World, id: u32, standing: i32) -> Result<Value, String> {
    allowed(world)?;
    let mut rivals = world.resource_mut::<crate::rivals::Rivals>();
    let r = rivals
        .0
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("No rival house numbered {id}."))?;
    r.standing = standing.clamp(-100, 100);
    if r.treaty == crate::rivals::Treaty::War && r.standing > -50 {
        r.treaty = crate::rivals::Treaty::Hostile;
    }
    Ok(json!({ "rival": id, "standing": r.standing, "treaty": r.treaty.name() }))
}

/// Game speed beyond the usual eight, up to 64.
pub fn speed(world: &mut World, s: f32) -> Result<Value, String> {
    allowed(world)?;
    let s = s.clamp(0.0, 64.0);
    world.resource_mut::<Economy>().speed = s;
    Ok(json!({ "speed": s }))
}

pub fn register(app: &mut App) {
    app.register_api(
        "isle.cheats",
        "The cheat orders and whether this game's rules allow them.",
        "{}",
        |world, _| {
            Ok(json!({
                "on": world.resource::<Economy>().rules.cheats,
                "unlocked": world.resource::<Economy>().rules.unlocked,
                "orders": [
                    {"api": "isle.cheat.coins", "label": "Coins", "args": {"coins": 100000}},
                    {"api": "isle.cheat.goods", "label": "Fill the warehouses", "args": {"amount": 100}},
                    {"api": "isle.cheat.chart", "label": "Reveal the map", "args": {}},
                    {"api": "isle.cheat.unlock", "label": "Open every building", "args": {"on": true}},
                    {"api": "isle.cheat.ships", "label": "Finish ships on the slips", "args": {}},
                    {"api": "isle.cheat.houses", "label": "Fill the houses", "args": {}},
                    {"api": "isle.cheat.up", "label": "Move every house up a tier", "args": {}},
                    {"api": "isle.cheat.standing", "label": "Befriend a house", "args": {"rival": 1, "standing": 80}},
                    {"api": "isle.cheat.speed", "label": "Speed 32", "args": {"speed": 32}},
                ],
            }))
        },
    )
    .register_api(
        "isle.cheat.coins",
        "Cheat: add coins to your purse (rules must allow cheats).",
        r#"{"coins": 100000}"#,
        |world, args| coins(world, args["coins"].as_f64().unwrap_or(100_000.0)),
    )
    .register_api(
        "isle.cheat.goods",
        "Cheat: put `amount` of each good (or of `goods`, a list of names) into the warehouse of `island`, or of every island of yours, up to its room.",
        r#"{"island": 0, "goods": ["Planks", "Bricks"], "amount": 100}"#,
        |world, args| {
            let at = isles(world, &args)?;
            let gs: Vec<Good> = match args.get("goods").and_then(Value::as_array) {
                Some(a) => a
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .and_then(Good::from_name)
                            .ok_or_else(|| format!("Not a good: {v}"))
                    })
                    .collect::<Result<_, _>>()?,
                None => Vec::new(),
            };
            goods(world, &at, &gs, args["amount"].as_f64().unwrap_or(100.0))
        },
    )
    .register_api(
        "isle.cheat.chart",
        "Cheat: clear the fog from the whole map and chart every island.",
        "{}",
        |world, _| chart(world),
    )
    .register_api(
        "isle.cheat.unlock",
        "Cheat: open every building whatever your people (on false closes them again).",
        r#"{"on": true}"#,
        |world, args| unlock(world, args["on"].as_bool().unwrap_or(true)),
    )
    .register_api(
        "isle.cheat.ships",
        "Cheat: finish what is on the slip of every yard of yours now.",
        "{}",
        |world, _| ships(world),
    )
    .register_api(
        "isle.cheat.houses",
        "Cheat: fill every house of yours (on `island`, or everywhere) and make it content.",
        r#"{"island": 0}"#,
        |world, args| {
            let at = isles(world, &args)?;
            houses(world, &at, false, Tier::Patricians)
        },
    )
    .register_api(
        "isle.cheat.up",
        "Cheat: move every house of yours (on `island`, or everywhere) up a tier for free, to `top` at most.",
        r#"{"island": 0, "top": "Masters"}"#,
        |world, args| {
            let at = isles(world, &args)?;
            let top = args["top"]
                .as_str()
                .and_then(|s| Tier::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(s)))
                .unwrap_or(Tier::Patricians);
            houses(world, &at, true, top)
        },
    )
    .register_api(
        "isle.cheat.standing",
        "Cheat: set where rival house `rival` stands with you (-100 to 100); above -50 ends a war.",
        r#"{"rival": 1, "standing": 80}"#,
        |world, args| {
            let id = args["rival"].as_u64().ok_or("rival: a number")? as u32;
            standing(world, id, args["standing"].as_i64().unwrap_or(80) as i32)
        },
    )
    .register_api(
        "isle.cheat.speed",
        "Cheat: game speed up to 64 (the usual top is 8).",
        r#"{"speed": 32}"#,
        |world, args| speed(world, args["speed"].as_f64().unwrap_or(32.0) as f32),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheats_work_only_when_the_rules_allow_them() {
        let mut w = World::new();
        let mut econ = Economy::default();
        econ.islands[0].owner = PLAYER;
        econ.islands[0].harbours = 1;
        econ.islands[0].capacity = 50.0;
        w.insert_resource(econ);
        coins(&mut w, 1_000.0).unwrap();
        assert_eq!(w.resource::<Economy>().coins, 13_000.0);
        goods(&mut w, &[0], &[Good::Planks], 80.0).unwrap();
        assert_eq!(w.resource::<Economy>().islands[0].get(Good::Planks), 50.0);
        unlock(&mut w, true).unwrap();
        let l = crate::unlocks::lock_of(w.resource::<Economy>(), PLAYER, crate::defs::Kind::Bank);
        assert!(!l.locked);
        let r = crate::unlocks::lock_of(w.resource::<Economy>(), 1, crate::defs::Kind::Bank);
        assert!(r.locked, "the unlock cheat is the player's only");
        w.resource_mut::<Economy>().rules.cheats = false;
        assert!(coins(&mut w, 1.0).is_err());
        assert!(speed(&mut w, 30.0).is_err());
    }
}
