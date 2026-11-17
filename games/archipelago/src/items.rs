//! Items and the three buildings that hold them: the Trading Union (works and farms), the
//! Town Hall (houses) and the Harbour Office (the quay, fisheries, masts and the ships that
//! call). An item sits in a slot and acts on everything of its kind inside the building's
//! reach; what it does is data ([`Effect`]), so the economy only ever reads a building's
//! summed-up [`Boost`].
//!
//! Beyond the usual: three items of one guild in a building add a set effect, specialists
//! grow with experience (stars), a specialist working in the climate they come from does
//! half as much again, and every item can say what it is doing right now ([`panel`]).
//!
//! Items come from the pedlar's barque that calls at the home quay, from flotsam the ocean
//! packets fish up, and from specialists who arrive where Burghers live well.

use std::collections::{HashMap, HashSet};

use engine::prelude::*;
use serde::Serialize;

use crate::build::Changed;
use crate::defs::{CELL_M, Good, Kind, Need, Tier};
use crate::globe::Biome;
use crate::grid;
use crate::island::islands;
use crate::sim::{Building, Economy, House, Producer};

/// Which of the three buildings an item belongs in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Union,
    Hall,
    Office,
}

impl Host {
    #[cfg(test)]
    pub const ALL: [Host; 3] = [Host::Union, Host::Hall, Host::Office];

    pub fn of(kind: Kind) -> Option<Host> {
        match kind {
            Kind::TradingUnion => Some(Host::Union),
            Kind::TownHall => Some(Host::Hall),
            Kind::HarbourOffice => Some(Host::Office),
            _ => None,
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Host::Union => Kind::TradingUnion,
            Host::Hall => Kind::TownHall,
            Host::Office => Kind::HarbourOffice,
        }
    }

    /// Whether this building's items can act on `kind` at all.
    pub fn acts_on(self, kind: Kind) -> bool {
        match self {
            Host::Union => kind.def().output.is_some() && kind != Kind::Fishery,
            Host::Hall => kind == Kind::House,
            Host::Office => matches!(kind, Kind::Harbour | Kind::Fishery | Kind::AirshipMast),
        }
    }

    /// The colour of its reach ring and markers.
    pub fn colour(self) -> u32 {
        match self {
            Host::Union => 0xf0a030,
            Host::Hall => 0xb070f0,
            Host::Office => 0x38a8e8,
        }
    }
}

/// What kind of thing an item is. Only specialists gain experience.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    Specialist,
    Device,
    Charter,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Rank {
    Common,
    Fine,
    Rare,
    Storied,
}

impl Rank {
    pub fn price(self) -> u32 {
        match self {
            Rank::Common => 150,
            Rank::Fine => 400,
            Rank::Rare => 900,
            Rank::Storied => 2500,
        }
    }

    /// How often it turns up, against the others.
    fn weight(self) -> u32 {
        match self {
            Rank::Common => 50,
            Rank::Fine => 30,
            Rank::Rare => 15,
            Rank::Storied => 5,
        }
    }
}

/// Three items of one guild in one building add the guild's set effect.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Guild {
    Millers,
    Clothiers,
    Firemasters,
    Victuallers,
    Almoners,
    Wharfingers,
    Pilots,
}

impl Guild {
    pub const ALL: [Guild; 7] = [
        Guild::Millers,
        Guild::Clothiers,
        Guild::Firemasters,
        Guild::Victuallers,
        Guild::Almoners,
        Guild::Wharfingers,
        Guild::Pilots,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Guild::Millers => "Millers and Brewers",
            Guild::Clothiers => "Clothiers",
            Guild::Firemasters => "Firemasters",
            Guild::Victuallers => "Victuallers",
            Guild::Almoners => "Almoners",
            Guild::Wharfingers => "Wharfingers",
            Guild::Pilots => "Pilots",
        }
    }

    /// What three of them together add, on everything their building acts on.
    pub fn set(self) -> Effect {
        match self {
            Guild::Millers => Effect::Thrift(4.0),
            Guild::Clothiers => Effect::Hands(0.2),
            Guild::Firemasters => Effect::Faster(0.2),
            Guild::Victuallers => Effect::Pays(None, 0.8),
            Guild::Almoners => Effect::Lodgers(2.0),
            Guild::Wharfingers => Effect::Storage(40.0),
            Guild::Pilots => Effect::Sail(0.15),
        }
    }
}

/// What an item does to a building in reach. Shares are of the building's own figure;
/// coins and people are plain values.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Effect {
    /// Works: batches this share quicker.
    Faster(f32),
    /// Works: this share fewer workers.
    Hands(f32),
    /// Works: upkeep this many coins a minute less.
    Thrift(f32),
    /// Works: one of this good extra every nth batch.
    Bonus(Good, u32),
    /// Works: takes the second good instead of the first.
    Instead(Good, Good),
    /// Works: does without its second input.
    Spare2,
    /// Houses: this many coins a minute more from a house that has the need met (None: from
    /// every house, by how full it is).
    Pays(Option<Need>, f32),
    /// Houses: the good counts as supplied, and none of it is used.
    Provides(Good),
    /// Houses: the service counts as in reach.
    Serves(Kind),
    /// Houses: room for this many more people.
    Lodgers(f32),
    /// Quays and masts: ships turn round this share sooner.
    Loading(f32),
    /// The island: this much more warehouse room.
    Storage(f32),
    /// Quays and masts: this many coins for every unit a ship lands.
    Duty(f32),
    /// Quays and masts: ships leave this share faster for the next leg.
    Sail(f32),
}

impl Effect {
    /// The effect in words, at `k` times its written strength.
    pub fn line(self, k: f32) -> String {
        let pct = |x: f32| (x * k * 100.0).round();
        match self {
            Effect::Faster(x) => format!("Batches {} % quicker", pct(x)),
            Effect::Hands(x) => format!("Needs {} % fewer workers", pct(x)),
            Effect::Thrift(c) => format!("Upkeep {:.0} coins a minute less", c * k),
            Effect::Bonus(g, n) => format!(
                "Every {} batch also makes {}",
                nth(nth_at(n, k)),
                low(g.name())
            ),
            Effect::Instead(a, b) => {
                format!("Takes {} instead of {}", low(b.name()), low(a.name()))
            }
            Effect::Spare2 => "Does without its second good".into(),
            Effect::Pays(Some(Need::Good(g)), c) => format!(
                "+{:.1} coins a minute from each house supplied with {}",
                c * k,
                low(g.name())
            ),
            Effect::Pays(Some(Need::Service(s)), c) => format!(
                "+{:.1} coins a minute from each house with a {} in reach",
                c * k,
                low(s.def().name)
            ),
            Effect::Pays(None, c) => format!("+{:.1} coins a minute from each house", c * k),
            Effect::Provides(g) => format!("Houses need no {}", low(g.name())),
            Effect::Serves(s) => format!("Houses count as having a {}", low(s.def().name)),
            Effect::Lodgers(n) => format!("Room for {:.0} more people in each house", n * k),
            Effect::Loading(x) => format!("Ships turn round {} % sooner", pct(x)),
            Effect::Storage(n) => format!("+{:.0} warehouse room on the island", n * k),
            Effect::Duty(c) => format!("{:.1} coins for every unit a ship lands", c * k),
            Effect::Sail(x) => format!("Ships leave {} % faster", pct(x)),
        }
    }
}

fn nth(n: u32) -> String {
    match n {
        2 => "second".into(),
        3 => "third".into(),
        4 => "fourth".into(),
        5 => "fifth".into(),
        n => format!("{n}th"),
    }
}

/// A stronger hand makes the extra good come round sooner (never more often than every
/// second batch).
fn nth_at(n: u32, k: f32) -> u32 {
    ((n as f32 / k).round() as u32).max(2)
}

/// A name as it reads inside a sentence.
fn low(name: &str) -> String {
    name.to_lowercase()
}

/// One row of the item table.
#[derive(Clone, Copy, Debug)]
pub struct ItemDef {
    pub name: &'static str,
    pub blurb: &'static str,
    pub host: Host,
    pub sort: Sort,
    pub rank: Rank,
    pub guild: Option<Guild>,
    /// The climate a specialist comes from: there they do half as much again.
    pub home: Option<Biome>,
    /// The kinds it acts on (empty: everything its building acts on).
    pub on: &'static [Kind],
    pub effects: &'static [Effect],
    /// What its picture shows: a good or a building.
    pub icon: Icon,
}

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Good(Good),
    Kind(Kind),
}

impl ItemDef {
    fn fits(&self, kind: Kind) -> bool {
        self.host.acts_on(kind) && (self.on.is_empty() || self.on.contains(&kind))
    }

    /// The icon's key in the interface's sheet.
    pub fn icon_key(&self) -> &'static str {
        match self.icon {
            Icon::Good(g) => g.icon(),
            Icon::Kind(Kind::Harbour | Kind::HarbourOffice) => "isle_anchor",
            Icon::Kind(Kind::House | Kind::TownHall) => "isle_house",
            Icon::Kind(Kind::Chapel) => "isle_chapel",
            Icon::Kind(Kind::School) => "isle_school",
            Icon::Kind(Kind::Tavern) => "isle_tavern",
            Icon::Kind(Kind::Market) => "isle_market",
            Icon::Kind(Kind::AirshipMast) => "isle_airship",
            Icon::Kind(k) => k.def().output.map_or("isle_workshop", Good::icon),
        }
    }
}

