//! The rival houses the player shares the seas with, and where the player stands with each:
//! goodwill from -100 to 100, a treaty, trade, fleets and what has passed between them.
//!
//! This first round keeps the standing, treaties and the history for real (gifts, pacts and
//! war change them, and the relations screen shows it), while each rival's own wealth, people,
//! islands and fleet are set numbers until rival AI arrives (round 9 of the economy plan).

use std::collections::VecDeque;

use engine::prelude::*;
use serde::Serialize;

use crate::defs::Good;
use crate::sim::Economy;

/// Where two houses stand by treaty, worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Treaty {
    War,
    Hostile,
    Neutral,
    TradePact,
    Alliance,
}

impl Treaty {
    pub fn name(self) -> &'static str {
        match self {
            Treaty::War => "At war",
            Treaty::Hostile => "Hostile",
            Treaty::Neutral => "Neutral",
            Treaty::TradePact => "Trade pact",
            Treaty::Alliance => "Alliance",
        }
    }
}

/// Standing a trade pact asks for, and an alliance.
pub const PACT_STANDING: i32 = 25;
pub const ALLIANCE_STANDING: i32 = 60;
/// A gift: its coins, and the standing it buys (less from a house that already likes you).
pub const GIFT_COINS: u32 = 500;
/// Events kept per rival.
const KEEP_EVENTS: usize = 8;

/// One rival house.
#[derive(Clone, Debug)]
pub struct Rival {
    pub id: u32,
    pub name: &'static str,
    pub colour: u32,
    pub emblem: &'static str,
    pub temperament: &'static str,
    pub standing: i32,
    pub treaty: Treaty,
    /// What they make and sell, and what they are short of and buy.
    pub sells: &'static [Good],
    pub buys: &'static [Good],
    pub wealth: f64,
    pub people: u32,
    pub islands: u32,
    pub warships: u32,
    pub firepower: f32,
    /// Ships each side lost to the other lately.
    pub lost_ours: u32,
    pub lost_theirs: u32,
    /// Signed coins a minute of trade with us (what we sell them less what we buy).
    pub trade_per_min: f32,
    /// (game clock seconds, text, change in standing), newest last.
    pub events: VecDeque<(f64, String, i32)>,
    /// When a gift last went (a second gift soon after buys less).
    pub last_gift_s: f64,
}

impl Rival {
    fn log(&mut self, clock: f64, text: impl Into<String>, delta: i32) {
        self.events.push_back((clock, text.into(), delta));
        while self.events.len() > KEEP_EVENTS {
            self.events.pop_front();
        }
    }

    fn shift(&mut self, delta: i32) -> i32 {
        let before = self.standing;
        self.standing = (self.standing + delta).clamp(-100, 100);
        self.standing - before
    }

    /// One line per thing that would raise their standing.
    fn wants(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.treaty == Treaty::War {
            out.push("Make peace: every month at war costs standing.".into());
        }
        if let Some(g) = self.buys.first() {
            out.push(format!("Sell them {}: they are short of it.", g.name()));
        }
        out.push(format!(
            "A gift of {GIFT_COINS} coins (worth less if you sent one lately)."
        ));
        if self.treaty < Treaty::TradePact && self.standing < PACT_STANDING {
            out.push(format!(
                "Reach {PACT_STANDING} standing for a trade pact; {} to go.",
                PACT_STANDING - self.standing
            ));
        }
        out
    }
}

/// Every rival house, in a fixed order.
#[derive(Resource, Clone, Debug)]
pub struct Rivals(pub Vec<Rival>);

