//! What can be built and what flows between buildings: goods, population tiers and the
//! building table. Plain data, so the economy, the build bar, the API and the tests all read
//! the same numbers.

use serde::{Deserialize, Serialize};

/// A good stored in an island's warehouses and carried by carts, ships and airships.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Good {
    Fish,
    Logs,
    Planks,
    Wool,
    Clothes,
    Grain,
    Flour,
    Bread,
    Hops,
    Ale,
    Clay,
    Bricks,
    // Goods of the far regions (added after the home goods, so those keep their numbers).
    Cane,
    Rum,
    Dates,
    Salt,
    Furs,
    Oil,
    // Goods of the manufactories, most of them made from two regions' produce.
    Glass,
    SaltFish,
    DateCakes,
    Coats,
    Lamps,
}

impl Good {
    /// How many goods there are.
    pub const COUNT: usize = 23;

    pub const ALL: [Good; Good::COUNT] = [
        Good::Fish,
        Good::Logs,
        Good::Planks,
        Good::Wool,
        Good::Clothes,
        Good::Grain,
        Good::Flour,
        Good::Bread,
        Good::Hops,
        Good::Ale,
        Good::Clay,
        Good::Bricks,
        Good::Cane,
        Good::Rum,
        Good::Dates,
        Good::Salt,
        Good::Furs,
        Good::Oil,
        Good::Glass,
        Good::SaltFish,
        Good::DateCakes,
        Good::Coats,
        Good::Lamps,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Good::Fish => "Fish",
            Good::Logs => "Logs",
            Good::Planks => "Planks",
            Good::Wool => "Wool",
            Good::Clothes => "Clothes",
            Good::Grain => "Grain",
            Good::Flour => "Flour",
            Good::Bread => "Bread",
            Good::Hops => "Hops",
            Good::Ale => "Ale",
            Good::Clay => "Clay",
            Good::Bricks => "Bricks",
            Good::Cane => "Sugar cane",
            Good::Rum => "Rum",
            Good::Dates => "Dates",
            Good::Salt => "Salt",
            Good::Furs => "Furs",
            Good::Oil => "Lamp oil",
            Good::Glass => "Glass",
            Good::SaltFish => "Salt fish",
            Good::DateCakes => "Date cakes",
            Good::Coats => "Coats",
            Good::Lamps => "Lamps",
        }
    }

    /// The `ui` icon that stands for it.
    pub fn icon(self) -> &'static str {
        match self {
            Good::Fish => "fish",
            Good::Logs => "isle_logs",
            Good::Planks => "isle_planks",
            Good::Wool => "isle_wool",
            Good::Clothes => "isle_clothes",
            Good::Grain => "isle_grain",
            Good::Flour => "isle_flour",
            Good::Bread => "bread",
            Good::Hops => "isle_hops",
            Good::Ale => "isle_ale",
            Good::Clay => "isle_clay",
            Good::Bricks => "isle_bricks",
            Good::Cane => "isle_cane",
            Good::Rum => "isle_rum",
            Good::Dates => "isle_dates",
            Good::Salt => "isle_salt",
            Good::Furs => "isle_furs",
            Good::Oil => "isle_oil",
            Good::Glass => "isle_glass",
            Good::SaltFish => "isle_saltfish",
            Good::DateCakes => "isle_datecakes",
            Good::Coats => "isle_coats",
            Good::Lamps => "isle_lamps",
        }
    }

    /// Comes from one of the far regions: a treat at home, used a quarter as fast as bread.
    pub fn overseas(self) -> bool {
        matches!(
            self,
            Good::Rum | Good::Dates | Good::Salt | Good::Furs | Good::Oil
        )
    }

    /// How much of it a person who wants it uses, against a staple.
    pub fn need_share(self) -> f32 {
        match self {
            g if g.overseas() => 0.25,
            Good::SaltFish | Good::Coats | Good::DateCakes => 0.25,
            Good::Lamps => 0.125,
            _ => 1.0,
        }
    }

    pub fn from_name(s: &str) -> Option<Good> {
        Good::ALL
            .into_iter()
            .find(|g| g.name().eq_ignore_ascii_case(s))
    }
}

/// What a house wants: a good from the island's warehouse, or a public building in reach.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Need {
    Good(Good),
    Service(Kind),
}

/// The group a need counts toward. A basic category is met when the points of its needs,
/// each times how well it is supplied, reach the tier's threshold; luxuries only pay and
/// please.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Cat {
    Food,
    Clothing,
    Square,
    Luxury,
}

impl Cat {
    /// The categories that bring people and decide moving up.
    pub const BASIC: [Cat; 3] = [Cat::Food, Cat::Clothing, Cat::Square];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Cat::Food => "Food",
            Cat::Clothing => "Clothing",
            Cat::Square => "Square",
            Cat::Luxury => "Luxury",
        }
    }
}

