//! Rival captains: the AI that plays a rival house's company with the same orders the
//! player has (build, settle, order ships, open routes, move houses up), paying from its
//! own purse, on islands it has settled itself. No cheats at any level: a captain sees only
//! the islands its ships have seen ([`Charted`]), and the game refuses it what it refuses
//! the player (no coins, no planks, another company's island).
//!
//! The play itself is the playtest's careful player ([`crate::playtest`]), run for the
//! company whose turn it is ([`crate::sim::Acting`]). Five levels of difficulty scale it
//! down by how often it looks at its towns, how often it forgets something, and how many
//! coins it keeps in hand ([`Level::knobs`]).
//!
//! `--rivals N` (0 to 3) and `--rival-level admiral` start a new game with rivals; the
//! economy rig also plays the player's seat at a level (`--play-level`).

use engine::prelude::*;
use serde::Serialize;

use crate::fleet::{Doing, Roam, Vessel, spawn_vessel};
use crate::island::{self, islands};
use crate::playtest::Log;
use crate::sim::{Acting, Company, Economy, PLAYER};
use crate::vessels::Craft;

/// How hard the rivals play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Deckhand,
    Mate,
    Captain,
    Commodore,
    Admiral,
}

/// What a level changes: nothing the game gives, only how well the captain plays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Knobs {
    /// Game seconds between its turns.
    pub turn_s: f64,
    /// Chance that it forgets to look at something on a turn (an island's needs, its trade).
    pub slip: f32,
    /// Coins it keeps in hand before building anything but the basics.
    pub reserve: f64,
    /// It settles new islands early and on a smaller town, racing for the best ones.
    pub race: bool,
    /// How many links of a chain it plans at once: 0 is the works alone, 1 its inputs too,
    /// and so on.
    pub depth: u32,
    /// It plans chains across its islands (a farm on one feeding a works on another).
    pub across: bool,
    /// It buys what it lacks at other companies' harbours.
    pub abroad: bool,
    /// Works beyond the basics (timber, fish, markets, houses, what the first two tiers
    /// cannot do without) it puts up in one turn.
    pub works: u32,
    /// The most islands it holds.
    pub isles: usize,
    /// On a full island it takes down a works it can spare to make room.
    pub clears: bool,
}

impl Level {
    pub const ALL: [Level; 5] = [
        Level::Deckhand,
        Level::Mate,
        Level::Captain,
        Level::Commodore,
        Level::Admiral,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Level::Deckhand => "Deckhand",
            Level::Mate => "Mate",
            Level::Captain => "Captain",
            Level::Commodore => "Commodore",
            Level::Admiral => "Admiral",
        }
    }

    pub fn from_name(s: &str) -> Option<Level> {
        Level::ALL
            .into_iter()
            .find(|l| l.name().eq_ignore_ascii_case(s))
    }

    pub fn knobs(self) -> Knobs {
        let (turn_s, slip, reserve, race, depth, across, abroad, works, isles) = match self {
            Level::Deckhand => (120.0, 1.0 / 3.0, 3_000.0, false, 1, false, false, 1, 1),
            Level::Mate => (60.0, 1.0 / 5.0, 2_000.0, false, 1, false, false, 1, 2),
            Level::Captain => (30.0, 1.0 / 10.0, 1_200.0, false, 2, false, false, 2, 3),
            Level::Commodore => (15.0, 1.0 / 25.0, 800.0, true, 3, true, false, 6, 5),
            Level::Admiral => (5.0, 0.0, 800.0, true, 3, true, true, 99, 99),
        };
        Knobs {
            turn_s,
            slip,
            reserve,
            race,
            depth,
            across,
            abroad,
            works,
            isles,
            clears: self == Level::Admiral,
        }
    }

    /// Whether one rival starts in the player's home waters.
    pub fn one_near(self) -> bool {
        matches!(self, Level::Commodore | Level::Admiral)
    }
}

/// The knobs of whoever is playing this turn (the bot reads them).
#[derive(Resource, Clone, Copy, Debug)]
pub struct Seat {
    pub knobs: Knobs,
}

/// How a new game's rivals are set up (`--rivals`, `--rival-level`).
#[derive(Resource, Clone, Copy, Debug)]
pub struct RivalSetup {
    pub count: usize,
    pub level: Level,
}

/// One rival house's captain.
pub struct Captain {
    pub company: Company,
    pub level: Level,
    /// The island it began on.
    pub home: usize,
    next_s: f64,
    pub log: Log,
}

/// Every rival captain at work.
#[derive(Resource, Default)]
pub struct Captains(pub Vec<Captain>);

/// The islands each company's ships have seen, as bits by island index (company `c` is
/// entry `c`).
#[derive(Resource, Default, Debug, Clone)]
pub struct Charted {
    seen: Vec<u64>,
    next_s: f64,
}

