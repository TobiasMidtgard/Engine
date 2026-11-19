//! A shipyard's or airship mast's card: the craft on the slip with how far along it is, the
//! ones waiting, and every craft it can build with its price in coins and goods (short ones in
//! red) and an Order button that says why not when it can't.
//!
//! Plans and rules are the game's (crate::shipyard); this is the UI thread's.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors, Rank, SocketState};
use engine_ui::style::{Align, Justify, Pick, pct};

use crate::defs::Kind;
use crate::shipyard::{MAX_QUEUE, PLANS, Plan, Yard};
use crate::sim::{Building, Economy, IslandBooks};
use crate::tools::Hand;
use crate::vessels::Craft;

/// The card's yard section, filled by [`refresh`].
pub const CARD_ID: &str = "isle.sel.yard";
const SLIP_ID: &str = "isle.yard.slip";
const BAR_ID: &str = "isle.yard.bar";
const LEFT_ID: &str = "isle.yard.left";

#[derive(Resource, Default)]
pub struct YardCard {
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<YardCard>();
}

pub fn builds_craft(k: Kind) -> bool {
    PLANS.iter().any(|p| p.at == k)
}

pub fn card_placeholder() -> El {
    El::new()
        .class("col")
        .id(CARD_ID)
        .with_style(|s| s.gap(8.0))
}

/// The craft's own picture.
pub fn craft_icon(c: Craft) -> &'static str {
    match c {
        Craft::Schooner => "isle_craft_schooner",
        Craft::Steamer => "isle_craft_steamer",
        Craft::Smack => "isle_craft_smack",
        Craft::Barque => "isle_craft_barque",
        Craft::Airship => "isle_airship",
        Craft::Biplane => "isle_craft_biplane",
        Craft::Clipper => "isle_craft_clipper",
        Craft::PaddleSteamer => "isle_craft_paddle_steamer",
        Craft::Seaplane => "isle_craft_seaplane",
    }
}

fn key(c: Craft) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn mins(s: f32) -> String {
    let s = s.max(0.0).round() as u32;
    match (s / 60, s % 60) {
        (0, r) => format!("{r} s"),
        (m, 0) => format!("{m} min"),
        (m, r) => format!("{m} min {r} s"),
    }
}

/// Why this yard can't take an order for `p` now, in the game's own words.
fn why_not(p: &Plan, queued: usize, coins: f64, books: &IslandBooks, isle: &str) -> Option<String> {
    if queued >= MAX_QUEUE {
        return Some(format!("The yard has {MAX_QUEUE} on its list already."));
    }
    if books.population[p.tier.index()] < 1.0 {
        return Some(format!(
            "Building a {} takes {} on {isle}.",
            p.craft.name(),
            p.tier.name()
        ));
    }
    if coins < p.coins as f64 {
        return Some(format!(
            "A {} costs {} coins; you have {coins:.0}.",
            p.craft.name(),
            p.coins
        ));
    }
    for (g, n) in p.cost {
        if books.get(*g) + 1e-6 < *n as f64 {
            return Some(format!(
                "A {} needs {n} {} on {isle}; the warehouse has {:.0}.",
                p.craft.name(),
                g.name().to_lowercase(),
                books.get(*g)
            ));
        }
    }
    None
}

/// A price chip: a small picture and a number, red when the island lacks it.
fn chip(icon: &str, n: u32, short: bool) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(2.0).align_items(Align::Center))
        .child(El::icon(icon).with_style(|s| s.size(18.0, 18.0).pick(Pick::Ignore)))
        .child(
            El::text(n.to_string())
                .class("ink")
                .text_style(|t| t.size(13.0).bold().tabular().nowrap())
                .with_style(move |s| {
                    s.color(if short {
                        Colors::bad_ink()
                    } else {
                        Colors::ink()
                    })
                })
                .pick_ignore(),
        )
}

