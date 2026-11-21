//! The free traders: barques of no company that call at every settled harbour in turn and
//! sell raw goods to whoever holds it, dearly.
//!
//! A company leaves a standing order at a harbour: keep so much of a raw good in store
//! (wool, grain, hops, clay, cane, dates, salt, furs, oil, fish, logs). At each call the
//! trader sells it up to that much, at most [`LOT`] of each good, at [`MARKUP`] times what the
//! good costs to make ([`crate::market::base_price`]), and only what the purse can pay
//! outright. What is made from raw goods (bread, coats, bricks) is never for sale: it still
//! takes the company's own works, workers and ground. So coins that pile up once every
//! island is taken buy the ground a company lacks, at a price that hurts.

use engine::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};

use crate::defs::{DEFS, Good};
use crate::sim::{Company, Economy};

/// What the trader asks over what a good costs to make.
pub const MARKUP: f32 = 2.5;
/// The most of one good a call sells.
pub const LOT: f64 = 25.0;
/// Game seconds between calls at one harbour.
pub const CALL_S: f64 = 480.0;
/// The most a standing order may keep in store.
pub const MOST: f64 = 200.0;

/// One standing order.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Order {
    pub company: Company,
    pub island: usize,
    pub good: Good,
    /// Bring the store up to this much at each call.
    pub keep: f64,
}

/// One call's sale, for the books.
#[derive(Clone, Debug, Serialize)]
pub struct Sale {
    pub clock_s: f64,
    pub company: Company,
    pub island: usize,
    pub good: Good,
    pub amount: f64,
    pub coins: f64,
}

#[derive(Resource, Default)]
pub struct Traders {
    pub orders: Vec<Order>,
    /// When each island's harbour last had a call (game seconds).
    last_call: Vec<f64>,
    /// The latest sales, newest last (the last 40).
    pub sales: Vec<Sale>,
}

/// Whether the free traders sell `g`: raw goods only, what no works makes from another good.
pub fn sells(g: Good) -> bool {
    DEFS.iter()
        .filter(|d| d.output == Some(g))
        .all(|d| d.input.is_none() && d.input2.is_none())
}

/// What one of `g` costs from the trader, coins.
pub fn price(g: Good) -> f64 {
    (crate::market::base_price(g) * MARKUP).ceil() as f64
}

/// Leave, change or (keep 0) cancel company `c`'s standing order for `g` at island `i`.
pub fn order(world: &mut World, c: Company, i: usize, g: Good, keep: f64) -> Result<(), String> {
    if !sells(g) {
        return Err(format!(
            "The free traders sell only raw goods, not {}.",
            g.name()
        ));
    }
    let econ = world.resource::<Economy>();
    if !econ.rules.free_traders {
        return Err("This game is played without the free traders.".into());
    }
    let Some(b) = econ.islands.get(i) else {
        return Err("No such island.".into());
    };
    if !econ.owns(c, i) || b.harbours == 0 {
        return Err("The free traders call only at your own harbours.".into());
    }
    let keep = keep.clamp(0.0, MOST);
    let mut t = world.get_resource_or_insert_with(Traders::default);
    t.orders
        .retain(|o| !(o.company == c && o.island == i && o.good == g));
    if keep > 0.0 {
        t.orders.push(Order {
            company: c,
            island: i,
            good: g,
            keep,
        });
    }
    Ok(())
}

