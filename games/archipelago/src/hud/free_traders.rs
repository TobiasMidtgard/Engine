//! The free traders' desk at a harbour: the raw goods the free traders sell and their price,
//! when they next call at each of the company's harbours, standing orders to keep a good
//! topped up, and what they sold of late.
//!
//! Data and rules are the game's (crate::traders); this is the UI thread's.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Direction, Enter, Justify, Pick};

use crate::defs::Good;
use crate::sim::Economy;
use crate::traders::{self, CALL_S, LOT, MARKUP, MOST};

pub const SCREEN: &str = "isle_free_traders";
const BODY_ID: &str = "isle.trd.body";
/// The steps the keep buttons take.
const STEP: f64 = 10.0;
/// One good's row, two to a line.
const ROW_W: f32 = 596.0;

#[derive(Resource, Default)]
pub struct Desk {
    pub open: bool,
    /// The island whose harbour the desk is for.
    island: usize,
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<Desk>();
}

/// The company's harbour islands, with seconds to their next call (from `isle.trader`).
fn calls(world: &mut World) -> Vec<(usize, String, f64)> {
    let Ok(v) = engine::core::registry::call_api(world, "isle.trader", serde_json::json!({}))
    else {
        return Vec::new();
    };
    v["calls"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some((
                        c["island"].as_u64()? as usize,
                        c["name"].as_str()?.to_string(),
                        c["next_call_s"].as_f64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn clock(s: f64) -> String {
    let s = s.max(0.0).ceil() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

fn ink(text: impl Into<String>, size: f32) -> El {
    let text = text.into();
    El::text(text)
        .class("ink")
        .text_style(move |t| t.size(size))
        .pick_ignore()
}

fn dim(text: impl Into<String>, size: f32) -> El {
    ink(text, size).with_style(|s| s.color(Colors::ink_dim()))
}

/// One good's row: picture, name, price, the island's store, and the standing order.
fn row(g: Good, stock: f64, keep: f64, i: usize) -> El {
    let on = keep > 0.0;
    let gi = g.index();
    let step = |d: f64| ((keep + d) / STEP).round() * STEP;
    let cell = |t: String, w: f32| {
        El::text(t)
            .class("ink")
            .text_style(|s| {
                s.size(15.0)
                    .tabular()
                    .nowrap()
                    .align(engine_ui::node::TextAlign::Right)
            })
            .with_style(move |s| s.width(w))
            .pick_ignore()
    };
    El::new()
        .class(if on { "brass_tile" } else { "row" })
        .selected(on)
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(12.0)
                .align_items(Align::Center)
                .padding([3.0, 10.0])
                .width(ROW_W)
        })
        .child(brass::badge(g.icon(), 34.0))
        .child(
            El::text(g.name())
                .class("ink")
                .text_style(|t| t.size(15.5).bold().nowrap())
                .with_style(|s| s.width(130.0))
                .pick_ignore(),
        )
        .child(cell(format!("{:.0}", traders::price(g)), 60.0))
        .child(cell(format!("{stock:.0}"), 60.0))
        .child(El::new().with_style(|s| s.width(16.0)))
        .child(
            El::button(
                "−",
                format!("isle.trd.keep:{i}:{gi}:{}", step(-STEP).max(0.0)),
            )
            .class("brass_button_dark")
            .with_style(|s| s.min_width(34.0))
            .disabled(!on),
        )
        .child(
            El::text(if on {
                format!("keep {keep:.0}")
            } else {
                "no order".to_string()
            })
            .class("ink")
            .text_style(|t| t.size(15.0).bold().tabular().nowrap().center())
            .with_style(move |s| {
                s.width(96.0).color(if on {
                    Colors::good_ink()
                } else {
                    Colors::ink_dim()
                })
            })
            .pick_ignore(),
        )
        .child(
            El::button(
                "+",
                format!("isle.trd.keep:{i}:{gi}:{}", step(STEP).min(MOST)),
            )
            .class("brass_button")
            .with_style(|s| s.min_width(34.0))
            .disabled(keep >= MOST),
        )
        .child_opt(on.then(|| {
            brass::medallion("isle_close", 26.0, &format!("isle.trd.keep:{i}:{gi}:0"))
                .tooltip("", "Cancel the standing order")
        }))
}

fn body(world: &mut World) -> Vec<El> {
    let me = crate::sim::acting(world);
    let list = calls(world);
    let picked = {
        let want = world.resource::<Desk>().island;
        if list.iter().any(|c| c.0 == want) {
            want
        } else {
            list.first().map_or(want, |c| c.0)
        }
    };
    world.resource_mut::<Desk>().island = picked;
    let econ = world.resource::<Economy>();
    let books = econ.islands.get(picked).cloned();
    let orders = traders::orders_of(world, me);
    let sales: Vec<traders::Sale> = world
        .get_resource::<traders::Traders>()
        .map(|t| {
            t.sales
                .iter()
                .filter(|s| s.company == me)
                .rev()
                .take(6)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let now = econ.clock_s;
    let mut out = Vec::new();
    if list.is_empty() {
        out.push(dim(
            "The free traders call only at the company's own harbours: build one first.",
            15.0,
        ));
        return out;
    }
    // The harbours, each with its next call.
    out.push(
        El::new()
            .class("row")
            .with_style(|s| s.gap(8.0).wrap(true))
            .children(list.iter().map(|(i, name, s)| {
                El::button(
                    format!("{name} · {}", clock(*s)),
                    format!("isle.trd.isle:{i}"),
                )
                .class(if *i == picked {
                    "brass_button"
                } else {
                    "brass_button_dark"
                })
                .text_style(|t| t.nowrap())
                .tooltip(
                    name.clone(),
                    format!("Their next call here is in {}.", clock(*s)),
                )
            })),
    );
    let head = |t: &str, w: f32| {
        El::text(t.to_string())
            .class("ink_label")
            .text_style(|s| {
                s.size(11.5)
                    .nowrap()
                    .align(engine_ui::node::TextAlign::Right)
            })
            .with_style(move |s| s.width(w))
            .pick_ignore()
    };
    let header = || {
        El::new()
            .class("row")
            .with_style(|s| {
                s.gap(12.0)
                    .padding([0.0, 10.0])
                    .align_items(Align::Center)
                    .width(ROW_W)
            })
            .child(El::new().with_style(|s| s.width(34.0)))
            .child(
                El::text("good")
                    .class("ink_label")
                    .text_style(|t| t.size(11.5))
                    .with_style(|s| s.width(130.0))
                    .pick_ignore(),
            )
            .child(head("each", 60.0))
            .child(head("in store", 60.0))
            .child(El::new().with_style(|s| s.width(16.0)))
            .child(
                El::text("standing order")
                    .class("ink_label")
                    .text_style(|t| t.size(11.5).nowrap())
                    .pick_ignore(),
            )
    };
    out.push(
        El::new()
            .class("row")
            .with_style(|s| s.gap(6.0))
            .child(header())
            .child(header()),
    );
    let goods: Vec<Good> = Good::ALL
        .into_iter()
        .filter(|g| traders::sells(*g))
        .collect();
    out.push(
        El::new()
            .class("row")
            .with_style(|s| s.gap(6.0).wrap(true).width(ROW_W * 2.0 + 6.0))
            .children(goods.iter().map(|g| {
                let keep = orders
                    .iter()
                    .find(|o| o.island == picked && o.good == *g)
                    .map_or(0.0, |o| o.keep);
                row(*g, books.as_ref().map_or(0.0, |b| b.get(*g)), keep, picked)
            })),
    );
    // Of late.
    out.push(brass::divider(420.0).with_style(|s| s.align_self(Align::Center)));
    out.push(brass::label("Bought of late", true));
    if sales.is_empty() {
        out.push(dim("Nothing yet.", 14.0));
    }
    let names = crate::island::islands();
    for s in &sales {
        out.push(
            El::new()
                .class("row")
                .with_style(|st| st.gap(10.0).align_items(Align::Center))
                .child(brass::badge(s.good.icon(), 24.0))
                .child(ink(
                    format!(
                        "{:.0} {} at {} for {:.0} coins",
                        s.amount,
                        s.good.name().to_lowercase(),
                        names.get(s.island).map_or("?", |i| i.name),
                        s.coins
                    ),
                    14.0,
                ))
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(dim(
                    format!("{:.0} min ago", ((now - s.clock_s) / 60.0).max(0.0)),
                    12.5,
                )),
        );
    }
    out
}

fn el(world: &mut World) -> El {
    let book = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(12.0)
                .padding([16.0, 22.0, 20.0, 22.0])
                .max_width(1290.0)
                .enter(Enter::rise(0.3, 30.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).align_items(Align::Center))
                .child(brass::badge("isle_craft_barque", 58.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(2.0))
                        .child(
                            El::text("The free traders")
                                .class("brass_title")
                                .text_style(|t| t.nowrap().shadow().size(30.0))
                                .pick_ignore(),
                        )
                        .child(
                            El::text(format!(
                                "Barques of no house: they call at each of your harbours every {} minutes and sell raw goods at {MARKUP} times their cost, at most {LOT:.0} of a good a call, paid outright",
                                (CALL_S / 60.0).round()
                            ))
                            .class("brass_label")
                            .text_style(|t| t.size(13.0))
                            .with_style(|s| s.max_width(640.0))
                            .pick_ignore(),
                        ),
                )
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    brass::medallion("isle_close", 36.0, "isle.traders")
                        .tooltip("", "Close [color=#c9b07a](Esc)[/color]"),
                ),
        )
        .child(
            brass::parchment()
                .id(BODY_ID)
                .with_style(|s| s.direction(Direction::Column).gap(8.0).padding([14.0, 18.0]))
                .children(body(world)),
        );
    El::new()
        .with_style(|s| {
            s.fill_parent()
                .justify(Justify::Center)
                .align_items(Align::Center)
                .pick(Pick::Ignore)
        })
        .child(book)
}

fn sig(world: &mut World) -> String {
    let me = crate::sim::acting(world);
    let d = world.resource::<Desk>().island;
    let econ = world.resource::<Economy>();
    let stock: Vec<i64> = econ
        .islands
        .get(d)
        .map(|b| {
            Good::ALL
                .iter()
                .filter(|g| traders::sells(**g))
                .map(|g| b.get(*g) as i64)
                .collect()
        })
        .unwrap_or_default();
    let calls: Vec<i64> = calls(world).iter().map(|c| c.2 as i64).collect();
    let n = world
        .get_resource::<traders::Traders>()
        .map_or(0, |t| t.sales.len());
    format!(
        "{d}|{:?}|{stock:?}|{calls:?}|{n}",
        traders::orders_of(world, me)
    )
}

/// Open the desk (on island `isle`'s harbour, if given), or close it.
pub fn toggle(world: &mut World, isle: Option<usize>) {
    if world.resource::<Desk>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        world.resource_mut::<Desk>().open = false;
        if isle.is_none() {
            return;
        }
    }
    if let Some(i) = isle {
        world.resource_mut::<Desk>().island = i;
    }
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    let el = el(world);
    let sig = sig(world);
    if engine_ui::screens::spawn_screen(world, screen, &el).is_ok() {
        let mut d = world.resource_mut::<Desk>();
        d.open = true;
        d.sig = sig;
    }
}

/// A few times a second: rebuild when what the desk shows changed.
pub fn update(world: &mut World) {
    if !world.resource::<Desk>().open {
        return;
    }
    let sig = sig(world);
    if world.resource::<Desk>().sig == sig {
        return;
    }
    let els = body(world);
    if let Some(e) = engine_ui::input::find_id(world, BODY_ID) {
        let _ = engine_ui::el::replace_children(world, e, &els);
        world.resource_mut::<Desk>().sig = sig;
    }
}

/// `isle.traders`, `isle.trd.*`. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a == "isle.traders" {
        // From a Harbour card: the desk for that island.
        let isle = world
            .resource::<crate::tools::Hand>()
            .selected
            .and_then(|e| world.get::<crate::sim::Building>(e))
            .map(|b| b.island);
        if world.resource::<Desk>().open {
            toggle(world, None);
        } else {
            toggle(world, isle.or(Some(world.resource::<super::Hud>().island)));
        }
    } else if let Some(i) = a
        .strip_prefix("isle.trd.isle:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        world.resource_mut::<Desk>().island = i;
        world.resource_mut::<Desk>().sig.clear();
        update(world);
    } else if let Some(rest) = a.strip_prefix("isle.trd.keep:") {
        let p: Vec<&str> = rest.split(':').collect();
        if let [i, g, keep] = p[..]
            && let (Ok(i), Some(g), Ok(keep)) = (
                i.parse::<usize>(),
                g.parse::<usize>()
                    .ok()
                    .and_then(|k| Good::ALL.get(k).copied()),
                keep.parse::<f64>(),
            )
        {
            let me = crate::sim::acting(world);
            if let Err(e) = traders::order(world, me, i, g, keep) {
                super::complain(world, "Free traders", &e);
            }
            update(world);
        }
    } else {
        return false;
    }
    true
}
