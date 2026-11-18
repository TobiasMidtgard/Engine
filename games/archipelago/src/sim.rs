//! The economy: houses fill and pay, workshops turn goods into goods, each island keeps its
//! stock in its harbour's warehouse.
//!
//! It steps in game seconds ([`Economy::speed`] times real time, 0 pauses), reading only
//! components and its own resource, so a headless test can run years of it in a moment.

use engine::prelude::*;
use serde::Serialize;

use crate::defs::{Good, Kind, MAX_SERVICES, NEED_PER_PERSON_MIN, Need, Tier};
use crate::island;
use crate::items::Boost;

/// A placed building.
#[derive(Component, Clone, Debug)]
pub struct Building {
    pub kind: Kind,
    /// Footprint origin cell and quarter turns (see `grid`).
    pub origin: IVec2,
    pub rot: u8,
    pub island: usize,
    /// Floor height, meters.
    pub base_y: f64,
    /// Looks and variations come from this, never from the entity id.
    pub seed: u64,
    /// Stable number for the API and the UI.
    pub id: u32,
}

/// Why a workshop isn't working.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Working,
    NoInput,
    NoWorkers,
    NoRoad,
    Full,
    NoTrees,
    NoWilds,
    NoFields,
    Paused,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Working => "Working",
            Status::NoInput => "Waiting for goods",
            Status::NoWorkers => "Not enough workers",
            Status::NoRoad => "No road to a harbour",
            Status::Full => "Store full: carts are busy",
            Status::NoTrees => "No trees in reach",
            Status::NoWilds => "No wild country in reach",
            Status::NoFields => "No room for fields",
            Status::Paused => "Paused",
        }
    }
}

/// A building that makes goods.
#[derive(Component, Clone, Debug, Default)]
pub struct Producer {
    /// 0..1 through the current batch.
    pub progress: f32,
    /// Finished goods waiting for a cart.
    pub held: f32,
    /// The input for the current batch has been taken from the warehouse.
    pub input_ready: bool,
    /// Output share last step, 0..1 (workforce times site).
    pub efficiency: f32,
    pub status: Status,
    /// How well its site suits it, 0..1 (woodcutters: trees in reach).
    pub site: f32,
    pub paused: bool,
    /// Batches finished, for the panel and tests.
    pub made: u32,
    /// Share of a batch's goods an item has saved so far (a whole one makes a batch free).
    pub saved: f32,
}

/// A house and its people.
#[derive(Component, Clone, Debug)]
pub struct House {
    pub tier: Tier,
    pub residents: f32,
    /// Seconds its needs have all been met while full (it moves up a tier after a while).
    pub content_s: f32,
    /// Which of its tier's services are in reach.
    pub services: [bool; MAX_SERVICES],
    /// Coins a minute it pays right now: the sum of what each met need is worth.
    pub income: f32,
    /// Its door is on a paved road.
    pub paved: bool,
}

impl House {
    pub fn new(tier: Tier) -> Self {
        Self {
            tier,
            residents: 2.0,
            content_s: 0.0,
            services: [false; MAX_SERVICES],
            income: 0.0,
            paved: false,
        }
    }
}

/// One island's books.
#[derive(Clone, Debug, Default, Serialize)]
pub struct IslandBooks {
    pub stock: [f64; Good::COUNT],
    /// Warehouse room for each good.
    pub capacity: f64,
    pub population: [f32; Tier::COUNT],
    /// Workers asked for by its workshops, per tier.
    pub jobs: [f32; Tier::COUNT],
    /// Share of those jobs filled, per tier.
    pub workforce: [f32; Tier::COUNT],
    /// Share of each good's demand met lately, 0..1.
    pub supply: [f32; Good::COUNT],
    /// Goods made and used per minute lately (smoothed).
    pub made_per_min: [f32; Good::COUNT],
    pub used_per_min: [f32; Good::COUNT],
    pub harbours: u32,
    /// Warehouse room that items add (see `items::apply`).
    #[serde(default)]
    pub extra_room: f64,
}

impl IslandBooks {
    pub fn get(&self, g: Good) -> f64 {
        self.stock[g.index()]
    }

    pub fn add(&mut self, g: Good, n: f64) {
        let s = &mut self.stock[g.index()];
        *s = (*s + n).clamp(0.0, self.capacity.max(0.0));
    }

    /// Take `n` if there is that much.
    pub fn take(&mut self, g: Good, n: f64) -> bool {
        let s = &mut self.stock[g.index()];
        if *s + 1e-9 >= n {
            *s = (*s - n).max(0.0);
            true
        } else {
            false
        }
    }
}

