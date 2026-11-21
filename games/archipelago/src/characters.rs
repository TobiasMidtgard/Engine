//! The rival characters: the heads of the houses a new game is played against, each with a
//! personality and a way of playing, as the rivals of a city builder's custom game.
//!
//! A character shapes its captain's play ([`shape`]): how many islands it reaches for and
//! how early, how much it trades and buys abroad, how high it takes its towns, how many coins
//! it keeps in hand. Its temper drives diplomacy ([`diplomacy`]): how its standing with the
//! player drifts, what gifts are worth to it, and whether and when it goes to war. The level
//! ([`crate::brain::Level`]) still says how well it plays; the character says how.
//!
//! `--rival-picks gullwick:admiral,vane:captain,hask:mate` picks the houses of a new game
//! (and the level of each); `isle.characters` lists them all.

use engine::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};

use crate::brain::{Knobs, Level};
use crate::defs::{Good, Tier};
use crate::island::islands;
use crate::rivals::{Rivals, Treaty};
use crate::sim::{Company, Economy, PLAYER};

/// How a character plays, each about 1 for the usual.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Traits {
    /// How many islands it reaches for, and how early (0.6 a homebody, 1.8 an explorer).
    pub expand: f32,
    /// How much it trades: buying abroad, the free traders, the worth of a pact.
    pub trade: f32,
    /// How quickly ill will grows, and how ready it is for war.
    pub aggression: f32,
    /// How high it takes its towns, and how soon.
    pub ambition: f32,
    /// How many coins it keeps in hand (1.6 a miser, 0.6 a spendthrift).
    pub thrift: f32,
    /// What a gift is worth to it.
    pub gifts: f32,
    /// How slowly it forgets a slight.
    pub grudge: f32,
}

/// One character.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Character {
    /// The word `--rival-picks` takes.
    pub key: &'static str,
    pub leader: &'static str,
    pub house: &'static str,
    pub colour: u32,
    pub emblem: &'static str,
    /// In two or three words, for the relations screen.
    pub temperament: &'static str,
    /// How it plays, a sentence for the setup screen.
    pub style: &'static str,
    pub traits: Traits,
    pub standing: i32,
    pub treaty: Treaty,
    pub sells: &'static [Good],
    pub buys: &'static [Good],
    pub ships: &'static [&'static str],
}

const fn t(
    expand: f32,
    trade: f32,
    aggression: f32,
    ambition: f32,
    thrift: f32,
    gifts: f32,
    grudge: f32,
) -> Traits {
    Traits {
        expand,
        trade,
        aggression,
        ambition,
        thrift,
        gifts,
        grudge,
    }
}

