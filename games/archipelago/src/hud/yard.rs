//! A shipyard's or airship mast's card: the craft on the slip with how far along it is, the
//! ones waiting, and every craft it can build with its price in coins and goods (short ones in
//! red) and an Order button that says why not when it can't. "Fit out" opens a craft's
//! designer: its fitting slots, the fittings that go on it, what they do to its speed, hull,
//! guns and holds, and the price with them.
//!
//! Plans and rules are the game's (crate::shipyard); this is the UI thread's.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors, Rank, SocketState};
use engine_ui::style::{Align, Justify, Pick, pct};

use crate::defs::Kind;
use crate::fittings::{self, Fit, Fitting};
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
    /// The craft being fitted out and its fittings so far.
    design: Option<(Craft, Fit)>,
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
        Craft::Cutter => "isle_craft_cutter",
        Craft::Brig => "isle_craft_brig",
        Craft::Frigate => "isle_craft_frigate",
        Craft::Gunboat => "isle_craft_gunboat",
        Craft::Liner => "isle_craft_liner",
        Craft::FlyingBoat => "isle_craft_flying_boat",
        Craft::Dreadnought => "isle_craft_dreadnought",
        Craft::Leviathan => "isle_craft_leviathan",
        Craft::SkyCarrier => "isle_craft_sky_carrier",
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
fn why_not(
    p: &Plan,
    fit: &Fit,
    queued: usize,
    coins: f64,
    books: &IslandBooks,
    isle: &str,
) -> Option<String> {
    if let Err(e) = fittings::check(p.craft, fit) {
        return Some(e);
    }
    let (price, cost) = total(p, fit);
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
    if coins < price as f64 {
        return Some(format!(
            "A {} costs {price} coins; you have {coins:.0}.",
            p.craft.name(),
        ));
    }
    for (g, n) in &cost {
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

/// The hull's price with `fit`'s fittings on top: coins and goods.
fn total(p: &Plan, fit: &Fit) -> (u32, Vec<(crate::defs::Good, u32)>) {
    let (coins, goods) = fittings::price(fit);
    let mut cost: Vec<(crate::defs::Good, u32)> = p.cost.to_vec();
    for (g, n) in goods {
        match cost.iter_mut().find(|x| x.0 == g) {
            Some(x) => x.1 += n,
            None => cost.push((g, n)),
        }
    }
    (p.coins + coins, cost)
}

fn fit_key(f: Fitting) -> String {
    serde_json::to_value(f)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A fitting's picture (the ship-gear set has one for each).
pub fn fitting_icon(f: Fitting) -> &'static str {
    match f {
        Fitting::Canvas => "isle_gear_sails",
        Fitting::Copper => "isle_gear_sheathing",
        Fitting::Compound => "isle_gear_engine",
        Fitting::Oak => "isle_gear_repair",
        Fitting::Plate => "isle_gear_armour",
        Fitting::Swivels => "isle_gear_sights",
        Fitting::Carronades => "isle_gear_shot",
        Fitting::LongGuns => "isle_gear_cannon",
        Fitting::Mortar => "isle_gear_bomb",
        Fitting::Hold => "isle_gear_hatch",
        Fitting::Marines => "isle_gear_cutlass",
        Fitting::GasCell => "isle_gear_gas_cell",
        Fitting::Engines => "isle_gear_propeller",
        Fitting::Spotter => "isle_gear_spyglass",
        Fitting::MailRoom => "isle_gear_mail",
    }
}

/// What a fitting does, in a line.
fn fitting_line(f: Fitting) -> String {
    let d = f.def();
    let mut v = Vec::new();
    let pct = |x: f32| (x * 100.0).round();
    if d.speed.abs() > 0.001 {
        v.push(format!("{:+.0}% speed", pct(d.speed)));
    }
    if d.hull.abs() > 0.001 {
        v.push(format!("{:+.0}% hull", pct(d.hull)));
    }
    if d.guns.0 > 0.0 {
        v.push(format!(
            "{:.0} damage a minute, {:.0} m reach",
            d.guns.0, d.guns.1
        ));
    }
    if d.holds > 0 {
        v.push(format!("+{} hold", d.holds));
    }
    if d.sight > 0.001 {
        v.push(format!("+{:.0}% sight", pct(d.sight)));
    }
    if d.postage > 0.001 {
        v.push(format!("+{:.0} coins a landing", d.postage));
    }
    if d.room > 0.001 {
        v.push(format!("takes {:.0}% of each hold", pct(d.room)));
    }
    if d.upkeep > 0.001 {
        v.push(format!("+{:.0} upkeep", d.upkeep));
    }
    v.join(" · ")
}

/// The usual fittings as small pictures (on a plan row).
fn fit_icons(fit: &Fit) -> Vec<El> {
    fit.iter()
        .map(|f| {
            El::icon(fitting_icon(f))
                .with_style(|s| s.size(20.0, 20.0))
                .tooltip(f.def().name, fitting_line(f))
        })
        .collect()
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
    let fit = fittings::usual(p.craft);
    let (price, cost) = total(p, &fit);
    let slots = fittings::slots(p.craft);
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
                        .child(chip("coin", price, coins < price as f64))
                        .children(
                            cost.iter()
                                .map(|(g, n)| chip(g.icon(), *n, books.get(*g) + 1e-6 < *n as f64)),
                        )
                        .children(fit_icons(&fit))
                        .child(
                            El::text(format!("· {}", mins(p.build_s)))
                                .class("ink")
                                .text_style(|t| t.size(12.0).nowrap())
                                .with_style(|s| s.color(Colors::ink_dim()))
                                .pick_ignore(),
                        ),
                ),
        )
        .child_opt((slots > 0).then(|| {
            El::button("Fit out", format!("isle.yard.design:{}", key(p.craft)))
                .class("brass_button_dark")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.shrink(0.0).padding([6.0, 10.0]))
                .tooltip(
                    "Fit out",
                    format!(
                        "Choose its {slots} fittings before the keel is laid: canvas, engines, \
                         guns, armour, holds."
                    ),
                )
        }))
        .child(
            El::button("Order", format!("isle.yard.order:{}", key(p.craft)))
                .class("brass_button")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.shrink(0.0))
                .disabled(!ok),
        )
}

