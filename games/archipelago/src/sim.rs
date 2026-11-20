//! The economy: houses fill and pay, workshops turn goods into goods, each island keeps its
//! stock in its harbour's warehouse.
//!
//! It steps in fixed slices of game time ([`STEP_S`], [`Economy::speed`] times real time,
//! 0 pauses), reading only components and its own resource, so a headless test can run years
//! of it in a moment and a game plays the same at any speed.
//!
//! Houses: each need of a tier belongs to a category ([`Cat`]); a basic category is met when
//! the points of its needs, each times its supply, reach the tier's threshold. Needs unlock
//! as the tier's people on the island grow. Every met need brings its people, coins and joy;
//! happiness drifts toward a target and decides rent, goodwill, riots and moving up.

use engine::prelude::*;
use serde::Serialize;

use crate::defs::{
    Cat, Good, Kind, MAX_SERVICES, MAX_WANTS, NEED_PER_PERSON_MIN, Need, Tier, Want,
};
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
    /// On fire (see `incidents`).
    Burning,
    /// Its workers are out with a riot nearby.
    Striking,
    /// Its workers are sick.
    Sick,
    /// Burnt down: rebuild or clear it.
    Burnt,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Working => "Working",
            Status::NoInput => "Waiting for goods",
            Status::NoWorkers => "Not enough workers",
            Status::NoRoad => "No road to a harbour",
            Status::Full => "Store full",
            Status::NoTrees => "No trees in reach",
            Status::NoWilds => "No wild country in reach",
            Status::NoFields => "No room for fields",
            Status::Paused => "Paused",
            Status::Burning => "On fire",
            Status::Striking => "On strike: a riot nearby",
            Status::Sick => "Workers sick",
            Status::Burnt => "Burnt down: rebuild or clear it",
        }
    }
}

/// What an incident is doing to a building (see `incidents`); the economy reads only this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hit {
    #[default]
    Clear,
    Fire,
    Riot,
    Sick,
    /// Burnt down.
    Ruin,
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
    /// A fire, riot or sickness here (or a riot next door).
    pub hit: Hit,
    /// Seconds it has been idle (waiting for goods, no road, store full, no site): after
    /// [`LET_GO_S`] it asks for no workers, after [`IDLE_CHEAP_S`] it costs half.
    pub idle_s: f32,
    /// How far its crew is staffed up again after being let go, 0..1 (10 s to full).
    pub crew: f32,
    /// The player marked it first: its tier staffs it before the rest.
    pub first: bool,
    /// It paused itself while the company was in debt.
    pub auto_paused: bool,
}

/// Seconds idle before a works lets its workers go.
pub const LET_GO_S: f32 = 30.0;
/// Seconds idle before a works costs half its upkeep.
pub const IDLE_CHEAP_S: f32 = 60.0;
/// Seconds a works takes to staff up again.
pub const STAFF_UP_S: f32 = 10.0;
/// Seconds in debt before idle works pause themselves.
pub const DEBT_PAUSE_S: f64 = 120.0;

impl Producer {
    /// It asks its tier for workers (not paused, not let go).
    pub fn asks_workers(&self) -> bool {
        !self.paused && self.idle_s < LET_GO_S
    }

    /// Share of its upkeep it costs now.
    pub fn upkeep_share(&self) -> f32 {
        if self.paused {
            0.25
        } else if self.idle_s >= IDLE_CHEAP_S {
            0.5
        } else {
            1.0
        }
    }
}

/// How a house feels, from its happiness (0 to 100).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mood {
    /// Below 20: pays no rent, riots most, cannot move up.
    Angry,
    /// 20 to 39: fills at half speed, may riot.
    Unhappy,
    /// 40 to 59.
    #[default]
    Content,
    /// 60 to 79: pays goodwill.
    Happy,
    /// 80 and up: twice the goodwill, fills fastest.
    Euphoric,
}

impl Mood {
    pub const COUNT: usize = 5;
    pub const ALL: [Mood; Mood::COUNT] = [
        Mood::Angry,
        Mood::Unhappy,
        Mood::Content,
        Mood::Happy,
        Mood::Euphoric,
    ];

    pub fn of(happiness: f32) -> Mood {
        match happiness {
            h if h < 20.0 => Mood::Angry,
            h if h < 40.0 => Mood::Unhappy,
            h if h < 60.0 => Mood::Content,
            h if h < 80.0 => Mood::Happy,
            _ => Mood::Euphoric,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Mood::Angry => "Angry",
            Mood::Unhappy => "Unhappy",
            Mood::Content => "Content",
            Mood::Happy => "Happy",
            Mood::Euphoric => "Euphoric",
        }
    }
}

