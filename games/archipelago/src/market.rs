//! Trade between companies: a ship that calls at another company's harbour buys what that
//! island has to spare and sells it what it lacks, for coins, at prices set by the stock.
//!
//! Every good has a base price, what it costs to make one (the upkeep of the works for one
//! batch, and the goods that go into it) with a margin ([`base_price`]). A harbour asks more
//! for what it has little of and pays more for what its houses and works are short of, so a
//! company that makes what others lack earns from them, and one that cannot make a good
//! itself can still have it, dearly. A harbour sells only what it has above what it keeps for
//! itself ([`keeps`]), and never trades with a company it is at war with.

use std::sync::OnceLock;

use crate::defs::{DEFS, Good};
use crate::sim::{Company, Economy, IslandBooks, PLAYER};

/// What a seller asks above what a good costs to make.
const MARGIN: f32 = 1.5;
/// The least a good is worth, coins.
const FLOOR: f32 = 2.0;

/// What one of `g` costs to make (upkeep for a batch, its inputs), times the margin.
pub fn base_price(g: Good) -> f32 {
    static PRICES: OnceLock<Vec<f32>> = OnceLock::new();
    PRICES.get_or_init(|| {
        let mut cost = [f32::NAN; Good::COUNT];
        // Raw goods first, then whatever is made of what is known, until nothing changes.
        for _ in 0..8 {
            for d in DEFS.iter() {
                let Some(out) = d.output else { continue };
                let inputs: Vec<f32> = [d.input, d.input2]
                    .into_iter()
                    .flatten()
                    .map(|g| cost[g.index()])
                    .collect();
                if inputs.iter().any(|c| c.is_nan()) {
                    continue;
                }
                let c = d.upkeep * d.cycle_s / 60.0 + inputs.iter().sum::<f32>();
                let slot = &mut cost[out.index()];
                if slot.is_nan() || c < *slot {
                    *slot = c;
                }
            }
        }
        cost.iter()
            .map(|c| {
                if c.is_nan() {
                    FLOOR
                } else {
                    (c * MARGIN).max(FLOOR)
                }
            })
            .collect()
    })[g.index()]
}

/// How much of `g` an island keeps for itself: ten minutes of what it uses and its houses
/// ask for, and never fewer than 20.
pub fn keeps(books: &IslandBooks, g: Good) -> f64 {
    let i = g.index();
    (10.0 * (books.used_per_min[i].max(books.wanted_per_min[i]) as f64)).max(20.0)
}

/// What a foreign ship pays a unit for `g` loaded at this island: dearer the less there is
/// above what it keeps.
pub fn ask(books: &IslandBooks, g: Good) -> f32 {
    let keep = keeps(books, g);
    let spare = ((books.get(g) - keep) / keep.max(1.0)).clamp(0.0, 2.0) as f32;
    base_price(g) * (1.6 - 0.35 * spare)
}

/// What this island pays a unit for `g` a foreign ship lands: more the shorter its houses
/// and works are of it.
pub fn bid(books: &IslandBooks, g: Good) -> f32 {
    let i = g.index();
    let short = if books.wanted_per_min[i] > 0.05 || books.used_per_min[i] > 0.05 {
        1.0 - books.supply[i].clamp(0.0, 1.0)
    } else {
        0.0
    };
    let full = (books.get(g) / keeps(books, g)).clamp(0.0, 1.0) as f32;
    base_price(g) * (0.5 + 0.9 * short) * (1.0 - 0.4 * full)
}

/// Whether companies `a` and `b` trade, and at what factor on the prices (a pact makes it
/// cheaper, ill will dearer).
pub fn terms(world: &engine::prelude::World, a: Company, b: Company) -> Result<f32, String> {
    if a == b {
        return Ok(1.0);
    }
    // Between two rival houses: plain trade. With the player: by the treaty.
    let rival = if a == PLAYER {
        b
    } else if b == PLAYER {
        a
    } else {
        return Ok(1.0);
    };
    let Some(r) = world
        .get_resource::<crate::rivals::Rivals>()
        .and_then(|rs| rs.0.iter().find(|r| r.id == rival as u32).cloned())
    else {
        return Ok(1.0);
    };
    use crate::rivals::Treaty;
    match r.treaty {
        Treaty::War => Err(format!("{} trades with nobody it is at war with.", r.name)),
        Treaty::Hostile => Ok(1.25),
        Treaty::Neutral => Ok(1.0),
        Treaty::TradePact | Treaty::Alliance => Ok(0.9),
    }
}

/// What a call at another company's quay came to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deal {
    /// Goods landed (sold to the island) and loaded (bought from it).
    pub sold: u32,
    pub bought: u32,
    /// Coins the ship's company took in and paid out.
    pub earned: f64,
    pub paid: f64,
}