/// One need of a tier, with what meeting it is worth: plain numbers, never a percentage.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub need: Need,
    pub cat: Cat,
    /// Points toward its category's threshold (0 for a luxury).
    pub points: f32,
    /// People it brings into the house (0: a luxury, nobody leaves without it).
    pub people: f32,
    /// Coins a minute a full house pays while it is met.
    pub coins: f32,
    /// Happiness it adds while met.
    pub joy: f32,
    /// People of the tier the island must have before the need is asked for.
    pub unlock: f32,
}

impl Want {
    pub fn basic(&self) -> bool {
        self.cat != Cat::Luxury
    }
}

/// A basic need: category, points, people, coins, joy, unlock.
const fn good(
    g: Good,
    cat: Cat,
    points: f32,
    people: f32,
    coins: f32,
    joy: f32,
    unlock: f32,
) -> Want {
    Want {
        need: Need::Good(g),
        cat,
        points,
        people,
        coins,
        joy,
        unlock,
    }
}

const fn service(k: Kind, points: f32, people: f32, coins: f32, joy: f32, unlock: f32) -> Want {
    Want {
        need: Need::Service(k),
        cat: Cat::Square,
        points,
        people,
        coins,
        joy,
        unlock,
    }
}

/// A luxury: coins, joy, unlock.
const fn luxury(need: Need, coins: f32, joy: f32, unlock: f32) -> Want {
    Want {
        need,
        cat: Cat::Luxury,
        points: 0.0,
        people: 0.0,
        coins,
        joy,
        unlock,
    }
}

/// Who lives in a house and works in the buildings that ask for them.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Default, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Fisherfolk,
    Labourers,
    Burghers,
}

/// The most services a tier asks for (the size of [`crate::sim::House::services`]).
pub const MAX_SERVICES: usize = 4;

/// The most needs a tier has (bits of [`crate::sim::IslandBooks::unlocked`]).
pub const MAX_WANTS: usize = 16;

use Cat::{Clothing, Food};

impl Tier {
    /// How many tiers there are.
    pub const COUNT: usize = 3;

