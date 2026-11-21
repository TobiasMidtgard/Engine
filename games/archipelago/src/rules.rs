//! The rules a game is played by, chosen before it starts, as in the custom game of a city
//! builder: how hard the economy is on the player's company, how quarrelsome the rival
//! houses are, how much raw works yield, how much room there is, and what is left out.
//!
//! The economy rule weighs on the player's company only (each rival's character and level
//! say how well it plays); resources and space are the map's and hold for every company;
//! the military rule is the rivals' temper ([`crate::characters`]).
//!
//! `--rules economy=hard,military=ruthless,resources=scarce,space=crowded,top=burghers,airships=off`
//! sets them at the start; `isle.rules` lists every option with its choices and
//! `isle.rules.set` changes them.

use engine::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};

use crate::defs::{Def, Tier};
use crate::sim::{Company, Economy, PLAYER};

/// How hard the economy is on the player's company.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hardship {
    Easy,
    Normal,
    Hard,
    Brutal,
}

/// How quarrelsome the rival houses are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Menace {
    /// Nobody declares war on the player.
    Peaceful,
    Normal,
    Hard,
    Ruthless,
}

/// How much the raw works (farms, mines, groves, fisheries, woodcutters) yield.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bounty {
    Scarce,
    Normal,
    Rich,
}

/// How much room there is on the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Room {
    /// Four islands are closed to settlers and a rival always starts in the player's waters.
    Crowded,
    Normal,
    /// No rival starts in the player's waters.
    Roomy,
}

/// The islands closed to settlers on a crowded map (every kind of ground is still to be had
/// elsewhere).
pub const CLOSED_WHEN_CROWDED: [&str; 4] =
    ["Skerryvale", "Whalers' Rock", "Little Skarn", "Palm Key"];

/// One game's rules.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Rules {
    pub economy: Hardship,
    pub military: Menace,
    pub resources: Bounty,
    pub space: Room,
    /// The highest tier houses may reach.
    pub top_tier: Tier,
    /// Airship masts may be built.
    pub airships: bool,
    /// The pedlar sells items.
    pub pedlar: bool,
    /// The free traders sell raw goods ([`crate::traders`]).
    pub free_traders: bool,
    /// Fires, riots and sickness break out.
    pub incidents: bool,
    /// Ships trade at other companies' harbours.
    pub house_trade: bool,
    /// The cheat orders work (`isle.cheat.*`).
    pub cheats: bool,
    /// Every building is open to the player whatever its people (a cheat, not a rule).
    pub unlocked: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            economy: Hardship::Normal,
            military: Menace::Normal,
            resources: Bounty::Normal,
            space: Room::Normal,
            top_tier: Tier::Patricians,
            airships: true,
            pedlar: true,
            free_traders: true,
            incidents: true,
            house_trade: true,
            // (On while the game is being tested; the rivals never use them.)
            cheats: true,
            unlocked: false,
        }
    }
}

impl Rules {
    /// What company `c`'s houses pay, times what they would.
    pub fn tax(&self, c: Company) -> f64 {
        if c != PLAYER {
            return 1.0;
        }
        match self.economy {
            Hardship::Easy => 1.25,
            Hardship::Normal => 1.0,
            Hardship::Hard => 0.9,
            Hardship::Brutal => 0.8,
        }
    }

    /// What company `c`'s buildings and ships cost to keep, times what they would.
    pub fn upkeep(&self, c: Company) -> f64 {
        if c != PLAYER {
            return 1.0;
        }
        match self.economy {
            Hardship::Easy => 0.8,
            Hardship::Normal => 1.0,
            Hardship::Hard => 1.15,
            Hardship::Brutal => 1.3,
        }
    }

    /// What company `c` pays in coins to build, times the list price.
    pub fn build_cost(&self, c: Company) -> f64 {
        if c != PLAYER {
            return 1.0;
        }
        match self.economy {
            Hardship::Easy => 0.8,
            Hardship::Normal => 1.0,
            Hardship::Hard => 1.25,
            Hardship::Brutal => 1.5,
        }
    }

    /// The player's first purse, times the usual.
    pub fn start_coins(&self) -> f64 {
        match self.economy {
            Hardship::Easy => 1.5,
            Hardship::Normal => 1.0,
            Hardship::Hard => 0.8,
            Hardship::Brutal => 0.6,
        }
    }

