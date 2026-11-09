//! The economy: houses fill and pay, workshops turn goods into goods, each island keeps its
//! stock in its harbour's warehouse.
//!
//! It steps in game seconds ([`Economy::speed`] times real time, 0 pauses), reading only
//! components and its own resource, so a headless test can run years of it in a moment.

use engine::prelude::*;
use serde::Serialize;

use crate::defs::{Good, Kind, NEED_PER_PERSON_MIN, Tier};
use crate::island;

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
}

/// A house and its people.
#[derive(Component, Clone, Debug)]
pub struct House {
    pub tier: Tier,
    pub residents: f32,
    /// Seconds its needs have all been met while full (it moves up a tier after a while).
    pub content_s: f32,
    /// Which of its tier's services are in reach.
    pub services: [bool; 2],
}

impl House {
    pub fn new(tier: Tier) -> Self {
        Self {
            tier,
            residents: 2.0,
            content_s: 0.0,
            services: [false; 2],
        }
    }
}

/// One island's books.
#[derive(Clone, Debug, Default, Serialize)]
pub struct IslandBooks {
    pub stock: [f64; 12],
    /// Warehouse room for each good.
    pub capacity: f64,
    pub population: [f32; 2],
    /// Workers asked for by its workshops, per tier.
    pub jobs: [f32; 2],
    /// Share of those jobs filled, per tier.
    pub workforce: [f32; 2],
    /// Share of each good's demand met lately, 0..1.
    pub supply: [f32; 12],
    /// Goods made and used per minute lately (smoothed).
    pub made_per_min: [f32; 12],
    pub used_per_min: [f32; 12],
    pub harbours: u32,
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
    houses: &mut Query<(&Building, &mut House)>,
    producers: &mut Query<(Entity, &Building, &mut Producer, Option<&RoadLink>)>,
    shipments: &mut Shipments,
) {
    if dt <= 0.0 {
        return;
    }
    econ.clock_s += dt as f64;
    let n = econ.islands.len();
    let k = (dt / SMOOTH_S).min(1.0);
    for b in econ.islands.iter_mut() {
        b.population = [0.0; 2];
        b.jobs = [0.0; 2];
    }
    for (b, h) in houses.iter() {
        econ.islands[b.island].population[h.tier.index()] += h.residents;
    }
    for (_, b, p, _) in producers.iter() {
        if p.paused {
            continue;
        }
        if let Some((tier, w)) = b.kind.def().workers {
            econ.islands[b.island].jobs[tier.index()] += w as f32;
        }
    }
    // Workers: higher tiers can't fill lower jobs in this round; each tier staffs its own.
    for b in econ.islands.iter_mut() {
        for t in 0..2 {
            b.workforce[t] = if b.jobs[t] <= 0.0 {
                1.0
            } else {
                (b.population[t] / b.jobs[t]).min(1.0)
            };
        }
    }
    // Consumption, island by island.
    let mut used = vec![[0.0f32; 12]; n];
    for (i, b) in econ.islands.iter_mut().enumerate() {
        let mut demand = [0.0f64; 12];
        for t in Tier::ALL {
            for g in t.needs().0 {
                demand[g.index()] +=
                    (b.population[t.index()] * NEED_PER_PERSON_MIN / 60.0 * dt) as f64;
            }
        }
        for g in Good::ALL {
            let d = demand[g.index()];
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
    // Houses: fill toward what their needs allow, pay taxes, move up.
    let mut taxes = 0.0f64;
    for (b, mut h) in houses.iter_mut() {
        let books = &econ.islands[b.island];
        let (needs, basic) = h.tier.needs();
        let services = h.tier.services();
        let mut met = 0.0;
        let total = (basic + services.len()) as f32;
        for g in &needs[..basic] {
            met += books.supply[g.index()].min(1.0);
        }
        for (s, _) in services.iter().enumerate() {
            if h.services[s] {
                met += 1.0;
            }
        }
        let share = met / total;
        let cap = h.tier.capacity();
        let target = cap * (0.2 + 0.8 * share);
        let rate = 0.4 * dt;
        h.residents += (target - h.residents).clamp(-rate, rate);
        let lux: Vec<f32> = needs[basic..]
            .iter()
            .map(|g| books.supply[g.index()])
            .collect();
        let happy = if lux.is_empty() {
            1.0
        } else {
            lux.iter().sum::<f32>() / lux.len() as f32
        };
        taxes += (h.residents * h.tier.tax() * (0.6 + 0.4 * happy) / 60.0 * dt) as f64;
        let content = h.residents >= cap - 0.25 && share > 0.95 && happy > 0.8;
        h.content_s = if content { h.content_s + dt } else { 0.0 };
    }
    // Upgrades take materials from the house's island.
    for (b, mut h) in houses.iter_mut() {
        if h.content_s < 20.0 {
            continue;
        }
        let Some((next, cost)) = h.tier.next() else {
            continue;
        };
        let books = &mut econ.islands[b.island];
        // Don't empty the workshops of the tier they leave.
        let t = h.tier.index();
        if books.jobs[t] > 0.0 && books.population[t] - h.tier.capacity() < books.jobs[t] * 1.5 {
            continue;
        }
        if cost.iter().all(|(g, c)| books.get(*g) >= *c as f64) {
            for (g, c) in cost {
                books.take(*g, *c as f64);
            }
            books.population[t] -= h.residents;
            h.tier = next;
            h.content_s = 0.0;
        }
    }
    // Workshops.
    let mut upkeep = 0.0f64;
    let mut made = vec![[0.0f32; 12]; n];
    for (e, b, mut p, link) in producers.iter_mut() {
        let def = b.kind.def();
        upkeep += (def.upkeep / 60.0 * dt) as f64;
        let Some(out) = def.output else { continue };
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
        if p.held >= 4.0 {
            p.status = Status::Full;
            p.efficiency = 0.0;
            continue;
        }
        let staff = def.workers.map_or(1.0, |(t, _)| books.workforce[t.index()]);
        if staff < 0.05 {
            p.status = Status::NoWorkers;
            p.efficiency = 0.0;
            continue;
        }
        if p.site < 0.05 {
            p.status = if b.kind == Kind::Woodcutter {
                Status::NoTrees
            } else {
                Status::NoFields
            };
            p.efficiency = 0.0;
            continue;
        }
        if let Some(inp) = def.input
            && !p.input_ready
        {
            if books.take(inp, 1.0) {
                p.input_ready = true;
                shipments.carts.push((e, inp, false));
            } else {
                p.status = Status::NoInput;
                p.efficiency = 0.0;
                continue;
            }
        }
        p.efficiency = staff * p.site;
        p.status = if staff < 0.99 {
            Status::NoWorkers
        } else {
            Status::Working
        };
        p.progress += dt / def.cycle_s * p.efficiency;
        if p.progress >= 1.0 {
            p.progress -= 1.0;
            p.held += 1.0;
            p.made += 1;
            p.input_ready = false;
            made[b.island][out.index()] += 1.0;
        }
        while p.held >= 1.0 {
            p.held -= 1.0;
            shipments.carts.push((e, out, true));
        }
    }
    // Warehouses.
    for b in econ.islands.iter_mut() {
        b.capacity = b.harbours as f64 * HARBOUR_CAPACITY;
    }
    for (i, b) in econ.islands.iter_mut().enumerate() {
        for g in 0..12 {
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
    mut houses: Query<(&Building, &mut House)>,
    mut producers: Query<(Entity, &Building, &mut Producer, Option<&RoadLink>)>,
    mut shipments: ResMut<Shipments>,
) {
    let dt = time.delta_secs() * econ.speed;
    step(dt, &mut econ, &mut houses, &mut producers, &mut shipments);
}

/// Total people on an island.
pub fn population(books: &IslandBooks) -> f32 {
    books.population.iter().sum()
}