    pub const ALL: [Tier; Tier::COUNT] = [Tier::Fisherfolk, Tier::Labourers, Tier::Burghers];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Fisherfolk => "Fisherfolk",
            Tier::Labourers => "Labourers",
            Tier::Burghers => "Burghers",
        }
    }

    /// Everything the tier wants: basic needs first (in category order), then luxuries.
    pub fn wants(self) -> &'static [Want] {
        use Good as G;
        use Kind as K;
        match self {
            Tier::Fisherfolk => {
                const W: &[Want] = &[
                    good(G::Fish, Food, 1.0, 4.0, 1.2, 0.0, 0.0),
                    good(G::Clothes, Clothing, 1.0, 3.0, 1.0, 0.0, 60.0),
                    service(K::Market, 1.0, 3.0, 0.6, 5.0, 0.0),
                    luxury(Need::Service(K::Chapel), 0.4, 15.0, 100.0),
                    luxury(Need::Good(G::Ale), 1.0, 20.0, 150.0),
                ];
                W
            }
            Tier::Labourers => {
                const W: &[Want] = &[
                    good(G::Fish, Food, 1.0, 4.0, 1.2, 0.0, 0.0),
                    good(G::Bread, Food, 2.0, 6.0, 2.2, 5.0, 0.0),
                    good(G::SaltFish, Food, 2.0, 5.0, 2.0, 10.0, 300.0),
                    good(G::Clothes, Clothing, 1.0, 4.0, 1.4, 0.0, 0.0),
                    good(G::Coats, Clothing, 2.0, 4.0, 2.6, 10.0, 400.0),
                    service(K::Market, 1.0, 3.0, 0.8, 5.0, 0.0),
                    service(K::Chapel, 1.0, 3.0, 0.8, 10.0, 50.0),
                    luxury(Need::Good(G::Ale), 3.0, 20.0, 100.0),
                    luxury(Need::Good(G::Salt), 1.0, 8.0, 200.0),
                    luxury(Need::Good(G::Rum), 2.0, 15.0, 300.0),
                    luxury(Need::Good(G::Dates), 1.4, 10.0, 300.0),
                    luxury(Need::Good(G::Oil), 1.8, 10.0, 400.0),
                    luxury(Need::Good(G::Furs), 2.4, 10.0, 500.0),
                ];
                W
            }
            Tier::Burghers => {
                const W: &[Want] = &[
                    good(G::Bread, Food, 1.0, 5.0, 3.0, 0.0, 0.0),
                    good(G::SaltFish, Food, 2.0, 5.0, 2.6, 5.0, 0.0),
                    good(G::DateCakes, Food, 2.0, 5.0, 3.4, 10.0, 200.0),
                    good(G::Clothes, Clothing, 1.0, 2.0, 1.2, 0.0, 0.0),
                    good(G::Coats, Clothing, 2.0, 6.0, 3.2, 10.0, 0.0),
                    service(K::Market, 1.0, 3.0, 1.0, 5.0, 0.0),
                    service(K::Chapel, 1.0, 3.0, 1.0, 5.0, 0.0),
                    service(K::School, 1.0, 4.0, 1.6, 5.0, 100.0),
                    service(K::Tavern, 1.0, 4.0, 1.6, 15.0, 0.0),
                    luxury(Need::Good(G::Ale), 3.0, 15.0, 0.0),
                    luxury(Need::Good(G::Rum), 3.0, 15.0, 150.0),
                    luxury(Need::Good(G::Lamps), 4.0, 20.0, 300.0),
                ];
                W
            }
        }
    }

    /// Points a basic category needs to count as met (0 for luxuries).
    pub fn threshold(self, cat: Cat) -> f32 {
        let t = match self {
            Tier::Fisherfolk => [1.0, 1.0, 1.0],
            Tier::Labourers => [2.0, 1.0, 2.0],
            Tier::Burghers => [3.0, 2.0, 3.0],
        };
        if cat == Cat::Luxury {
            0.0
        } else {
            t[cat.index()]
        }
    }

    /// People in a full house of this tier.
    pub fn capacity(self) -> f32 {
        [10.0, 20.0, 30.0][self.index()]
    }

    /// Coins a minute every lived-in house pays (none while Angry).
    pub fn rent(self) -> f32 {
        [0.3, 0.6, 1.0][self.index()]
    }

    /// Coins a minute a Happy house adds (twice as much when Euphoric).
    pub fn goodwill(self) -> f32 {
        [0.3, 0.8, 1.5][self.index()]
    }

    /// Coins a minute a full, Euphoric house pays with every need met.
    pub fn full_income(self) -> f32 {
        self.rent() + 2.0 * self.goodwill() + self.wants().iter().map(|w| w.coins).sum::<f32>()
    }

    /// Goods the tier consumes, in the order of [`Tier::wants`]. The first `basic` of them
    /// bring people into the house; the rest are luxuries that only pay and please.
    pub fn needs(self) -> (&'static [Good], usize) {
        match self {
            Tier::Fisherfolk => (&[Good::Fish, Good::Clothes, Good::Ale], 2),
            Tier::Labourers => (
                &[
                    Good::Fish,
                    Good::Bread,
                    Good::SaltFish,
                    Good::Clothes,
                    Good::Coats,
                    Good::Ale,
                    Good::Salt,
                    Good::Rum,
                    Good::Dates,
                    Good::Oil,
                    Good::Furs,
                ],
                5,
            ),
            Tier::Burghers => (
                &[
                    Good::Bread,
                    Good::SaltFish,
                    Good::DateCakes,
                    Good::Clothes,
                    Good::Coats,
                    Good::Ale,
                    Good::Rum,
                    Good::Lamps,
                ],
                5,
            ),
        }
    }

    /// Buildings that must be in reach of the house, in the order of [`Tier::wants`]. Each
    /// list is a prefix of `build::SERVICES`, so its positions index `House::services`.
    pub fn services(self) -> &'static [Kind] {
        match self {
            Tier::Fisherfolk => &[Kind::Market, Kind::Chapel],
            Tier::Labourers => &[Kind::Market, Kind::Chapel],
            Tier::Burghers => &[Kind::Market, Kind::Chapel, Kind::School, Kind::Tavern],
        }
    }

    /// The tier below (a house moved down goes back to it).
    pub fn prev(self) -> Option<Tier> {
        match self {
            Tier::Fisherfolk => None,
            Tier::Labourers => Some(Tier::Fisherfolk),
            Tier::Burghers => Some(Tier::Labourers),
        }
    }

    /// What the next tier is and what moving up costs.
    pub fn next(self) -> Option<(Tier, &'static [(Good, f32)])> {
        match self {
            Tier::Fisherfolk => {
                // Timber only: bricks are made by Labourers, so a new settlement can climb.
                Some((Tier::Labourers, &[(Good::Planks, 3.0)]))
            }
            Tier::Labourers => Some((
                Tier::Burghers,
                &[(Good::Planks, 3.0), (Good::Bricks, 4.0), (Good::Glass, 2.0)],
            )),
            Tier::Burghers => None,
        }
    }

    /// The house must stand on a paved road to be of this tier.
    pub fn wants_paving(self) -> bool {
        self == Tier::Burghers
    }
}

/// How far (cells) a smoky works spoils the houses around it.
pub const SMOKE_CELLS: f32 = 3.0;

/// Goods each person eats or wears per minute of a need (1/240: one fish every four minutes).
pub const NEED_PER_PERSON_MIN: f32 = 1.0 / 240.0;

/// How much faster carts drive on a paved road.
pub const PAVED_SPEED: f64 = 1.6;
/// A paved cell counts this much of a dirt one when a service's reach is measured along roads.
pub const PAVED_REACH_COST: f32 = 0.5;
/// Bricks a cell of paving takes from the island's warehouse.
pub const PAVING_BRICKS: f64 = 0.25;