impl Default for Rivals {
    fn default() -> Self {
        let rival = |id, name, colour, emblem, temperament, standing, treaty| Rival {
            id,
            name,
            colour,
            emblem,
            temperament,
            standing,
            treaty,
            sells: &[],
            buys: &[],
            wealth: 0.0,
            people: 0,
            islands: 0,
            warships: 0,
            firepower: 0.0,
            lost_ours: 0,
            lost_theirs: 0,
            trade_per_min: 0.0,
            events: VecDeque::new(),
            last_gift_s: f64::NEG_INFINITY,
        };
        let mut v = vec![
            Rival {
                sells: &[Good::Salt, Good::Furs],
                buys: &[Good::Bread, Good::Clothes],
                wealth: 24_000.0,
                people: 3_100,
                islands: 4,
                warships: 3,
                firepower: 90.0,
                trade_per_min: 6.0,
                ..rival(
                    1,
                    "The Gullwick Company",
                    0x2a78d6,
                    "gull",
                    "Shrewd trader",
                    15,
                    Treaty::Neutral,
                )
            },
            Rival {
                sells: &[Good::Oil, Good::Rum],
                buys: &[Good::Planks, Good::Bricks],
                wealth: 41_000.0,
                people: 5_400,
                islands: 6,
                warships: 7,
                firepower: 260.0,
                trade_per_min: -4.0,
                ..rival(
                    2,
                    "House Ostrava",
                    0x7a4ec9,
                    "lantern",
                    "Proud and ambitious",
                    -10,
                    Treaty::Neutral,
                )
            },
            Rival {
                sells: &[Good::Bricks, Good::Planks],
                buys: &[Good::Ale, Good::Fish],
                wealth: 9_500.0,
                people: 1_700,
                islands: 2,
                warships: 1,
                firepower: 30.0,
                trade_per_min: 0.0,
                ..rival(
                    3,
                    "The Harrowgate Free Towns",
                    0x3c9a5f,
                    "anvil",
                    "Wary but loyal",
                    -30,
                    Treaty::Hostile,
                )
            },
        ];
        v[0].log(0.0, "Their factor came calling with a price list", 5);
        v[1].log(0.0, "Their envoy took offence at our flag", -10);
        v[2].log(0.0, "A border quarrel over fishing grounds", -15);
        Rivals(v)
    }
}