/// Every character, the first three the houses of a game that names none.
pub const ALL: [Character; 8] = [
    Character {
        key: "gullwick",
        leader: "Edda Gullwick",
        house: "The Gullwick Company",
        colour: 0x2a78d6,
        emblem: "gull",
        temperament: "Shrewd trader",
        style: "Trades with everyone, buys what it lacks abroad and from the free traders, and keeps a deep purse. Slow to anger.",
        traits: t(1.0, 1.6, 0.4, 1.0, 1.25, 1.0, 0.8),
        standing: 15,
        treaty: Treaty::Neutral,
        sells: &[Good::Salt, Good::Furs],
        buys: &[Good::Bread, Good::Clothes],
        ships: &[
            "Gullwick Pride",
            "Tern",
            "Factor's Purse",
            "Bright Ledger",
            "Fulmar",
            "Sandpiper",
            "Gannet",
            "Kestrel Bay",
            "Thrift",
            "Gull's Wing",
            "Ready Coin",
            "Skua",
        ],
    },
    Character {
        key: "ostrava",
        leader: "Countess Vesna Ostrava",
        house: "House Ostrava",
        colour: 0x7a4ec9,
        emblem: "lantern",
        temperament: "Proud and ambitious",
        style: "Races for the best islands, spends freely and climbs to Patricians. Takes every slight to heart and goes to war on a neighbour who grows too big.",
        traits: t(1.5, 0.8, 1.4, 1.3, 0.75, 0.8, 1.3),
        standing: -10,
        treaty: Treaty::Neutral,
        sells: &[Good::Oil, Good::Rum],
        buys: &[Good::Planks, Good::Bricks],
        ships: &[
            "Ostrava",
            "Lantern Bearer",
            "Countess Vesna",
            "Iron Countess",
            "Vigil",
            "Ambition",
            "Black Amber",
            "Duke Teodor",
            "Ember",
            "High Tower",
            "Proud Lantern",
            "Brazen",
        ],
    },
    Character {
        key: "harrowgate",
        leader: "Elder Aldous Harrow",
        house: "The Harrowgate Free Towns",
        colour: 0x3c9a5f,
        emblem: "anvil",
        temperament: "Wary but loyal",
        style: "Builds deep on few islands and keeps a reserve. Remembers every quarrel, but once a friend, a steadfast one.",
        traits: t(0.7, 0.9, 0.7, 1.0, 1.2, 0.7, 1.8),
        standing: -30,
        treaty: Treaty::Hostile,
        sells: &[Good::Bricks, Good::Planks],
        buys: &[Good::Ale, Good::Fish],
        ships: &[
            "Free Hammer",
            "Anvil Song",
            "Harrowgate",
            "Common Weal",
            "Honest Toil",
            "Plough",
            "Smithy Smoke",
            "Fair Share",
            "Bellows",
            "Tinker",
            "Mill Race",
            "Ironwood",
        ],
    },
    Character {
        key: "vane",
        leader: "Captain Isolde Vane",
        house: "The Vane Line",
        colour: 0xd2a12c,
        emblem: "compass",
        temperament: "Daring explorer",
        style: "Settles far and fast with little in hand and stays a town of fisherfolk and labourers longer. Spread thin: a strong neighbour can outbuild her.",
        traits: t(1.8, 1.0, 0.8, 0.8, 0.6, 1.0, 1.0),
        standing: 0,
        treaty: Treaty::Neutral,
        sells: &[Good::Fish, Good::Logs],
        buys: &[Good::Coats, Good::Ale],
        ships: &[
            "Far Horizon",
            "Wayfinder",
            "Compass Rose",
            "Swift Vane",
            "Northing",
            "Landfall",
            "Outward Bound",
            "Sea Lark",
            "Long Reach",
            "True Bearing",
            "Driftwood",
            "Morning Star",
        ],
    },
    Character {
        key: "quill",
        leader: "Professor Linnea Quill",
        house: "The Quill Academy",
        colour: 0x2f8f9d,
        emblem: "owl",
        temperament: "Learned and patient",
        style: "Holds few islands and climbs the tiers fast: Burghers early, Masters and Patricians as soon as the goods allow. Rich in taxes late, peaceable throughout.",
        traits: t(0.6, 1.1, 0.25, 1.7, 1.0, 1.2, 0.7),
        standing: 20,
        treaty: Treaty::Neutral,
        sells: &[Good::Glass, Good::Lamps],
        buys: &[Good::Furs, Good::Coffee],
        ships: &[
            "Lectern",
            "Inkwell",
            "Astrolabe",
            "Night Owl",
            "Treatise",
            "Folio",
            "Orrery",
            "Quillmark",
            "Margin Note",
            "Proof",
            "Ex Libris",
            "Gaslight",
        ],
    },
    Character {
        key: "hask",
        leader: "Old Hask",
        house: "The Saltmarsh Brethren",
        colour: 0xb23a3a,
        emblem: "cutlass",
        temperament: "Smuggler and brawler",
        style: "Grabs islands, trades in anything and turns on whoever grows faster than the Brethren. Expect war on the harder military settings.",
        traits: t(1.3, 1.3, 1.9, 0.7, 0.9, 0.9, 1.4),
        standing: -25,
        treaty: Treaty::Hostile,
        sells: &[Good::Rum, Good::Salt],
        buys: &[Good::Planks, Good::Coats],
        ships: &[
            "Saltmarsh",
            "Black Gull",
            "Mudlark",
            "Brine Dog",
            "Low Tide",
            "Cutlass",
            "Wrecker",
            "Grey Eel",
            "Fog Runner",
            "Bilge Rat",
            "Night Heron",
            "Old Hask",
        ],
    },
    Character {
        key: "amari",
        leader: "Lady Solenne Amari",
        house: "House Amari",
        colour: 0xc2477e,
        emblem: "rose",
        temperament: "Lover of luxury",
        style: "Builds the finest towns and buys luxuries at any price, spending fast. Easily pleased by gifts, easily offended by neglect.",
        traits: t(0.9, 1.3, 0.5, 1.5, 0.8, 1.8, 0.9),
        standing: 25,
        treaty: Treaty::Neutral,
        sells: &[Good::Dates, Good::Cane],
        buys: &[Good::Lamps, Good::Jewellery],
        ships: &[
            "Damask",
            "Rosewater",
            "Velvet",
            "Solenne",
            "Gilded Rose",
            "Silk Road",
            "Pearl",
            "Saffron",
            "Opaline",
            "Chandelier",
            "Masquerade",
            "Belle",
        ],
    },
    Character {
        key: "penhallow",
        leader: "Jory Penhallow",
        house: "Penhallow and Sons",
        colour: 0x7d7d6a,
        emblem: "scales",
        temperament: "Cautious miser",
        style: "Never spends what it cannot spare: slow to grow and slow to climb, but hard to ruin, and it never starts a war.",
        traits: t(0.8, 1.1, 0.15, 0.8, 1.7, 0.6, 1.0),
        standing: 5,
        treaty: Treaty::Neutral,
        sells: &[Good::Wool, Good::Clay],
        buys: &[Good::Bread],
        ships: &[
            "Penny Wise",
            "Counting House",
            "Steady",
            "Ledger Line",
            "Prudence",
            "Sound Coin",
            "Balance",
            "Strongbox",
            "Small Mercies",
            "Patience",
            "Weights",
            "Sixpence",
        ],
    },
];