const fn item(name: &'static str, host: Host, sort: Sort, rank: Rank, icon: Icon) -> ItemDef {
    ItemDef {
        name,
        blurb: "",
        host,
        sort,
        rank,
        guild: None,
        home: None,
        on: &[],
        effects: &[],
        icon,
    }
}

use Effect::*;
use Host::{Hall, Office, Union};
use Rank::{Common, Fine, Rare, Storied};
use Sort::{Charter, Device, Specialist};

/// Every item there is. An item is known by its place in this table.
pub static ITEMS: &[ItemDef] = &[
    // ---- Trading Union: works and farms.
    ItemDef {
        blurb: "Keeps the stones dressed and the sails trimmed.",
        guild: Some(Guild::Millers),
        on: &[Kind::Mill],
        effects: &[Faster(0.3)],
        ..item(
            "Journeyman miller",
            Union,
            Specialist,
            Common,
            Icon::Good(Good::Flour),
        )
    },
    ItemDef {
        blurb: "A finer cloth on the sieve: less flour lost to the bran.",
        guild: Some(Guild::Millers),
        on: &[Kind::Mill],
        effects: &[Bonus(Good::Flour, 3)],
        ..item(
            "Bolting sieve",
            Union,
            Device,
            Common,
            Icon::Good(Good::Flour),
        )
    },
    ItemDef {
        blurb: "Her ovens never cool, and the last of the dough goes into cakes.",
        guild: Some(Guild::Millers),
        on: &[Kind::Bakery],
        effects: &[Faster(0.3), Bonus(Good::DateCakes, 4)],
        ..item(
            "Master baker Odalys Brandt",
            Union,
            Specialist,
            Fine,
            Icon::Good(Good::Bread),
        )
    },
    ItemDef {
        blurb: "Twice the mash at every brewing, and it does for the stills as well.",
        guild: Some(Guild::Millers),
        on: &[Kind::Brewery, Kind::Distillery],
        effects: &[Faster(0.4)],
        ..item(
            "Copper mash tun",
            Union,
            Device,
            Fine,
            Icon::Good(Good::Ale),
        )
    },
    ItemDef {
        blurb: "Learned his trade where no hops grow: he brews a dark ale from dates.",
        home: Some(Biome::Arid),
        on: &[Kind::Brewery],
        effects: &[Instead(Good::Hops, Good::Dates), Faster(0.15)],
        ..item(
            "Brewer Idris of the Reach",
            Union,
            Specialist,
            Rare,
            Icon::Good(Good::Dates),
        )
    },
    ItemDef {
        blurb: "Two collies that can pen a flock before the shepherd has his boots on.",
        guild: Some(Guild::Clothiers),
        on: &[Kind::SheepFarm],
        effects: &[Faster(0.3)],
        ..item(
            "Shepherd's dogs",
            Union,
            Specialist,
            Common,
            Icon::Good(Good::Wool),
        )
    },
    ItemDef {
        blurb: "One weaver does the work of two.",
        guild: Some(Guild::Clothiers),
        on: &[Kind::Weaver],
        effects: &[Faster(0.5), Hands(0.2)],
        ..item(
            "Flying-shuttle loom",
            Union,
            Device,
            Fine,
            Icon::Good(Good::Clothes),
        )
    },
    ItemDef {
        blurb: "Nothing is wasted on her bench: the trimmings make work clothes.",
        guild: Some(Guild::Clothiers),
        home: Some(Biome::Arctic),
        on: &[Kind::Furrier],
        effects: &[Faster(0.25), Bonus(Good::Clothes, 3)],
        ..item(
            "Master furrier Ingrid Solvang",
            Union,
            Specialist,
            Rare,
            Icon::Good(Good::Coats),
        )
    },
    ItemDef {
        blurb: "Blows a lamp chimney from the last gather of every pot.",
        guild: Some(Guild::Firemasters),
        on: &[Kind::Glassworks],
        effects: &[Faster(0.25), Bonus(Good::Lamps, 4)],
        ..item(
            "Glassblower Tomas Vey",
            Union,
            Specialist,
            Rare,
            Icon::Good(Good::Glass),
        )
    },
    ItemDef {
        blurb: "The furnace's own heat warms the air it draws.",
        guild: Some(Guild::Firemasters),
        on: &[Kind::Glassworks, Kind::Brickworks],
        effects: &[Faster(0.3), Thrift(4.0)],
        ..item(
            "Regenerative furnace",
            Union,
            Device,
            Fine,
            Icon::Good(Good::Bricks),
        )
    },
    ItemDef {
        blurb: "Clean wicks and trimmed burners by the gross.",
        guild: Some(Guild::Firemasters),
        on: &[Kind::LampMaker],
        effects: &[Faster(0.35)],
        ..item(
            "Wick-trimming jig",
            Union,
            Device,
            Common,
            Icon::Good(Good::Lamps),
        )
    },
    ItemDef {
        blurb: "Renders lamp oil from the fishery's leavings: no need to wait for the whalers.",
        home: Some(Biome::Arctic),
        on: &[Kind::LampMaker],
        effects: &[Instead(Good::Oil, Good::Fish)],
        ..item(
            "Oil-presser Runa of the Ice",
            Union,
            Specialist,
            Rare,
            Icon::Good(Good::Oil),
        )
    },
    ItemDef {
        blurb: "Smokes the catch over alder: the saltery needs no salt.",
        on: &[Kind::Saltery],
        effects: &[Spare2],
        ..item(
            "Smokehouse keeper Halvard",
            Union,
            Specialist,
            Rare,
            Icon::Good(Good::SaltFish),
        )
    },
    ItemDef {
        blurb: "A boiler and a belt where two men pulled a pit saw.",
        on: &[Kind::Sawmill],
        effects: &[Faster(0.6)],
        ..item("Steam saw", Union, Device, Fine, Icon::Good(Good::Planks))
    },
    ItemDef {
        blurb: "The right to fell in the common woods, and to cart on the common roads.",
        on: &[Kind::Woodcutter, Kind::Sawmill],
        effects: &[Faster(0.2), Thrift(3.0)],
        ..item(
            "Timber charter",
            Union,
            Charter,
            Common,
            Icon::Good(Good::Logs),
        )
    },
    ItemDef {
        blurb: "Knows every hand by name and where each is best put.",
        effects: &[Hands(0.15)],
        ..item(
            "Works foreman",
            Union,
            Specialist,
            Common,
            Icon::Kind(Kind::Sawmill),
        )
    },
    ItemDef {
        blurb: "She has rebuilt every engine she ever met, and each ran better for it.",
        effects: &[Faster(0.25), Hands(0.25), Thrift(2.0)],
        ..item(
            "Aurelia Montcalm, engineer",
            Union,
            Specialist,
            Storied,
            Icon::Kind(Kind::Brickworks),
        )
    },
    // ---- Town Hall: houses.
    ItemDef {
        blurb: "The news at every corner, and the rates fall due on time.",
        guild: Some(Guild::Victuallers),
        effects: &[Pays(None, 0.4)],
        ..item(
            "Town crier",
            Hall,
            Specialist,
            Common,
            Icon::Kind(Kind::TownHall),
        )
    },
    ItemDef {
        blurb: "A stall at every corner: fish sells for a little more.",
        guild: Some(Guild::Victuallers),
        effects: &[Pays(Some(Need::Good(Good::Fish)), 0.4)],
        ..item(
            "Fishwives' stalls",
            Hall,
            Charter,
            Common,
            Icon::Good(Good::Fish),
        )
    },
    ItemDef {
        blurb: "Warm loaves at the door before the house is up.",
        guild: Some(Guild::Victuallers),
        effects: &[Pays(Some(Need::Good(Good::Bread)), 0.6)],
        ..item(
            "Baker's round",
            Hall,
            Charter,
            Common,
            Icon::Good(Good::Bread),
        )
    },
    ItemDef {
        blurb: "Every house may draw its own ale from the brewery's cart.",
        guild: Some(Guild::Victuallers),
        effects: &[Pays(Some(Need::Good(Good::Ale)), 1.0)],
        ..item("Tap licence", Hall, Charter, Fine, Icon::Good(Good::Ale))
    },
    ItemDef {
        blurb: "Lamps lit along the streets at dusk: people pay gladly for a bright town.",
        effects: &[Pays(Some(Need::Good(Good::Lamps)), 1.5)],
        ..item(
            "Night-watch lanterns",
            Hall,
            Device,
            Fine,
            Icon::Good(Good::Lamps),
        )
    },
    ItemDef {
        blurb: "Finds a bed for anyone who steps off a boat.",
        guild: Some(Guild::Almoners),
        effects: &[Lodgers(2.0)],
        ..item(
            "Lodging-house keeper",
            Hall,
            Specialist,
            Common,
            Icon::Kind(Kind::House),
        )
    },
    ItemDef {
        blurb: "Sees that no garret stands empty.",
        guild: Some(Guild::Almoners),
        effects: &[Lodgers(3.0), Pays(None, 0.3)],
        ..item(
            "Almoner Sister Benedikte",
            Hall,
            Specialist,
            Fine,
            Icon::Kind(Kind::House),
        )
    },
    ItemDef {
        blurb: "Holds service in any street that will have him.",
        guild: Some(Guild::Almoners),
        effects: &[Serves(Kind::Chapel)],
        ..item(
            "Chaplain of the fleet",
            Hall,
            Specialist,
            Fine,
            Icon::Kind(Kind::Chapel),
        )
    },
    ItemDef {
        blurb: "A slate, a bell and thirty children in any parlour.",
        effects: &[Serves(Kind::School)],
        ..item(
            "Travelling schoolmistress",
            Hall,
            Specialist,
            Fine,
            Icon::Kind(Kind::School),
        )
    },
    ItemDef {
        blurb: "His puppets pack the square: who needs a tavern?",
        effects: &[Serves(Kind::Tavern), Pays(None, 0.2)],
        ..item(
            "Showman Leopold Quill",
            Hall,
            Specialist,
            Rare,
            Icon::Kind(Kind::Tavern),
        )
    },
    ItemDef {
        blurb: "Her soup kitchen feeds the quarter better than any barrel of salt fish.",
        home: Some(Biome::Tropical),
        effects: &[Provides(Good::SaltFish)],
        ..item(
            "Cook Orsolya Mar",
            Hall,
            Specialist,
            Rare,
            Icon::Good(Good::SaltFish),
        )
    },
    ItemDef {
        blurb: "Turns last winter's coats so well that nobody buys new.",
        effects: &[Provides(Good::Coats)],
        ..item(
            "Tailor Casimir Roon",
            Hall,
            Specialist,
            Rare,
            Icon::Good(Good::Coats),
        )
    },
    ItemDef {
        blurb: "Every house weighed, measured and rated to the farthing.",
        effects: &[Pays(None, 1.5)],
        ..item(
            "Assessor Eudora Finch",
            Hall,
            Specialist,
            Rare,
            Icon::Kind(Kind::TownHall),
        )
    },
    ItemDef {
        blurb: "A clock that the whole town sets its day by.",
        effects: &[
            Pays(None, 0.3),
            Pays(Some(Need::Service(Kind::Market)), 0.3),
        ],
        ..item(
            "Town clock chimes",
            Hall,
            Device,
            Common,
            Icon::Kind(Kind::Market),
        )
    },
    ItemDef {
        blurb: "Keeps a cellar the whole quarter drinks from: no ale or rum need be bought.",
        effects: &[Provides(Good::Ale), Provides(Good::Rum)],
        ..item(
            "Cellarer of the old vintage",
            Hall,
            Specialist,
            Storied,
            Icon::Good(Good::Rum),
        )
    },
    ItemDef {
        blurb: "Thirty years in the chair: the town runs as he likes it, and it likes him.",
        effects: &[Pays(None, 2.0), Lodgers(3.0), Serves(Kind::School)],
        ..item(
            "Burgomaster Henrik Vane",
            Hall,
            Specialist,
            Storied,
            Icon::Kind(Kind::TownHall),
        )
    },
    // ---- Harbour Office: the quay, fisheries, masts and the ships that call.
    ItemDef {
        blurb: "Forty backs and one song.",
        guild: Some(Guild::Wharfingers),
        on: &[Kind::Harbour, Kind::AirshipMast],
        effects: &[Loading(0.3)],
        ..item(
            "Stevedore gang",
            Office,
            Specialist,
            Common,
            Icon::Kind(Kind::Harbour),
        )
    },
    ItemDef {
        blurb: "Lifts a hold's worth in three swings.",
        guild: Some(Guild::Wharfingers),
        on: &[Kind::Harbour],
        effects: &[Loading(0.5)],
        ..item(
            "Steam crane",
            Office,
            Device,
            Fine,
            Icon::Kind(Kind::Harbour),
        )
    },
    ItemDef {
        blurb: "Casks for everything, and somewhere to stack them.",
        guild: Some(Guild::Wharfingers),
        on: &[Kind::Harbour],
        effects: &[Storage(20.0)],
        ..item(
            "Cooper's yard",
            Office,
            Device,
            Common,
            Icon::Good(Good::Planks),
        )
    },
    ItemDef {
        blurb: "Goods under the Crown's seal, stacked to the rafters.",
        guild: Some(Guild::Wharfingers),
        on: &[Kind::Harbour],
        effects: &[Storage(40.0)],
        ..item(
            "Bonded warehouse",
            Office,
            Charter,
            Fine,
            Icon::Kind(Kind::Harbour),
        )
    },
    ItemDef {
        blurb: "Lamps along the quay: ships are worked through the night.",
        on: &[Kind::Harbour, Kind::AirshipMast],
        effects: &[Loading(0.2)],
        ..item(
            "Quay lights",
            Office,
            Device,
            Common,
            Icon::Good(Good::Lamps),
        )
    },
    ItemDef {
        blurb: "Takes ships out past the bar on any tide.",
        guild: Some(Guild::Pilots),
        on: &[Kind::Harbour],
        effects: &[Sail(0.1)],
        ..item(
            "Harbour pilot",
            Office,
            Specialist,
            Common,
            Icon::Kind(Kind::HarbourOffice),
        )
    },
    ItemDef {
        blurb: "High water to the minute for every day of the year.",
        guild: Some(Guild::Pilots),
        on: &[Kind::Harbour],
        effects: &[Loading(0.15), Sail(0.05)],
        ..item(
            "Tide tables",
            Office,
            Charter,
            Common,
            Icon::Kind(Kind::HarbourOffice),
        )
    },
    ItemDef {
        blurb: "Her charts show the currents no other chart has.",
        guild: Some(Guild::Pilots),
        on: &[Kind::Harbour, Kind::AirshipMast],
        effects: &[Sail(0.25)],
        ..item(
            "Chartmaker Isaura Bell",
            Office,
            Specialist,
            Rare,
            Icon::Kind(Kind::HarbourOffice),
        )
    },
    ItemDef {
        blurb: "Orders flashed to ships still an hour out.",
        guild: Some(Guild::Pilots),
        on: &[Kind::Harbour, Kind::AirshipMast],
        effects: &[Sail(0.15), Loading(0.15)],
        ..item(
            "Signal-lamp battery",
            Office,
            Device,
            Fine,
            Icon::Kind(Kind::AirshipMast),
        )
    },
    ItemDef {
        blurb: "A ledger, a seal and a sharp eye on every bill of lading.",
        on: &[Kind::Harbour],
        effects: &[Duty(0.3)],
        ..item(
            "Customs collector",
            Office,
            Specialist,
            Fine,
            Icon::Kind(Kind::Harbour),
        )
    },
    ItemDef {
        blurb: "Any flag may land here, for a fee.",
        on: &[Kind::Harbour],
        effects: &[Duty(0.6)],
        ..item(
            "Free-port charter",
            Office,
            Charter,
            Rare,
            Icon::Kind(Kind::Harbour),
        )
    },
    ItemDef {
        blurb: "Mends in an evening what the sea tore in a day.",
        on: &[Kind::Fishery],
        effects: &[Faster(0.3)],
        ..item(
            "Net-maker",
            Office,
            Specialist,
            Common,
            Icon::Good(Good::Fish),
        )
    },
    ItemDef {
        blurb: "Hauls a full trawl with two hands on the handle.",
        on: &[Kind::Fishery],
        effects: &[Faster(0.5), Hands(0.2)],
        ..item("Trawl winch", Office, Device, Fine, Icon::Good(Good::Fish))
    },
    ItemDef {
        blurb: "Salts the catch in the boat: every third haul comes ashore ready for the barrel.",
        home: Some(Biome::Arctic),
        on: &[Kind::Fishery],
        effects: &[Bonus(Good::SaltFish, 3)],
        ..item(
            "Skipper Bertil Aune",
            Office,
            Specialist,
            Rare,
            Icon::Good(Good::SaltFish),
        )
    },
    ItemDef {
        blurb: "Nothing moves on the water without his nod, and everything moves faster for it.",
        on: &[Kind::Harbour, Kind::AirshipMast],
        effects: &[Loading(0.5), Storage(40.0), Duty(0.4)],
        ..item(
            "Harbour master Cornelius Raaf",
            Office,
            Specialist,
            Storied,
            Icon::Kind(Kind::HarbourOffice),
        )
    },
];