/// Build bar groups.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Roads,
    Homes,
    Production,
    Farms,
    Public,
    Harbour,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Roads,
        Category::Homes,
        Category::Production,
        Category::Farms,
        Category::Public,
        Category::Harbour,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Roads => "Roads",
            Category::Homes => "Homes",
            Category::Production => "Workshops",
            Category::Farms => "Farms",
            Category::Public => "Public",
            Category::Harbour => "Harbour and air",
        }
    }
}

/// A kind of building.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Road,
    PavedRoad,
    Harbour,
    House,
    Market,
    Chapel,
    Fishery,
    Woodcutter,
    Sawmill,
    SheepFarm,
    Weaver,
    GrainFarm,
    Mill,
    Bakery,
    HopFarm,
    Brewery,
    ClayPit,
    Brickworks,
    AirshipMast,
    // Works of the far regions.
    CanePlantation,
    Distillery,
    DateGrove,
    SaltWorks,
    TrapperLodge,
    OilWorks,
    // Manufactories and what Burghers ask for.
    Glassworks,
    Saltery,
    Confectioner,
    Furrier,
    LampMaker,
    School,
    Tavern,
    // The three buildings that hold items.
    TradingUnion,
    TownHall,
    HarbourOffice,
    /// Makes consumable items (oil, bunting, coal, ale, spares, fireworks) for the shelf.
    Chandler,
    /// Builds ships on its slipway (see `shipyard`).
    Shipyard,
    // The three that answer fires, riots and sickness (see `incidents`).
    FireWatch,
    WatchHouse,
    Infirmary,
}

/// Where a building may stand.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Site {
    /// Anywhere flat on land.
    Land,
    /// Its front row must touch deep enough water (quays, fisheries).
    Coast,
}

/// One row of the building table.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    pub kind: Kind,
    pub name: &'static str,
    pub blurb: &'static str,
    pub category: Category,
    /// Footprint in cells (x along the building's right, z along its depth) at rotation 0.
    pub size: (i32, i32),
    pub coins: u32,
    pub cost: &'static [(Good, u32)],
    /// Workers it needs at full output.
    pub workers: Option<(Tier, u32)>,
    pub input: Option<Good>,
    /// A second good each batch takes (manufactories that join two regions' produce).
    pub input2: Option<Good>,
    pub output: Option<Good>,
    /// Seconds per batch of one good at full workforce.
    pub cycle_s: f32,
    /// Coins per minute it costs to run.
    pub upkeep: f32,
    /// Reach in cells (service radius for markets and chapels, forest radius for woodcutters).
    pub reach: i32,
    pub site: Site,
    /// Raw goods that only grow on islands fertile for them.
    pub needs_fertility: bool,
}

pub const CELL_M: f64 = 4.0;

const fn def(kind: Kind, name: &'static str, category: Category, size: (i32, i32)) -> Def {
    Def {
        kind,
        name,
        blurb: "",
        category,
        size,
        coins: 0,
        cost: &[],
        workers: None,
        input: None,
        input2: None,
        output: None,
        cycle_s: 0.0,
        upkeep: 0.0,
        reach: 0,
        site: Site::Land,
        needs_fertility: false,
    }
}

