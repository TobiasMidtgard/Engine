//! The top bar, slim like Anno's: coins and balance, people, then a row of goods slots the
//! player picks (click a slot to choose what it shows, + to add one), the date and the game
//! speed. The slots read the warehouse of the island in view, with an arrow for which way
//! the store is going.

use engine::prelude::*;
use engine_ui::data::UiData;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Direction, Enter, Justify, Pick, Position};

use crate::defs::Good;
use crate::sim::Economy;

const SLOTS_ID: &str = "isle.slots";
const POP_ID: &str = "isle.slot.pop";
/// The most slots the bar holds.
pub const MAX_SLOTS: usize = 10;

/// The goods the bar shows, in order, and the slot being chosen for.
#[derive(Resource)]
pub struct Slots {
    pub goods: Vec<Good>,
    picking: Option<usize>,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            goods: vec![
                Good::Planks,
                Good::Bricks,
                Good::Fish,
                Good::Clothes,
                Good::Bread,
                Good::Ale,
                Good::Glass,
            ],
            picking: None,
        }
    }
}

fn slot_id(i: usize) -> String {
    format!("isle.slot.{i}")
}

/// Coins or people: a badge, a number and a small line under it.
fn stat(icon: &str, bind: &str, label_bind: &str, tip: &str, tip_text: &str) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(8.0).align_items(Align::Center))
        .tooltip(tip, tip_text)
        .child(brass::badge(icon, 36.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text(bind)
                        .class("brass_number")
                        .text_style(|t| t.nowrap().tabular().shadow().size(21.0))
                        .pick_ignore(),
                )
                .child(
                    El::text(label_bind)
                        .class("brass_label")
                        .text_style(|t| t.nowrap().size(11.5))
                        .pick_ignore(),
                ),
        )
}

/// One slot: the good's picture and its stock, click to change.
fn slot(i: usize, g: Good) -> El {
    El::new()
        .class("brass_well")
        .on_click(format!("isle.slot.pick:{i}"))
        .with_style(|s| s.padding([2.0, 8.0, 2.0, 2.0]).gap(4.0))
        .child(brass::badge(g.icon(), 28.0))
        .child(
            El::text(format!("{{{}}}", slot_id(i)))
                .text_style(|t| t.nowrap().tabular().bold().size(15.5))
                .with_style(|s| s.color(Colors::cream()).min_width(30.0))
                .pick_ignore(),
        )
        .tooltip(
            g.name(),
            format!(
                "{} in the warehouse of the island in view; the arrow shows whether it is growing or falling.\n[i]Click to show another good here.[/i]",
                g.name()
            ),
        )
}

fn add_slot(n: usize) -> El {
    brass::medallion("isle_plus", 26.0, &format!("isle.slot.pick:{n}"))
        .tooltip("Add a slot", "Show one more good in the top bar.")
}

fn slot_row(goods: &[Good]) -> Vec<El> {
    let mut v: Vec<El> = goods.iter().enumerate().map(|(i, g)| slot(i, *g)).collect();
    if goods.len() < MAX_SLOTS {
        v.push(add_slot(goods.len()));
    }
    v
}

/// The bar; `speed` holds the speed medallions.
pub fn bar(goods: &[Good], speed: El) -> El {
    let bar = El::new()
        .class("brass_bar")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(14.0)
                .right(14.0)
                .top(8.0)
                .min_height(56.0)
                .padding([5.0, 18.0])
                .gap(18.0)
                .align_items(Align::Center)
        })
        .child(stat(
            "coin",
            "{isle.coins}",
            "{isle.balance}",
            "Coins",
            "Taxes from houses, minus every building's upkeep. The line under it is the balance per minute.",
        ))
        .child(stat(
            "isle_people",
            "{isle.people}",
            "people",
            "People",
            "Every tier, on every island of yours.",
        ))
        .child(
            El::new()
                .with_style(|s| {
                    s.width(2.0)
                        .height(38.0)
                        .background(Colors::cream_dim().fade(0.35))
                        .pick(Pick::Ignore)
                }),
        )
        .child(
            El::new()
                .class("row")
                .id(SLOTS_ID)
                .with_style(|s| s.gap(6.0).align_items(Align::Center))
                .children(slot_row(goods)),
        )
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0).align_items(Align::End))
                .child(
                    El::text("{isle.clock}")
                        .class("brass_title")
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| s.font_size(18.0))
                        .pick_ignore(),
                )
                .child(
                    El::text("{isle.season}")
                        .class("brass_label")
                        .text_style(|t| t.nowrap().size(11.5))
                        .pick_ignore(),
                ),
        )
        .child(speed);
    // The island in view, on a plaque hanging under the bar.
    let cartouche = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(62.0)
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .class("cartouche")
                .tooltip(
                    "Island in view",
                    "The slots in the top bar show this island's warehouse.",
                )
                .with_style(|s| {
                    s.min_width(210.0)
                        .enter(Enter::rise(0.5, -24.0).delay(0.15))
                })
                .child(
                    El::text("{isle.island}")
                        .class("ink_title")
                        .text_style(|t| t.nowrap().size(19.0))
                        .pick_ignore(),
                )
                .child(
                    El::text("{isle.island_people}")
                        .class("ink_label")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                ),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(bar)
        .child(cartouche)
        .child(El::new().id(POP_ID).with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(70.0)
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        }))
}