/// Seconds a specialist works before the first and the second star.
const STAR_S: [f32; 2] = [480.0, 1500.0];
/// How much each star adds to what an item does.
const STAR_GAIN: f32 = 0.2;
/// How much more a specialist does in the climate they come from.
const HOME_GAIN: f32 = 0.5;
/// Items the shelf holds.
pub const SHELF: usize = 12;
/// The most slots a building can have.
pub const MAX_SLOTS: usize = 5;
/// Seconds between specialists arriving where Burghers live well.
const ARRIVAL_EVERY_S: f64 = 300.0;
/// Renown each further Trading Union, Town Hall and Harbour Office asks for.
pub const RENOWN_STEP: f32 = 4000.0;
/// Burghers an island needs before its buildings open a fourth slot.
const FOURTH_SLOT_BURGHERS: f32 = 60.0;

/// An item somebody owns.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Held {
    /// Stable number for the interface.
    pub uid: u32,
    /// Its row in [`ITEMS`].
    pub item: usize,
    /// Seconds worked in a slot (specialists).
    pub xp: f32,
}

impl Held {
    pub fn def(&self) -> &'static ItemDef {
        &ITEMS[self.item]
    }

    pub fn stars(&self) -> u32 {
        STAR_S.iter().filter(|s| self.xp >= **s).count() as u32
    }

    /// How strongly it acts in a building standing in `biome`.
    pub fn power(&self, biome: Biome) -> f32 {
        let home = if self.def().home == Some(biome) {
            HOME_GAIN
        } else {
            0.0
        };
        1.0 + STAR_GAIN * self.stars() as f32 + home
    }
}

