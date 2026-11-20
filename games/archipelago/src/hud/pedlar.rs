//! The pedlar's stall: a window of its own for buying items (Tobias's playtest note 18), in
//! place of a buy list on the shelf. His barque calls at the home quay now and then with five
//! items; the stall lays them out as cards with what each does, its price and Buy, says how
//! long he stays or that he is away, and takes items off the shelf for coins.
//!
//! Data and rules are the game's (crate::items, crate::fleet); this is the UI thread's.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Direction, Enter, Fill, Justify, Pick};

use super::shelf;
use crate::fleet::{Doing, Roam, Vessel};
use crate::items::{self, ItemView, OfferView};
use crate::sim::Economy;

pub const SCREEN: &str = "isle_pedlar";
const BODY_ID: &str = "isle.ped.body";
const STATUS_ID: &str = "isle.ped.status";

/// The stall's state.
#[derive(Resource, Default)]
pub struct Stall {
    pub open: bool,
    /// The shelf item picked out for selling.
    chosen: Option<u32>,
    /// What the body last showed (rebuilt only when it changes).
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<Stall>();
}

/// Whether the pedlar lies at the quay now.
pub fn here(world: &World) -> bool {
    world
        .get_resource::<items::Items>()
        .is_some_and(|i| i.market_open)
}

/// The pedlar's barque: its name and what it is doing.
fn barque(world: &mut World) -> Option<(String, Doing)> {
    world
        .query::<&Vessel>()
        .iter(world)
        .find(|v| matches!(v.roam, Roam::Pedlar { .. }))
        .map(|v| (v.name.clone(), v.doing.clone()))
}

fn clock(s: f32) -> String {
    let s = s.max(0.0).ceil() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The line under the title: moored and how long for, or away.
fn status(world: &mut World) -> (String, bool) {
    let at = here(world);
    let b = barque(world);
    let name = b
        .as_ref()
        .map_or("His barque".to_string(), |(n, _)| n.clone());
    match (at, b.map(|(_, d)| d)) {
        (true, Some(Doing::Moored(s))) => (
            format!("{name} lies off the quay · sails in {}", clock(s)),
            true,
        ),
        (true, _) => (format!("{name} lies off the quay"), true),
        (false, _) => (format!("{name} is away at sea, fetching more wares"), false),
    }
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

/// One of his wares: a card with the item, what it does, its price and Buy.
fn offer_card(o: &OfferView, shelf_full: bool) -> El {
    let iv = &o.item;
    let rank = shelf::ui_rank(iv.rank);
    let rank_color = rank.color();
    let can = o.affordable && !shelf_full;
    let why = if shelf_full {
        "The shelf is full: sell or socket an item first."
    } else if !o.affordable {
        "Not enough coins."
    } else {
        "Buy it onto the shelf."
    };
    brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(272.0)
                .min_height(470.0)
                .padding([0.0, 0.0, 14.0, 0.0])
                .gap(0.0)
        })
        // A ribbon in the item's rank colour, with the rank and sort.
        .child(
            El::new()
                .with_style(move |s| {
                    s.height(30.0)
                        .padding([0.0, 14.0])
                        .align_items(Align::Center)
                        .justify(Justify::Center)
                        .background(Fill::vertical(rank_color, rank_color.fade(0.55)))
                        .radius(engine_ui::style::Corners::top(6.0))
                        .pick(Pick::Ignore)
                })
                .child(
                    El::text(shelf::kind_line(iv))
                        .class("brass_label")
                        .text_style(|t| t.nowrap().bold().size(13.0).shadow())
                        .with_style(|s| s.color(Colors::cream()))
                        .pick_ignore(),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| {
                    s.padding([14.0, 16.0, 0.0, 16.0])
                        .gap(6.0)
                        .align_items(Align::Center)
                })
                .child(shelf::item_socket(iv, 84.0, "", ""))
                .child(
                    El::text(iv.name)
                        .class("ink_title")
                        .text_style(|t| t.size(20.0).center())
                        .pick_ignore(),
                )
                .child(
                    El::text(format!("For a {}", shelf::host_name(iv.host)))
                        .class("ink_label")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.padding([8.0, 16.0, 0.0, 16.0]).gap(4.0).grow(1.0))
                .child(brass::divider(240.0))
                .child(
                    El::text(iv.blurb)
                        .class("ink")
                        .text_style(|t| t.size(13.0).italic())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                )
                .child(dim(format!("On {}", iv.on), 13.0))
                .child_opt(shelf::way_line(iv).map(|w| {
                    El::text(w)
                        .class("ink")
                        .text_style(|t| t.size(13.0).bold())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore()
                }))
                .children(iv.effects.iter().map(|e| ink(format!("· {e}"), 14.0)))
                .child_opt(
                    iv.home
                        .map(|h| dim(format!("Half as strong again in {h}"), 13.0)),
                )
                .child_opt(iv.later.map(|r| {
                    El::text(format!("Starts working in a later update (round {r})"))
                        .class("ink")
                        .text_style(|t| t.size(13.0).italic())
                        .with_style(|s| s.color(Colors::bad_ink()))
                        .pick_ignore()
                })),
        )
        // The price and Buy.
        .child(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.padding([10.0, 16.0, 0.0, 16.0])
                        .gap(8.0)
                        .align_items(Align::Center)
                })
                .child(brass::badge("coin", 30.0))
                .child(
                    El::text(engine_ui::data::group_thousands(o.price.to_string()))
                        .class("ink")
                        .text_style(|t| t.size(21.0).bold().tabular().nowrap())
                        .with_style(move |s| {
                            s.color(if o.affordable {
                                Colors::ink()
                            } else {
                                Colors::bad_ink()
                            })
                        })
                        .pick_ignore(),
                )
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    El::button("Buy", format!("isle.item.buy:{}", o.index))
                        .class("brass_button")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.shrink(0.0).min_width(92.0))
                        .tooltip("", why)
                        .disabled(!can),
                ),
        )
        .child_opt((!can).then(|| {
            El::text(why)
                .class("ink")
                .text_style(|t| t.size(12.5).italic())
                .with_style(|s| s.color(Colors::bad_ink()).padding([4.0, 16.0, 0.0, 16.0]))
                .pick_ignore()
        }))
}