/// The standing orders of company `c`.
pub fn orders_of(world: &World, c: Company) -> Vec<Order> {
    world
        .get_resource::<Traders>()
        .map(|t| {
            t.orders
                .iter()
                .filter(|o| o.company == c)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Every frame: a call at each harbour whose turn it is.
pub fn tick(world: &mut World) {
    let now = world.resource::<Economy>().clock_s;
    step(world, now);
}

/// Calls due at game time `now` (tests call this directly).
pub fn step(world: &mut World, now: f64) {
    let Some(mut t) = world.remove_resource::<Traders>() else {
        return;
    };
    let mut econ = world.resource_mut::<Economy>();
    if !econ.rules.free_traders {
        world.insert_resource(t);
        return;
    }
    let n = econ.islands.len();
    if t.last_call.len() != n {
        // (The calls are spread over the round so not every harbour gets one at once.)
        t.last_call = (0..n)
            .map(|i| now - CALL_S * i as f64 / n.max(1) as f64)
            .collect();
    }
    // Orders for islands a company no longer holds lapse.
    t.orders
        .retain(|o| econ.owns(o.company, o.island) && econ.islands[o.island].harbours > 0);
    for i in 0..n {
        if now - t.last_call[i] < CALL_S {
            continue;
        }
        t.last_call[i] = now;
        for o in t.orders.iter().filter(|o| o.island == i) {
            let have = econ.islands[i].get(o.good);
            let room = (econ.islands[i].capacity - have).max(0.0);
            let want = (o.keep - have).min(LOT).min(room).floor();
            if want < 1.0 {
                continue;
            }
            let each = price(o.good);
            let coins = econ.coins_of(o.company);
            let amount = want.min((coins / each).floor());
            if amount < 1.0 {
                continue;
            }
            *econ.coins_mut(o.company) -= amount * each;
            econ.islands[i].add(o.good, amount);
            t.sales.push(Sale {
                clock_s: now,
                company: o.company,
                island: i,
                good: o.good,
                amount,
                coins: amount * each,
            });
        }
    }
    let extra = t.sales.len().saturating_sub(40);
    t.sales.drain(..extra);
    world.insert_resource(t);
}

pub fn register(app: &mut App) {
    app.init_resource::<Traders>()
        .register_api(
            "isle.trader",
            "The free traders: the raw goods they sell and at what price, how long until each of your harbours gets its next call, your standing orders, and your latest purchases.",
            "{}",
            |world, _| {
                let me = crate::sim::acting(world);
                let now = world.resource::<Economy>().clock_s;
                let goods: Vec<Value> = Good::ALL
                    .into_iter()
                    .filter(|g| sells(*g))
                    .map(|g| json!({"good": g.name(), "price": price(g)}))
                    .collect();
                let t = world.resource::<Traders>();
                let econ = world.resource::<Economy>();
                let calls: Vec<Value> = (0..econ.islands.len())
                    .filter(|&i| econ.owns(me, i) && econ.islands[i].harbours > 0)
                    .map(|i| {
                        let last = t.last_call.get(i).copied().unwrap_or(now);
                        json!({
                            "island": i,
                            "name": crate::island::islands()[i].name,
                            "next_call_s": (CALL_S - (now - last)).max(0.0),
                        })
                    })
                    .collect();
                let orders: Vec<Value> = t
                    .orders
                    .iter()
                    .filter(|o| o.company == me)
                    .map(|o| json!({"island": o.island, "good": o.good.name(), "keep": o.keep}))
                    .collect();
                let sales: Vec<&Sale> = t.sales.iter().filter(|s| s.company == me).collect();
                Ok(json!({
                    "markup": MARKUP,
                    "lot": LOT,
                    "call_s": CALL_S,
                    "goods": goods,
                    "calls": calls,
                    "orders": orders,
                    "sales": sales,
                }))
            },
        )
        .register_api(
            "isle.trader.order",
            "Leave a standing order with the free traders: at each call at this island's harbour they bring the store of a raw good up to `keep` (at most 25 a call, paid outright at their price). keep 0 cancels it.",
            r#"{"island": 0, "good": "Wool", "keep": 60}"#,
            |world, args| {
                let i = args["island"].as_u64().ok_or("island is missing")? as usize;
                let g = args["good"]
                    .as_str()
                    .and_then(Good::from_name)
                    .ok_or("good is missing or unknown")?;
                let keep = args["keep"].as_f64().ok_or("keep is missing")?;
                let me = crate::sim::acting(world);
                order(world, me, i, g, keep)?;
                Ok(json!({"orders": orders_of(world, me)}))
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_free_traders_sell_raw_goods_dearly_and_only_what_the_purse_can_pay() {
        assert!(sells(Good::Wool) && sells(Good::Salt) && sells(Good::Furs));
        assert!(!sells(Good::Coats) && !sells(Good::Bricks) && !sells(Good::Bread));
        assert!(price(Good::Wool) >= crate::market::base_price(Good::Wool) as f64 * 2.5);

        let mut world = World::new();
        let mut econ = Economy::default();
        econ.islands[0].harbours = 1;
        econ.islands[0].owner = 0;
        econ.islands[0].capacity = 100.0;
        econ.clock_s = 0.0;
        world.insert_resource(econ);
        world.insert_resource(Traders::default());
        assert!(order(&mut world, 0, 0, Good::Coats, 50.0).is_err());
        assert!(order(&mut world, 1, 0, Good::Wool, 50.0).is_err());
        order(&mut world, 0, 0, Good::Wool, 60.0).unwrap();
        let each = price(Good::Wool);
        *world.resource_mut::<Economy>().coins_mut(0) = each * 10.5;
        step(&mut world, 0.0);
        // (Island 0's first call comes a full round after the start.)
        step(&mut world, CALL_S);
        let econ = world.resource::<Economy>();
        assert_eq!(econ.islands[0].get(Good::Wool), 10.0);
        assert!((econ.coins_of(0) - each * 0.5).abs() < 1e-6);
        // Plenty of coins: one lot a call, up to what the order keeps.
        *world.resource_mut::<Economy>().coins_mut(0) = 1e6;
        step(&mut world, 2.0 * CALL_S);
        assert_eq!(world.resource::<Economy>().islands[0].get(Good::Wool), 35.0);
        step(&mut world, 3.0 * CALL_S);
        step(&mut world, 4.0 * CALL_S);
        assert_eq!(world.resource::<Economy>().islands[0].get(Good::Wool), 60.0);
        order(&mut world, 0, 0, Good::Wool, 0.0).unwrap();
        assert!(orders_of(&world, 0).is_empty());
    }
}