pub const DEFS: &[Def] = &[
    Def {
        blurb: "An earth track. Carts drive goods from every workshop to the warehouse along roads.",
        coins: 1,
        ..def(Kind::Road, "Dirt road", Category::Roads, (1, 1))
    },
    Def {
        blurb: "Setts and kerbs, laid over dirt or new. Carts drive faster, markets and other \
                public buildings reach further along it, and Burghers want it at the door. \
                Four cells take a brick.",
        coins: 3,
        ..def(Kind::PavedRoad, "Paved road", Category::Roads, (1, 1))
    },
    Def {
        blurb: "Warehouse and quay. Ships load and unload here; every road leads to it.",
        coins: 400,
        cost: &[(Good::Planks, 8)],
        upkeep: 6.0,
        site: Site::Coast,
        ..def(Kind::Harbour, "Harbour", Category::Harbour, (6, 4))
    },
    Def {
        blurb: "Fisherfolk move in when fish and clothes reach them and a market is near.",
        cost: &[(Good::Planks, 1)],
        ..def(Kind::House, "House", Category::Homes, (3, 3))
    },
    Def {
        blurb: "Houses within reach count as having a market.",
        coins: 150,
        cost: &[(Good::Planks, 4)],
        upkeep: 3.0,
        reach: 16,
        ..def(Kind::Market, "Market square", Category::Public, (5, 5))
    },
    Def {
        blurb: "Labourers want a chapel within reach.",
        coins: 300,
        cost: &[(Good::Planks, 6), (Good::Bricks, 6)],
        upkeep: 15.0,
        reach: 18,
        ..def(Kind::Chapel, "Chapel", Category::Public, (4, 5))
    },
    Def {
        blurb: "Boats net fish off the coast.",
        coins: 100,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 20)),
        output: Some(Good::Fish),
        cycle_s: 30.0,
        upkeep: 3.0,
        site: Site::Coast,
        ..def(Kind::Fishery, "Fishery", Category::Harbour, (3, 3))
    },
    Def {
        blurb: "Fells trees within reach. Works best inside a wood.",
        coins: 50,
        workers: Some((Tier::Fisherfolk, 10)),
        output: Some(Good::Logs),
        cycle_s: 15.0,
        upkeep: 2.0,
        reach: 8,
        ..def(Kind::Woodcutter, "Woodcutter", Category::Production, (3, 3))
    },
    Def {
        blurb: "Saws logs into planks.",
        coins: 100,
        workers: Some((Tier::Fisherfolk, 10)),
        input: Some(Good::Logs),
        output: Some(Good::Planks),
        cycle_s: 15.0,
        upkeep: 3.0,
        ..def(Kind::Sawmill, "Sawmill", Category::Production, (3, 4))
    },
    Def {
        blurb: "Sheep graze its paddocks for wool.",
        coins: 100,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 10)),
        output: Some(Good::Wool),
        cycle_s: 30.0,
        upkeep: 3.0,
        needs_fertility: true,
        ..def(Kind::SheepFarm, "Sheep farm", Category::Farms, (6, 6))
    },
    Def {
        blurb: "Weaves wool into work clothes.",
        coins: 200,
        cost: &[(Good::Planks, 4)],
        workers: Some((Tier::Fisherfolk, 30)),
        input: Some(Good::Wool),
        output: Some(Good::Clothes),
        cycle_s: 30.0,
        upkeep: 6.0,
        ..def(Kind::Weaver, "Weaver", Category::Production, (3, 4))
    },
    Def {
        blurb: "Wheat fields. Only on fertile islands.",
        coins: 100,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Labourers, 20)),
        output: Some(Good::Grain),
        cycle_s: 30.0,
        upkeep: 5.0,
        needs_fertility: true,
        ..def(Kind::GrainFarm, "Grain farm", Category::Farms, (7, 7))
    },
    Def {
        blurb: "A windmill grinds grain to flour.",
        coins: 200,
        cost: &[(Good::Planks, 4), (Good::Bricks, 2)],
        workers: Some((Tier::Labourers, 10)),
        input: Some(Good::Grain),
        output: Some(Good::Flour),
        cycle_s: 30.0,
        upkeep: 8.0,
        ..def(Kind::Mill, "Windmill", Category::Production, (3, 3))
    },
    Def {
        blurb: "Bakes flour into bread.",
        coins: 200,
        cost: &[(Good::Planks, 2), (Good::Bricks, 4)],
        workers: Some((Tier::Labourers, 40)),
        input: Some(Good::Flour),
        output: Some(Good::Bread),
        cycle_s: 30.0,
        upkeep: 10.0,
        ..def(Kind::Bakery, "Bakery", Category::Production, (3, 3))
    },
    Def {
        blurb: "Hop poles. Only on fertile islands.",
        coins: 100,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Labourers, 20)),
        output: Some(Good::Hops),
        cycle_s: 30.0,
        upkeep: 5.0,
        needs_fertility: true,
        ..def(Kind::HopFarm, "Hop farm", Category::Farms, (6, 6))
    },
    Def {
        blurb: "Brews hops into ale.",
        coins: 250,
        cost: &[(Good::Planks, 4), (Good::Bricks, 4)],
        workers: Some((Tier::Labourers, 30)),
        input: Some(Good::Hops),
        output: Some(Good::Ale),
        cycle_s: 30.0,
        upkeep: 15.0,
        ..def(Kind::Brewery, "Brewery", Category::Production, (4, 4))
    },
    Def {
        blurb: "Digs clay. Only where the ground has it.",
        coins: 100,
        workers: Some((Tier::Labourers, 20)),
        output: Some(Good::Clay),
        cycle_s: 30.0,
        upkeep: 5.0,
        needs_fertility: true,
        ..def(Kind::ClayPit, "Clay pit", Category::Production, (4, 4))
    },
    Def {
        blurb: "Fires clay into bricks.",
        coins: 200,
        cost: &[(Good::Planks, 4)],
        workers: Some((Tier::Labourers, 30)),
        input: Some(Good::Clay),
        output: Some(Good::Bricks),
        cycle_s: 30.0,
        upkeep: 10.0,
        ..def(Kind::Brickworks, "Brickworks", Category::Production, (4, 4))
    },
    Def {
        blurb: "Sugar cane in long green rows. Only in the tropics.",
        coins: 150,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 20)),
        output: Some(Good::Cane),
        cycle_s: 60.0,
        upkeep: 5.0,
        needs_fertility: true,
        ..def(
            Kind::CanePlantation,
            "Cane plantation",
            Category::Farms,
            (6, 6),
        )
    },
    Def {
        blurb: "Crushes cane and distils the juice to rum in copper stills.",
        coins: 300,
        cost: &[(Good::Planks, 4), (Good::Bricks, 4)],
        workers: Some((Tier::Fisherfolk, 20)),
        input: Some(Good::Cane),
        output: Some(Good::Rum),
        cycle_s: 60.0,
        upkeep: 15.0,
        ..def(
            Kind::Distillery,
            "Rum distillery",
            Category::Production,
            (4, 4),
        )
    },
    Def {
        blurb: "Date palms in watered rows. Only on the dry coasts of the Amber Reach.",
        coins: 150,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 10)),
        output: Some(Good::Dates),
        cycle_s: 45.0,
        upkeep: 5.0,
        needs_fertility: true,
        ..def(Kind::DateGrove, "Date grove", Category::Farms, (6, 6))
    },
    Def {
        blurb: "Sea water dries to salt in clay pans. Only on flat, sun-baked shores.",
        coins: 150,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 20)),
        output: Some(Good::Salt),
        cycle_s: 45.0,
        upkeep: 5.0,
        needs_fertility: true,
        ..def(Kind::SaltWorks, "Salt works", Category::Production, (5, 5))
    },
    Def {
        blurb: "Trappers work the wild snow country for furs: keep the land round them empty.",
        coins: 100,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 10)),
        output: Some(Good::Furs),
        cycle_s: 60.0,
        upkeep: 5.0,
        needs_fertility: true,
        reach: 8,
        ..def(
            Kind::TrapperLodge,
            "Trapper's lodge",
            Category::Production,
            (3, 3),
        )
    },
    Def {
        blurb: "Boats work the ice edge; the try-pots on the quay render lamp oil.",
        coins: 300,
        cost: &[(Good::Planks, 4), (Good::Bricks, 4)],
        workers: Some((Tier::Fisherfolk, 20)),
        output: Some(Good::Oil),
        cycle_s: 60.0,
        upkeep: 15.0,
        site: Site::Coast,
        needs_fertility: true,
        ..def(Kind::OilWorks, "Oil works", Category::Harbour, (4, 4))
    },
    Def {
        blurb: "Melts sand to window glass. Houses need glass to become Burghers' houses.",
        coins: 300,
        cost: &[(Good::Planks, 4), (Good::Bricks, 6)],
        workers: Some((Tier::Labourers, 20)),
        output: Some(Good::Glass),
        cycle_s: 45.0,
        upkeep: 15.0,
        ..def(Kind::Glassworks, "Glassworks", Category::Production, (4, 4))
    },
    Def {
        blurb: "Packs fish in salt from the Amber Reach: salt fish keeps, and Burghers eat it.",
        coins: 250,
        cost: &[(Good::Planks, 4), (Good::Bricks, 2)],
        workers: Some((Tier::Labourers, 20)),
        input: Some(Good::Fish),
        input2: Some(Good::Salt),
        output: Some(Good::SaltFish),
        cycle_s: 30.0,
        upkeep: 10.0,
        ..def(Kind::Saltery, "Saltery", Category::Production, (3, 4))
    },
    Def {
        blurb: "Bakes flour and dates into date cakes, a Burgher's treat.",
        coins: 300,
        cost: &[(Good::Planks, 2), (Good::Bricks, 6)],
        workers: Some((Tier::Labourers, 20)),
        input: Some(Good::Flour),
        input2: Some(Good::Dates),
        output: Some(Good::DateCakes),
        cycle_s: 45.0,
        upkeep: 15.0,
        ..def(
            Kind::Confectioner,
            "Confectioner",
            Category::Production,
            (3, 3),
        )
    },
    Def {
        blurb: "Lines woollen cloth with Frostholm furs: coats for Burghers.",
        coins: 350,
        cost: &[(Good::Planks, 4), (Good::Bricks, 6)],
        workers: Some((Tier::Burghers, 20)),
        input: Some(Good::Wool),
        input2: Some(Good::Furs),
        output: Some(Good::Coats),
        cycle_s: 45.0,
        upkeep: 20.0,
        ..def(Kind::Furrier, "Furrier", Category::Production, (3, 4))
    },
    Def {
        blurb: "Glass chimneys and brass founts filled with lamp oil: light for the parlour.",
        coins: 400,
        cost: &[(Good::Planks, 4), (Good::Bricks, 6), (Good::Glass, 2)],
        workers: Some((Tier::Burghers, 20)),
        input: Some(Good::Glass),
        input2: Some(Good::Oil),
        output: Some(Good::Lamps),
        cycle_s: 60.0,
        upkeep: 20.0,
        ..def(Kind::LampMaker, "Lamp maker", Category::Production, (3, 3))
    },
    Def {
        blurb: "Burghers want a school within reach.",
        coins: 400,
        cost: &[(Good::Planks, 6), (Good::Bricks, 8), (Good::Glass, 2)],
        upkeep: 20.0,
        reach: 20,
        ..def(Kind::School, "School", Category::Public, (4, 4))
    },
    Def {
        blurb: "Burghers want a tavern within reach.",
        coins: 300,
        cost: &[(Good::Planks, 6), (Good::Bricks, 4)],
        upkeep: 15.0,
        reach: 16,
        ..def(Kind::Tavern, "Tavern", Category::Public, (4, 3))
    },
    Def {
        blurb: "Holds items that act on every works and farm in its reach: specialists, \
                devices and charters that speed batches, spare workers or change what a works \
                takes and gives.",
        coins: 600,
        cost: &[(Good::Planks, 10), (Good::Bricks, 8)],
        upkeep: 25.0,
        reach: 26,
        ..def(
            Kind::TradingUnion,
            "Trading Union",
            Category::Public,
            (5, 4),
        )
    },
    Def {
        blurb: "Holds items that act on every house in its reach: more coins for a met need, \
                room for more people, a need or a public building done without.",
        coins: 800,
        cost: &[(Good::Planks, 10), (Good::Bricks, 12)],
        upkeep: 30.0,
        reach: 24,
        ..def(Kind::TownHall, "Town Hall", Category::Public, (5, 5))
    },
    Def {
        blurb: "Holds items that act on the quay, fisheries and masts in its reach: ships turn \
                round sooner and sail faster, the warehouse holds more, customs take a duty.",
        coins: 600,
        cost: &[(Good::Planks, 12), (Good::Bricks, 6)],
        upkeep: 25.0,
        reach: 40,
        ..def(
            Kind::HarbourOffice,
            "Harbour Office",
            Category::Harbour,
            (4, 4),
        )
    },
    Def {
        blurb: "Mooring mast for cargo airships that fly routes over land and sea.",
        coins: 800,
        cost: &[(Good::Planks, 10), (Good::Bricks, 10)],
        upkeep: 30.0,
        ..def(Kind::AirshipMast, "Airship mast", Category::Harbour, (5, 5))
    },
    Def {
        blurb: "Rope, canvas and barrels: makes the consumable items, one each batch, in turn \
                a barrel of lamp oil, bunting, a coal bunker, a cask of ale, a crate of spares \
                and fireworks.",
        coins: 350,
        cost: &[(Good::Planks, 6), (Good::Bricks, 2)],
        workers: Some((Tier::Labourers, 12)),
        input: Some(Good::Planks),
        input2: Some(Good::Wool),
        cycle_s: 120.0,
        upkeep: 15.0,
        ..def(Kind::Chandler, "Ship chandler", Category::Harbour, (4, 3))
    },
    Def {
        blurb: "A slipway, a sawpit and a sail loft: builds schooners for a start, then barques, \
                paddle steamers, steamers and clippers as the island's people move up. Each \
                ship is paid for with coins, planks and sailcloth when it is laid down.",
        coins: 1_000,
        cost: &[(Good::Planks, 20)],
        upkeep: 10.0,
        site: Site::Coast,
        ..def(Kind::Shipyard, "Shipyard", Category::Harbour, (9, 6))
    },
    Def {
        blurb: "A hand pump cart and its crew. Fires are a quarter as likely in reach, and the \
                pump sets out down the streets as soon as one starts.",
        coins: 300,
        cost: &[(Good::Planks, 8)],
        upkeep: 4.0,
        reach: 22,
        ..def(Kind::FireWatch, "Fire Watch", Category::Public, (4, 5))
    },
    Def {
        blurb: "Constables keep the peace: riots are a quarter as likely in reach, and the \
                watch walks out to break one up.",
        coins: 500,
        cost: &[(Good::Planks, 6), (Good::Bricks, 6)],
        upkeep: 6.0,
        reach: 26,
        ..def(Kind::WatchHouse, "Watch House", Category::Public, (4, 4))
    },
    Def {
        blurb: "A doctor and her gig: sickness is a quarter as likely in reach, and she drives \
                out to every sick house.",
        coins: 600,
        cost: &[(Good::Planks, 8), (Good::Bricks, 8)],
        upkeep: 8.0,
        reach: 30,
        ..def(Kind::Infirmary, "Infirmary", Category::Public, (5, 5))
    },
];