impl Charted {
    pub fn knows(&self, c: Company, isle: usize) -> bool {
        self.seen
            .get(c as usize)
            .is_some_and(|b| b & (1 << isle) != 0)
    }

    fn mark(&mut self, c: Company, isle: usize) {
        let c = c as usize;
        if self.seen.len() <= c {
            self.seen.resize(c + 1, 0);
        }
        self.seen[c] |= 1 << isle;
    }
}

/// The island company `c` began on (the player's is Haverholm).
pub fn home_of(world: &World, c: Company) -> usize {
    if c == PLAYER {
        return 0;
    }
    world
        .get_resource::<Captains>()
        .and_then(|k| k.0.iter().find(|x| x.company == c))
        .map_or(0, |x| x.home)
}

/// Whether company `c`'s ships have seen island `isle` (always, in a game that does not
/// keep a chart).
pub fn knows(world: &World, c: Company, isle: usize) -> bool {
    world
        .get_resource::<Charted>()
        .is_none_or(|ch| ch.knows(c, isle))
}

/// How far a ship sees an island's coast, meters.
const SIGHT_M: f64 = 1_400.0;

/// Mark the islands each company's ships are near, and those it holds.
pub fn chart(world: &mut World) {
    let Some(mut ch) = world.remove_resource::<Charted>() else {
        return;
    };
    let now = world.resource::<Economy>().clock_s;
    if now >= ch.next_s {
        ch.next_s = now + 2.0;
        let econ = world.resource::<Economy>();
        for (i, b) in econ.islands.iter().enumerate() {
            if b.harbours > 0 {
                ch.mark(b.owner, i);
            }
        }
        let mut q = world.query::<&Vessel>();
        for v in q.iter(world).filter(|v| v.owned()) {
            let here = DVec2::new(v.at.x, v.at.z);
            let far = SIGHT_M * (1.0 + v.kit.sight as f64);
            for (i, isle) in islands().iter().enumerate() {
                if (isle.center - here).length() < isle.radius + far {
                    ch.mark(v.company, i);
                }
            }
        }
    }
    world.insert_resource(ch);
}

/// Give each captain whose turn it is its turn, as its company.
pub fn tick(world: &mut World) {
    chart(world);
    if world
        .get_resource::<Captains>()
        .is_none_or(|k| k.0.is_empty())
    {
        return;
    }
    let now = world.resource::<Economy>().clock_s;
    let was_acting = world.get_resource::<Acting>().copied();
    let was_seat = world.get_resource::<Seat>().copied();
    let n = world.resource::<Captains>().0.len();
    for k in 0..n {
        // (The captains stay in place, so [`home_of`] finds them; only the log is lent out.)
        let (company, knobs, mut log) = {
            let mut caps = world.resource_mut::<Captains>();
            let cap = &mut caps.0[k];
            if now < cap.next_s {
                continue;
            }
            let knobs = cap.level.knobs();
            cap.next_s = now + knobs.turn_s;
            (cap.company, knobs, std::mem::take(&mut cap.log))
        };
        world.insert_resource(Acting(company));
        world.insert_resource(Seat { knobs });
        crate::playtest::turn(world, &mut log);
        world.resource_mut::<Captains>().0[k].log = log;
    }
    match was_acting {
        Some(a) => world.insert_resource(a),
        None => {
            world.remove_resource::<Acting>();
        }
    }
    match was_seat {
        Some(s) => world.insert_resource(s),
        None => {
            world.remove_resource::<Seat>();
        }
    }
}

/// Where rival `c` starts: one in the player's home waters on the hardest levels, the
/// others each in a region of its own.
fn start_island(c: Company, level: Level) -> usize {
    let by_name = |n: &str| islands().iter().position(|i| i.name == n).unwrap_or(0);
    match (c, level.one_near()) {
        (1, true) => by_name("Northmoor"),
        (1, false) => by_name("Amberland"),
        (2, _) => by_name("Frostholm"),
        _ => by_name("Sarrowmere"),
    }
}

