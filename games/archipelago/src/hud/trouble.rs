//! The card's trouble section: when the selected building is on fire, rioting or sick, a
//! banner with how bad it is (a bar that moves in place), how long it has gone on and whether
//! help is coming; when it has burnt down, the ruin with its rebuild price (short goods in
//! red) and a Rebuild button. Empty otherwise.
//!
//! Rules and data are the game's (`crate::incidents`); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors, Urgency};
use engine_ui::style::{Align, Fill, Pick, pct};

use crate::defs::{Good, Kind};
use crate::incidents::{Incident, Ruin, Trouble};
use crate::sim::{Building, Economy, IslandBooks};
use crate::tools::Hand;

pub const CARD_ID: &str = "isle.sel.trouble";
const BAR_ID: &str = "isle.trouble.bar";
const SINCE_ID: &str = "isle.trouble.since";

#[derive(Resource, Default)]
pub struct TroubleCard {
    sig: String,
    /// The building the card was opened for and whether it was a ruin then (its buttons
    /// differ, so the card is opened afresh when that changes).
    built: Option<(Entity, bool)>,
}

pub fn install(world: &mut World) {
    world.init_resource::<TroubleCard>();
}

pub fn card_placeholder() -> El {
    El::new()
        .class("col")
        .id(CARD_ID)
        .with_style(|s| s.gap(8.0))
}

fn mins(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    match (s / 60, s % 60) {
        (0, r) => format!("{r} s"),
        (m, _) => format!("{m} min"),
    }
}

fn words(what: Trouble) -> (&'static str, &'static str, &'static str) {
    match what {
        Trouble::Fire => ("On fire", "isle_alert_fire", "the fire watch"),
        Trouble::Riot => ("A riot", "isle_alert_riot", "the constables"),
        Trouble::Sickness => ("Sickness", "isle_alert_sick", "the doctor"),
    }
}

fn answered_by(what: Trouble) -> &'static str {
    what.answered_by().def().name
}

/// A banner on parchment: a coloured wash with a stripe, its picture, words and whatever
/// goes on the right.
fn banner(c: Color, icon: &str, title: String, sub: El, right: Option<El>) -> El {
    El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(10.0)
                .align_items(Align::Center)
                .padding([8.0, 10.0, 8.0, 14.0])
                .radius(8.0)
                .background(Fill::Solid(c.with_alpha(0.16)))
                .border(1.0, c.with_alpha(0.55))
        })
        .child(El::new().with_style(move |s| {
            s.position(engine_ui::style::Position::Absolute)
                .left(4.0)
                .top(6.0)
                .bottom(6.0)
                .width(4.0)
                .radius(2.0)
                .background(Fill::Solid(c))
                .pick(Pick::Ignore)
        }))
        .child(brass::badge(icon, 40.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(title)
                        .class("ink")
                        .text_style(|t| t.size(15.0).bold().nowrap())
                        .pick_ignore(),
                )
                .child(sub),
        )
        .child_opt(right)
}

fn incident(i: &Incident, clock: f64) -> El {
    let (title, icon, _) = words(i.what);
    let urgency = if i.what == Trouble::Fire || i.level >= 0.6 || i.crew.is_none() {
        Urgency::Danger
    } else {
        Urgency::Warning
    };
    let c = urgency.color();
    let help = help(i);
    let level = i.level.clamp(0.0, 1.0);
    let crewless = i.crew.is_none();
    let sub = El::new()
        .class("col")
        .with_style(|s| s.gap(3.0))
        .child(
            El::new()
                .class("brass_track")
                .with_style(|s| s.width(pct(100.0)).height(8.0))
                .child(El::new().id(BAR_ID).with_style(move |s| {
                    s.width(pct(level * 100.0))
                        .height(pct(100.0))
                        .radius(4.0)
                        .background(Fill::vertical(c.lerp(Color::hex(0xffffff), 0.3), c))
                        .pick(Pick::Ignore)
                })),
        )
        .child(
            El::text(format!("{help} · for {}", mins(clock - i.since_s)))
                .class("ink")
                .id(SINCE_ID)
                .text_style(|t| t.size(12.5))
                .with_style(move |s| {
                    s.color(if crewless {
                        Colors::bad_ink()
                    } else {
                        Colors::ink_dim()
                    })
                })
                .pick_ignore(),
        );
    banner(c, icon, title.to_string(), sub, None)
}