impl Def {
    /// Whether it runs batches: everything with an output, and the Ship chandler.
    pub fn works(&self) -> bool {
        self.output.is_some() || self.kind == Kind::Chandler
    }
}

impl Kind {
    /// Its smoke spoils the street: houses within [`SMOKE_CELLS`] are less happy.
    pub fn smoky(self) -> bool {
        matches!(
            self,
            Kind::Brickworks | Kind::Glassworks | Kind::OilWorks | Kind::Distillery
        )
    }

    pub fn def(self) -> &'static Def {
        DEFS.iter()
            .find(|d| d.kind == self)
            .expect("every kind has a row in DEFS")
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        let s = s.replace(['-', ' '], "_").to_ascii_lowercase();
        DEFS.iter().map(|d| d.kind).find(|k| {
            serde_json::to_value(k)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                == Some(s.clone())
        })
    }

    /// Footprint in cells after `rot` quarter turns.
    pub fn footprint(self, rot: u8) -> (i32, i32) {
        let (w, d) = self.def().size;
        if rot % 2 == 1 { (d, w) } else { (w, d) }
    }
}

/// Which raw goods an island's ground can give.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fertility {
    pub wool: bool,
    pub grain: bool,
    pub hops: bool,
    pub clay: bool,
    #[serde(default)]
    pub cane: bool,
    #[serde(default)]
    pub dates: bool,
    #[serde(default)]
    pub salt: bool,
    #[serde(default)]
    pub furs: bool,
    #[serde(default)]
    pub oil: bool,
}