/// The slots of a Trading Union, Town Hall or Harbour Office.
#[derive(Component, Clone, Debug, Default)]
pub struct Institution {
    pub slots: [Option<Held>; MAX_SLOTS],
}

/// What the items in reach add up to for one building. The economy reads only this.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct Boost {
    pub faster: f32,
    pub hands: f32,
    pub thrift: f32,
    pub bonus: Vec<(Good, u32)>,
    pub swap: Vec<(Good, Good)>,
    pub spare2: bool,
    pub pays: Vec<(Option<Need>, f32)>,
    pub provides: Vec<Good>,
    pub serves: Vec<Kind>,
    pub lodgers: f32,
    pub loading: f32,
    pub duty: f32,
    pub sail: f32,
    /// How many items act on it.
    pub items: u32,
}

impl Boost {
    fn add(&mut self, e: Effect, k: f32) {
        match e {
            Effect::Faster(x) => self.faster = (self.faster + x * k).min(2.0),
            Effect::Hands(x) => self.hands = (self.hands + x * k).min(0.6),
            Effect::Thrift(c) => self.thrift += c * k,
            Effect::Bonus(g, n) => self.bonus.push((g, nth_at(n, k))),
            Effect::Instead(a, b) => {
                if !self.swap.iter().any(|s| s.0 == a) {
                    self.swap.push((a, b));
                }
            }
            Effect::Spare2 => self.spare2 = true,
            Effect::Pays(n, c) => match self.pays.iter_mut().find(|p| p.0 == n) {
                Some(p) => p.1 += c * k,
                None => self.pays.push((n, c * k)),
            },
            Effect::Provides(g) => {
                if !self.provides.contains(&g) {
                    self.provides.push(g);
                }
            }
            Effect::Serves(s) => {
                if !self.serves.contains(&s) {
                    self.serves.push(s);
                }
            }
            Effect::Lodgers(n) => self.lodgers += n * k,
            Effect::Loading(x) => self.loading = (self.loading + x * k).min(0.8),
            Effect::Duty(c) => self.duty += c * k,
            Effect::Sail(x) => self.sail = (self.sail + x * k).min(0.6),
            // (Warehouse room is the island's, not a building's: see `apply`.)
            Effect::Storage(_) => {}
        }
    }

    /// The good a works takes where its table says `g`.
    pub fn input(&self, g: Good) -> Good {
        self.swap.iter().find(|s| s.0 == g).map_or(g, |s| s.1)
    }

    /// Coins a minute more from a house for a met need.
    pub fn pay_for(&self, need: Need) -> f32 {
        self.pays
            .iter()
            .filter(|p| p.0 == Some(need))
            .map(|p| p.1)
            .sum()
    }

    /// Coins a minute more from a house whatever it has.
    pub fn flat_pay(&self) -> f32 {
        self.pays
            .iter()
            .filter(|p| p.0.is_none())
            .map(|p| p.1)
            .sum()
    }
}

/// Something the pedlar has for sale.
#[derive(Clone, Debug, Serialize)]
pub struct Offer {
    pub item: usize,
    pub price: u32,
}

/// The company's items that are not in a slot, what is for sale, and the dice.
#[derive(Resource, Clone, Debug)]
pub struct Items {
    pub shelf: Vec<Held>,
    next_uid: u32,
    /// What the pedlar offers while moored at the home quay.
    pub offers: Vec<Offer>,
    pub market_open: bool,
    /// Game time of the next specialist's arrival.
    next_arrival_s: f64,
    seed: u64,
    /// Coins the customs have brought in so far.
    pub duty_earned: f64,
}

impl Default for Items {
    fn default() -> Self {
        Self {
            shelf: Vec::new(),
            next_uid: 1,
            offers: Vec::new(),
            market_open: false,
            next_arrival_s: 420.0,
            seed: 0x51ED_270B_A5C3_9E11,
            duty_earned: 0.0,
        }
    }
}

impl Items {
    fn roll(&mut self) -> u64 {
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A new item of this row, off the shelf (the caller puts it somewhere).
    pub fn mint(&mut self, item: usize) -> Held {
        let uid = self.next_uid;
        self.next_uid += 1;
        Held { uid, item, xp: 0.0 }
    }

    /// A random row by rank weight; `only` narrows the choice.
    fn draw(&mut self, only: impl Fn(&ItemDef) -> bool) -> Option<usize> {
        let pool: Vec<(usize, u32)> = ITEMS
            .iter()
            .enumerate()
            .filter(|(_, d)| only(d))
            .map(|(i, d)| (i, d.rank.weight()))
            .collect();
        let total: u32 = pool.iter().map(|p| p.1).sum();
        if total == 0 {
            return None;
        }
        let mut at = (self.roll() % total as u64) as u32;
        for (i, w) in pool {
            if at < w {
                return Some(i);
            }
            at -= w;
        }
        None
    }

    /// The pedlar lays out a new stock of five. Specialists are one of a kind: he brings
    /// none the company already has (`owned`, see [`specialists_owned`]).
    pub fn restock(&mut self, owned: &HashSet<usize>) {
        self.offers.clear();
        let mut seen = HashSet::new();
        for _ in 0..40 {
            if self.offers.len() >= 5 {
                break;
            }
            if let Some(i) = self.draw(|_| true)
                && !owned.contains(&i)
                && seen.insert(i)
            {
                // Prices wander a fifth either way.
                let k = 0.8 + (self.roll() % 41) as f32 / 100.0;
                self.offers.push(Offer {
                    item: i,
                    price: ((ITEMS[i].rank.price() as f32 * k / 10.0).round() * 10.0) as u32,
                });
            }
        }
    }

    /// Put a new item of this row on the shelf; false if the shelf is full.
    pub fn gain(&mut self, item: usize) -> bool {
        if self.shelf.len() >= SHELF {
            return false;
        }
        let h = self.mint(item);
        self.shelf.push(h);
        true
    }
}

/// Good news about items, as a toast.
fn news(world: &mut World, kind: engine_ui::toast::ToastKind, title: &str, text: &str) {
    engine_ui::toast::notify(world, kind, "", title, text, 6.0);
}

/// The specialists the company has, on the shelf or in a slot: each is one of a kind.
pub fn specialists_owned(world: &mut World) -> HashSet<usize> {
    let mut out: HashSet<usize> = world
        .resource::<Items>()
        .shelf
        .iter()
        .map(|h| h.item)
        .collect();
    for inst in world.query::<&Institution>().iter(world) {
        out.extend(inst.slots.iter().flatten().map(|h| h.item));
    }
    out.retain(|i| ITEMS[*i].sort == Sort::Specialist);
    out
}

/// How well known the company is: people count for more the higher their tier.
pub fn renown(econ: &Economy) -> f32 {
    econ.islands
        .iter()
        .map(|b| b.population[0] + 2.0 * b.population[1] + 4.0 * b.population[2])
        .sum()
}

/// How many of each of the three buildings the company's renown allows.
pub fn allowed(econ: &Economy) -> usize {
    (1 + (renown(econ) / RENOWN_STEP) as usize).min(4)
}

/// Slots open in the buildings of an island: three, four once Burghers live there.
pub fn open_slots(econ: &Economy, island: usize) -> usize {
    let burghers = econ
        .islands
        .get(island)
        .map_or(0.0, |b| b.population[Tier::Burghers.index()]);
    if burghers >= FOURTH_SLOT_BURGHERS {
        4
    } else {
        3
    }
}

fn centre(b: &Building) -> DVec2 {
    grid::footprint_center(b.kind, b.origin, b.rot)
}

fn reach_m(kind: Kind) -> f64 {
    kind.def().reach as f64 * CELL_M
}

/// One building that holds items, as `apply` and `panel` see it.
struct Seat {
    island: usize,
    host: Host,
    at: DVec2,
    reach: f64,
    biome: Biome,
    held: Vec<Held>,
}

impl Seat {
    fn covers(&self, b: &Building) -> bool {
        b.island == self.island
            && self.host.acts_on(b.kind)
            && (centre(b) - self.at).length() <= self.reach
    }