/// The character with `key` (its key or leader's name, any case).
pub fn find(key: &str) -> Option<usize> {
    let k = key.trim().to_ascii_lowercase();
    ALL.iter().position(|c| {
        c.key == k || c.leader.to_ascii_lowercase() == k || c.house.to_ascii_lowercase() == k
    })
}

/// The houses a new game is played against, and the level of each (`--rival-picks`).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct RivalPicks(pub Vec<(usize, Level)>);

impl RivalPicks {
    /// `gullwick:admiral,vane,hask:mate` (a pick without a level plays at `level`).
    pub fn parse(s: &str, level: Level) -> Result<RivalPicks, String> {
        let mut v = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (who, lv) = part.split_once(':').unwrap_or((part, ""));
            let c = find(who).ok_or_else(|| {
                format!(
                    "No character {who:?}: {}.",
                    ALL.iter().map(|c| c.key).collect::<Vec<_>>().join(", ")
                )
            })?;
            if v.iter().any(|(x, _)| *x == c) {
                return Err(format!("{} is picked twice.", ALL[c].leader));
            }
            let lv = if lv.is_empty() {
                level
            } else {
                Level::from_name(lv).ok_or_else(|| format!("No level {lv:?}."))?
            };
            v.push((c, lv));
        }
        if v.len() > 3 {
            return Err("At most three rival houses.".into());
        }
        Ok(RivalPicks(v))
    }

    /// The `--rival-picks` text.
    pub fn flag(&self) -> String {
        self.0
            .iter()
            .map(|(c, l)| format!("{}:{}", ALL[*c].key, l.name().to_ascii_lowercase()))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A captain's knobs at its level, as its character plays: the level says how well, the
/// character how.
pub fn shape(mut k: Knobs, t: &Traits) -> Knobs {
    k.reserve *= t.thrift as f64;
    // (The levels that plan two links or more are the ones that can race or trade far.)
    let able = k.depth >= 2;
    if k.isles < 99 {
        k.isles = ((k.isles as f32 * t.expand).round() as usize).max(1);
    } else if t.expand < 0.8 {
        k.isles = 5;
    }
    if t.expand >= 1.4 && able {
        k.race = true;
    }
    if t.expand <= 0.7 {
        k.race = false;
    }
    if t.trade >= 1.3 && able {
        k.abroad = true;
        k.traders = true;
    }
    // (One who trades little leaves the free traders alone, but still buys from other
    // houses what it cannot make: without that a level would play worse, not differently.)
    if t.trade <= 0.8 {
        k.traders = false;
    }
    if t.ambition >= 1.4 {
        k.works += 1;
    }
    k.top = if t.ambition < 0.85 {
        Tier::Masters
    } else {
        Tier::Patricians
    };
    k
}

/// Make rival house `c` (1..=3) the house of character `ch`.
pub fn install(world: &mut World, c: Company, ch: usize) {
    let me = &ALL[ch];
    let mut rivals = world.get_resource_or_insert_with(Rivals::default);
    let Some(r) = rivals.0.iter_mut().find(|r| r.id == c as u32) else {
        return;
    };
    r.name = me.house;
    r.leader = me.leader;
    r.style = me.style;
    r.character = Some(ch);
    r.colour = me.colour;
    r.emblem = me.emblem;
    r.temperament = me.temperament;
    r.standing = me.standing;
    r.treaty = me.treaty;
    r.sells = me.sells;
    r.buys = me.buys;
    r.events.clear();
    r.mood = 0.0;
    r.war_s = 0.0;
    r.truce_until = 0.0;
}

/// The character of rival house `c`, if it has one.
pub fn of(world: &World, c: Company) -> Option<&'static Character> {
    world
        .get_resource::<Rivals>()?
        .0
        .iter()
        .find(|r| r.id == c as u32)?
        .character
        .map(|i| &ALL[i])
}