/// A sold card: the space an item left.
fn sold_card() -> El {
    brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(272.0)
                .min_height(470.0)
                .justify(Justify::Center)
                .align_items(Align::Center)
                .gap(8.0)
                .opacity(0.55)
        })
        .child(brass::badge("isle_pedlar", 56.0))
        .child(
            El::text("Sold")
                .class("ink_title")
                .text_style(|t| t.size(22.0))
                .pick_ignore(),
        )
        .child(dim("Gone onto a shelf.", 13.0))
}

/// While he is away: what to expect.
fn away_card() -> El {
    brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(1424.0)
                .min_height(300.0)
                .justify(Justify::Center)
                .align_items(Align::Center)
                .gap(10.0)
        })
        .child(brass::badge("isle_pedlar", 84.0))
        .child(
            El::text("The stall is shut")
                .class("ink_title")
                .text_style(|t| t.size(26.0))
                .pick_ignore(),
        )
        .child(dim(
            "He comes back to the home quay before long with five new items, never a specialist the company already has. A note goes up when he is in.",
            15.0,
        ))
}

/// The shelf under the stall: pick an item to sell it.
fn shelf_row(shelf_items: &[ItemView], chosen: Option<u32>) -> El {
    let mut row = El::new()
        .class("row")
        .with_style(|s| s.gap(6.0).wrap(true).align_items(Align::Center));
    if shelf_items.is_empty() {
        row = row.child(dim(
            "Nothing on the shelf yet: what you buy lands here.",
            14.0,
        ));
    }
    row = row.children(shelf_items.iter().map(|iv| {
        shelf::item_socket(iv, 46.0, &format!("isle.ped.pick:{}", iv.uid), "")
            .selected(chosen == Some(iv.uid))
    }));
    let pick = chosen.and_then(|u| shelf_items.iter().find(|i| i.uid == u));
    El::new()
        .class("col")
        .with_style(|s| s.gap(8.0))
        .child(row)
        .child_opt(pick.map(|iv| {
            El::new()
                .class("row")
                .with_style(|s| s.gap(14.0).align_items(Align::Center))
                .child(
                    El::text(iv.name)
                        .class("ink")
                        .text_style(|t| t.size(16.0).bold().nowrap())
                        .pick_ignore(),
                )
                .child(brass::label(&shelf::kind_line(iv), true))
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    El::button(
                        format!("Sell to him for {} coins", iv.worth),
                        format!("isle.item.sell:{}", iv.uid),
                    )
                    .class("brass_button_dark")
                    .text_style(|t| t.nowrap()),
                )
        }))
}

/// The stall's body: the wares (or the shut stall) and the shelf.
fn body(world: &World) -> Vec<El> {
    let st = world.resource::<Stall>();
    let shelf_items = items::shelf_view(world);
    let offers = items::offers_view(world);
    let shelf_full = shelf_items.len() >= items::SHELF;
    let mut out = Vec::new();
    if here(world) {
        let mut cards: Vec<El> = offers.iter().map(|o| offer_card(o, shelf_full)).collect();
        while cards.len() < 5 {
            cards.push(sold_card());
        }
        out.push(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).align_items(Align::Stretch))
                .children(cards),
        );
    } else {
        out.push(away_card());
    }
    out.push(
        brass::parchment()
            .with_style(|s| {
                s.direction(Direction::Column)
                    .gap(8.0)
                    .padding([12.0, 18.0])
            })
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(10.0).align_items(Align::Center))
                    .child(
                        El::text("Your shelf")
                            .class("ink_title")
                            .text_style(|t| t.size(18.0).nowrap())
                            .pick_ignore(),
                    )
                    .child(brass::label(
                        &format!(
                            "{} of {} · pick one to sell it for two fifths of its worth",
                            shelf_items.len(),
                            items::SHELF
                        ),
                        true,
                    )),
            )
            .child(shelf_row(&shelf_items, st.chosen)),
    );
    out
}