    /// How fast a works of `def` turns out its goods, times the usual: raw works only.
    pub fn yield_of(&self, def: &Def) -> f32 {
        if def.input.is_some() || def.input2.is_some() || def.output.is_none() {
            return 1.0;
        }
        match self.resources {
            Bounty::Scarce => 0.75,
            Bounty::Normal => 1.0,
            Bounty::Rich => 1.3,
        }
    }

    /// The first ship's cargo, times the usual (every company's).
    pub fn cargo(&self) -> f64 {
        match self.resources {
            Bounty::Scarce => 0.7,
            Bounty::Normal => 1.0,
            Bounty::Rich => 1.5,
        }
    }

    /// Whether island `isle` is closed to settlers.
    pub fn closed(&self, isle: usize) -> bool {
        self.space == Room::Crowded
            && crate::island::islands()
                .get(isle)
                .is_some_and(|i| CLOSED_WHEN_CROWDED.contains(&i.name))
    }

    /// Whether a rival playing at `level` starts in the player's home waters.
    pub fn rival_near(&self, level: crate::brain::Level) -> bool {
        match self.space {
            Room::Crowded => true,
            Room::Normal => level.one_near(),
            Room::Roomy => false,
        }
    }

    /// Whether a rival house may declare war on the player.
    pub fn rivals_declare_war(&self) -> bool {
        self.military != Menace::Peaceful
    }

    /// How fast the rivals' ill will grows and how soon they go to war, times the usual.
    pub fn temper(&self) -> f32 {
        match self.military {
            Menace::Peaceful => 0.4,
            Menace::Normal => 1.0,
            Menace::Hard => 1.5,
            Menace::Ruthless => 2.2,
        }
    }

    /// Whether houses may move up into `t`.
    pub fn allows_tier(&self, t: Tier) -> bool {
        t.within(self.top_tier)
    }