/// Who is answering it, in words.
fn help(i: &Incident) -> String {
    let (_, _, helper) = words(i.what);
    if i.fought {
        format!(
            "{} {} at work",
            capital(helper),
            if helper.ends_with('s') { "are" } else { "is" }
        )
    } else if i.crew.is_some() {
        format!("{} on the way", capital(helper))
    } else {
        format!("No {} in reach: build one nearby", answered_by(i.what))
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn ruin(kind: Kind, r: &Ruin, clock: f64, coins: f64, books: &IslandBooks) -> El {
    let (price, cost) = crate::incidents::rebuild_cost(kind);
    let short = |g: Good, n: u32| books.get(g) + 1e-6 < n as f64;
    let can = coins >= price as f64 && cost.iter().all(|(g, n)| !short(*g, *n));
    let chip = |icon: &str, n: u32, bad: bool| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(2.0).align_items(Align::Center))
            .child(El::icon(icon).with_style(|s| s.size(18.0, 18.0).pick(Pick::Ignore)))
            .child(
                El::text(n.to_string())
                    .class("ink")
                    .text_style(|t| t.size(13.0).bold().tabular().nowrap())
                    .with_style(move |s| {
                        s.color(if bad {
                            Colors::bad_ink()
                        } else {
                            Colors::ink()
                        })
                    })
                    .pick_ignore(),
            )
    };
    let sub = El::new()
        .class("col")
        .with_style(|s| s.gap(3.0))
        .child(
            El::text(format!(
                "Burnt down {} ago. Rebuild for half its price:",
                mins(clock - r.since_s)
            ))
            .class("ink")
            .text_style(|t| t.size(12.5))
            .with_style(|s| s.color(Colors::ink_dim()))
            .pick_ignore(),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(7.0).wrap(true).align_items(Align::Center))
                .child(chip("coin", price, coins < price as f64))
                .children(cost.iter().map(|(g, n)| chip(g.icon(), *n, short(*g, *n)))),
        );
    banner(
        Color::hex(0x6a5a4a),
        "isle_ruin",
        format!("{} in ruins", kind.def().name),
        sub,
        Some(
            El::button("Rebuild", "isle.ruin.rebuild")
                .class("brass_button")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.shrink(0.0))
                .disabled(!can)
                .tooltip(
                    "Rebuild",
                    if can {
                        "Put it up again as it was."
                    } else {
                        "Not enough coins or goods on the island yet."
                    },
                ),
        ),
    )
}

/// Fill or update the section for the selected building.
pub fn refresh(world: &mut World) {
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return;
    };
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    let inc = world.get::<Incident>(e).cloned();
    let ruin_c = world.get::<Ruin>(e).cloned();
    if let Some((was, r)) = world.resource::<TroubleCard>().built
        && was == e
        && r != ruin_c.is_some()
    {
        super::show_card(world);
        return;
    }
    let econ = world.resource::<Economy>();
    let clock = econ.clock_s;
    let coins = econ.coins;
    let Some(books) = econ.islands.get(b.island).cloned() else {
        return;
    };
    let price = crate::incidents::rebuild_cost(b.kind);
    let can = coins >= price.0 as f64
        && price
            .1
            .iter()
            .all(|(g, n)| books.get(*g) + 1e-6 >= *n as f64);
    let sig = format!(
        "{e:?}|{:?}|{:?}|{}|{can}",
        inc.as_ref()
            .map(|i| (i.what, i.crew.is_some(), i.fought, i.level >= 0.6)),
        ruin_c.as_ref().map(|r| (r.since_s / 60.0).floor()),
        b.kind.def().name,
    );
    if world.resource::<TroubleCard>().sig != sig {
        let els: Vec<El> = match (&inc, &ruin_c) {
            (_, Some(r)) => vec![ruin(b.kind, r, clock, coins, &books)],
            (Some(i), None) => vec![incident(i, clock)],
            _ => Vec::new(),
        };
        if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
            let _ = engine_ui::el::replace_children(world, node, &els);
            world.resource_mut::<TroubleCard>().sig = sig;
        }
    }
    // The bar and the time move in place.
    if let Some(i) = inc {
        if let Some(n) = engine_ui::input::find_id(world, BAR_ID)
            && let Some(mut node) = world.get_mut::<engine_ui::node::Node>(n)
        {
            let w = Some(pct(i.level.clamp(0.0, 1.0) * 100.0));
            if node.style.width != w {
                node.style.width = w;
            }
        }
        if let Some(n) = engine_ui::input::find_id(world, SINCE_ID)
            && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(n)
        {
            let s = format!("{} · for {}", help(&i), mins(clock - i.since_s));
            if t.text != s {
                t.text = s;
            }
        }
    }
}

/// A new card was opened: fill the section afresh.
pub fn card_opened(world: &mut World) {
    let built = world
        .get_resource::<Hand>()
        .and_then(|h| h.selected)
        .map(|e| (e, world.get::<Ruin>(e).is_some()));
    let mut t = world.resource_mut::<TroubleCard>();
    t.sig.clear();
    t.built = built;
    refresh(world);
}