/// A new game's rivals: each house gets the player's start, a barque off its own island with
/// planks, fish and clothes aboard and the same purse, and a captain at `level`.
pub fn start_rivals(world: &mut World, count: usize, level: Level) {
    world.init_resource::<Charted>();
    world.init_resource::<Captains>();
    {
        // The player knows Haverholm from the start.
        let mut ch = world.resource_mut::<Charted>();
        ch.mark(PLAYER, 0);
    }
    for k in 0..count.min(3) {
        let c = (k + 1) as Company;
        let home = start_island(c, level);
        *world.resource_mut::<Economy>().coins_mut(c) = crate::settle::FIRST_COINS;
        let isle = &islands()[home];
        let coast = island::landing_coast(isle).unwrap_or(isle.center);
        let out = (coast - isle.center).normalize_or(DVec2::X);
        let mut at = coast + out * 420.0;
        let mut n = 0;
        while island::height(at.x, at.y) > -8.0 && n < 20 {
            at += out * 40.0;
            n += 1;
        }
        let heading = (out.y).atan2(-out.x) as f32;
        let name = crate::rivals::ship_names(c)[0];
        let was = world.get_resource::<Acting>().copied();
        world.insert_resource(Acting(c));
        let e = spawn_vessel(
            world,
            Craft::Barque,
            name,
            DVec3::new(at.x, 0.0, at.y),
            heading,
            None,
            Roam::Idle {
                at: DVec3::new(at.x, 0.0, at.y),
            },
            9_000 + c as u64,
        );
        match was {
            Some(a) => world.insert_resource(a),
            None => {
                world.remove_resource::<Acting>();
            }
        }
        if let Some(mut v) = world.get_mut::<Vessel>(e) {
            v.cargo = crate::settle::FIRST_CARGO.to_vec();
            v.doing = Doing::Moored(30.0);
            v.company = c;
        }
        world.resource_mut::<Charted>().mark(c, home);
        world.resource_mut::<Captains>().0.push(Captain {
            company: c,
            level,
            home,
            // Each takes its turns at its own second.
            next_s: 3.0 * c as f64,
            log: Log::for_captain(1 + c as u64),
        });
        engine::core::log::info!(
            "archipelago: {} starts off {} ({})",
            crate::rivals::company_name(world, c),
            isle.name,
            level.name()
        );
    }
}

/// The sandbox start with rivals (`--rivals N` on a tour or `--sandbox`): the far regions'
/// towns, already built, go to the houses (Harrowgate the Sarrowmere coast, Gullwick the
/// Amber Reach, Ostrava the north), each with the start purse and a captain at `level`.
pub fn adopt_far_towns(world: &mut World, count: usize, level: Level) {
    world.init_resource::<Charted>();
    world.init_resource::<Captains>();
    world.resource_mut::<Charted>().mark(PLAYER, 0);
    for k in 0..count.min(3) {
        let c = (k + 1) as Company;
        let region = match c {
            1 => 2,
            2 => 3,
            _ => 1,
        };
        let isles: Vec<usize> = islands()
            .iter()
            .enumerate()
            .filter(|(i, isle)| {
                isle.region() == region && world.resource::<Economy>().islands[*i].harbours > 0
            })
            .map(|(i, _)| i)
            .collect();
        let Some(&home) = isles.first() else {
            continue;
        };
        {
            let mut econ = world.resource_mut::<Economy>();
            *econ.coins_mut(c) = crate::settle::FIRST_COINS;
            for &i in &isles {
                econ.islands[i].owner = c;
            }
        }
        for &i in &isles {
            world.resource_mut::<Charted>().mark(c, i);
        }
        world.resource_mut::<Captains>().0.push(Captain {
            company: c,
            level,
            home,
            next_s: 3.0 * c as f64,
            log: Log::for_captain(1 + c as u64),
        });
    }
}

/// One line per company for the playtest's timeline: purse, people and islands.
pub fn standings(world: &World) -> Vec<serde_json::Value> {
    let econ = world.resource::<Economy>();
    (0..econ.companies())
        .map(|c| {
            let c = c as Company;
            let isles: Vec<usize> = (0..econ.islands.len())
                .filter(|&i| econ.owns(c, i))
                .collect();
            let people: f32 = isles
                .iter()
                .map(|&i| econ.islands[i].population.iter().sum::<f32>())
                .sum();
            let tiers: Vec<f32> = (0..3)
                .map(|t| {
                    isles
                        .iter()
                        .map(|&i| econ.islands[i].population[t])
                        .sum::<f32>()
                        .round()
                })
                .collect();
            // (Where the money goes: taxes in, upkeep out, of which ships and public works.)
            let (taxes, upkeep, fleet, public) = if c == PLAYER {
                (
                    econ.taxes_per_min,
                    econ.upkeep_per_min,
                    econ.fleet_upkeep,
                    econ.public_upkeep,
                )
            } else {
                econ.rivals
                    .get(c as usize - 1)
                    .map_or((0.0, 0.0, 0.0, 0.0), |p| {
                        (
                            p.taxes_per_min,
                            p.upkeep_per_min,
                            p.fleet_upkeep,
                            p.public_upkeep,
                        )
                    })
            };
            serde_json::json!({
                "company": c,
                "coins": econ.coins_of(c).round(),
                "balance": econ.balance_of(c).round(),
                "taxes": taxes.round(),
                "upkeep": upkeep.round(),
                "fleet": fleet.round(),
                "public": public.round(),
                "people": people.round(),
                "tiers": tiers,
                "islands": isles.len(),
            })
        })
        .collect()
}