/// Game seconds between looks at the standings.
const LOOK_S: f64 = 30.0;

/// The houses' tempers at work: standing drifts with how the player's company grows beside
/// them and on their doorstep, pacts warm it, and a house that has come to hate the player
/// and feels strong enough declares war (never on a peaceful game); a war that drags on ends
/// in a plea for peace.
pub fn diplomacy(world: &mut World) {
    let Some(mut rivals) = world.remove_resource::<Rivals>() else {
        return;
    };
    let econ = world.resource::<Economy>();
    let now = econ.clock_s;
    let rules = econ.rules;
    let people = |c: Company| -> f32 {
        (0..econ.islands.len())
            .filter(|&i| econ.owns(c, i))
            .map(|i| econ.islands[i].population.iter().sum::<f32>())
            .sum()
    };
    let ours = people(PLAYER);
    let temper = rules.temper();
    for r in rivals.0.iter_mut() {
        let Some(ch) = r.character else { continue };
        let ch = &ALL[ch];
        if now - r.looked_s < LOOK_S {
            continue;
        }
        let dt_min = ((now - r.looked_s).min(600.0) / 60.0) as f32;
        r.looked_s = now;
        let c = r.id as Company;
        let theirs = people(c);
        let t = ch.traits;
        // Standing a minute (fractions add up in `mood`).
        let mut per_min = 0.0f32;
        // Envy: the player's company outgrowing theirs.
        if ours > 200.0 && ours > theirs * 1.3 {
            per_min -= 0.5 * t.aggression * t.ambition.max(0.8) * temper;
        }
        // The player on their doorstep: islands in their home region.
        let home = crate::brain::home_of(world, c);
        let region = islands()[home].region();
        let near = (0..econ.islands.len())
            .filter(|&i| econ.owns(PLAYER, i) && islands()[i].region() == region)
            .count();
        per_min -= 0.4 * near as f32 * t.aggression * temper;
        // Pacts warm, the more so to a trader.
        if r.treaty >= Treaty::TradePact {
            per_min += 0.4 * t.trade;
        }
        // Slights fade back toward the house's own humour, slowly for those who hold grudges.
        if r.treaty != Treaty::War && (r.standing as f32) < ch.standing as f32 {
            per_min += 0.6 / t.grudge / temper.max(0.5);
        }
        if r.treaty == Treaty::War {
            per_min -= 0.2 * temper;
            r.war_s += dt_min as f64 * 60.0;
        }
        r.mood += per_min * dt_min;
        let whole = r.mood.trunc();
        if whole != 0.0 {
            r.mood -= whole;
            r.standing = (r.standing + whole as i32).clamp(-100, 100);
        }
        // The treaty follows the standing.
        let war_at = (-80.0 + 25.0 * (t.aggression * temper - 1.0)).clamp(-95.0, -35.0) as i32;
        match r.treaty {
            Treaty::Neutral if r.standing <= -45 => {
                r.treaty = Treaty::Hostile;
                r.log(now, "They broke off friendly dealings", 0);
            }
            Treaty::Hostile if r.standing >= -15 => {
                r.treaty = Treaty::Neutral;
                r.log(now, "Their envoy came back to our harbour", 0);
            }
            Treaty::Hostile
                if rules.rivals_declare_war()
                    && now >= r.truce_until
                    && r.standing <= war_at
                    && t.aggression * temper >= 0.6
                    && (theirs >= ours * 0.6 || t.aggression * temper >= 2.5) =>
            {
                r.treaty = Treaty::War;
                r.war_s = 0.0;
                let d = r.shift(-15);
                r.log(now, "They declared war on us", d);
                engine::core::log::info!("archipelago: {} declared war on the player", r.name);
            }
            Treaty::War
                if r.war_s >= 1_800.0 / (t.aggression * temper).max(0.3) as f64
                    || theirs < ours * 0.4 =>
            {
                r.treaty = Treaty::Hostile;
                r.truce_until = now + 1_800.0;
                let d = r.shift(10);
                r.log(now, "They sued for peace", d);
            }
            _ => {}
        }
    }
    world.insert_resource(rivals);
}