impl Fertility {
    pub fn allows(&self, kind: Kind) -> bool {
        match kind {
            Kind::SheepFarm => self.wool,
            Kind::GrainFarm => self.grain,
            Kind::HopFarm => self.hops,
            Kind::ClayPit => self.clay,
            Kind::CanePlantation => self.cane,
            Kind::DateGrove => self.dates,
            Kind::SaltWorks => self.salt,
            Kind::TrapperLodge => self.furs,
            Kind::OilWorks => self.oil,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_one_row_and_a_name_round_trip() {
        for d in DEFS {
            assert_eq!(DEFS.iter().filter(|e| e.kind == d.kind).count(), 1);
            let name = serde_json::to_value(d.kind).unwrap();
            assert_eq!(Kind::from_name(name.as_str().unwrap()), Some(d.kind));
        }
        assert_eq!(Kind::from_name("sheep farm"), Some(Kind::SheepFarm));
    }

    #[test]
    fn chains_close() {
        // Every input is someone's output.
        for d in DEFS {
            for i in d.input.into_iter().chain(d.input2) {
                assert!(
                    DEFS.iter().any(|e| e.output == Some(i)),
                    "{:?} has no producer",
                    i
                );
            }
        }
        for t in Tier::ALL {
            for g in t.needs().0 {
                assert!(DEFS.iter().any(|e| e.output == Some(*g)), "{g:?}");
            }
        }
    }

    #[test]
    fn needs_are_plain_values_and_the_lists_agree() {
        for t in Tier::ALL {
            let wants = t.wants();
            // The goods and services lists are the wants, in order.
            let goods: Vec<Good> = wants
                .iter()
                .filter_map(|w| match w.need {
                    Need::Good(g) => Some(g),
                    Need::Service(_) => None,
                })
                .collect();
            let services: Vec<Kind> = wants
                .iter()
                .filter_map(|w| match w.need {
                    Need::Service(k) => Some(k),
                    Need::Good(_) => None,
                })
                .collect();
            let (needs, basic) = t.needs();
            assert_eq!(goods, needs);
            assert_eq!(services, t.services());
            assert!(services.len() <= MAX_SERVICES);
            assert!(wants.len() <= MAX_WANTS);
            // Each tier's services index `House::services` by their place in `SERVICES`.
            assert_eq!(services, crate::build::SERVICES[..services.len()]);
            for (i, g) in needs.iter().enumerate() {
                let w = wants.iter().find(|w| w.need == Need::Good(*g)).unwrap();
                assert_eq!(w.basic(), i < basic, "{g:?}");
            }
            // Basic needs first, in category order; each brings people and points, luxuries
            // neither.
            let cats: Vec<Cat> = wants.iter().map(|w| w.cat).collect();
            assert!(cats.is_sorted(), "{t:?} wants out of category order");
            for w in wants {
                assert_eq!(w.basic(), w.people > 0.0, "{:?}", w.need);
                assert_eq!(w.basic(), w.points > 0.0, "{:?}", w.need);
                assert!(w.coins > 0.0 && w.joy >= 0.0 && w.unlock >= 0.0);
            }
            // Every basic category can be met from what unlocks first, and the people of the
            // needs asked from the start fill at least half a house.
            for c in Cat::BASIC {
                let all: f32 = wants.iter().filter(|w| w.cat == c).map(|w| w.points).sum();
                assert!(all >= t.threshold(c), "{t:?} can never meet {c:?}");
            }
            let first: f32 = wants
                .iter()
                .filter(|w| w.unlock == 0.0)
                .map(|w| w.people)
                .sum();
            assert!(first >= 0.5 * t.capacity(), "{t:?}: {first}");
            // Every need pays something, and a full house holds a round number of people.
            assert_eq!(t.capacity(), [10.0, 20.0, 30.0][t.index()]);
        }
        // Labourers meet every basic category from goods made at home (Burghers' homes
        // are fed from them too, so nobody needs the far continents to reach them).
        let home = |t: Tier, c: Cat| -> f32 {
            t.wants()
                .iter()
                .filter(|w| w.cat == c)
                .filter(|w| match w.need {
                    Need::Good(g) => {
                        !g.overseas()
                            && !matches!(g, Good::SaltFish | Good::Coats | Good::DateCakes)
                    }
                    Need::Service(_) => true,
                })
                .map(|w| w.points)
                .sum()
        };
        for c in Cat::BASIC {
            assert!(
                home(Tier::Labourers, c) >= Tier::Labourers.threshold(c),
                "{c:?}"
            );
        }
        // Each tier's full house pays more than the one below.
        assert!(Tier::Labourers.full_income() > Tier::Fisherfolk.full_income() * 2.0);
        assert!(Tier::Burghers.full_income() > Tier::Labourers.full_income());
    }
}