/// The player's purse and every island's books.
#[derive(Resource, Clone, Debug, Serialize)]
pub struct Economy {
    pub coins: f64,
    pub islands: Vec<IslandBooks>,
    /// Game seconds per real second (0 = paused).
    pub speed: f32,
    /// Taxes minus upkeep per minute, smoothed.
    pub balance_per_min: f64,
    pub taxes_per_min: f64,
    pub upkeep_per_min: f64,
    /// Game seconds since the start.
    pub clock_s: f64,
    /// Coins a minute the buildings that make nothing cost to run (set by `build::refresh`).
    pub public_upkeep: f32,
    /// Batches the Ship chandler finished since `items::tick` last looked: each is a
    /// consumable for the shelf.
    pub chandlery: u32,
}

impl Default for Economy {
    fn default() -> Self {
        Self {
            coins: 12_000.0,
            islands: vec![IslandBooks::default(); island::islands().len()],
            speed: 1.0,
            balance_per_min: 0.0,
            taxes_per_min: 0.0,
            upkeep_per_min: 0.0,
            clock_s: 0.0,
            public_upkeep: 0.0,
            chandlery: 0,
        }
    }
}

/// Goods finished this step, waiting for carts (logistics drains it).
#[derive(Resource, Default, Debug)]
pub struct Shipments {
    /// (building, good, from the building to the warehouse = true; the other way = false).
    pub carts: Vec<(Entity, Good, bool)>,
}

/// A building's road route to its warehouse, set by logistics (None: not connected).
#[derive(Component, Clone, Debug, Default)]
pub struct RoadLink {
    pub path: Option<std::sync::Arc<Vec<DVec3>>>,
}

pub const HARBOUR_CAPACITY: f64 = 120.0;
const SMOOTH_S: f32 = 30.0;