/// What the relations screen shows for one rival.
#[derive(Clone, Debug, Serialize)]
pub struct RivalView {
    pub id: u32,
    pub name: String,
    pub colour: u32,
    pub emblem: String,
    pub temperament: String,
    pub standing: i32,
    pub treaty: Treaty,
    pub treaty_name: String,
    pub economic: Economic,
    pub military: Military,
    pub events: Vec<EventView>,
    pub wants: Vec<String>,
    pub actions: Vec<ActionView>,
    /// True while their wealth, people, islands and fleet are set numbers (no rival AI yet).
    pub placeholder: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Economic {
    pub trade_per_min: f32,
    pub wealth: f64,
    pub people: u32,
    pub islands: u32,
    pub they_sell: Vec<String>,
    pub they_buy: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Military {
    pub warships: u32,
    pub firepower: f32,
    pub our_warships: u32,
    pub our_firepower: f32,
    pub lost_ours: u32,
    pub lost_theirs: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventView {
    pub clock_s: f64,
    pub text: String,
    pub delta: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActionView {
    pub label: String,
    /// The API to call, `isle.rival.<verb>`, with [`ActionView::args`]. (`tools::action` also
    /// takes `isle.rival.<verb>.<id>`.)
    pub action: String,
    pub args: serde_json::Value,
    pub enabled: bool,
    pub why_not: Option<String>,
}

/// Our warships (vessels with guns) and their damage a minute.
fn our_fleet(world: &mut World) -> (u32, f32) {
    let mut q = world.query::<&crate::fleet::Vessel>();
    let mut n = 0;
    let mut fire = 0.0;
    for v in q.iter(world) {
        let f = crate::fittings::stats(v.craft, &v.fit).firepower;
        if f > 0.0 {
            n += 1;
            fire += f;
        }
    }
    (n, fire)
}

/// Why `verb` cannot be done with `r` now (None: it can).
fn why_not(r: &Rival, verb: &str, coins: f64) -> Option<String> {
    match verb {
        "gift" if coins < GIFT_COINS as f64 => Some(format!(
            "A gift costs {GIFT_COINS} coins; you have {coins:.0}."
        )),
        "gift" if r.treaty == Treaty::War => Some("They take no gifts while at war.".into()),
        "pact" if r.treaty >= Treaty::TradePact => Some("A trade pact stands already.".into()),
        "pact" if r.treaty == Treaty::War => Some("Make peace first.".into()),
        "pact" if r.standing < PACT_STANDING => Some(format!(
            "They want {PACT_STANDING} standing for a pact; they are at {}.",
            r.standing
        )),
        "alliance" if r.treaty == Treaty::Alliance => Some("You are allies already.".into()),
        "alliance" if r.treaty < Treaty::TradePact => Some("A trade pact comes first.".into()),
        "alliance" if r.standing < ALLIANCE_STANDING => Some(format!(
            "They want {ALLIANCE_STANDING} standing for an alliance; they are at {}.",
            r.standing
        )),
        "war" if r.treaty == Treaty::War => Some("You are at war already.".into()),
        "peace" if r.treaty != Treaty::War => Some("You are not at war.".into()),
        "peace" if r.standing < -60 => Some(format!(
            "They will not talk below -60 standing; they are at {}.",
            r.standing
        )),
        _ => None,
    }
}

const VERBS: [(&str, &str); 5] = [
    ("gift", "Send a gift"),
    ("pact", "Propose a trade pact"),
    ("alliance", "Propose an alliance"),
    ("peace", "Sue for peace"),
    ("war", "Declare war"),
];

/// The relations screen's rows.
pub fn relations(world: &mut World) -> Vec<RivalView> {
    let (ours, our_fire) = our_fleet(world);
    let coins = world.resource::<Economy>().coins;
    let rivals = world.resource::<Rivals>().0.clone();
    rivals
        .iter()
        .map(|r| RivalView {
            id: r.id,
            name: r.name.into(),
            colour: r.colour,
            emblem: r.emblem.into(),
            temperament: r.temperament.into(),
            standing: r.standing,
            treaty: r.treaty,
            treaty_name: r.treaty.name().into(),
            economic: Economic {
                trade_per_min: r.trade_per_min,
                wealth: r.wealth,
                people: r.people,
                islands: r.islands,
                they_sell: r.sells.iter().map(|g| g.name().to_string()).collect(),
                they_buy: r.buys.iter().map(|g| g.name().to_string()).collect(),
            },
            military: Military {
                warships: r.warships,
                firepower: r.firepower,
                our_warships: ours,
                our_firepower: our_fire,
                lost_ours: r.lost_ours,
                lost_theirs: r.lost_theirs,
            },
            events: r
                .events
                .iter()
                .rev()
                .map(|(t, text, d)| EventView {
                    clock_s: *t,
                    text: text.clone(),
                    delta: *d,
                })
                .collect(),
            wants: r.wants(),
            actions: VERBS
                .iter()
                // (Peace only shows at war, war only when not.)
                .filter(|(v, _)| match *v {
                    "peace" => r.treaty == Treaty::War,
                    "war" => r.treaty != Treaty::War,
                    _ => true,
                })
                .map(|(v, label)| {
                    let why = why_not(r, v, coins);
                    ActionView {
                        label: (*label).into(),
                        action: format!("isle.rival.{v}"),
                        args: if *v == "gift" {
                            serde_json::json!({ "rival": r.id, "coins": GIFT_COINS })
                        } else {
                            serde_json::json!({ "rival": r.id })
                        },
                        enabled: why.is_none(),
                        why_not: why,
                    }
                })
                .collect(),
            placeholder: true,
        })
        .collect()
}

/// Do `verb` with rival `id`; Ok is the line to tell the player.
pub fn act(world: &mut World, verb: &str, id: u32) -> Result<String, String> {
    let coins = world.resource::<Economy>().coins;
    let clock = world.resource::<Economy>().clock_s;
    let mut rivals = world.resource::<Rivals>().0.clone();
    let r = rivals
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("No rival house numbered {id}."))?;
    if !VERBS.iter().any(|(v, _)| *v == verb) {
        return Err(format!(
            "Unknown order {verb:?}: gift, pact, alliance, peace or war."
        ));
    }
    if let Some(why) = why_not(r, verb, coins) {
        return Err(why);
    }
    let name = r.name;
    let line = match verb {
        "gift" => {
            // A second gift within ten minutes is worth half; a friend is harder to please.
            let base = if clock - r.last_gift_s < 600.0 { 4 } else { 8 };
            let delta = if r.standing > 50 { base / 2 } else { base };
            r.last_gift_s = clock;
            let d = r.shift(delta);
            r.log(clock, format!("We sent a gift of {GIFT_COINS} coins"), d);
            world.resource_mut::<Economy>().coins -= GIFT_COINS as f64;
            format!("{name} thank you for the gift: standing {:+}.", d)
        }
        "pact" => {
            r.treaty = Treaty::TradePact;
            let d = r.shift(5);
            r.log(clock, "We signed a trade pact", d);
            format!("{name} signed a trade pact.")
        }
        "alliance" => {
            r.treaty = Treaty::Alliance;
            let d = r.shift(10);
            r.log(clock, "We swore an alliance", d);
            format!("{name} are now our allies.")
        }
        "war" => {
            r.treaty = Treaty::War;
            let d = r.shift(-50);
            r.log(clock, "We declared war", d);
            format!("We are at war with {name}.")
        }
        "peace" => {
            r.treaty = Treaty::Hostile;
            let d = r.shift(5);
            r.log(clock, "We made peace", d);
            format!("Peace with {name}, though they remember.")
        }
        _ => unreachable!(),
    };
    world.resource_mut::<Rivals>().0 = rivals;
    Ok(line)
}

/// `isle.rival.<verb>.<id>` from the bars.
pub fn action(world: &mut World, rest: &str) -> Result<String, String> {
    let (verb, id) = rest
        .split_once('.')
        .ok_or("isle.rival.<verb>.<id>, such as isle.rival.gift.1")?;
    let id = id
        .parse::<u32>()
        .map_err(|_| format!("Not a rival number: {id}"))?;
    act(world, verb, id)
}

/// One API per order, in [`VERBS`]' order.
const ORDER_APIS: [&str; 5] = [
    "isle.rival.gift",
    "isle.rival.pact",
    "isle.rival.alliance",
    "isle.rival.peace",
    "isle.rival.war",
];

pub fn register(app: &mut App) {
    for (name, (verb, label)) in ORDER_APIS.into_iter().zip(VERBS) {
        app.register_api(name, label, r#"{"rival": 1}"#, move |world, args| {
            let id = args["rival"].as_u64().ok_or("rival: a number")? as u32;
            let text = act(world, verb, id)?;
            Ok(serde_json::json!({ "text": text }))
        });
    }
    app.init_resource::<Rivals>()
        .register_api(
            "isle.relations",
            "Each rival house: standing (-100 to 100), treaty, trade with us, wealth, people, islands, fleets and ours, recent events, what would raise standing, and the orders that can be given now (with why not).",
            "{}",
            |world, _| Ok(serde_json::to_value(crate::reports::relations(world)).unwrap_or_default()),
        )
        .register_api(
            "isle.rival",
            "Give an order to a rival house: gift, pact, alliance, peace or war.",
            r#"{"rival": 1, "order": "gift"}"#,
            |world, args| {
                let id = args["rival"].as_u64().ok_or("rival: a number")? as u32;
                let verb = args["order"].as_str().ok_or("order: gift, pact, alliance, peace or war")?;
                let text = act(world, verb, id)?;
                Ok(serde_json::json!({ "text": text }))
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut w = World::new();
        w.insert_resource(Economy {
            coins: 1_500.0,
            ..Default::default()
        });
        w.insert_resource(Rivals::default());
        w
    }

    #[test]
    fn gifts_and_treaties_move_the_standing_and_are_remembered() {
        let mut w = world();
        let start = relations(&mut w);
        assert_eq!(start.len(), 3);
        let gull = &start[0];
        assert_eq!(gull.standing, 15);
        // Not friends enough for a pact yet: the screen says why.
        let pact = gull
            .actions
            .iter()
            .find(|a| a.action == "isle.rival.pact")
            .unwrap();
        assert_eq!(pact.args["rival"], 1);
        assert!(!pact.enabled && pact.why_not.as_deref().unwrap().contains("25"));
        assert!(act(&mut w, "pact", 1).is_err());
        // Two gifts: the second soon after buys less.
        act(&mut w, "gift", 1).unwrap();
        act(&mut w, "gift", 1).unwrap();
        let r = &relations(&mut w)[0];
        assert_eq!(r.standing, 15 + 8 + 4);
        assert_eq!(w.resource::<Economy>().coins, 500.0);
        assert_eq!(r.events[0].delta, 4, "newest first");
        // Now the pact goes through.
        act(&mut w, "pact", 1).unwrap();
        assert_eq!(relations(&mut w)[0].treaty, Treaty::TradePact);
        // A third gift: no money left for it.
        assert!(act(&mut w, "gift", 1).is_ok());
        assert!(act(&mut w, "gift", 1).unwrap_err().contains("costs"));
    }

    #[test]
    fn war_and_peace() {
        let mut w = world();
        act(&mut w, "war", 3).unwrap();
        let r = &relations(&mut w)[2];
        assert_eq!(r.treaty, Treaty::War);
        assert_eq!(r.standing, -80);
        // Too angry to talk, and the screen offers peace, not war.
        assert!(r.actions.iter().all(|a| a.action != "isle.rival.war"));
        assert!(act(&mut w, "peace", 3).unwrap_err().contains("-60"));
        assert!(action(&mut w, "gift.3").unwrap_err().contains("war"));
        assert!(action(&mut w, "bribe.3").is_err());
        assert!(action(&mut w, "gift.9").is_err());
    }
}