/// The goods to choose from for slot `i`.
fn picker(i: usize, current: Option<Good>) -> El {
    brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(10.0)
                .padding([12.0, 16.0, 14.0, 16.0])
                .max_width(720.0)
                .enter(Enter::rise(0.2, -12.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center))
                .child(
                    El::text(if current.is_some() {
                        format!("Show in slot {}", i + 1)
                    } else {
                        "Add a slot".to_string()
                    })
                    .class("brass_title")
                    .text_style(|t| t.nowrap().shadow().size(20.0))
                    .pick_ignore(),
                )
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child_opt(current.map(|_| {
                    El::button("Remove slot", format!("isle.slot.clear:{i}"))
                        .class("brass_button_dark")
                }))
                .child(brass::medallion("isle_close", 30.0, "isle.slot.close")),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).wrap(true))
                .children(Good::ALL.iter().map(|g| {
                    El::new()
                        .class("brass_tile")
                        .on_click(format!("isle.slot.set:{i}:{}", g.index()))
                        .selected(current == Some(*g))
                        .with_style(|s| {
                            s.direction(Direction::Column)
                                .align_items(Align::Center)
                                .gap(2.0)
                                .padding([6.0, 4.0])
                                .width(76.0)
                        })
                        .child(brass::badge(g.icon(), 36.0))
                        .child(
                            El::text(g.name())
                                .class("ink")
                                .text_style(|t| t.size(12.0).bold().center().nowrap())
                                .pick_ignore(),
                        )
                })),
        )
}

fn rebuild(world: &mut World) {
    let (goods, picking) = {
        let s = world.resource::<Slots>();
        (s.goods.clone(), s.picking)
    };
    if let Some(e) = engine_ui::input::find_id(world, SLOTS_ID) {
        let _ = engine_ui::el::replace_children(world, e, &slot_row(&goods));
    }
    let pop: Vec<El> = picking
        .map(|i| picker(i, goods.get(i).copied()))
        .into_iter()
        .collect();
    if let Some(e) = engine_ui::input::find_id(world, POP_ID) {
        let _ = engine_ui::el::replace_children(world, e, &pop);
    }
}

/// Fill the slots for island `isle`: the stock, and an arrow when it grows or falls.
pub fn fill(world: &mut World, econ: &Economy, isle: usize) {
    let goods = world.resource::<Slots>().goods.clone();
    let Some(b) = econ.islands.get(isle) else {
        return;
    };
    let rival = econ.owner(isle).is_some_and(|c| c != crate::sim::PLAYER);
    let mut data = world.resource_mut::<UiData>();
    for (i, g) in goods.iter().enumerate() {
        let n = super::ledger::nums(b, *g);
        let arrow = if n.net > 0.05 {
            " [color=#9be08a]▲[/color]"
        } else if n.net < -0.05 {
            " [color=#ff8a70]▼[/color]"
        } else {
            ""
        };
        data.set(
            &slot_id(i),
            if rival {
                "–".to_string()
            } else {
                format!("{:.0}{arrow}", n.stock)
            },
        );
    }
}

/// `isle.slot.*`. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    let Some(rest) = a.strip_prefix("isle.slot.") else {
        return false;
    };
    let mut parts = rest.split(':');
    let verb = parts.next().unwrap_or("");
    let i: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    {
        let mut s = world.resource_mut::<Slots>();
        match verb {
            "pick" => s.picking = if s.picking == Some(i) { None } else { Some(i) },
            "close" => s.picking = None,
            "clear" => {
                if i < s.goods.len() {
                    s.goods.remove(i);
                }
                s.picking = None;
            }
            "set" => {
                if let Some(g) = parts
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|k| Good::ALL.get(k).copied())
                {
                    if i < s.goods.len() {
                        s.goods[i] = g;
                    } else if s.goods.len() < MAX_SLOTS && !s.goods.contains(&g) {
                        s.goods.push(g);
                    }
                }
                s.picking = None;
            }
            _ => return false,
        }
    }
    rebuild(world);
    true
}