fn plan_row(p: &Plan, why: Option<String>, coins: f64, books: &IslandBooks) -> El {
    let ok = why.is_none();
    let tip = format!(
        "[i]{}[/i]\n[b]Build[/b]  {} on the slip\n[b]Keep[/b]  {:.0} coins a minute once launched\n[b]Needs[/b]  {} on the island{}",
        p.blurb,
        mins(p.build_s),
        p.upkeep,
        p.tier.name(),
        why.as_ref()
            .map_or(String::new(), |w| format!("\n[color=#9a2f1c]{w}[/color]"))
    );
    El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(8.0)
                .align_items(Align::Center)
                .opacity(if ok { 1.0 } else { 0.8 })
        })
        .tooltip(p.craft.name(), tip)
        .child(brass::badge(craft_icon(p.craft), 38.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(1.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(p.craft.name())
                        .class("ink")
                        .text_style(|t| t.size(14.0).bold().nowrap())
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(7.0).align_items(Align::Center).wrap(true))
                        .child(chip("coin", p.coins, coins < p.coins as f64))
                        .children(
                            p.cost
                                .iter()
                                .map(|(g, n)| chip(g.icon(), *n, books.get(*g) + 1e-6 < *n as f64)),
                        )
                        .child(
                            El::text(format!("· {}", mins(p.build_s)))
                                .class("ink")
                                .text_style(|t| t.size(12.0).nowrap())
                                .with_style(|s| s.color(Colors::ink_dim()))
                                .pick_ignore(),
                        ),
                ),
        )
        .child(
            El::button("Order", format!("isle.yard.order:{}", key(p.craft)))
                .class("brass_button")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.shrink(0.0))
                .disabled(!ok),
        )
}

/// The slip: the craft being built with a ring filling round it, the time to launch and a
/// bar, then the ones waiting their turn.
fn slip(yard: &Yard) -> El {
    let Some(first) = yard.queue.first().copied() else {
        return El::new()
            .class("row")
            .with_style(|s| s.gap(10.0).align_items(Align::Center))
            .child(brass::socket(None, 50.0, ""))
            .child(
                El::text("The slip is empty: order a craft below.")
                    .class("ink")
                    .text_style(|t| t.size(14.0).italic())
                    .with_style(|s| s.color(Colors::ink_dim()))
                    .pick_ignore(),
            );
    };
    let share = yard.share();
    let left = crate::shipyard::plan(first).map_or(0.0, |p| p.build_s - yard.progress_s);
    let waiting = yard.queue.iter().skip(1).copied().collect::<Vec<_>>();
    El::new()
        .class("row")
        .with_style(|s| s.gap(12.0).align_items(Align::Center))
        .child(brass::socket_with(
            Some((craft_icon(first), Rank::Fine)),
            56.0,
            "",
            &slip_state(share),
            SLIP_ID,
        ))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(4.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(first.name())
                        .class("ink")
                        .text_style(|t| t.size(16.0).bold().nowrap())
                        .pick_ignore(),
                )
                .child(
                    El::text(format!("Launches in {}", mins(left)))
                        .class("ink")
                        .id(LEFT_ID)
                        .text_style(|t| t.size(13.0).nowrap())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("brass_track")
                        .with_style(|s| s.width(pct(100.0)).height(9.0))
                        .child(
                            El::new()
                                .class("brass_fill")
                                .id(BAR_ID)
                                .with_style(move |s| s.width(pct(share * 100.0))),
                        ),
                )
                .child_opt((!waiting.is_empty()).then(|| {
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(4.0).align_items(Align::Center))
                        .child(
                            El::text("Then")
                                .class("ink")
                                .text_style(|t| t.size(12.0).italic())
                                .with_style(|s| s.color(Colors::ink_dim()))
                                .pick_ignore(),
                        )
                        .children(waiting.into_iter().map(|c| {
                            El::icon(craft_icon(c))
                                .with_style(|s| s.size(26.0, 26.0))
                                .tooltip(c.name(), "Waits for the slip.")
                        }))
                })),
        )
        .child(
            El::button("Cancel", "isle.yard.cancel")
                .class("brass_button_dark")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.shrink(0.0))
                .tooltip(
                    "Cancel the last order",
                    "All of its price back while it waits; half once it is on the slip.",
                ),
        )
}