    /// The guilds with three or more items here.
    fn sets(&self) -> Vec<Guild> {
        let mut n: HashMap<Guild, u32> = HashMap::new();
        for h in &self.held {
            if let Some(g) = h.def().guild {
                *n.entry(g).or_default() += 1;
            }
        }
        let mut out: Vec<Guild> = n
            .into_iter()
            .filter(|(_, c)| *c >= 3)
            .map(|g| g.0)
            .collect();
        out.sort_by_key(|g| g.name());
        out
    }
}

fn seats(world: &mut World) -> Vec<(Entity, Seat)> {
    let open: Vec<usize> = {
        let econ = world.resource::<Economy>();
        (0..econ.islands.len())
            .map(|i| open_slots(econ, i))
            .collect()
    };
    let mut q = world.query::<(Entity, &Building, &Institution)>();
    let mut out: Vec<(Entity, u32, Seat)> = q
        .iter(world)
        .filter_map(|(e, b, inst)| {
            let host = Host::of(b.kind)?;
            Some((
                e,
                b.id,
                Seat {
                    island: b.island,
                    host,
                    at: centre(b),
                    reach: reach_m(b.kind),
                    biome: islands()[b.island].biome(),
                    held: inst
                        .slots
                        .iter()
                        .take(open[b.island])
                        .flatten()
                        .cloned()
                        .collect(),
                },
            ))
        })
        .collect();
    out.sort_by_key(|s| s.1);
    out.into_iter().map(|(e, _, s)| (e, s)).collect()
}

/// Work out every building's [`Boost`] from the items in reach, and each island's extra
/// warehouse room. Runs after anything is built, pulled down or moved between slots.
pub fn apply(world: &mut World) {
    let seats: Vec<Seat> = seats(world).into_iter().map(|s| s.1).collect();
    let all: Vec<(Entity, Building)> = world
        .query::<(Entity, &Building)>()
        .iter(world)
        .map(|(e, b)| (e, b.clone()))
        .collect();
    let mut room = vec![0.0f64; islands().len()];
    let mut stored: HashSet<(usize, usize)> = HashSet::new();
    for s in &seats {
        let mut effects: Vec<(usize, Effect, f32)> = Vec::new();
        for h in &s.held {
            for e in h.def().effects {
                effects.push((h.item, *e, h.power(s.biome)));
            }
        }
        for g in s.sets() {
            effects.push((usize::MAX - g as usize, g.set(), 1.0));
        }
        for (item, e, k) in effects {
            if let Effect::Storage(n) = e
                && stored.insert((s.island, item))
            {
                room[s.island] += (n * k) as f64;
            }
        }
    }
    for (e, b) in &all {
        let mut boost = Boost::default();
        // The same item in two buildings acts once.
        let mut seen: HashSet<usize> = HashSet::new();
        for s in seats.iter().filter(|s| s.covers(b)) {
            for h in &s.held {
                let d = h.def();
                if d.fits(b.kind) && seen.insert(h.item) {
                    for eff in d.effects {
                        boost.add(*eff, h.power(s.biome));
                    }
                    boost.items += 1;
                }
            }
            for g in s.sets() {
                if seen.insert(usize::MAX - g as usize) {
                    boost.add(g.set(), 1.0);
                    boost.items += 1;
                }
            }
        }
        let had = world.get::<Boost>(*e).cloned();
        if boost == Boost::default() {
            if had.is_some() {
                world.entity_mut(*e).remove::<Boost>();
            }
        } else if had.as_ref() != Some(&boost) {
            world.entity_mut(*e).insert(boost);
        }
    }
    let mut econ = world.resource_mut::<Economy>();
    for (i, r) in room.into_iter().enumerate() {
        if let Some(b) = econ.islands.get_mut(i) {
            b.extra_room = r;
        }
    }
}

/// Put the shelf item `uid` into a slot (what was there goes back on the shelf).
pub fn socket(world: &mut World, building: u32, slot: usize, uid: u32) -> Result<(), String> {
    let e = crate::build::by_id(world, building).ok_or("No such building.")?;
    let b = world
        .get::<Building>(e)
        .cloned()
        .ok_or("No such building.")?;
    let host = Host::of(b.kind).ok_or("That building holds no items.")?;
    let open = open_slots(world.resource::<Economy>(), b.island);
    if slot >= open {
        return Err(if slot < 4 {
            "That slot opens once Burghers live on the island.".into()
        } else {
            "That slot is not open yet.".into()
        });
    }
    let at = world
        .resource::<Items>()
        .shelf
        .iter()
        .position(|h| h.uid == uid)
        .ok_or("That item is not on the shelf.")?;
    if world.resource::<Items>().shelf[at].def().host != host {
        return Err(format!(
            "{} belongs in a {}.",
            world.resource::<Items>().shelf[at].def().name,
            world.resource::<Items>().shelf[at]
                .def()
                .host
                .kind()
                .def()
                .name
        ));
    }
    let item = world.resource_mut::<Items>().shelf.remove(at);
    let old = world
        .get_mut::<Institution>(e)
        .and_then(|mut inst| inst.slots[slot].replace(item));
    if let Some(old) = old {
        world.resource_mut::<Items>().shelf.push(old);
    }
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Take the item out of a slot, back to the shelf.
pub fn unsocket(world: &mut World, building: u32, slot: usize) -> Result<(), String> {
    let e = crate::build::by_id(world, building).ok_or("No such building.")?;
    if world.resource::<Items>().shelf.len() >= SHELF {
        return Err("The shelf is full.".into());
    }
    let item = world
        .get_mut::<Institution>(e)
        .and_then(|mut inst| inst.slots.get_mut(slot).and_then(Option::take))
        .ok_or("That slot is empty.")?;
    world.resource_mut::<Items>().shelf.push(item);
    world.resource_mut::<Changed>().any = true;
    Ok(())
}

/// Buy the pedlar's `index`th offer.
pub fn buy(world: &mut World, index: usize) -> Result<&'static str, String> {
    let items = world.resource::<Items>();
    if !items.market_open {
        return Err("The pedlar is not at the quay.".into());
    }
    let offer = items.offers.get(index).cloned().ok_or("No such offer.")?;
    if items.shelf.len() >= SHELF {
        return Err("The shelf is full.".into());
    }
    if world.resource::<Economy>().coins < offer.price as f64 {
        return Err(format!("That costs {} coins.", offer.price));
    }
    world.resource_mut::<Economy>().coins -= offer.price as f64;
    let mut items = world.resource_mut::<Items>();
    items.offers.remove(index);
    items.gain(offer.item);
    Ok(ITEMS[offer.item].name)
}

/// Sell a shelf item for two fifths of its worth.
pub fn sell(world: &mut World, uid: u32) -> Result<u32, String> {
    let mut items = world.resource_mut::<Items>();
    let at = items
        .shelf
        .iter()
        .position(|h| h.uid == uid)
        .ok_or("That item is not on the shelf.")?;
    let h = items.shelf.remove(at);
    let coins = h.def().rank.price() * 2 / 5;
    world.resource_mut::<Economy>().coins += coins as f64;
    Ok(coins)
}

/// A building pulled down: its items go back on the shelf (or are lost if it is full).
pub fn vacate(world: &mut World, e: Entity) {
    let Some(inst) = world.get::<Institution>(e).cloned() else {
        return;
    };
    let mut items = world.resource_mut::<Items>();
    for h in inst.slots.into_iter().flatten() {
        if items.shelf.len() < SHELF {
            items.shelf.push(h);
        }
    }
}

/// An ocean packet fished something out of the sea: one voyage in eight brings an item.
pub fn flotsam(world: &mut World, ship: &str) {
    let mut items = world.resource_mut::<Items>();
    if !items.roll().is_multiple_of(8) {
        return;
    }
    // (Nothing storied floats about.)
    let Some(i) = items.draw(|d| d.rank < Rank::Storied && d.sort != Sort::Specialist) else {
        return;
    };
    if items.gain(i) {
        news(
            world,
            engine_ui::toast::ToastKind::Loot,
            "Flotsam",
            &format!("{ship} fished a crate from the sea: {}.", ITEMS[i].name),
        );
    }
}

/// Each frame: specialists in slots gain experience, and now and then one arrives on an
/// island where Burghers live well.
pub fn tick(world: &mut World) {
    let dt = {
        let speed = world.resource::<Economy>().speed;
        world.resource::<Time>().delta_secs() * speed
    };
    if dt <= 0.0 {
        return;
    }
    // Experience.
    let mut grown: Vec<String> = Vec::new();
    let mut q = world.query::<&mut Institution>();
    for mut inst in q.iter_mut(world) {
        for h in inst.slots.iter_mut().flatten() {
            if h.def().sort != Sort::Specialist {
                continue;
            }
            let before = h.stars();
            h.xp += dt;
            if h.stars() > before {
                grown.push(h.def().name.to_string());
            }
        }
    }
    if !grown.is_empty() {
        world.resource_mut::<Changed>().any = true;
        for name in grown {
            news(
                world,
                engine_ui::toast::ToastKind::Good,
                "A star earned",
                &format!("{name} has grown into the work and does a fifth more."),
            );
        }
    }
    // Arrivals.
    let clock = world.resource::<Economy>().clock_s;
    if clock < world.resource::<Items>().next_arrival_s {
        return;
    }
    let mut content: HashMap<usize, u32> = HashMap::new();
    for (b, h) in world.query::<(&Building, &House)>().iter(world) {
        if h.tier == Tier::Burghers && h.content_s > 0.0 {
            *content.entry(b.island).or_default() += 1;
        }
    }
    let mut best: Vec<(usize, u32)> = content.into_iter().filter(|c| c.1 >= 4).collect();
    best.sort_by_key(|c| (std::cmp::Reverse(c.1), c.0));
    let owned = specialists_owned(world);
    let mut items = world.resource_mut::<Items>();
    // (A game that starts warmed up does not owe an arrival for the time before it began.)
    let late = clock - items.next_arrival_s > ARRIVAL_EVERY_S;
    items.next_arrival_s = clock + ARRIVAL_EVERY_S;
    if late {
        return;
    }
    let Some((island, _)) = best.first().copied() else {
        return;
    };
    let Some(i) = items
        .draw(|d| d.sort == Sort::Specialist && !owned.iter().any(|o| ITEMS[*o].name == d.name))
    else {
        return;
    };
    if items.gain(i) {
        news(
            world,
            engine_ui::toast::ToastKind::Loot,
            "A specialist has arrived",
            &format!(
                "{} stepped off the packet at {} and waits on the shelf.",
                ITEMS[i].name,
                islands()[island].name
            ),
        );
    }
}

// ---- What the interface shows.

/// An item as the interface shows it.
#[derive(Clone, Debug, Serialize)]
pub struct ItemView {
    pub uid: u32,
    pub name: &'static str,
    pub blurb: &'static str,
    pub host: Host,
    pub sort: Sort,
    pub rank: Rank,
    pub guild: Option<&'static str>,
    pub icon: &'static str,
    pub stars: u32,
    /// Seconds to the next star (specialists below two stars).
    pub next_star_s: Option<f32>,
    /// The climate it does half as much again in, and whether it is there now.
    pub home: Option<&'static str>,
    pub at_home: bool,
    /// What it acts on, in words.
    pub on: String,
    /// What it does, at its present strength.
    pub effects: Vec<String>,
    /// What it is doing right now (items in a slot).
    pub now: Vec<String>,
    /// What it sells for.
    pub worth: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SlotView {
    pub slot: usize,
    pub open: bool,
    /// Why a closed slot is closed.
    pub locked: Option<&'static str>,
    pub item: Option<ItemView>,
}

/// One of the three buildings as the interface shows it.
#[derive(Clone, Debug, Serialize)]
pub struct Panel {
    pub building: u32,
    pub host: Host,
    pub name: &'static str,
    pub reach_m: f64,
    /// Buildings in reach its items could act on.
    pub in_reach: usize,
    pub slots: Vec<SlotView>,
    /// Guild sets complete here, with what each adds.
    pub sets: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OfferView {
    pub index: usize,
    pub price: u32,
    pub affordable: bool,
    pub item: ItemView,
}

fn biome_name(b: Biome) -> &'static str {
    match b {
        Biome::Temperate => "the Home Isles",
        Biome::Tropical => "Sarrowmere",
        Biome::Arid => "the Amber Reach",
        Biome::Arctic => "Frostholm",
    }
}

fn on_words(d: &ItemDef) -> String {
    if d.on.is_empty() {
        match d.host {
            Host::Union => "Every works and farm in reach".into(),
            Host::Hall => "Every house in reach".into(),
            Host::Office => "The quay, fisheries and masts in reach".into(),
        }
    } else {
        let names: Vec<&str> = d.on.iter().map(|k| k.def().name).collect();
        format!("{} in reach", names.join(", "))
    }
}

fn view(h: &Held, biome: Option<Biome>, now: Vec<String>) -> ItemView {
    let d = h.def();
    let at_home = d.home.is_some() && d.home == biome;
    let k = match biome {
        Some(b) => h.power(b),
        None => 1.0 + STAR_GAIN * h.stars() as f32,
    };
    ItemView {
        uid: h.uid,
        name: d.name,
        blurb: d.blurb,
        host: d.host,
        sort: d.sort,
        rank: d.rank,
        guild: d.guild.map(Guild::name),
        icon: d.icon_key(),
        stars: h.stars(),
        next_star_s: (d.sort == Sort::Specialist)
            .then(|| STAR_S.get(h.stars() as usize).map(|s| s - h.xp))
            .flatten(),
        home: d.home.map(biome_name),
        at_home,
        on: on_words(d),
        effects: d.effects.iter().map(|e| e.line(k)).collect(),
        now,
        worth: d.rank.price() * 2 / 5,
    }
}

/// What one item in a seat is doing right now, in goods, coins and people.
fn doing(world: &mut World, seat: &Seat, h: &Held) -> Vec<String> {
    let d = h.def();
    let k = h.power(seat.biome);
    let mut lines = Vec::new();
    let mut works: Vec<(Building, f32)> = Vec::new();
    for (b, p) in world.query::<(&Building, &Producer)>().iter(world) {
        if seat.covers(b) && d.fits(b.kind) {
            works.push((b.clone(), p.efficiency));
        }
    }
    let mut homes: Vec<(House, f32)> = Vec::new();
    let supply: Vec<[f32; Good::COUNT]> = world
        .resource::<Economy>()
        .islands
        .iter()
        .map(|b| b.supply)
        .collect();
    for (b, h) in world.query::<(&Building, &House)>().iter(world) {
        if seat.covers(b) && d.fits(b.kind) {
            homes.push((h.clone(), (h.residents / h.tier.capacity()).min(1.0)));
        }
    }
    let quays = world
        .query::<&Building>()
        .iter(world)
        .filter(|b| !matches!(b.kind, Kind::Fishery) && seat.covers(b) && d.fits(b.kind))
        .count();
    for e in d.effects {
        match *e {
            Effect::Faster(x) => {
                let mut by: Vec<(Good, f32)> = Vec::new();
                for (b, eff) in &works {
                    let def = b.kind.def();
                    if let Some(g) = def.output {
                        let n = 60.0 / def.cycle_s * eff * x * k;
                        match by.iter_mut().find(|r| r.0 == g) {
                            Some(r) => r.1 += n,
                            None => by.push((g, n)),
                        }
                    }
                }
                for (g, n) in by {
                    lines.push(format!("+{n:.1} {} a minute", low(g.name())));
                }
            }
            Effect::Hands(x) => {
                let n: f32 = works
                    .iter()
                    .filter_map(|(b, _)| b.kind.def().workers)
                    .map(|w| w.1 as f32 * x * k)
                    .sum();
                lines.push(format!("{n:.0} fewer workers needed"));
            }
            Effect::Thrift(c) => {
                let n: f32 = works
                    .iter()
                    .map(|(b, _)| (c * k).min(b.kind.def().upkeep))
                    .sum();
                lines.push(format!("{n:.0} coins a minute saved on upkeep"));
            }
            Effect::Bonus(g, n) => {
                let per: f32 = works
                    .iter()
                    .map(|(b, eff)| 60.0 / b.kind.def().cycle_s * eff / nth_at(n, k) as f32)
                    .sum();
                lines.push(format!("+{per:.1} {} a minute", low(g.name())));
            }
            Effect::Instead(_, b) => {
                lines.push(format!("{} works take {}", works.len(), low(b.name())));
            }
            Effect::Spare2 => {
                lines.push(format!(
                    "{} works do without their second good",
                    works.len()
                ));
            }
            Effect::Pays(need, c) => {
                let mut coins = 0.0;
                let mut n = 0;
                for (h, full) in &homes {
                    let met = match need {
                        None => 1.0,
                        Some(Need::Good(g)) if h.tier.needs().0.contains(&g) => {
                            supply[seat.island][g.index()].min(1.0)
                        }
                        Some(Need::Service(s)) => h
                            .tier
                            .services()
                            .iter()
                            .position(|k| *k == s)
                            .map_or(0.0, |i| if h.services[i] { 1.0 } else { 0.0 }),
                        _ => 0.0,
                    };
                    if met > 0.0 {
                        coins += c * k * met * full;
                        n += 1;
                    }
                }
                lines.push(format!("+{coins:.1} coins a minute from {n} houses"));
            }
            Effect::Provides(g) => {
                let n = homes
                    .iter()
                    .filter(|(h, _)| h.tier.needs().0.contains(&g))
                    .count();
                lines.push(format!("{n} houses need no {}", low(g.name())));
            }
            Effect::Serves(s) => {
                let n = homes
                    .iter()
                    .filter(|(h, _)| h.tier.services().contains(&s))
                    .count();
                lines.push(format!(
                    "{n} houses count as having a {}",
                    low(s.def().name)
                ));
            }
            Effect::Lodgers(n) => {
                let p: f32 = homes.iter().map(|(_, full)| n * k * full).sum();
                lines.push(format!("+{p:.0} people in {} houses", homes.len()));
            }
            Effect::Loading(x) => {
                if quays > 0 {
                    lines.push(format!(
                        "Ships turn round {:.0} s sooner",
                        18.0 * (x * k).min(0.8)
                    ));
                }
            }
            Effect::Storage(n) => lines.push(format!(
                "+{:.0} room in {}'s warehouse",
                n * k,
                islands()[seat.island].name
            )),
            Effect::Duty(_) => {
                let earned = world.resource::<Items>().duty_earned;
                lines.push(format!("{earned:.0} coins of duty taken so far"));
            }
            Effect::Sail(x) => {
                if quays > 0 {
                    lines.push(format!(
                        "Ships leave {:.0} % faster",
                        (x * k).min(0.6) * 100.0
                    ));
                }
            }
        }
    }
    if works.is_empty() && homes.is_empty() && quays == 0 {
        lines.clear();
        lines.push("Nothing in reach for it to work on".into());
    }
    lines
}

/// The slots of one of the three buildings and what each item is doing.
pub fn panel(world: &mut World, e: Entity) -> Option<Panel> {
    let b = world.get::<Building>(e)?.clone();
    let host = Host::of(b.kind)?;
    let inst = world.get::<Institution>(e)?.clone();
    let open = open_slots(world.resource::<Economy>(), b.island);
    let seat = seats(world).into_iter().find(|s| s.0 == e)?.1;
    let in_reach = world
        .query::<&Building>()
        .iter(world)
        .filter(|x| seat.covers(x))
        .count();
    let mut slots = Vec::new();
    for (i, s) in inst.slots.iter().enumerate().take(4) {
        let is_open = i < open;
        let item = s.as_ref().map(|h| {
            let now = if is_open {
                doing(world, &seat, h)
            } else {
                Vec::new()
            };
            view(h, Some(seat.biome), now)
        });
        slots.push(SlotView {
            slot: i,
            open: is_open,
            locked: (!is_open).then_some("Opens once 60 Burghers live on the island"),
            item,
        });
    }
    Some(Panel {
        building: b.id,
        host,
        name: b.kind.def().name,
        reach_m: seat.reach,
        in_reach,
        slots,
        sets: seat
            .sets()
            .into_iter()
            .map(|g| format!("{}: {}", g.name(), g.set().line(1.0)))
            .collect(),
    })
}

/// The shelf, as the interface shows it.
pub fn shelf_view(world: &World) -> Vec<ItemView> {
    world
        .resource::<Items>()
        .shelf
        .iter()
        .map(|h| view(h, None, Vec::new()))
        .collect()
}

/// The pedlar's stock (empty when he is away).
pub fn offers_view(world: &World) -> Vec<OfferView> {
    let items = world.resource::<Items>();
    if !items.market_open {
        return Vec::new();
    }
    let coins = world.resource::<Economy>().coins;
    items
        .offers
        .iter()
        .enumerate()
        .map(|(index, o)| OfferView {
            index,
            price: o.price,
            affordable: coins >= o.price as f64,
            item: view(
                &Held {
                    uid: 0,
                    item: o.item,
                    xp: 0.0,
                },
                None,
                Vec::new(),
            ),
        })
        .collect()
}

/// The buildings of `kind`'s host that an item-holder at `at` on `island` would act on:
/// where the reach markers go.
pub fn in_reach(world: &mut World, e: Entity) -> Vec<(Entity, Building, u32)> {
    let Some(seat) = seats(world).into_iter().find(|s| s.0 == e).map(|s| s.1) else {
        return Vec::new();
    };
    world
        .query::<(Entity, &Building, Option<&Boost>)>()
        .iter(world)
        .filter(|(_, b, _)| seat.covers(b))
        .map(|(e, b, boost)| (e, b.clone(), boost.map_or(0, |x| x.items)))
        .collect()
}

/// Every building that holds items, by id.
pub fn holders(world: &mut World) -> Vec<Entity> {
    seats(world).into_iter().map(|s| s.0).collect()
}

/// Everything about items at once, for the API: renown, how many of each building it
/// allows, every building's slots, the shelf and the pedlar's stock.
pub fn state(world: &mut World) -> serde_json::Value {
    let panels: Vec<Panel> = holders(world)
        .into_iter()
        .filter_map(|e| panel(world, e))
        .collect();
    let econ = world.resource::<Economy>();
    let items = world.resource::<Items>();
    serde_json::json!({
        "renown": renown(econ).round(),
        "allowed_of_each": allowed(econ),
        "next_at_renown": (allowed(econ) < 4).then(|| allowed(econ) as f32 * RENOWN_STEP),
        "buildings": panels,
        "shelf": shelf_view(world),
        "shelf_room": SHELF,
        "pedlar_here": items.market_open,
        "offers": offers_view(world),
        "duty_earned": items.duty_earned.round(),
    })
}

/// The whole item table in words, for the API and the design notes.
pub fn catalogue() -> serde_json::Value {
    let rows: Vec<serde_json::Value> = ITEMS
        .iter()
        .enumerate()
        .map(|(i, d)| {
            serde_json::json!({
                "item": i,
                "name": d.name,
                "blurb": d.blurb,
                "host": d.host,
                "sort": d.sort,
                "rank": d.rank,
                "price": d.rank.price(),
                "guild": d.guild.map(Guild::name),
                "home": d.home.map(biome_name),
                "on": on_words(d),
                "effects": d.effects.iter().map(|e| e.line(1.0)).collect::<Vec<_>>(),
                "icon": d.icon_key(),
            })
        })
        .collect();
    let guilds: Vec<serde_json::Value> = Guild::ALL
        .iter()
        .map(|g| serde_json::json!({"guild": g.name(), "three_add": g.set().line(1.0)}))
        .collect();
    serde_json::json!({"items": rows, "guilds": guilds})
}

/// `isle.item.<verb>:<args>` from the interface's buttons.
///
/// - `socket:<building id>:<slot>:<uid>`, `unsocket:<building id>:<slot>`
/// - `buy:<offer index>`, `sell:<uid>`
pub fn action(world: &mut World, a: &str) -> Result<(), String> {
    let mut parts = a.split(':');
    let verb = parts.next().unwrap_or("");
    let nums: Vec<u32> = parts
        .map(|s| s.parse::<u32>().map_err(|_| format!("{s} is not a number")))
        .collect::<Result<_, _>>()?;
    let arg = |k: usize| nums.get(k).copied().ok_or("missing a number".to_string());
    match verb {
        "socket" => socket(world, arg(0)?, arg(1)? as usize, arg(2)?),
        "unsocket" => unsocket(world, arg(0)?, arg(1)? as usize),
        "buy" => {
            let name = buy(world, arg(0)? as usize)?;
            news(
                world,
                engine_ui::toast::ToastKind::Good,
                "Bought",
                &format!("{name} is on the shelf."),
            );
            Ok(())
        }
        "sell" => {
            let coins = sell(world, arg(0)?)?;
            news(
                world,
                engine_ui::toast::ToastKind::Good,
                "Sold",
                &format!("The pedlar paid {coins} coins."),
            );
            Ok(())
        }
        _ => Err(format!("unknown item action {verb}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sound() {
        let mut names = HashSet::new();
        for d in ITEMS {
            assert!(names.insert(d.name), "{} twice", d.name);
            assert!(!d.effects.is_empty() && !d.blurb.is_empty(), "{}", d.name);
            // Everything it names is something its building acts on.
            for k in d.on {
                assert!(d.host.acts_on(*k), "{} cannot act on {k:?}", d.name);
            }
            // Only people come from somewhere.
            assert!(d.home.is_none() || d.sort == Sort::Specialist, "{}", d.name);
            for e in d.effects {
                let house = matches!(
                    e,
                    Effect::Pays(..) | Effect::Provides(_) | Effect::Serves(_) | Effect::Lodgers(_)
                );
                assert_eq!(house, d.host == Host::Hall, "{}: {e:?}", d.name);
                assert!(!e.line(1.0).is_empty());
            }
        }
        for host in Host::ALL {
            let n = ITEMS.iter().filter(|d| d.host == host).count();
            assert!(n >= 15, "{host:?} has {n} items");
        }
        // Every guild can be completed in one building.
        for g in Guild::ALL {
            let of: Vec<&ItemDef> = ITEMS.iter().filter(|d| d.guild == Some(g)).collect();
            assert!(of.len() >= 3, "{g:?}");
            assert!(of.iter().all(|d| d.host == of[0].host), "{g:?}");
        }
    }

    #[test]
    fn stars_and_home_make_an_item_stronger() {
        let furrier = ITEMS
            .iter()
            .position(|d| d.name.starts_with("Master furrier"))
            .unwrap();
        let mut h = Held {
            uid: 1,
            item: furrier,
            xp: 0.0,
        };
        assert_eq!(h.power(Biome::Temperate), 1.0);
        assert_eq!(h.power(Biome::Arctic), 1.5);
        h.xp = STAR_S[1];
        assert_eq!(h.stars(), 2);
        assert!((h.power(Biome::Arctic) - 1.9).abs() < 1e-6);
        // A stronger hand brings the extra good round sooner.
        assert_eq!(nth_at(3, 1.0), 3);
        assert_eq!(nth_at(3, 1.9), 2);
        let mut b = Boost::default();
        for e in h.def().effects {
            b.add(*e, h.power(Biome::Arctic));
        }
        assert!((b.faster - 0.475).abs() < 1e-6);
        assert_eq!(b.bonus, vec![(Good::Clothes, 2)]);
    }

    fn start() -> World {
        let mut w = crate::town::tests::bare_world();
        crate::town::build_start(&mut w);
        w
    }

    fn holder(w: &mut World, kind: Kind, island: usize) -> (Entity, Building) {
        w.query::<(Entity, &Building)>()
            .iter(w)
            .find(|(_, b)| b.kind == kind && b.island == island)
            .map(|(e, b)| (e, b.clone()))
            .unwrap_or_else(|| panic!("no {kind:?} on island {island}"))
    }

    #[test]
    fn the_start_has_the_three_buildings_at_work() {
        let mut w = start();
        crate::town::prewarm(&mut w, 300.0);
        let allowed = allowed(w.resource::<Economy>());
        for host in Host::ALL {
            let n = w
                .query::<&Building>()
                .iter(&w)
                .filter(|b| b.kind == host.kind())
                .count();
            assert!(n >= 1 && n <= allowed, "{n} {host:?}, {allowed} allowed");
            let (e, _) = holder(&mut w, host.kind(), 0);
            let p = panel(&mut w, e).unwrap();
            if p.in_reach < 2 {
                let (_, hb) = holder(&mut w, host.kind(), 0);
                let near: Vec<String> = w
                    .query::<&Building>()
                    .iter(&w)
                    .filter(|b| b.island == 0 && host.acts_on(b.kind))
                    .map(|b| format!("{:?} {:.0} m", b.kind, (centre(b) - centre(&hb)).length()))
                    .collect();
                panic!("{host:?} reaches {}: {near:?}", p.in_reach);
            }
            assert_eq!(p.slots.len(), 4);
            assert!(p.slots[0].open && p.slots[2].open);
            // Both starting items have something to work on and say what they are doing.
            for s in p.slots.iter().take(2) {
                let item = s.item.as_ref().unwrap();
                assert!(!item.now.is_empty(), "{}", item.name);
                assert!(
                    !item.now[0].starts_with("Nothing"),
                    "{} at the {host:?}: {:?}",
                    item.name,
                    item.now
                );
            }
        }
        // The harbour is in the office's reach, and ships turn round sooner there.
        let (he, _) = holder(&mut w, Kind::Harbour, 0);
        let boost = w.get::<Boost>(he).cloned().unwrap();
        assert!(boost.loading > 0.25 && boost.sail > 0.05, "{boost:?}");
        // The shelf has things to try, and the pedlar is in with five more.
        let state = state(&mut w);
        assert_eq!(state["shelf"].as_array().unwrap().len(), 5);
        assert_eq!(state["offers"].as_array().unwrap().len(), 5);
        assert_eq!(state["buildings"].as_array().unwrap().len(), 4);
        assert_eq!(catalogue()["items"].as_array().unwrap().len(), ITEMS.len());
    }

    #[test]
    #[ignore = "prints what each item building reaches; for laying out the start"]
    fn print_reach() {
        let mut w = start();
        for e in holders(&mut w) {
            let hb = w.get::<Building>(e).cloned().unwrap();
            let mut near: Vec<(i64, String)> = w
                .query::<&Building>()
                .iter(&w)
                .filter(|b| b.island == hb.island && Host::of(hb.kind).unwrap().acts_on(b.kind))
                .map(|b| {
                    (
                        (centre(b) - centre(&hb)).length() as i64,
                        format!("{:?}", b.kind),
                    )
                })
                .collect();
            near.sort();
            let reach = reach_m(hb.kind) as i64;
            let inside = near.iter().filter(|n| n.0 <= reach).count();
            let list: Vec<String> = near
                .iter()
                .filter(|n| hb.kind != Kind::TownHall || n.0 > reach - 30)
                .take(24)
                .map(|n| format!("{} {}", n.1, n.0))
                .collect();
            eprintln!(
                "{:?} on {} (reach {reach} m, {inside} of {} inside): {list:?}",
                hb.kind,
                islands()[hb.island].name,
                near.len()
            );
        }
        let econ = w.resource::<Economy>();
        eprintln!("renown {:.0}, allowed {}", renown(econ), allowed(econ));
    }

    #[test]
    #[ignore = "prints the item table as Markdown, for the design notes"]
    fn print_table() {
        for host in Host::ALL {
            println!("\n**{}**\n", host.kind().def().name);
            println!("| Item | Kind, rank | Acts on | Does | Guild |");
            println!("|---|---|---|---|---|");
            for d in ITEMS.iter().filter(|d| d.host == host) {
                let does: Vec<String> = d.effects.iter().map(|e| e.line(1.0)).collect();
                let home = d.home.map_or(String::new(), |b| {
                    format!(" (at home in {})", biome_name(b))
                });
                println!(
                    "| {} | {:?}, {:?}, {} coins | {} | {}{} | {} |",
                    d.name,
                    d.sort,
                    d.rank,
                    d.rank.price(),
                    on_words(d),
                    does.join("; "),
                    home,
                    d.guild.map_or("", Guild::name)
                );
            }
        }
        println!();
        for g in Guild::ALL {
            println!("- Three of the {}: {}", g.name(), g.set().line(1.0));
        }
    }

    #[test]
    fn slots_take_shelf_items_of_their_own_kind() {
        let mut w = start();
        let (_, hall) = holder(&mut w, Kind::TownHall, 0);
        let (_, union) = holder(&mut w, Kind::TradingUnion, 0);
        let uid = |w: &World, name: &str| {
            w.resource::<Items>()
                .shelf
                .iter()
                .find(|h| h.def().name == name)
                .map(|h| h.uid)
                .unwrap()
        };
        let fish = uid(&w, "Fishwives' stalls");
        // Not in a Trading Union; not in a slot that is not open; yes in the hall's third.
        assert!(socket(&mut w, union.id, 2, fish).is_err());
        assert!(socket(&mut w, hall.id, 4, fish).is_err());
        socket(&mut w, hall.id, 2, fish).unwrap();
        crate::build::refresh(&mut w);
        // Three Victuallers in one hall: the set pays from every house in reach.
        let (he, _) = holder(&mut w, Kind::TownHall, 0);
        let p = panel(&mut w, he).unwrap();
        assert_eq!(p.sets.len(), 1, "{:?}", p.sets);
        let with_set = w
            .query::<(&House, &Boost)>()
            .iter(&w)
            .map(|(_, b)| b.flat_pay())
            .fold(0.0f32, f32::max);
        assert!((with_set - 1.2).abs() < 1e-4, "{with_set}");
        // Out again: it is back on the shelf and the set is gone.
        action(&mut w, &format!("unsocket:{}:2", hall.id)).unwrap();
        crate::build::refresh(&mut w);
        assert_eq!(uid(&w, "Fishwives' stalls"), fish);
        let p = panel(&mut w, he).unwrap();
        assert!(p.sets.is_empty());
        // Pulling the hall down sends its items to the shelf and frees the houses.
        let before = w.resource::<Items>().shelf.len();
        crate::build::demolish(&mut w, he);
        crate::build::refresh(&mut w);
        assert_eq!(w.resource::<Items>().shelf.len(), before + 2);
        assert_eq!(w.query::<(&House, &Boost)>().iter(&w).count(), 0);
        // Buying and selling.
        let coins = w.resource::<Economy>().coins;
        let price = w.resource::<Items>().offers[0].price;
        buy(&mut w, 0).unwrap();
        assert_eq!(w.resource::<Economy>().coins, coins - price as f64);
        let last = w.resource::<Items>().shelf.last().unwrap().clone();
        let got = sell(&mut w, last.uid).unwrap();
        assert_eq!(got, last.def().rank.price() * 2 / 5);
    }

    #[test]
    fn items_change_what_works_and_houses_do() {
        let mut w = start();
        // The flying-shuttle loom: the weaver in the union's reach finishes half as many
        // batches again as the one out of it, with fewer hands.
        crate::town::prewarm(&mut w, 600.0);
        let mut q = w.query::<(&Building, &Producer, Option<&Boost>)>();
        let mut looms = Vec::new();
        for (b, p, boost) in q.iter(&w) {
            if b.kind == Kind::Weaver && b.island == 0 {
                looms.push((boost.map_or(0.0, |x| x.faster), p.made));
            }
        }
        looms.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(
            looms.len() == 2 && looms[0].0 == 0.0 && looms[1].0 == 0.5,
            "{looms:?}"
        );
        assert!(looms[1].1 as f32 > looms[0].1 as f32 * 1.3, "{looms:?}");
        // The master baker on Brannsey: date cakes come out of bread ovens.
        let cakes = w.resource::<Economy>().islands[1].made_per_min[Good::DateCakes.index()];
        let ovens = q
            .iter(&w)
            .filter(|(b, _, boost)| {
                b.kind == Kind::Bakery && boost.is_some_and(|x| !x.bonus.is_empty())
            })
            .count();
        assert!(ovens >= 2, "{ovens} bakeries in the baker's reach");
        assert!(cakes > 0.2, "{cakes} date cakes a minute from the ovens");
        // The town crier and the baker's round: houses in the hall's reach pay more than
        // their tier's plain sum.
        let mut more = 0;
        for (h, boost) in w.query::<(&House, &Boost)>().iter(&w) {
            assert!(boost.flat_pay() > 0.39);
            if h.income > h.tier.full_income() + 0.3 {
                more += 1;
            }
        }
        assert!(more >= 8, "{more} houses pay more for the hall's items");
    }

    #[test]
    fn the_pedlar_lays_out_five_different_items() {
        let mut items = Items::default();
        items.restock(&HashSet::new());
        assert_eq!(items.offers.len(), 5);
        let set: HashSet<usize> = items.offers.iter().map(|o| o.item).collect();
        assert_eq!(set.len(), 5);
        for o in &items.offers {
            let p = ITEMS[o.item].rank.price() as f32;
            assert!(o.price as f32 >= p * 0.79 && o.price as f32 <= p * 1.21);
        }
    }
}
