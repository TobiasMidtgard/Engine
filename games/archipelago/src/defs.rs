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
}

impl Good {
    pub const ALL: [Good; 12] = [
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
        }
    }

    pub fn from_name(s: &str) -> Option<Good> {
        Good::ALL
            .into_iter()
            .find(|g| g.name().eq_ignore_ascii_case(s))
    }
}

/// Who lives in a house and works in the buildings that ask for them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Fisherfolk,
    Labourers,
}

impl Tier {
    pub const ALL: [Tier; 2] = [Tier::Fisherfolk, Tier::Labourers];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Fisherfolk => "Fisherfolk",
            Tier::Labourers => "Labourers",
        }
    }

    /// People in a full house of this tier.
    pub fn capacity(self) -> f32 {
        match self {
            Tier::Fisherfolk => 10.0,
            Tier::Labourers => 20.0,
        }
    }

    /// Coins per person per minute when every need is met.
    pub fn tax(self) -> f32 {
        match self {
            Tier::Fisherfolk => 0.2,
            Tier::Labourers => 0.5,
        }
    }

    /// Goods the tier consumes. The first `basic` of them fill the house; the rest add
    /// happiness and taxes.
    pub fn needs(self) -> (&'static [Good], usize) {
        match self {
            Tier::Fisherfolk => (&[Good::Fish, Good::Clothes], 2),
            Tier::Labourers => (&[Good::Fish, Good::Clothes, Good::Bread, Good::Ale], 3),
        }
    }

    /// Buildings that must be in reach of the house.
    pub fn services(self) -> &'static [Kind] {
        match self {
            Tier::Fisherfolk => &[Kind::Market],
            Tier::Labourers => &[Kind::Market, Kind::Chapel],
        }
    }

    /// What the next tier is and what moving up costs.
    pub fn next(self) -> Option<(Tier, &'static [(Good, f32)])> {
        match self {
            Tier::Fisherfolk => {
                Some((Tier::Labourers, &[(Good::Planks, 2.0), (Good::Bricks, 2.0)]))
            }
            Tier::Labourers => None,
        }
    }
}

/// Goods each person eats or wears per minute of a need (1/120: one fish every two minutes).
pub const NEED_PER_PERSON_MIN: f32 = 1.0 / 120.0;

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
        blurb: "Carts drive goods from every workshop to the warehouse along roads.",
        coins: 5,
        ..def(Kind::Road, "Dirt road", Category::Roads, (1, 1))
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
        cycle_s: 60.0,
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
        cycle_s: 60.0,
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
        cycle_s: 60.0,
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
}

impl Fertility {
    pub fn allows(&self, kind: Kind) -> bool {
        match kind {
            Kind::SheepFarm => self.wool,
            Kind::GrainFarm => self.grain,
            Kind::HopFarm => self.hops,
            Kind::ClayPit => self.clay,
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
            if let Some(i) = d.input {
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
}