fn sig(world: &World) -> String {
    format!(
        "{}|{}|{}|{:?}|{:.0}",
        here(world),
        serde_json::to_string(&items::offers_view(world)).unwrap_or_default(),
        serde_json::to_string(&items::shelf_view(world)).unwrap_or_default(),
        world.resource::<Stall>().chosen,
        world.resource::<Economy>().coins / 50.0,
    )
}

fn el(world: &mut World) -> El {
    let (line, at) = status(world);
    let book = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(14.0)
                .padding([16.0, 22.0, 20.0, 22.0])
                .max_width(1560.0)
                .enter(Enter::rise(0.3, 30.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).align_items(Align::Center))
                .child(brass::badge("isle_pedlar", 58.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(2.0))
                        .child(
                            El::text("The pedlar's stall")
                                .class("brass_title")
                                .text_style(|t| t.nowrap().shadow().size(30.0))
                                .pick_ignore(),
                        )
                        .child(
                            El::text(status_markup(&line, at))
                                .id(STATUS_ID)
                                .class("brass_label")
                                .text_style(|t| t.nowrap().size(15.0))
                                .pick_ignore(),
                        ),
                )
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    El::new()
                        .class("brass_well")
                        .with_style(|s| s.padding([4.0, 14.0, 4.0, 4.0]).gap(6.0))
                        .tooltip("Coins", "What the company has to spend.")
                        .child(brass::badge("coin", 30.0))
                        .child(
                            El::text("{isle.coins}")
                                .text_style(|t| t.nowrap().tabular().bold().size(18.0))
                                .with_style(|s| s.color(Colors::cream()))
                                .pick_ignore(),
                        ),
                )
                .child(brass::medallion("isle_items", 36.0, "isle.shelf").tooltip(
                    "Items",
                    "The shelf and the sockets [color=#c9b07a](I)[/color]",
                ))
                .child(
                    brass::medallion("isle_close", 36.0, "isle.pedlar")
                        .tooltip("", "Close [color=#c9b07a](M or Esc)[/color]"),
                ),
        )
        .child(
            El::new()
                .id(BODY_ID)
                .class("col")
                .with_style(|s| s.gap(14.0))
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

fn status_markup(line: &str, at: bool) -> String {
    if at {
        format!("[color=#9be08a]●[/color] {line}")
    } else {
        format!("[color=#c9b07a]○[/color] {line}")
    }
}

/// Open the stall, or close it.
pub fn toggle(world: &mut World) {
    if world.resource::<Stall>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        let mut s = world.resource_mut::<Stall>();
        s.open = false;
        s.chosen = None;
        return;
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
        let mut s = world.resource_mut::<Stall>();
        s.open = true;
        s.sig = sig;
    }
}

/// A few times a second: the status line, and the body when what it shows changed.
pub fn update(world: &mut World) {
    if !world.resource::<Stall>().open {
        return;
    }
    let (line, at) = status(world);
    let text = status_markup(&line, at);
    if let Some(e) = engine_ui::input::find_id(world, STATUS_ID)
        && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(e)
        && t.text != text
    {
        t.text = text;
    }
    let sig = sig(world);
    if world.resource::<Stall>().sig == sig {
        return;
    }
    let els = body(world);
    if let Some(e) = engine_ui::input::find_id(world, BODY_ID) {
        let _ = engine_ui::el::replace_children(world, e, &els);
        world.resource_mut::<Stall>().sig = sig;
    }
}

/// `isle.pedlar` and `isle.ped.*`. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a == "isle.pedlar" {
        toggle(world);
    } else if let Some(u) = a
        .strip_prefix("isle.ped.pick:")
        .and_then(|s| s.parse::<u32>().ok())
    {
        let mut st = world.resource_mut::<Stall>();
        st.chosen = if st.chosen == Some(u) { None } else { Some(u) };
        update(world);
    } else {
        return false;
    }
    true
}

/// After an `isle.item.*` action went to the game: a sale clears the pick.
pub fn after_item_action(world: &mut World, a: &str) {
    if a.starts_with("isle.item.sell:") {
        world.resource_mut::<Stall>().chosen = None;
    }
    update(world);
}