fn slip_state(share: f32) -> SocketState {
    SocketState {
        fill: Some(share),
        color: Colors::brass_light(),
        ..Default::default()
    }
}

fn section(yard: &Yard, plans: &[&Plan], coins: f64, books: &IslandBooks, isle: &str) -> Vec<El> {
    let mut out = vec![
        brass::label(
            &format!("On the slip · {} of {MAX_QUEUE} ordered", yard.queue.len()),
            true,
        ),
        slip(yard),
        brass::label("Order", true),
    ];
    out.push(
        El::new()
            .class("col")
            .with_style(|s| s.gap(6.0).justify(Justify::Start))
            .children(plans.iter().map(|p| {
                plan_row(
                    p,
                    why_not(p, yard.queue.len(), coins, books, isle),
                    coins,
                    books,
                )
            })),
    );
    out
}

/// Fill or update the section for the selected yard, and move its slip every tick.
pub fn refresh(world: &mut World) {
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return;
    };
    let Some(b) = world.get::<Building>(e).cloned() else {
        return;
    };
    if !builds_craft(b.kind) {
        return;
    }
    let yard = world.get::<Yard>(e).cloned().unwrap_or_default();
    let econ = world.resource::<Economy>();
    let coins = econ.coins;
    let Some(books) = econ.islands.get(b.island).cloned() else {
        return;
    };
    let isle = crate::island::islands()[b.island].name;
    let plans: Vec<&Plan> = PLANS.iter().filter(|p| p.at == b.kind).collect();
    let can: Vec<bool> = plans
        .iter()
        .map(|p| why_not(p, yard.queue.len(), coins, &books, isle).is_none())
        .collect();
    let short: Vec<String> = plans
        .iter()
        .flat_map(|p| p.cost.iter())
        .map(|(g, n)| format!("{}", (books.get(*g) + 1e-6 >= *n as f64) as u8))
        .collect();
    let sig = format!(
        "{e:?}|{:?}|{can:?}|{}|{}",
        yard.queue,
        short.join(""),
        plans
            .iter()
            .map(|p| (coins >= p.coins as f64) as u8)
            .map(|x| x.to_string())
            .collect::<String>()
    );
    if world.resource::<YardCard>().sig != sig {
        let els = section(&yard, &plans, coins, &books, isle);
        if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
            let _ = engine_ui::el::replace_children(world, node, &els);
            world.resource_mut::<YardCard>().sig = sig;
        }
    }
    // The slip moves every tick: its ring, bar and countdown, in place.
    let share = yard.share();
    if let Some(n) = engine_ui::input::find_id(world, SLIP_ID)
        && let Some(mut st) = world.get_mut::<SocketState>(n)
    {
        let new = slip_state(share);
        if *st != new {
            *st = new;
        }
    }
    if let Some(n) = engine_ui::input::find_id(world, BAR_ID)
        && let Some(mut node) = world.get_mut::<engine_ui::node::Node>(n)
    {
        let w = Some(pct(share * 100.0));
        if node.style.width != w {
            node.style.width = w;
        }
    }
    if let Some(first) = yard.queue.first()
        && let Some(p) = crate::shipyard::plan(*first)
        && let Some(n) = engine_ui::input::find_id(world, LEFT_ID)
        && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(n)
    {
        let s = format!("Launches in {}", mins(p.build_s - yard.progress_s));
        if t.text != s {
            t.text = s;
        }
    }
}

/// A new card was opened: fill the section afresh.
pub fn card_opened(world: &mut World) {
    world.resource_mut::<YardCard>().sig.clear();
    refresh(world);
}