/// Why a house is not moving up a tier (the first that applies).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Wait {
    /// Ready, next in line on its island.
    #[default]
    None,
    /// Already the top tier.
    Top,
    /// A fire, riot or sickness in the house.
    Trouble,
    NotFull,
    /// A basic category below its threshold (or with no need unlocked yet).
    Short(Cat),
    Angry,
    NeedsPaving,
    NoMaterials,
    /// Ready, but moving up would leave the tier it leaves short of workers (the player's
    /// "move up all" skips it; a single order still may).
    WouldEmptyWorks,
}

/// Happiness the trouble at a house takes off: an incident in the house itself.
pub const TROUBLE_HOME: f32 = 25.0;
/// The same for an incident within [`TROUBLE_CELLS`].
pub const TROUBLE_NEAR: f32 = 15.0;
pub const TROUBLE_CELLS: i32 = 6;
/// Happiness a basic category takes off when it has nothing (less when partly met). Its
/// people and coins already go, so this is kept small: unmet basics alone never make a
/// house angry.
pub const SHORT_GLOOM: f32 = 10.0;
/// Happiness a smoky works within [`crate::defs::SMOKE_CELLS`] takes off.
pub const SMOKE_GLOOM: f32 = 10.0;
/// Building goods a house moving up on its own leaves in the warehouse, so moving up never
/// starves the builders (the player's button may take the last of them).
pub const BUILD_RESERVE: f64 = 10.0;
/// A ready house stays ready until a category is below this share of its threshold for
/// [`SLIP_S`] seconds.
pub const SLIP_SHARE: f32 = 0.9;
pub const SLIP_S: f32 = 30.0;

/// A house and its people.
#[derive(Component, Clone, Debug)]
pub struct House {
    pub tier: Tier,
    pub residents: f32,
    /// Seconds it has been ready to move up (0 while not ready).
    pub content_s: f32,
    /// Which of [`crate::build::SERVICES`] are in reach (in that order).
    pub services: [bool; MAX_SERVICES],
    /// Coins a minute it pays right now: rent, what each met need is worth, goodwill.
    pub income: f32,
    /// Of that: rent and goodwill.
    pub rent: f32,
    pub goodwill: f32,
    /// Its door is on a paved road.
    pub paved: bool,
    /// A fire, riot or sickness here.
    pub hit: Hit,
    /// 0 to 100, moving 1 point every 3 s toward `target`.
    pub happiness: f32,
    pub target: f32,
    /// Points of each basic category ([`Cat::BASIC`] order), each need times its supply.
    pub points: [f32; 3],
    /// Ready to move up a tier.
    pub ready: bool,
    /// Seconds a ready house has had a category below [`SLIP_SHARE`] of its threshold.
    pub slip_s: f32,
    pub wait: Wait,
    /// A smoky works stands within [`crate::defs::SMOKE_CELLS`] (set by `build::refresh`).
    pub smoke: bool,
    /// Happiness the incidents here or nearby take off.
    pub trouble: f32,
}

impl House {
    pub fn new(tier: Tier) -> Self {
        Self {
            tier,
            residents: 2.0,
            content_s: 0.0,
            services: [false; MAX_SERVICES],
            income: 0.0,
            rent: 0.0,
            goodwill: 0.0,
            paved: false,
            hit: Hit::Clear,
            happiness: 45.0,
            target: 45.0,
            points: [0.0; 3],
            ready: false,
            slip_s: 0.0,
            wait: Wait::NotFull,
            smoke: false,
            trouble: 0.0,
        }
    }

    pub fn mood(&self) -> Mood {
        Mood::of(self.happiness)
    }

    /// Content or better.
    pub fn is_content(&self) -> bool {
        self.mood() >= Mood::Content
    }

    /// Whether one of [`crate::build::SERVICES`] is in reach.
    pub fn has(&self, k: Kind) -> bool {
        crate::build::SERVICES
            .iter()
            .position(|s| *s == k)
            .is_some_and(|i| self.services[i])
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
    /// Goods per minute the houses ask for lately, met or not (smoothed).
    pub wanted_per_min: [f32; Good::COUNT],
    pub harbours: u32,
    /// Warehouse room that items add (see `items::apply`).
    #[serde(default)]
    pub extra_room: f64,
    /// Per tier, the bits of [`Tier::wants`] unlocked here (they stay unlocked).
    pub unlocked: [u32; Tier::COUNT],
    /// Goods the player stopped handing out here: nobody uses them and they give nothing.
    pub paused: [bool; Good::COUNT],
    /// Ready houses here move up on their own. Off unless the player turns it on: moving up
    /// is the player's order, so a town never loses the workers of a tier by surprise.
    pub auto_up: bool,
    /// Game time the next house here may move up on its own.
    pub next_up_s: f64,
    /// Jobs that idle works let go, per tier.
    pub let_go: [f32; Tier::COUNT],
    /// Share filled of the jobs of works marked first, and of the rest, per tier.
    pub staff_first: [f32; Tier::COUNT],
    pub staff_rest: [f32; Tier::COUNT],
    /// Houses per tier in each mood ([`Mood::ALL`] order), and ready ones.
    pub moods: [[u32; Mood::COUNT]; Tier::COUNT],
    pub ready: [u32; Tier::COUNT],
}

impl IslandBooks {
    pub fn get(&self, g: Good) -> f64 {
        self.stock[g.index()]
    }