    /// Change one rule: `key` as [`OPTIONS`] names it, `value` one of its choices.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let v = value.trim().to_ascii_lowercase();
        let on = || match v.as_str() {
            "on" | "yes" | "true" | "1" => Ok(true),
            "off" | "no" | "false" | "0" => Ok(false),
            _ => Err(format!("{key} is on or off, not {value:?}.")),
        };
        let bad = || {
            let choices = OPTIONS
                .iter()
                .find(|o| o.key == key)
                .map_or(String::new(), |o| o.choices.join(", "));
            format!("{key} is one of {choices}, not {value:?}.")
        };
        match key {
            "economy" => {
                self.economy = match v.as_str() {
                    "easy" => Hardship::Easy,
                    "normal" => Hardship::Normal,
                    "hard" => Hardship::Hard,
                    "brutal" => Hardship::Brutal,
                    _ => return Err(bad()),
                }
            }
            "military" => {
                self.military = match v.as_str() {
                    "peaceful" => Menace::Peaceful,
                    "normal" => Menace::Normal,
                    "hard" => Menace::Hard,
                    "ruthless" => Menace::Ruthless,
                    _ => return Err(bad()),
                }
            }
            "resources" => {
                self.resources = match v.as_str() {
                    "scarce" => Bounty::Scarce,
                    "normal" => Bounty::Normal,
                    "rich" => Bounty::Rich,
                    _ => return Err(bad()),
                }
            }
            "space" => {
                self.space = match v.as_str() {
                    "crowded" => Room::Crowded,
                    "normal" => Room::Normal,
                    "roomy" => Room::Roomy,
                    _ => return Err(bad()),
                }
            }
            "top" => {
                self.top_tier = Tier::ALL
                    .into_iter()
                    .find(|t| t.name().eq_ignore_ascii_case(&v))
                    .ok_or_else(bad)?;
            }
            "airships" => self.airships = on()?,
            "pedlar" => self.pedlar = on()?,
            "free_traders" => self.free_traders = on()?,
            "incidents" => self.incidents = on()?,
            "house_trade" => self.house_trade = on()?,
            "cheats" => self.cheats = on()?,
            _ => {
                return Err(format!(
                    "No rule called {key:?}: {}.",
                    OPTIONS.iter().map(|o| o.key).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        Ok(())
    }

    /// Rules from `key=value,key=value` (what is not named stays as it is).
    pub fn parse(&mut self, s: &str) -> Result<(), String> {
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| format!("{part:?}: a rule is key=value."))?;
            self.set(k.trim(), v)?;
        }
        Ok(())
    }

    /// The value of rule `key` as [`Rules::set`] takes it.
    pub fn value(&self, key: &str) -> String {
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        match key {
            "economy" => word(&self.economy),
            "military" => word(&self.military),
            "resources" => word(&self.resources),
            "space" => word(&self.space),
            "top" => self.top_tier.name().to_ascii_lowercase(),
            "airships" => on(self.airships),
            "pedlar" => on(self.pedlar),
            "free_traders" => on(self.free_traders),
            "incidents" => on(self.incidents),
            "house_trade" => on(self.house_trade),
            "cheats" => on(self.cheats),
            _ => String::new(),
        }
    }

    /// The `--rules` text for these rules: only what differs from the default.
    pub fn flag(&self) -> String {
        let d = Rules::default();
        OPTIONS
            .iter()
            .filter(|o| self.value(o.key) != d.value(o.key))
            .map(|o| format!("{}={}", o.key, self.value(o.key)))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A rule's choice as the word `--rules` takes.
fn word<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// One option of the setup screen.
pub struct Opt {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub choices: &'static [&'static str],
    /// What each choice does, in the order of `choices`.
    pub help: &'static [&'static str],
}

/// Every rule, in the order the setup screen shows them.
pub const OPTIONS: [Opt; 11] = [
    Opt {
        key: "economy",
        group: "Difficulty",
        label: "Economy",
        choices: &["easy", "normal", "hard", "brutal"],
        help: &[
            "Your houses pay a quarter more, upkeep and building cost a fifth less, and you start with half again the coins.",
            "As the game is meant to be played.",
            "Taxes a tenth lower, upkeep 15% and building a quarter dearer, a smaller first purse.",
            "Taxes a fifth lower, upkeep 30% and building half again as dear, little to start with.",
        ],
    },
    Opt {
        key: "military",
        group: "Difficulty",
        label: "Military",
        choices: &["peaceful", "normal", "hard", "ruthless"],
        help: &[
            "No rival house ever declares war on you, and slights are soon forgotten.",
            "Rivals go to war when they hate you and feel strong enough.",
            "Ill will grows half again as fast and rivals strike sooner.",
            "Ill will grows twice as fast; the quarrelsome go to war at the first chance.",
        ],
    },
    Opt {
        key: "resources",
        group: "Map",
        label: "Resources",
        choices: &["scarce", "normal", "rich"],
        help: &[
            "Farms, mines, groves, fisheries and woodcutters yield a quarter less; every first ship carries less.",
            "The usual yields.",
            "Raw works yield 30% more; every first ship carries half again as much.",
        ],
    },
    Opt {
        key: "space",
        group: "Map",
        label: "Space",
        choices: &["crowded", "normal", "roomy"],
        help: &[
            "Four islands are closed to settlers and a rival always starts in your waters.",
            "A rival starts in your waters on the hardest levels.",
            "No rival starts in your waters.",
        ],
    },
    Opt {
        key: "top",
        group: "Restrictions",
        label: "Highest tier",
        choices: &[
            "fisherfolk",
            "labourers",
            "burghers",
            "masters",
            "patricians",
        ],
        help: &[
            "Houses never move up.",
            "Up to Labourers.",
            "Up to Burghers.",
            "Up to Masters.",
            "All five tiers.",
        ],
    },
    Opt {
        key: "airships",
        group: "Restrictions",
        label: "Airships",
        choices: &["on", "off"],
        help: &["Airship masts may be built.", "No airship masts."],
    },
    Opt {
        key: "pedlar",
        group: "Restrictions",
        label: "Pedlar and items",
        choices: &["on", "off"],
        help: &["The pedlar calls with items.", "The pedlar sells nothing."],
    },
    Opt {
        key: "free_traders",
        group: "Restrictions",
        label: "Free traders",
        choices: &["on", "off"],
        help: &[
            "Free traders sell raw goods dearly at your harbours.",
            "No free traders.",
        ],
    },
    Opt {
        key: "incidents",
        group: "Restrictions",
        label: "Fires, riots and sickness",
        choices: &["on", "off"],
        help: &["Incidents break out.", "No incidents."],
    },
    Opt {
        key: "house_trade",
        group: "Restrictions",
        label: "Trade between houses",
        choices: &["on", "off"],
        help: &[
            "Ships buy and sell at other companies' harbours.",
            "Every company lives on what it makes.",
        ],
    },
    Opt {
        key: "cheats",
        group: "Testing",
        label: "Cheats",
        choices: &["on", "off"],
        help: &[
            "The cheat orders work (coins, goods, the chart, moving houses up, speed).",
            "No cheats.",
        ],
    },
];

/// The rules in play.
pub fn of(world: &World) -> Rules {
    world.resource::<Economy>().rules
}

/// Every option, its choices with what each does, and the value in play (for the setup
/// screen).
pub fn options(rules: &Rules) -> Value {
    let list: Vec<Value> = OPTIONS
        .iter()
        .map(|o| {
            json!({
                "key": o.key,
                "group": o.group,
                "label": o.label,
                "value": rules.value(o.key),
                "default": Rules::default().value(o.key),
                "choices": o.choices.iter().zip(o.help).map(|(c, h)| json!({"value": c, "help": h})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "options": list, "flag": rules.flag() })
}

/// Put `rules` in play: what they change at once (incidents on or off) as well as from now on.
pub fn apply(world: &mut World, rules: Rules) {
    world.resource_mut::<Economy>().rules = rules;
    if let Some(mut inc) = world.get_resource_mut::<crate::incidents::Incidents>() {
        inc.rolls = rules.incidents;
    }
}

pub fn register(app: &mut App) {
    app.register_api(
        "isle.rules",
        "The rules of this game: economy, military, resources, space, the highest tier and what is left out, each with its choices and what they do, and the --rules text that starts a game with them.",
        "{}",
        |world, _| Ok(options(&of(world))),
    )
    .register_api(
        "isle.rules.set",
        "Change rules, as key=value pairs (economy easy/normal/hard/brutal, military peaceful/normal/hard/ruthless, resources scarce/normal/rich, space crowded/normal/roomy, top a tier, airships, pedlar, free_traders, incidents, house_trade, cheats on/off). Space and resources are best set before the start.",
        r#"{"economy": "hard", "military": "ruthless", "top": "burghers"}"#,
        |world, args| {
            let mut rules = of(world);
            let obj = args.as_object().ok_or("an object of rule: value")?;
            for (k, v) in obj {
                let v = match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => if *b { "on" } else { "off" }.into(),
                    other => other.to_string(),
                };
                rules.set(k, &v)?;
            }
            apply(world, rules);
            Ok(options(&rules))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_round_trip_through_their_flag_and_weigh_on_the_player_only() {
        let mut r = Rules::default();
        assert_eq!(r.flag(), "");
        r.parse("economy=hard, military=ruthless,top=burghers,airships=off,space=crowded")
            .unwrap();
        assert_eq!(r.economy, Hardship::Hard);
        assert!(!r.allows_tier(Tier::Masters) && r.allows_tier(Tier::Burghers));
        let mut back = Rules::default();
        back.parse(&r.flag()).unwrap();
        assert_eq!(back, r);
        assert!(r.parse("economy=silly").is_err());
        assert!(r.parse("colour=red").is_err());
        assert!(r.tax(PLAYER) < 1.0 && r.tax(2) == 1.0);
        assert!(r.build_cost(PLAYER) > 1.0 && r.build_cost(1) == 1.0);
        assert_eq!(
            options(&r)["options"].as_array().unwrap().len(),
            OPTIONS.len()
        );
        for o in OPTIONS {
            assert_eq!(o.choices.len(), o.help.len(), "{}", o.key);
            for c in o.choices {
                Rules::default().set(o.key, c).unwrap();
            }
        }
    }

    #[test]
    fn a_crowded_map_still_has_every_kind_of_ground_open() {
        let r = Rules {
            space: Room::Crowded,
            ..Default::default()
        };
        let isles = crate::island::islands();
        let closed: Vec<usize> = (0..isles.len()).filter(|&i| r.closed(i)).collect();
        assert_eq!(closed.len(), CLOSED_WHEN_CROWDED.len(), "{closed:?}");
        for d in crate::defs::DEFS.iter().filter(|d| d.needs_fertility) {
            assert!(
                (0..isles.len()).any(|i| !r.closed(i) && isles[i].fertility.allows(d.kind)),
                "no open island allows {}",
                d.name
            );
        }
        // (The rivals' and the player's start islands stay open.)
        for name in [
            "Haverholm",
            "Northmoor",
            "Amberland",
            "Frostholm",
            "Sarrowmere",
        ] {
            let i = isles.iter().position(|x| x.name == name).unwrap();
            assert!(!r.closed(i), "{name}");
        }
    }
}
