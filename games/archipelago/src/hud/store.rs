//! The harbour card's warehouse as a picture: every good the island keeps, with how full its
//! room is and whether more comes in than goes out; the goods it doesn't keep, faded, below.
//!
//! Numbers are the game's (`IslandBooks`); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Fill, Justify, Pick, pct};

use crate::defs::{Good, Kind};
use crate::sim::{Building, Economy, IslandBooks};
use crate::tools::Hand;

/// The card's warehouse section, filled by [`refresh`].
pub const CARD_ID: &str = "isle.sel.store";

/// What the section last showed (rebuilt only when it changes).
#[derive(Resource, Default)]
pub struct StoreCard {
    sig: String,
    /// Refreshes since the last rebuild: a busy warehouse changes every tick, so it is
    /// redrawn at most every two seconds.
    ticks: u32,
}

pub fn install(world: &mut World) {
    world.init_resource::<StoreCard>();
}

pub fn card_placeholder() -> El {
    El::new()
        .class("col")
        .id(CARD_ID)
        .with_style(|s| s.gap(8.0))
}

/// Goods a minute that count as "steady" either way.
const STEADY: f32 = 0.05;

/// One good's cell: its picture, the count, a bar for the room it fills, and an arrow for
/// whether the stock grows or shrinks.
fn cell(g: Good, b: &IslandBooks, room: f64) -> El {
    let n = g.index();
    let stock = b.stock[n];
    let net = b.made_per_min[n] - b.used_per_min[n];
    let full = (stock / room.max(1.0)) as f32;
    let (arrow, arrow_c) = if net > STEADY {
        ("▲", Colors::good_ink())
    } else if net < -STEADY {
        ("▼", Colors::bad_ink())
    } else {
        ("", Colors::ink_dim())
    };
    let bar = if stock < 1.0 {
        Color::hex(0xb03a24)
    } else if full >= 0.95 {
        Color::hex(0xd99a2a)
    } else {
        Color::hex(0x4f8a3c)
    };
    let tip = format!(
        "{stock:.0} of {room:.0} in the warehouse.\nMade {:.1} a minute, used {:.1} a minute{}.",
        b.made_per_min[n],
        b.used_per_min[n],
        if stock < 1.0 {
            ": none left"
        } else if full >= 0.95 {
            ": the room is full, more is wasted"
        } else {
            ""
        }
    );
    El::new()
        .class("col")
        .with_style(|s| s.gap(2.0).align_items(Align::Center).width(58.0))
        .tooltip(g.name(), tip)
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(2.0).align_items(Align::Center))
                .child(
                    brass::badge(g.icon(), 32.0)
                        .with_style(move |s| s.opacity(if stock < 1.0 { 0.65 } else { 1.0 })),
                )
                .child(
                    El::text(arrow)
                        .class("ink")
                        .text_style(|t| t.size(11.0).nowrap())
                        .with_style(move |s| s.color(arrow_c).width(9.0))
                        .pick_ignore(),
                ),
        )
        .child(
            El::text(format!("{stock:.0}"))
                .class("ink")
                .text_style(|t| t.size(14.0).bold().tabular().nowrap())
                .with_style(move |s| {
                    s.color(if stock < 1.0 {
                        Colors::bad_ink()
                    } else {
                        Colors::ink()
                    })
                })
                .pick_ignore(),
        )
        .child(
            El::new()
                .class("brass_track")
                .with_style(|s| s.width(44.0).height(6.0))
                .child(El::new().with_style(move |s| {
                    s.width(pct(full.clamp(0.04, 1.0) * 100.0))
                        .height(pct(100.0))
                        .radius(3.0)
                        .background(Fill::vertical(bar.lerp(Color::hex(0xffffff), 0.25), bar))
                        .pick(Pick::Ignore)
                })),
        )
}

fn section(b: &IslandBooks, goods: &[Good]) -> Vec<El> {
    let room = b.capacity;
    let empty = goods.iter().filter(|g| b.stock[g.index()] < 1.0).count();
    let full = goods
        .iter()
        .filter(|g| b.stock[g.index()] >= room * 0.95)
        .count();
    let mut note = vec![format!("room for {room:.0} of each")];
    if empty > 0 {
        note.push(format!("{empty} out"));
    }
    if full > 0 {
        note.push(format!("{full} full"));
    }
    let mut out = vec![
        brass::label(&format!("Warehouse · {}", note.join(" · ")), true),
        El::new()
            .class("row")
            .with_style(|s| s.gap(4.0).wrap(true).justify(Justify::Center))
            .children(goods.iter().map(|g| cell(*g, b, room))),
    ];
    let rest: Vec<Good> = Good::ALL
        .into_iter()
        .filter(|g| !goods.contains(g))
        .collect();
    if !rest.is_empty() {
        out.push(
            El::new()
                .class("row")
                .with_style(|s| s.gap(4.0).align_items(Align::Center).wrap(true))
                .child(
                    El::text("Not kept here")
                        .class("ink")
                        .text_style(|t| t.size(13.0).italic().nowrap())
                        .with_style(|s| s.color(Colors::ink_dim()).margin([0.0, 6.0, 0.0, 0.0]))
                        .pick_ignore(),
                )
                .children(rest.into_iter().map(|g| {
                    El::icon(g.icon())
                        .with_style(|s| s.size(20.0, 20.0).opacity(0.4))
                        .tooltip(g.name(), "Nothing on this island makes or uses it yet.")
                })),
        );
    }
    out
}

/// Fill or update the section for the selected harbour.
pub fn refresh(world: &mut World) {
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return;
    };
    let Some(b) = world.get::<Building>(e) else {
        return;
    };
    if b.kind != Kind::Harbour {
        return;
    }
    let isle = b.island;
    let econ = world.resource::<Economy>();
    let Some(books) = econ.islands.get(isle).cloned() else {
        return;
    };
    let goods = super::ledger::island_goods(econ, isle);
    let sig = format!(
        "{e:?}|{:.0}|{}",
        books.capacity,
        goods
            .iter()
            .map(|g| {
                let n = g.index();
                let net = books.made_per_min[n] - books.used_per_min[n];
                let trend = if net > STEADY {
                    1
                } else if net < -STEADY {
                    -1
                } else {
                    0
                };
                format!("{}:{:.0}:{trend}", n, books.stock[n])
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    {
        let mut card = world.resource_mut::<StoreCard>();
        card.ticks += 1;
        if card.sig == sig || (!card.sig.is_empty() && card.ticks < 10) {
            return;
        }
    }
    let els = section(&books, &goods);
    if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
        let _ = engine_ui::el::replace_children(world, node, &els);
        let mut card = world.resource_mut::<StoreCard>();
        card.sig = sig;
        card.ticks = 0;
    }
}

/// A new card was opened: fill the section afresh.
pub fn card_opened(world: &mut World) {
    world.resource_mut::<StoreCard>().sig.clear();
    refresh(world);
}