    /// Whether the island asks its houses of `tier` for want `i` of [`Tier::wants`].
    pub fn asks(&self, tier: Tier, i: usize, w: &Want) -> bool {
        self.unlocked[tier.index()] & (1 << i) != 0
            && !matches!(w.need, Need::Good(g) if self.paused[g.index()])
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
    /// Coins a minute the company's ships and aircraft cost to keep (set by `shipyard::tick`).
    pub fleet_upkeep: f32,
    /// Game seconds the purse has been below zero (0 when it is not).
    pub debt_s: f64,
    /// The three biggest costs right now, biggest first.
    pub drains: Vec<Drain>,
    /// Works that paused themselves in debt since the start.
    pub auto_paused: u32,
    /// Game seconds waiting for the next fixed step.
    pub acc: f32,
}

/// One line of what costs the most: works of a kind on an island, running or idle.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Drain {
    pub kind: Kind,
    pub island: usize,
    pub count: u32,
    pub idle: bool,
    pub per_min: f32,
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
            fleet_upkeep: 0.0,
            debt_s: 0.0,
            drains: Vec::new(),
            auto_paused: 0,
            acc: 0.0,
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
/// Game seconds of one economy step.
pub const STEP_S: f32 = 0.25;
/// Happiness points a second a house moves toward its target.
pub const MOOD_RATE: f32 = 1.0 / 3.0;

/// One economy step of `dt` game seconds (the game uses [`STEP_S`]; tests may take more).
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
    let clock = econ.clock_s;
    let n = econ.islands.len();
    let k = (dt / SMOOTH_S).min(1.0);
    for b in econ.islands.iter_mut() {
        b.population = [0.0; Tier::COUNT];
        b.jobs = [0.0; Tier::COUNT];
        b.let_go = [0.0; Tier::COUNT];
        b.moods = [[0; Mood::COUNT]; Tier::COUNT];
        b.ready = [0; Tier::COUNT];
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
    // Incidents, for the houses around them: (island, cell).
    let mut trouble: Vec<(usize, IVec2)> = Vec::new();
    for (b, h, _) in houses.iter() {
        if h.hit != Hit::Clear {
            trouble.push((b.island, b.origin));
        }
    }
    // Jobs: works that cannot run let their workers go; works marked first are staffed first.
    let mut first = vec![[0.0f32; Tier::COUNT]; n];
    for (_, b, p, _, boost) in producers.iter() {
        if p.hit != Hit::Clear && p.hit != Hit::Ruin {
            trouble.push((b.island, b.origin));
        }
        if p.paused {
            continue;
        }
        if let Some((tier, w)) = b.kind.def().workers {
            let tier = boost.and_then(|x| x.workers).unwrap_or(tier);
            let w = w as f32 * (1.0 - boost.map_or(0.0, |x| x.hands));
            let books = &mut econ.islands[b.island];
            if !p.asks_workers() {
                books.let_go[tier.index()] += w;
                continue;
            }
            books.jobs[tier.index()] += w;
            if p.first {
                first[b.island][tier.index()] += w;
            }
        }
    }
    // Each tier staffs its own works.
    for (i, b) in econ.islands.iter_mut().enumerate() {
        for (t, &f) in first[i].iter().enumerate() {
            let (pop, jobs) = (b.population[t], b.jobs[t]);
            b.workforce[t] = if jobs <= 0.0 {
                1.0
            } else {
                (pop / jobs).min(1.0)
            };
            b.staff_first[t] = if f <= 0.0 { 1.0 } else { (pop / f).min(1.0) };
            b.staff_rest[t] = if jobs - f <= 0.0 {
                1.0
            } else {
                ((pop - f).max(0.0) / (jobs - f)).min(1.0)
            };
        }
        // Needs unlock as the tier grows, and stay unlocked.
        for t in Tier::ALL {
            for (j, w) in t.wants().iter().enumerate() {
                if b.population[t.index()] >= w.unlock {
                    b.unlocked[t.index()] |= 1 << j;
                }
            }
        }
    }
    // Consumption, island by island: only what is asked for.
    let mut used = vec![[0.0f32; Good::COUNT]; n];
    for (i, b) in econ.islands.iter_mut().enumerate() {
        let mut demand = [0.0f64; Good::COUNT];
        for t in Tier::ALL {
            for (j, w) in t.wants().iter().enumerate() {
                let Need::Good(g) = w.need else { continue };
                if b.asks(t, j, w) {
                    demand[g.index()] += (b.population[t.index()]
                        * NEED_PER_PERSON_MIN
                        * g.need_share()
                        / 60.0
                        * dt) as f64;
                }
            }
        }
        for g in Good::ALL {
            let d = (demand[g.index()] - spared[i][g.index()]).max(0.0);
            let w = &mut b.wanted_per_min[g.index()];
            *w += (d as f32 / dt * 60.0 - *w) * k;
            let have = b.stock[g.index()];
            // Nothing asked: the supply reads what the warehouse could hand out.
            let ratio = if d <= 0.0 {
                if have >= 1.0 { 1.0 } else { 0.0 }
            } else {
                (have / d).min(1.0) as f32
            };
            let take = d.min(have);
            b.stock[g.index()] -= take;
            used[i][g.index()] = take as f32;
            let s = &mut b.supply[g.index()];
            *s += (ratio - *s) * k;
        }
    }
    // Houses: each asked-for need brings its people, coins, joy and points.
    let mut taxes = 0.0f64;
    let mood_step = MOOD_RATE * dt;
    for (b, mut h, boost) in houses.iter_mut() {
        let books = &econ.islands[b.island];
        let tier = h.tier;
        h.trouble = match h.hit {
            Hit::Clear => {
                let near = trouble.iter().any(|(isle, c)| {
                    *isle == b.island
                        && (*c - b.origin).as_i64vec2().length_squared()
                            <= (TROUBLE_CELLS as i64).pow(2)
                });
                if near { TROUBLE_NEAR } else { 0.0 }
            }
            _ => TROUBLE_HOME,
        };
        let a = appraise(&h, books, boost);
        let mut coins = a.coins;
        let (short, slipping) = (a.short, a.slipping);
        let base = tier.capacity();
        let cap = a.size;
        let mut target = a.residents;
        h.target = a.target;
        h.happiness += (h.target - h.happiness).clamp(-mood_step, mood_step);
        h.points = a.points;
        let mood = h.mood();
        // Rent from every lived-in house that is not angry; goodwill from happy ones.
        let mut rent = if h.residents > 0.5 && mood != Mood::Angry {
            tier.rent()
        } else {
            0.0
        };
        let mut goodwill = match mood {
            Mood::Happy => tier.goodwill(),
            Mood::Euphoric => 2.0 * tier.goodwill(),
            _ => 0.0,
        };
        // A fire or riot pays nothing; the sick pay a little, and some leave or die.
        match h.hit {
            Hit::Clear => {}
            Hit::Fire | Hit::Ruin => {
                coins = 0.0;
                rent = 0.0;
                goodwill = 0.0;
                target = 0.0;
            }
            Hit::Riot => {
                coins = 0.0;
                rent = 0.0;
                goodwill = 0.0;
            }
            Hit::Sick => {
                coins *= 0.35;
                goodwill = 0.0;
                target *= 0.55;
            }
        }
        let speed = match mood {
            _ if h.hit != Hit::Clear => 3.0,
            Mood::Angry | Mood::Unhappy => 0.5,
            Mood::Euphoric => 1.5,
            _ => 1.0,
        };
        let up = 0.4 * speed * dt;
        let down = if h.hit == Hit::Clear { 0.4 } else { 1.2 } * dt;
        h.residents += (target - h.residents).clamp(-down, up);
        // A house that is not full pays for the people it has.
        h.rent = rent;
        h.goodwill = goodwill;
        h.income = coins * (h.residents / base).min(cap / base) + rent + goodwill;
        taxes += (h.income / 60.0 * dt) as f64;
        // Ready to move up: full, every basic category met, not angry, no trouble. Once
        // ready it stays so until a category has been well short for a while.
        let full = h.residents >= cap - 0.25;
        let calm = h.hit == Hit::Clear && h.happiness >= 20.0;
        if h.ready {
            h.slip_s = if slipping { h.slip_s + dt } else { 0.0 };
            if !calm || h.slip_s >= SLIP_S {
                h.ready = false;
            }
        } else {
            h.slip_s = 0.0;
            h.ready = full && short.is_none() && calm;
        }
        h.content_s = if h.ready { h.content_s + dt } else { 0.0 };
        h.wait = if tier.next().is_none() {
            Wait::Top
        } else if h.ready {
            Wait::None
        } else if h.hit != Hit::Clear {
            Wait::Trouble
        } else if !full {
            Wait::NotFull
        } else if let Some(c) = short {
            Wait::Short(c)
        } else {
            Wait::Angry
        };
    }
    // Ready houses wait for the player's order. Where the player turned moving up on its own
    // on: at most one house a second per island, the longest ready first, when the warehouse
    // holds the materials and the tier it leaves can still staff its works.
    let mut best: Vec<Option<(f32, usize)>> = vec![None; n];
    for (i, (b, mut h, boost)) in houses.iter_mut().enumerate() {
        let books = &econ.islands[b.island];
        if h.ready && h.tier.next().is_some() {
            h.wait = up_wait(&h, books, boost.is_some_and(|x| x.rise), false);
            if h.wait == Wait::None
                && books.auto_up
                && clock >= books.next_up_s
                && best[b.island].is_none_or(|(s, _)| h.content_s > s)
            {
                best[b.island] = Some((h.content_s, i));
            }
        }
        let books = &mut econ.islands[b.island];
        books.moods[h.tier.index()][h.mood().index()] += 1;
        if h.ready {
            books.ready[h.tier.index()] += 1;
        }
    }
    if best.iter().any(Option::is_some) {
        for (i, (b, mut h, boost)) in houses.iter_mut().enumerate() {
            if best[b.island].is_some_and(|(_, j)| j == i) {
                let books = &mut econ.islands[b.island];
                move_up(&mut h, books, boost.is_some_and(|x| x.rise));
                books.next_up_s = clock + 1.0;
            }
        }
    }
    // Workshops.
    let mut upkeep = ((econ.public_upkeep + econ.fleet_upkeep) / 60.0 * dt) as f64;
    let mut econ_chandlery = 0;
    let mut made = vec![[0.0f32; Good::COUNT]; n];
    let plain = Boost::default();
    let in_debt = econ.debt_s >= DEBT_PAUSE_S;
    let mut drains: Vec<Drain> = Vec::new();
    for (e, b, mut p, link, boost) in producers.iter_mut() {
        let def = b.kind.def();
        let boost = boost.unwrap_or(&plain);
        // A works that paused itself for want of work wakes once its goods are in: it
        // stopped to save its upkeep, not to keep the bread from the town.
        if p.auto_paused && p.paused && def.input.is_some() {
            let stock = &econ.islands[b.island].stock;
            if [def.input, def.input2]
                .into_iter()
                .flatten()
                .all(|g| stock[g.index()] >= 1.0)
            {
                p.paused = false;
                p.auto_paused = false;
                p.idle_s = 0.0;
            }
        }
        // In debt, works that have stood idle a while pause themselves.
        if in_debt && !p.paused && p.idle_s >= IDLE_CHEAP_S {
            p.paused = true;
            p.auto_paused = true;
            econ.auto_paused += 1;
        }
        if p.hit != Hit::Clear {
            p.status = match p.hit {
                Hit::Fire => Status::Burning,
                Hit::Riot => Status::Striking,
                Hit::Sick => Status::Sick,
                _ => Status::Burnt,
            };
            p.efficiency = 0.0;
            // A ruin costs nothing to run.
            if p.hit == Hit::Ruin {
                continue;
            }
        }
        let cost = (def.upkeep * (1.0 - boost.upkeep_cut) - boost.thrift).max(0.0)
            * if p.hit == Hit::Clear {
                p.upkeep_share()
            } else {
                1.0
            };
        upkeep += (cost / 60.0 * dt) as f64;
        if cost > 0.0 {
            let idle = p.paused || p.idle_s >= LET_GO_S;
            match drains
                .iter_mut()
                .find(|d| d.kind == b.kind && d.island == b.island && d.idle == idle)
            {
                Some(d) => {
                    d.count += 1;
                    d.per_min += cost;
                }
                None => drains.push(Drain {
                    kind: b.kind,
                    island: b.island,
                    count: 1,
                    idle,
                    per_min: cost,
                }),
            }
        }
        // (The Ship chandler makes items, not goods: see `items::tick`.)
        let chandler = b.kind == Kind::Chandler;
        let out = def.output;
        if (out.is_none() && !chandler) || p.hit != Hit::Clear {
            continue;
        }
        let books = &mut econ.islands[b.island];
        if p.paused {
            p.status = Status::Paused;
            p.efficiency = 0.0;
            continue;
        }
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
        let site = (p.site * (1.0 + boost.fields)).min(1.0);
        // Why it cannot run, whoever works there.
        let connected = link.is_some_and(|l| l.path.is_some());
        let idle = if !connected {
            Some(Status::NoRoad)
        } else if p.held >= 4.0 + boost.stock
            || out.is_some_and(|g| books.capacity > 0.0 && books.get(g) >= books.capacity - 0.5)
        {
            // (Its carts cannot unload, or the warehouse is full of what it makes: it stands,
            // lets its workers go and costs half until there is room, as a player would pause it.)
            Some(Status::Full)
        } else if site < 0.05 {
            Some(match b.kind {
                Kind::Woodcutter => Status::NoTrees,
                Kind::TrapperLodge => Status::NoWilds,
                _ => Status::NoFields,
            })
        } else if let Some(inp) = input
            && !p.input_ready
            && !(boost.saves > 0.0 && p.saved + boost.saves >= 1.0)
            && (books.get(inp) < 1.0 || input2.is_some_and(|g| books.get(g) < 1.0))
        {
            Some(Status::NoInput)
        } else {
            None
        };
        if let Some(status) = idle {
            p.status = status;
            p.efficiency = 0.0;
            p.idle_s += dt;
            if p.idle_s >= LET_GO_S {
                p.crew = 0.0;
            }
            continue;
        }
        // It can run: staff up (again).
        p.idle_s = 0.0;
        p.crew = (p.crew + dt / STAFF_UP_S).min(1.0);
        let staff = def.workers.map_or(1.0, |(t, _)| {
            let t = boost.workers.unwrap_or(t).index();
            if p.first {
                books.staff_first[t]
            } else {
                books.staff_rest[t]
            }
        }) * p.crew;
        if staff < 0.05 {
            p.status = Status::NoWorkers;
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
            // Both goods of a batch or neither (checked above).
            if books.take(inp, 1.0) {
                p.input_ready = true;
                shipments.carts.push((e, inp, false));
                if let Some(g) = input2 {
                    books.take(g, 1.0);
                    shipments.carts.push((e, g, false));
                }
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
    drains.sort_by(|a, b| b.per_min.total_cmp(&a.per_min));
    drains.truncate(3);
    econ.drains = drains;
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
    econ.debt_s = if econ.coins < 0.0 {
        econ.debt_s + dt as f64
    } else {
        0.0
    };
    let per_min = |x: f64| x / dt as f64 * 60.0;
    let kk = k as f64;
    econ.taxes_per_min += (per_min(taxes) - econ.taxes_per_min) * kk;
    econ.upkeep_per_min += (per_min(upkeep) - econ.upkeep_per_min) * kk;
    econ.balance_per_min = econ.taxes_per_min - econ.upkeep_per_min;
}

/// One need of a house as the economy sees it now.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct NeedLine {
    /// The island asks for it (unlocked and not paused).
    pub asked: bool,
    /// How well it is supplied, 0..1.
    pub met: f32,
    /// People it brings, coins a minute it pays a full house, joy it adds (each times `met`).
    pub people: f32,
    pub coins: f32,
    pub joy: f32,
}

/// Everything a house's numbers are made of, so the step and the house card add up alike.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Appraisal {
    /// One line per want of the tier, in [`Tier::wants`] order (the first `count`).
    pub lines: [NeedLine; MAX_WANTS],
    pub count: usize,
    /// People the met needs bring, and the residents it fills toward (with lodgers).
    pub people: f32,
    pub residents: f32,
    /// House size with lodgers.
    pub size: f32,
    /// Coins a minute of the met needs and items for a full house (before rent).
    pub coins: f32,
    /// Points of each basic category, and whether the island asks for any need of it.
    pub points: [f32; 3],
    pub asked: [bool; 3],
    /// The first basic category not met.
    pub short: Option<Cat>,
    /// A category is below [`SLIP_SHARE`] of its threshold.
    pub slipping: bool,
    /// Happiness: 40 plus `joy`, less `shortage`, `smoke`, `trouble` and `crowding`.
    pub joy: f32,
    pub shortage: f32,
    pub smoke: f32,
    pub trouble: f32,
    pub crowding: f32,
    pub target: f32,
}

/// Appraises a house against its island's books: what each need gives and the happiness it
/// is drifting toward. `h.trouble` must be current.
pub fn appraise(h: &House, books: &IslandBooks, boost: Option<&Boost>) -> Appraisal {
    let tier = h.tier;
    let mut a = Appraisal::default();
    for (j, w) in tier.wants().iter().enumerate().take(MAX_WANTS) {
        a.count = j + 1;
        if !books.asks(tier, j, w) {
            continue;
        }
        let met = match w.need {
            Need::Good(g) if boost.is_some_and(|x| x.provides.contains(&g)) => 1.0,
            Need::Good(g) => books.supply[g.index()].clamp(0.0, 1.0),
            Need::Service(s) => {
                if h.has(s) {
                    1.0
                } else {
                    0.0
                }
            }
        };
        let line = NeedLine {
            asked: true,
            met,
            people: w.people * met,
            coins: (w.coins + boost.map_or(0.0, |x| x.pay_for(w.need) + x.per_need)) * met,
            joy: w.joy * met,
        };
        a.lines[j] = line;
        a.people += line.people;
        a.coins += line.coins;
        a.joy += line.joy;
        if w.basic() {
            a.points[w.cat.index()] += w.points * met;
            a.asked[w.cat.index()] = true;
        }
    }
    // A basic category below its threshold weighs on the house, by how far it is short.
    for c in Cat::BASIC {
        let th = tier.threshold(c);
        let (p, asked) = (a.points[c.index()], a.asked[c.index()]);
        if !asked || p < th - 1e-4 {
            a.short = a.short.or(Some(c));
        }
        // (Within half a percent counts as met, so a full supply never shows a sliver.)
        if asked && p < th * 0.995 {
            a.shortage += SHORT_GLOOM * (1.0 - p / th);
        }
        if !asked || p < th * SLIP_SHARE {
            a.slipping = true;
        }
    }
    let base = tier.capacity();
    let lodgers = boost.map_or(0.0, |x| x.lodgers);
    a.size = base + lodgers;
    let fill = (a.people / base).min(1.0);
    a.coins += boost.map_or(0.0, |x| x.flat_pay() + x.tier_pay(tier));
    a.residents = (a.people.min(base) + lodgers * fill).max(0.2 * base);
    a.smoke = if h.smoke { SMOKE_GLOOM } else { 0.0 };
    a.trouble = h.trouble;
    a.crowding = ((books.population[tier.index()] - 400.0) / 200.0).clamp(0.0, 15.0);
    a.target = (40.0 + a.joy - a.shortage - a.smoke - a.trouble - a.crowding).clamp(0.0, 100.0);
    a
}

/// What stops a ready house moving up now (`Wait::None`: nothing). `now` is the player's
/// order for this one house: it ignores the job guard (the caller warns instead).
pub fn up_wait(h: &House, books: &IslandBooks, free: bool, now: bool) -> Wait {
    let Some((next, cost)) = h.tier.next() else {
        return Wait::Top;
    };
    if next.wants_paving() && !h.paved {
        return Wait::NeedsPaving;
    }
    // The tier it leaves keeps enough people for its works.
    let t = h.tier.index();
    // (Jobs of idle works count too: they want their people back when they can run.)
    let jobs = books.jobs[t] + books.let_go[t];
    if !now && jobs > 0.0 && books.population[t] - h.residents < jobs {
        return Wait::WouldEmptyWorks;
    }
    // (A Founder's portrait in reach: they move up without the building goods.)
    // (On its own a house leaves a reserve of building goods for the builders.)
    let keep = if now { 0.0 } else { BUILD_RESERVE };
    if !free && !cost.iter().all(|(g, c)| books.get(*g) >= *c as f64 + keep) {
        return Wait::NoMaterials;
    }
    Wait::None
}

/// Moves a house up a tier, paying the materials (none when `free`). The caller checks
/// [`up_wait`] first.
pub fn move_up(h: &mut House, books: &mut IslandBooks, free: bool) {
    let Some((next, cost)) = h.tier.next() else {
        return;
    };
    if !free {
        for (g, c) in cost {
            books.take(*g, *c as f64);
        }
    }
    books.population[h.tier.index()] -= h.residents;
    books.population[next.index()] += h.residents;
    h.tier = next;
    h.ready = false;
    h.content_s = 0.0;
    h.slip_s = 0.0;
}

/// Moves a house down a tier: people beyond the smaller house move out, nothing is refunded.
/// This is how a player gets workers of the tier below back.
pub fn move_down(h: &mut House) -> bool {
    let Some(prev) = h.tier.prev() else {
        return false;
    };
    h.tier = prev;
    h.residents = h.residents.min(prev.capacity());
    h.ready = false;
    h.content_s = 0.0;
    h.slip_s = 0.0;
    true
}

/// Runs [`step`] in fixed slices of [`STEP_S`] game seconds at the game's speed, so the
/// outcome does not hang on the frame rate or the speed.
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
    // (A frame that stalls never asks for more than 30 s of catching up at once.)
    econ.acc = (econ.acc + time.delta_secs() * econ.speed).min(30.0);
    while econ.acc >= STEP_S {
        econ.acc -= STEP_S;
        step(
            STEP_S,
            &mut econ,
            &mut houses,
            &mut producers,
            &mut shipments,
        );
    }
}

/// Total people on an island.
pub fn population(books: &IslandBooks) -> f32 {
    books.population.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::town::tests::bare_world;

    fn stop_wool(w: &mut World) -> usize {
        let mut isle = 0;
        for (b, mut p) in w.query::<(&Building, &mut Producer)>().iter_mut(w) {
            if b.kind == Kind::SheepFarm {
                p.paused = true;
            }
            if b.kind == Kind::Weaver {
                isle = b.island;
            }
        }
        for b in w.resource_mut::<Economy>().islands.iter_mut() {
            b.stock[Good::Wool.index()] = 0.0;
        }
        isle
    }

    fn weavers(w: &mut World) -> Vec<Producer> {
        w.query::<(&Building, &Producer)>()
            .iter(w)
            .filter(|(b, _)| b.kind == Kind::Weaver)
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// A works waiting for goods lets its workers go after 30 s, costs half after 60 s, and
    /// staffs up again once the goods come back.
    #[test]
    fn idle_works_let_their_workers_go_and_cost_less() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 60.0);
        let isle = stop_wool(&mut w);
        crate::town::prewarm(&mut w, 45.0);
        let ws = weavers(&mut w);
        assert!(!ws.is_empty());
        assert!(
            ws.iter()
                .all(|p| p.status == Status::NoInput && !p.asks_workers())
        );
        assert!(w.resource::<Economy>().islands[isle].let_go[0] > 0.0);
        assert!(ws.iter().all(|p| p.upkeep_share() == 1.0));
        crate::town::prewarm(&mut w, 30.0);
        assert!(weavers(&mut w).iter().all(|p| p.upkeep_share() == 0.5));
        // Wool again: they ask for workers and are back at work within the staffing time.
        for b in w.resource_mut::<Economy>().islands.iter_mut() {
            b.stock[Good::Wool.index()] = 50.0;
        }
        crate::town::prewarm(&mut w, STAFF_UP_S + 2.0);
        let ws = weavers(&mut w);
        assert!(
            ws.iter().all(|p| p.asks_workers() && p.crew > 0.99),
            "{ws:?}"
        );
    }