/// A call by company `ship`'s vessel at island `isle` of another company: sell what the stop
/// unloads (as far as the island still takes it and can pay), then buy what it loads (what
/// the island has above [`keeps`], as far as the ship's company can pay). `factor` is from
/// [`terms`].
#[allow(clippy::too_many_arguments)]
pub fn deal(
    econ: &mut Economy,
    isle: usize,
    ship: Company,
    cargo: &mut Vec<(Good, u32)>,
    load: &[Good],
    unload: &[Good],
    slots: u32,
    per: u32,
    factor: f32,
) -> Deal {
    let owner = econ.islands[isle].owner;
    let mut d = Deal::default();
    for (g, n) in cargo.iter_mut() {
        if !unload.contains(g) {
            continue;
        }
        let books = &econ.islands[isle];
        let room = (crate::fleet::WAREHOUSE_WANTS - books.get(*g))
            .floor()
            .max(0.0) as u32;
        let price = (bid(books, *g) / factor) as f64;
        let purse = econ.coins_of(owner).max(0.0);
        let can_pay = if price > 0.0 {
            (purse / price).floor() as u32
        } else {
            0
        };
        let k = (*n).min(room).min(can_pay);
        if k == 0 {
            continue;
        }
        econ.islands[isle].add(*g, k as f64);
        *n -= k;
        let coins = price * k as f64;
        *econ.coins_mut(owner) -= coins;
        *econ.coins_mut(ship) += coins;
        d.sold += k;
        d.earned += coins;
    }
    cargo.retain(|(_, n)| *n > 0);
    for g in load {
        if unload.contains(g) {
            continue;
        }
        let aboard = cargo.iter().position(|c| c.0 == *g);
        if aboard.is_none() && cargo.len() as u32 >= slots {
            continue;
        }
        let books = &econ.islands[isle];
        let room = per - aboard.map_or(0, |i| cargo[i].1.min(per));
        let spare = (books.get(*g) - keeps(books, *g)).floor().max(0.0) as u32;
        let price = (ask(books, *g) * factor) as f64;
        let purse = econ.coins_of(ship).max(0.0);
        let can_pay = (purse / price.max(0.01)).floor() as u32;
        let n = room.min(spare).min(can_pay);
        if n == 0 {
            continue;
        }
        econ.islands[isle].take(*g, n as f64);
        let coins = price * n as f64;
        *econ.coins_mut(ship) -= coins;
        *econ.coins_mut(owner) += coins;
        match aboard {
            Some(i) => cargo[i].1 += n,
            None => cargo.push((*g, n)),
        }
        d.bought += n;
        d.paid += coins;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_goods_cost_more_than_what_goes_into_them() {
        for g in Good::ALL {
            assert!(base_price(g) >= FLOOR, "{g:?}");
        }
        assert!(base_price(Good::Clothes) > base_price(Good::Wool));
        assert!(base_price(Good::Bread) > base_price(Good::Flour));
    }

    #[test]
    fn a_rival_buys_only_the_spare_and_pays_for_it() {
        let mut econ = Economy::default();
        *econ.coins_mut(1) = 1_000.0;
        econ.islands[2].harbours = 1;
        econ.islands[2].owner = 2;
        econ.islands[2].capacity = 200.0;
        econ.islands[2].stock[Good::Salt.index()] = 50.0;
        let before = (econ.coins_of(1), econ.coins_of(2));
        let mut cargo = Vec::new();
        let d = deal(&mut econ, 2, 1, &mut cargo, &[Good::Salt], &[], 2, 40, 1.0);
        // It keeps 20; 30 go aboard, paid for.
        assert_eq!(d.bought, 30);
        assert_eq!(cargo, vec![(Good::Salt, 30)]);
        assert!(d.paid > 0.0);
        assert!((econ.coins_of(1) - (before.0 - d.paid)).abs() < 1e-6);
        assert!((econ.coins_of(2) - (before.1 + d.paid)).abs() < 1e-6);
        // A dear price the buyer cannot meet: it takes what it can pay for.
        *econ.coins_mut(1) = 0.0;
        econ.islands[2].stock[Good::Salt.index()] = 50.0;
        let mut cargo = Vec::new();
        let d = deal(&mut econ, 2, 1, &mut cargo, &[Good::Salt], &[], 2, 40, 1.0);
        assert_eq!(d.bought, 0);
    }

    #[test]
    fn short_islands_pay_more() {
        let mut books = IslandBooks {
            capacity: 200.0,
            ..Default::default()
        };
        books.wanted_per_min[Good::Bread.index()] = 3.0;
        books.supply[Good::Bread.index()] = 0.1;
        let short = bid(&books, Good::Bread);
        books.supply[Good::Bread.index()] = 1.0;
        let met = bid(&books, Good::Bread);
        assert!(short > met * 1.5, "{short} vs {met}");
    }
}