/// One figure of a design against the bare hull: "Speed  7.4 m/s  +21%".
fn stat(label: &str, value: String, base: f32, now: f32, more_is_good: bool) -> El {
    let d = if base.abs() > 1e-3 {
        (now / base - 1.0) * 100.0
    } else {
        0.0
    };
    let delta = if d.abs() < 0.5 {
        String::new()
    } else {
        format!("{d:+.0}%")
    };
    let good = (d > 0.0) == more_is_good;
    El::new()
        .class("row")
        .with_style(|s| s.gap(6.0).align_items(Align::Center).width(pct(48.0)))
        .child(
            El::text(label)
                .class("ink")
                .text_style(|t| t.size(12.5).nowrap())
                .with_style(|s| s.color(Colors::ink_dim()).width(54.0))
                .pick_ignore(),
        )
        .child(
            El::text(value)
                .class("ink")
                .text_style(|t| t.size(13.5).bold().tabular().nowrap())
                .pick_ignore(),
        )
        .child_opt((!delta.is_empty()).then(|| {
            El::text(delta)
                .class("ink")
                .text_style(|t| t.size(12.0).bold().nowrap())
                .with_style(move |s| {
                    s.color(if good {
                        Colors::good_ink()
                    } else {
                        Colors::bad_ink()
                    })
                })
                .pick_ignore()
        }))
}