/// One economy step of `dt` game seconds.
#[allow(clippy::type_complexity)]
pub fn step(
    dt: f32,
    econ: &mut Economy,
    houses: &mut Query<(&Building, &mut House, Option<&Boost>)>,
    producers: &mut Query<(
        Entity,
        &Building,
        &mut Producer,
        Option<&RoadLink>,
        Option<&Boost>,
    )>,
    shipments: &mut Shipments,
) {
    if dt <= 0.0 {
        return;
    }
    econ.clock_s += dt as f64;
    let n = econ.islands.len();
    let k = (dt / SMOOTH_S).min(1.0);
    for b in econ.islands.iter_mut() {
        b.population = [0.0; Tier::COUNT];
        b.jobs = [0.0; Tier::COUNT];
    }
    // What houses do without because an item provides it.
    let mut spared = vec![[0.0f64; Good::COUNT]; n];
    for (b, h, boost) in houses.iter() {
        econ.islands[b.island].population[h.tier.index()] += h.residents;
        let Some(boost) = boost else { continue };
        for g in h.tier.needs().0 {
            let less = if boost.provides.contains(g) {
                1.0
            } else {
                boost.eats_less(*g)
            };
            if less > 0.0 {
                spared[b.island][g.index()] +=
                    (h.residents * NEED_PER_PERSON_MIN * g.need_share() / 60.0 * dt * less) as f64;
            }
        }
    }
    for (_, b, p, _, boost) in producers.iter() {
        if p.paused {
            continue;
        }
        if let Some((tier, w)) = b.kind.def().workers {
            let tier = boost.and_then(|x| x.workers).unwrap_or(tier);
            econ.islands[b.island].jobs[tier.index()] +=
                w as f32 * (1.0 - boost.map_or(0.0, |x| x.hands));
        }
    }
    // Workers: higher tiers can't fill lower jobs in this round; each tier staffs its own.
    for b in econ.islands.iter_mut() {
        for t in 0..Tier::COUNT {
            b.workforce[t] = if b.jobs[t] <= 0.0 {
                1.0
            } else {
                (b.population[t] / b.jobs[t]).min(1.0)
            };
        }
    }
    // Consumption, island by island.
    let mut used = vec![[0.0f32; Good::COUNT]; n];
    for (i, b) in econ.islands.iter_mut().enumerate() {
        let mut demand = [0.0f64; Good::COUNT];
        for t in Tier::ALL {
            for g in t.needs().0 {
                demand[g.index()] +=
                    (b.population[t.index()] * NEED_PER_PERSON_MIN * g.need_share() / 60.0 * dt)
                        as f64;
            }
        }
        for g in Good::ALL {
            let d = (demand[g.index()] - spared[i][g.index()]).max(0.0);
            if d <= 0.0 {
                continue;
            }
            let have = b.stock[g.index()];
            let ratio = (have / d).min(1.0) as f32;
            let take = d.min(have);
            b.stock[g.index()] -= take;
            used[i][g.index()] = take as f32;
            let s = &mut b.supply[g.index()];
            *s += (ratio - *s) * k;
        }
    }
    // Houses: each met need brings its people and pays its coins; then they move up.
    let mut taxes = 0.0f64;
    for (b, mut h, boost) in houses.iter_mut() {
        let books = &econ.islands[b.island];
        let mut people = 0.0;
        let mut coins = 0.0;
        // Of the needs that bring people: how many there are and how well they are met.
        let (mut basic, mut basic_met) = (0.0, 0.0);
        // Of the home-made luxuries: the same.
        let (mut lux, mut lux_met) = (0.0, 0.0);
        let mut service = 0;
        for w in h.tier.wants() {
            let met = match w.need {
                Need::Good(g) if boost.is_some_and(|x| x.provides.contains(&g)) => 1.0,
                Need::Good(g) => books.supply[g.index()].min(1.0),
                Need::Service(_) => {
                    service += 1;
                    if h.services[service - 1] { 1.0 } else { 0.0 }
                }
            };
            people += w.people * met;
            coins += (w.coins + boost.map_or(0.0, |x| x.pay_for(w.need) + x.per_need)) * met;
            if w.people > 0.0 {
                basic += 1.0;
                basic_met += met;
            } else if let Need::Good(g) = w.need
                && !g.overseas()
            {
                lux += 1.0;
                lux_met += met;
            }
        }
        let share = basic_met / basic;
        // Lodgers an item finds room for come as the basic needs are met, and pay like
        // everyone else in the house.
        let base = h.tier.capacity();
        let lodgers = boost.map_or(0.0, |x| x.lodgers);
        let cap = base + lodgers;
        coins += boost.map_or(0.0, |x| x.flat_pay() + x.tier_pay(h.tier));
        let target = (people + lodgers * share).max(0.2 * base);
        let rate = 0.4 * dt;
        h.residents += (target - h.residents).clamp(-rate, rate);
        // A house that is not full pays for the people it has.
        h.income = coins * (h.residents / base).min(cap / base);
        taxes += (h.income / 60.0 * dt) as f64;
        let happy = if lux > 0.0 { lux_met / lux } else { 1.0 };
        let content = h.residents >= cap - 0.25 && share > 0.95 && happy > 0.8;
        h.content_s = if content { h.content_s + dt } else { 0.0 };
    }
    // Upgrades take materials from the house's island.
    for (b, mut h, boost) in houses.iter_mut() {
        if h.content_s < 20.0 {
            continue;
        }
        let Some((next, cost)) = h.tier.next() else {
            continue;
        };
        if next.wants_paving() && !h.paved {
            continue;
        }
        let books = &mut econ.islands[b.island];
        // Don't empty the workshops of the tier they leave.
        let t = h.tier.index();
        if books.jobs[t] > 0.0 && books.population[t] - h.tier.capacity() < books.jobs[t] * 1.5 {
            continue;
        }
        // (A Founder's portrait in reach: they move up without the building goods.)
        let free = boost.is_some_and(|x| x.rise);
        if free || cost.iter().all(|(g, c)| books.get(*g) >= *c as f64) {
            if !free {
                for (g, c) in cost {
                    books.take(*g, *c as f64);
                }
            }
            books.population[t] -= h.residents;
            h.tier = next;
            h.content_s = 0.0;
        }
    }
    // Workshops.
    let mut upkeep = (econ.public_upkeep / 60.0 * dt) as f64;
    let mut econ_chandlery = 0;
    let mut made = vec![[0.0f32; Good::COUNT]; n];
    let plain = Boost::default();
    for (e, b, mut p, link, boost) in producers.iter_mut() {
        let def = b.kind.def();
        let boost = boost.unwrap_or(&plain);
        upkeep +=
            ((def.upkeep * (1.0 - boost.upkeep_cut) - boost.thrift).max(0.0) / 60.0 * dt) as f64;
        // (The Ship chandler makes items, not goods: see `items::tick`.)
        let chandler = b.kind == Kind::Chandler;
        let out = def.output;
        if out.is_none() && !chandler {
            continue;
        }
        let books = &mut econ.islands[b.island];
        if p.paused {
            p.status = Status::Paused;
            p.efficiency = 0.0;
            continue;
        }
        let connected = link.is_some_and(|l| l.path.is_some());
        if !connected {
            p.status = Status::NoRoad;
            p.efficiency = 0.0;
            continue;
        }
        if p.held >= 4.0 + boost.stock {
            p.status = Status::Full;
            p.efficiency = 0.0;
            continue;
        }
        let staff = def.workers.map_or(1.0, |(t, _)| {
            books.workforce[boost.workers.unwrap_or(t).index()]
        });
        // Items can change what a batch takes.
        let input = if boost.free_input {
            None
        } else {
            def.input.map(|g| boost.input(g))
        };
        let input2 = if boost.spare2 || boost.free_input {
            None
        } else {
            def.input2.map(|g| boost.input(g))
        };
        if staff < 0.05 {
            p.status = Status::NoWorkers;
            p.efficiency = 0.0;
            continue;
        }
        let site = (p.site * (1.0 + boost.fields)).min(1.0);
        if site < 0.05 {
            p.status = match b.kind {
                Kind::Woodcutter => Status::NoTrees,
                Kind::TrapperLodge => Status::NoWilds,
                _ => Status::NoFields,
            };
            p.efficiency = 0.0;
            continue;
        }
        // An item that saves goods makes every so many batches free.
        if input.is_some() && !p.input_ready && boost.saves > 0.0 {
            p.saved += boost.saves;
            if p.saved >= 1.0 {
                p.saved -= 1.0;
                p.input_ready = true;
            }
        }
        if let Some(inp) = input
            && !p.input_ready
        {
            // Both goods of a batch or neither.
            let second = input2.is_none_or(|g| books.get(g) >= 1.0);
            if second && books.take(inp, 1.0) {
                p.input_ready = true;
                shipments.carts.push((e, inp, false));
                if let Some(g) = input2 {
                    books.take(g, 1.0);
                    shipments.carts.push((e, g, false));
                }
            } else {
                p.status = Status::NoInput;
                p.efficiency = 0.0;
                continue;
            }
        }
        p.efficiency = staff * site;
        p.status = if staff < 0.99 {
            Status::NoWorkers
        } else {
            Status::Working
        };
        p.progress += dt / def.cycle_s * p.efficiency * (1.0 + boost.faster);
        if p.progress >= 1.0 {
            p.progress -= 1.0;
            p.made += 1;
            p.input_ready = false;
            let Some(out) = out else {
                econ_chandlery += 1;
                continue;
            };
            // A spoilt batch is thrown away.
            if boost.spoil == 0 || !p.made.is_multiple_of(boost.spoil) {
                p.held += 1.0;
                made[b.island][out.index()] += 1.0;
            }
            // One more of its own good every nth batch.
            for nth in &boost.again {
                if p.made.is_multiple_of(*nth) {
                    p.held += 1.0;
                    made[b.island][out.index()] += 1.0;
                }
            }
            // The extra goods items bring, each every nth batch.
            for (g, nth) in &boost.bonus {
                if p.made.is_multiple_of(*nth) {
                    made[b.island][g.index()] += 1.0;
                    shipments.carts.push((e, *g, true));
                }
            }
        }
        let Some(out) = out else { continue };
        while p.held >= 1.0 {
            p.held -= 1.0;
            shipments.carts.push((e, out, true));
        }
    }
    econ.chandlery += econ_chandlery;
    // Warehouses.
    for b in econ.islands.iter_mut() {
        b.capacity = if b.harbours > 0 {
            b.harbours as f64 * HARBOUR_CAPACITY + b.extra_room
        } else {
            0.0
        };
    }
    for (i, b) in econ.islands.iter_mut().enumerate() {
        for g in 0..Good::COUNT {
            let per_min_made = made[i][g] / dt * 60.0;
            let per_min_used = used[i][g] / dt * 60.0;
            b.made_per_min[g] += (per_min_made - b.made_per_min[g]) * k;
            b.used_per_min[g] += (per_min_used - b.used_per_min[g]) * k;
        }
    }
    econ.coins += taxes - upkeep;
    let per_min = |x: f64| x / dt as f64 * 60.0;
    let kk = k as f64;
    econ.taxes_per_min += (per_min(taxes) - econ.taxes_per_min) * kk;
    econ.upkeep_per_min += (per_min(upkeep) - econ.upkeep_per_min) * kk;
    econ.balance_per_min = econ.taxes_per_min - econ.upkeep_per_min;
}

/// Runs [`step`] each fixed step at the game's speed.
#[allow(clippy::type_complexity)]
pub fn economy_system(
    time: Res<Time>,
    mut econ: ResMut<Economy>,
    mut houses: Query<(&Building, &mut House, Option<&Boost>)>,
    mut producers: Query<(
        Entity,
        &Building,
        &mut Producer,
        Option<&RoadLink>,
        Option<&Boost>,
    )>,
    mut shipments: ResMut<Shipments>,
) {
    let dt = time.delta_secs() * econ.speed;
    step(dt, &mut econ, &mut houses, &mut producers, &mut shipments);
}

/// Total people on an island.
pub fn population(books: &IslandBooks) -> f32 {
    books.population.iter().sum()
}
