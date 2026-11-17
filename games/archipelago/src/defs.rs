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

/// One need of a tier, with what meeting it is worth: plain numbers, never a percentage.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub need: Need,
    /// People it brings into a full house (0: a luxury, nobody leaves without it).
    pub people: f32,
    /// Coins a minute a house pays while it is met.
    pub coins: f32,
}

const fn good(g: Good, people: f32, coins: f32) -> Want {
    Want {
        need: Need::Good(g),
        people,
        coins,
    }
}

const fn service(k: Kind, people: f32, coins: f32) -> Want {
    Want {
        need: Need::Service(k),
        people,
        coins,
    }
}

/// Who lives in a house and works in the buildings that ask for them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Fisherfolk,
    Labourers,
    Burghers,
}

/// The most services a tier asks for (the size of [`crate::sim::House::services`]).
pub const MAX_SERVICES: usize = 4;

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

    /// Everything the tier wants, with the people and the coins each need is worth.
    pub fn wants(self) -> &'static [Want] {
        match self {
            Tier::Fisherfolk => {
                const W: &[Want] = &[
                    service(Kind::Market, 3.0, 0.4),
                    good(Good::Fish, 4.0, 0.8),
                    good(Good::Clothes, 3.0, 0.8),
                ];
                W
            }
            Tier::Labourers => {
                const W: &[Want] = &[
                    service(Kind::Market, 3.0, 0.8),
                    service(Kind::Chapel, 3.0, 0.8),
                    good(Good::Fish, 4.0, 1.2),
                    good(Good::Clothes, 4.0, 1.4),
                    good(Good::Bread, 6.0, 1.8),
                    good(Good::Ale, 0.0, 3.0),
                    good(Good::Rum, 0.0, 2.0),
                    good(Good::Salt, 0.0, 1.0),
                    good(Good::Dates, 0.0, 1.4),
                    good(Good::Furs, 0.0, 2.4),
                    good(Good::Oil, 0.0, 1.8),
                ];
                W
            }
            Tier::Burghers => {
                const W: &[Want] = &[
                    service(Kind::Market, 3.0, 1.0),
                    service(Kind::Chapel, 3.0, 1.0),
                    service(Kind::School, 4.0, 1.6),
                    service(Kind::Tavern, 4.0, 1.6),
                    good(Good::Bread, 5.0, 2.0),
                    good(Good::SaltFish, 5.0, 2.6),
                    good(Good::Coats, 6.0, 3.2),
                    good(Good::Ale, 0.0, 3.0),
                    good(Good::Rum, 0.0, 3.0),
                    good(Good::DateCakes, 0.0, 3.4),
                    good(Good::Lamps, 0.0, 4.0),
                ];
                W
            }
        }
    }

    /// People in a full house of this tier: what its needs bring in, all told.
    pub fn capacity(self) -> f32 {
        self.wants().iter().map(|w| w.people).sum()
    }

    /// Coins a minute a full house pays when every need is met.
    pub fn full_income(self) -> f32 {
        self.wants().iter().map(|w| w.coins).sum()
    }

    /// Goods the tier consumes, in the order of [`Tier::wants`]. The first `basic` of them
    /// bring people into the house; the rest are luxuries that only pay.
    pub fn needs(self) -> (&'static [Good], usize) {
        match self {
            Tier::Fisherfolk => (&[Good::Fish, Good::Clothes], 2),
            Tier::Labourers => (
                &[
                    Good::Fish,
                    Good::Clothes,
                    Good::Bread,
                    Good::Ale,
                    Good::Rum,
                    Good::Salt,
                    Good::Dates,
                    Good::Furs,
                    Good::Oil,
                ],
                3,
            ),
            Tier::Burghers => (
                &[
                    Good::Bread,
                    Good::SaltFish,
                    Good::Coats,
                    Good::Ale,
                    Good::Rum,
                    Good::DateCakes,
                    Good::Lamps,
                ],
                3,
            ),
        }
    }

    /// Buildings that must be in reach of the house, in the order of [`Tier::wants`].
    pub fn services(self) -> &'static [Kind] {
        match self {
            Tier::Fisherfolk => &[Kind::Market],
            Tier::Labourers => &[Kind::Market, Kind::Chapel],
            Tier::Burghers => &[Kind::Market, Kind::Chapel, Kind::School, Kind::Tavern],
        }
    }

    /// What the next tier is and what moving up costs.
    pub fn next(self) -> Option<(Tier, &'static [(Good, f32)])> {
        match self {
            Tier::Fisherfolk => {
                Some((Tier::Labourers, &[(Good::Planks, 2.0), (Good::Bricks, 2.0)]))
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
        upkeep: 10.0,
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
        upkeep: 5.0,
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
        upkeep: 5.0,
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
        cost: &[(Good::Logs, 2)],
        workers: Some((Tier::Fisherfolk, 10)),
        input: Some(Good::Logs),
        output: Some(Good::Planks),
        cycle_s: 15.0,
        upkeep: 5.0,
        ..def(Kind::Sawmill, "Sawmill", Category::Production, (3, 4))
    },
    Def {
        blurb: "Sheep graze its paddocks for wool.",
        coins: 100,
        cost: &[(Good::Planks, 2)],
        workers: Some((Tier::Fisherfolk, 10)),
        output: Some(Good::Wool),
        cycle_s: 30.0,
        upkeep: 5.0,
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
        upkeep: 10.0,
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
        upkeep: 10.0,
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
        blurb: "Mooring mast for cargo airships that fly routes over land and sea.",
        coins: 800,
        cost: &[(Good::Planks, 10), (Good::Bricks, 10)],
        upkeep: 30.0,
        ..def(Kind::AirshipMast, "Airship mast", Category::Harbour, (5, 5))
    },
];

impl Kind {
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
            for (i, g) in needs.iter().enumerate() {
                let w = wants.iter().find(|w| w.need == Need::Good(*g)).unwrap();
                assert_eq!(w.people > 0.0, i < basic, "{g:?}");
            }
            // Every need pays something, and a full house holds a round number of people.
            assert!(wants.iter().all(|w| w.coins > 0.0));
            assert_eq!(t.capacity(), [10.0, 20.0, 30.0][t.index()]);
        }
        // Each tier's full house pays more than the one below.
        assert!(Tier::Labourers.full_income() > Tier::Fisherfolk.full_income() * 2.0);
        assert!(Tier::Burghers.full_income() > Tier::Labourers.full_income());
    }
}