/// The designer for `p` with `fit` so far: its slots, the fittings to choose from, what it
/// adds up to against the bare hull, the price and Order.
fn designer(
    p: &Plan,
    fit: &Fit,
    why: Option<String>,
    coins: f64,
    books: &IslandBooks,
    saved: &[(usize, String, Fit)],
) -> El {
    let c = p.craft;
    let slots = fittings::slots(c);
    let taken: Vec<Fitting> = fit.iter().collect();
    let full = taken.len() >= slots;
    let sockets = (0..slots).map(|i| match taken.get(i) {
        Some(f) => brass::socket(
            Some((fitting_icon(*f), Rank::Fine)),
            44.0,
            &format!("isle.yard.unfit:{i}"),
        )
        .tooltip(
            f.def().name,
            format!("{}\n[i]Click to take it off.[/i]", fitting_line(*f)),
        ),
        None => brass::socket(None, 44.0, "").tooltip("Empty slot", "Pick a fitting below."),
    });
    let choices = fittings::FITTINGS
        .iter()
        .filter(|d| d.fitting.fits(c))
        .map(|d| {
            let f = d.fitting;
            let n = taken.iter().filter(|x| **x == f).count();
            let can = !full && n < d.most;
            El::new()
                .class("button brass_button")
                .on_click(format!("isle.yard.fit:{}", fit_key(f)))
                .disabled(!can)
                .with_style(|s| s.padding([3.0, 8.0, 3.0, 4.0]).gap(4.0))
                .tooltip(
                    d.name,
                    format!(
                        "[i]{}[/i]\n{}\n[b]Costs[/b]  {} coins{}",
                        d.blurb,
                        fitting_line(f),
                        d.coins,
                        d.cost
                            .iter()
                            .map(|(g, n)| format!(", {n} {}", g.name().to_lowercase()))
                            .collect::<String>()
                    ),
                )
                .child(
                    El::icon(fitting_icon(f)).with_style(|s| s.size(22.0, 22.0).pick(Pick::Ignore)),
                )
                .child(
                    El::text(d.name)
                        .text_style(|t| t.size(12.5).nowrap())
                        .pick_ignore(),
                )
        });
    let bare = fittings::stats(c, &Fit::default());
    let now = fittings::stats(c, fit);
    let (price, cost) = total(p, fit);
    let ok = why.is_none();
    let names = taken
        .iter()
        .map(|f| fit_key(*f))
        .collect::<Vec<_>>()
        .join(",");
    let mut figures = vec![
        stat(
            "Speed",
            format!("{:.1} m/s", now.speed),
            bare.speed,
            now.speed,
            true,
        ),
        stat(
            "Hull",
            format!("{:.0}", now.hull),
            bare.hull,
            now.hull,
            true,
        ),
        stat(
            "Holds",
            format!("{} × {}", now.holds, now.per_hold),
            (bare.holds * bare.per_hold) as f32,
            (now.holds * now.per_hold) as f32,
            true,
        ),
        stat(
            "Upkeep",
            format!("{:.0} a min", now.upkeep),
            bare.upkeep,
            now.upkeep,
            false,
        ),
    ];
    if now.firepower > 0.0 {
        figures.push(stat(
            "Guns",
            format!("{:.0} · {:.0} m", now.firepower, now.range),
            bare.firepower,
            now.firepower,
            true,
        ));
    }
    if now.sight > 0.0 || now.postage > 0.0 {
        figures.push(stat(
            if now.postage > 0.0 { "Post" } else { "Sight" },
            if now.postage > 0.0 {
                format!("+{:.0} a landing", now.postage)
            } else {
                format!("+{:.0}%", now.sight * 100.0)
            },
            0.0,
            0.0,
            true,
        ));
    }
    El::new()
        .class("col")
        .with_style(|s| {
            s.gap(8.0)
                .padding([10.0, 12.0])
                .radius(8.0)
                .background(engine_ui::style::Fill::Solid(
                    Colors::brass().with_alpha(0.16),
                ))
                .border(1.0, Colors::brass_dark().with_alpha(0.5))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(Align::Center))
                .child(brass::badge(craft_icon(c), 44.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0))
                        .child(
                            El::text(format!("Fitting out a {}", c.name().to_lowercase()))
                                .class("ink")
                                .text_style(|t| t.size(15.0).bold().nowrap())
                                .pick_ignore(),
                        )
                        .child(brass::label(
                            &format!("{} of {slots} slots taken", taken.len()),
                            true,
                        )),
                )
                .child(
                    brass::medallion("isle_close", 26.0, &format!("isle.yard.design:{}", key(c)))
                        .tooltip("", "Close the designer"),
                ),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).justify(Justify::Center))
                .children(sockets),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(5.0).wrap(true))
                .children(choices),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(4.0).wrap(true))
                .children(figures),
        )
        .child_opt((!saved.is_empty()).then(|| {
            // Designs saved for this craft: click one to load it.
            El::new()
                .class("row")
                .with_style(|s| s.gap(5.0).wrap(true).align_items(Align::Center))
                .child(brass::label("Saved", true))
                .children(saved.iter().map(|(_, name, f)| {
                    let same = f == fit;
                    El::new()
                        .class(if same {
                            "button brass_button"
                        } else {
                            "button brass_button_dark"
                        })
                        .on_click(format!("isle.yard.load:{name}"))
                        .with_style(|s| s.padding([3.0, 8.0]).gap(3.0))
                        .tooltip(
                            name.clone(),
                            format!("{}\n[i]Click to load it.[/i]", f.names().join(", ")),
                        )
                        .child(
                            El::text(name.clone())
                                .text_style(|t| t.size(12.5).bold().nowrap())
                                .pick_ignore(),
                        )
                        .children(f.iter().map(|x| {
                            El::icon(fitting_icon(x))
                                .with_style(|s| s.size(16.0, 16.0).pick(Pick::Ignore))
                        }))
                }))
        }))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Center))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| {
                            s.gap(7.0)
                                .align_items(Align::Center)
                                .wrap(true)
                                .grow(1.0)
                                .shrink(1.0)
                                .basis(0.0)
                        })
                        .child(chip("coin", price, coins < price as f64))
                        .children(
                            cost.iter()
                                .map(|(g, n)| chip(g.icon(), *n, books.get(*g) + 1e-6 < *n as f64)),
                        ),
                )
                .child(
                    El::button("Save", "isle.yard.save")
                        .class("brass_button_dark")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.shrink(0.0).padding([6.0, 10.0]))
                        .disabled(saved.iter().any(|(_, _, f)| f == fit))
                        .tooltip(
                            "Save this design",
                            "Keep it under a name, to load and order again.",
                        ),
                )
                .child(
                    El::button("Usual", "isle.yard.usual")
                        .class("brass_button_dark")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.shrink(0.0).padding([6.0, 10.0]))
                        .tooltip("Usual fittings", "Back to what this craft is known for."),
                )
                .child(
                    El::button("Order", format!("isle.yard.fitted:{}:{names}", key(c)))
                        .class("brass_button")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.shrink(0.0))
                        .disabled(!ok)
                        .tooltip(
                            if ok { "Order it" } else { "Can't order yet" },
                            why.unwrap_or_else(|| "Paid now; launched when built.".into()),
                        ),
                ),
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