    /// A works whose warehouse is full of what it makes stands, lets its hands go and costs
    /// half, instead of paying full upkeep to throw its goods away.
    #[test]
    fn a_full_warehouse_stands_its_works_down() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 30.0);
        let home = w
            .query::<&Building>()
            .iter(&w)
            .find(|b| b.kind == Kind::Sawmill)
            .map_or(0, |b| b.island);
        let fill = |w: &mut World| {
            let books = &mut w.resource_mut::<Economy>().islands[home];
            let cap = books.capacity;
            books.add(Good::Planks, cap);
        };
        for _ in 0..90 {
            fill(&mut w);
            crate::town::prewarm(&mut w, 1.0);
        }
        let mills: Vec<Producer> = w
            .query::<(&Building, &Producer)>()
            .iter(&w)
            .filter(|(b, _)| b.kind == Kind::Sawmill && b.island == home)
            .map(|(_, p)| p.clone())
            .collect();
        assert!(!mills.is_empty());
        for p in &mills {
            assert_eq!(p.status, Status::Full);
            assert_eq!(p.upkeep_share(), 0.5);
        }
    }

    /// In debt for two minutes, works idle for a minute pause themselves.
    #[test]
    fn debt_pauses_idle_works() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 30.0);
        stop_wool(&mut w);
        w.resource_mut::<Economy>().coins = -1.0e7;
        crate::town::prewarm(&mut w, 200.0);
        let econ = w.resource::<Economy>();
        assert!(econ.debt_s >= DEBT_PAUSE_S);
        assert!(econ.auto_paused > 0);
        assert!(!econ.drains.is_empty());
        assert!(weavers(&mut w).iter().all(|p| p.paused && p.auto_paused));
        // Wool comes in: the weavers wake by themselves, still in debt.
        let home = w
            .query::<&Building>()
            .iter(&w)
            .find(|b| b.kind == Kind::Weaver)
            .map_or(0, |b| b.island);
        w.resource_mut::<Economy>().islands[home].add(Good::Wool, 20.0);
        crate::town::prewarm(&mut w, 1.0);
        assert!(weavers(&mut w).iter().any(|p| !p.paused));
    }

    /// A ready house does not stop being ready on a short dip in supply.
    #[test]
    fn ready_houses_ride_out_a_dip() {
        let mut w = bare_world();
        crate::town::build_start(&mut w);
        crate::town::prewarm(&mut w, 300.0);
        let ready = |w: &mut World| {
            w.query::<&House>()
                .iter(w)
                .filter(|h| h.ready && h.tier == Tier::Fisherfolk)
                .count()
        };
        let before = ready(&mut w);
        if before == 0 {
            return; // (Nothing to test in a start whose Fisherfolk all moved up.)
        }
        // Fish runs out for 10 s: below 90 % but for less than 30 s.
        for b in w.resource_mut::<Economy>().islands.iter_mut() {
            b.supply[Good::Fish.index()] = 0.5;
        }
        crate::town::prewarm(&mut w, 1.0);
        assert_eq!(ready(&mut w), before);
    }

    #[test]
    fn moods_follow_happiness() {
        assert_eq!(Mood::of(0.0), Mood::Angry);
        assert_eq!(Mood::of(19.9), Mood::Angry);
        assert_eq!(Mood::of(45.0), Mood::Content);
        assert_eq!(Mood::of(80.0), Mood::Euphoric);
        let mut h = House::new(Tier::Labourers);
        h.residents = 20.0;
        assert!(move_down(&mut h));
        assert_eq!(h.tier, Tier::Fisherfolk);
        assert_eq!(h.residents, 10.0);
        assert!(!move_down(&mut h));
    }
}