/// Every character, for the setup screen.
pub fn list() -> Value {
    json!(
        ALL.iter()
            .map(|c| json!({
                "key": c.key,
                "leader": c.leader,
                "house": c.house,
                "colour": c.colour,
                "emblem": c.emblem,
                "temperament": c.temperament,
                "style": c.style,
                "traits": c.traits,
                "standing": c.standing,
                "treaty": c.treaty.name(),
            }))
            .collect::<Vec<_>>()
    )
}

pub fn register(app: &mut App) {
    app.register_api(
        "isle.characters",
        "The rival characters a new game can be played against: leader, house, colours, temperament, how each plays, its traits (expand, trade, aggression, ambition, thrift, gifts, grudge; 1 is usual) and where it starts with you. Also the picks of this game and the levels.",
        "{}",
        |world, _| {
            let picks = world
                .get_resource::<RivalPicks>()
                .map(|p| p.flag())
                .unwrap_or_default();
            Ok(json!({
                "characters": list(),
                "levels": Level::ALL.iter().map(|l| l.name()).collect::<Vec<_>>(),
                "picks": picks,
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_are_distinct_and_picks_round_trip() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.house, b.house);
                assert_ne!(a.colour, b.colour);
            }
            assert!(a.ships.len() >= 10);
        }
        let p = RivalPicks::parse("vane:admiral, Old Hask ,quill:mate", Level::Captain).unwrap();
        assert_eq!(
            p.0,
            vec![(3, Level::Admiral), (5, Level::Captain), (4, Level::Mate)]
        );
        assert_eq!(RivalPicks::parse(&p.flag(), Level::Deckhand).unwrap(), p);
        assert!(RivalPicks::parse("vane,vane", Level::Captain).is_err());
        assert!(RivalPicks::parse("nobody", Level::Captain).is_err());
    }

    #[test]
    fn a_character_changes_how_a_level_plays_not_how_well() {
        let k = Level::Captain.knobs();
        let vane = shape(k, &ALL[3].traits);
        let quill = shape(k, &ALL[4].traits);
        let miser = shape(k, &ALL[7].traits);
        assert!(vane.isles > k.isles && vane.race);
        assert!(quill.isles < vane.isles && quill.top == Tier::Patricians);
        assert_eq!(vane.top, Tier::Masters);
        assert!(miser.reserve > k.reserve * 1.5);
        assert!(shape(k, &ALL[0].traits).abroad);
        // The level's own pace stays.
        assert_eq!((vane.turn_s, vane.slip), (k.turn_s, k.slip));
        // A deckhand explorer does not race.
        assert!(!shape(Level::Deckhand.knobs(), &ALL[3].traits).race);
    }

    #[test]
    fn a_quarrelsome_house_that_hates_you_goes_to_war_but_never_on_a_peaceful_game() {
        let run = |menace: crate::rules::Menace| {
            let mut w = World::new();
            let mut econ = Economy::default();
            econ.rules.military = menace;
            econ.rivals = vec![Default::default(); 3];
            // The player has grown big; house 1 is small.
            econ.islands[0].owner = PLAYER;
            econ.islands[0].harbours = 1;
            econ.islands[0].population = [
                400.0, 400.0, 200.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ];
            econ.islands[3].owner = 1;
            econ.islands[3].harbours = 1;
            econ.islands[3].population = [
                300.0, 300.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ];
            w.insert_resource(econ);
            w.insert_resource(Rivals::default());
            install(&mut w, 1, find("hask").unwrap());
            for m in 0..240 {
                w.resource_mut::<Economy>().clock_s = 60.0 * m as f64;
                diplomacy(&mut w);
            }
            w.resource::<Rivals>().0[0].clone()
        };
        let calm = run(crate::rules::Menace::Peaceful);
        assert_ne!(calm.treaty, Treaty::War);
        let hard = run(crate::rules::Menace::Ruthless);
        assert!(
            hard.events.iter().any(|e| e.1.contains("declared war")),
            "{:?}",
            hard.events
        );
        assert!(hard.standing < calm.standing);
    }
}