fn section(
    yard: &Yard,
    plans: &[&Plan],
    design: Option<(Craft, Fit)>,
    saved: &[(usize, String, Fit)],
    coins: f64,
    books: &IslandBooks,
    isle: &str,
) -> Vec<El> {
    let mut out = vec![
        brass::label(
            &format!("On the slip · {} of {MAX_QUEUE} ordered", yard.queue.len()),
            true,
        ),
        slip(yard),
    ];
    // Fitting one out: the designer alone (its close button goes back to the list).
    if let Some((c, fit)) = design
        && let Some(p) = plans.iter().find(|p| p.craft == c)
    {
        out.push(brass::label("Fit out and order", true));
        out.push(designer(
            p,
            &fit,
            why_not(p, &fit, yard.queue.len(), coins, books, isle),
            coins,
            books,
            saved,
        ));
        return out;
    }
    out.push(brass::label(
        &format!("Order · {} craft", plans.len()),
        true,
    ));
    // A long list scrolls, so the card stays clear of the map.
    out.push(
        El::new()
            .class("scroll")
            .with_style(|s| {
                s.direction(engine_ui::style::Direction::Column)
                    .gap(6.0)
                    .max_height(430.0)
                    .justify(Justify::Start)
            })
            .children(plans.iter().map(|p| {
                plan_row(
                    p,
                    why_not(
                        p,
                        &fittings::usual(p.craft),
                        yard.queue.len(),
                        coins,
                        books,
                        isle,
                    ),
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
    let design = world
        .resource::<YardCard>()
        .design
        .filter(|(c, _)| plans.iter().any(|p| p.craft == *c));
    let fit_of = |p: &Plan| match design {
        Some((c, f)) if c == p.craft => f,
        _ => fittings::usual(p.craft),
    };
    let can: Vec<bool> = plans
        .iter()
        .map(|p| why_not(p, &fit_of(p), yard.queue.len(), coins, &books, isle).is_none())
        .collect();
    let short: Vec<String> = plans
        .iter()
        .flat_map(|p| total(p, &fit_of(p)).1)
        .map(|(g, n)| format!("{}", (books.get(g) + 1e-6 >= n as f64) as u8))
        .collect();
    let saved: Vec<(usize, String, Fit)> = design
        .and_then(|(c, _)| {
            world.get_resource::<fittings::Designs>().map(|d| {
                d.0.iter()
                    .enumerate()
                    .filter(|(_, x)| x.craft == c)
                    .map(|(i, x)| (i, x.name.clone(), x.fit))
                    .collect()
            })
        })
        .unwrap_or_default();
    let sig = format!(
        "{e:?}|{:?}|{can:?}|{}|{}|{design:?}|{saved:?}",
        yard.queue,
        short.join(""),
        plans
            .iter()
            .map(|p| (coins >= total(p, &fit_of(p)).0 as f64) as u8)
            .map(|x| x.to_string())
            .collect::<String>()
    );
    if world.resource::<YardCard>().sig != sig {
        let els = section(&yard, &plans, design, &saved, coins, &books, isle);
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

/// `isle.yard.design:<craft>` (open or close a craft's designer), `isle.yard.fit:<fitting>`,
/// `isle.yard.unfit:<slot>`, `isle.yard.usual`, and `isle.yard.fitted:…` (ordered through the
/// game; the designer closes once the order is in). Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a.starts_with("isle.yard.fitted:") {
        let queued = |world: &mut World| {
            world
                .get_resource::<Hand>()
                .and_then(|h| h.selected)
                .and_then(|e| world.get::<Yard>(e))
                .map_or(0, |y| y.queue.len())
        };
        let before = queued(world);
        crate::tools::action(world, a);
        if queued(world) > before {
            world.resource_mut::<YardCard>().design = None;
        }
        refresh(world);
        return true;
    }
    if a == "isle.yard.save" {
        if let Some((c, fit)) = world.resource::<YardCard>().design {
            let n = world
                .get_resource::<fittings::Designs>()
                .map_or(0, |d| d.0.iter().filter(|x| x.craft == c).count());
            let name = format!("{} {}", c.name(), n + 1);
            if let Err(e) = fittings::save(world, &name, c, fit) {
                super::complain(world, "Shipyard", &e);
            }
        }
        refresh(world);
        return true;
    }
    if let Some(name) = a.strip_prefix("isle.yard.load:") {
        let d = world.get_resource::<fittings::Designs>().and_then(|d| {
            d.0.iter()
                .find(|x| x.name == name)
                .map(|x| (x.craft, x.fit))
        });
        if let Some(d) = d {
            world.resource_mut::<YardCard>().design = Some(d);
        }
        refresh(world);
        return true;
    }
    let mut card = world.resource_mut::<YardCard>();
    if let Some(k) = a.strip_prefix("isle.yard.design:") {
        let c = Craft::from_name(k);
        card.design = match (card.design, c) {
            (Some((was, _)), Some(c)) if was == c => None,
            (_, Some(c)) => Some((c, fittings::usual(c))),
            _ => None,
        };
    } else if let Some(f) = a
        .strip_prefix("isle.yard.fit:")
        .and_then(Fitting::from_name)
    {
        if let Some((c, fit)) = card.design.as_mut() {
            let mut list: Vec<Fitting> = fit.iter().collect();
            list.push(f);
            let next = Fit::of(&list);
            if fittings::check(*c, &next).is_ok() {
                *fit = next;
            }
        }
    } else if let Some(i) = a
        .strip_prefix("isle.yard.unfit:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        if let Some((_, fit)) = card.design.as_mut() {
            let mut list: Vec<Fitting> = fit.iter().collect();
            if i < list.len() {
                list.remove(i);
            }
            *fit = Fit::of(&list);
        }
    } else if a == "isle.yard.usual" {
        if let Some((c, fit)) = card.design.as_mut() {
            *fit = fittings::usual(*c);
        }
    } else {
        return false;
    }
    refresh(world);
    true
}

/// `--hud-design=<craft>[:fitting,fitting]`: open that craft's designer (tour shots).
pub fn demo(world: &mut World, spec: &str) {
    let (c, list) = spec.split_once(':').unwrap_or((spec, ""));
    let Some(c) = Craft::from_name(c) else {
        return;
    };
    let fit = if list.is_empty() {
        fittings::usual(c)
    } else {
        fittings::parse_list(c, list).unwrap_or_else(|_| fittings::usual(c))
    };
    world.resource_mut::<YardCard>().design = Some((c, fit));
    // `--hud-design-save`: save it, and the usual fit as a second design.
    if std::env::args().any(|a| a == "--hud-design-save") {
        let _ = fittings::save(world, &format!("{} 1", c.name()), c, fittings::usual(c));
        let _ = fittings::save(world, &format!("{} 2", c.name()), c, fit);
    }
    refresh(world);
}
