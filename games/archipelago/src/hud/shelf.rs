//! Items in the HUD: the sockets on a Trading Union's, Town Hall's or Harbour Office's card
//! (with what the item in each does right now), and the shelf: the company's spare items, the
//! pedlar's offers while he lies at the quay, and the place to choose an item for a socket.
//!
//! Data and rules are the game's (crate::items); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors, Rank as UiRank};
use engine_ui::style::{Align, Direction, Enter, Justify, Pick, Position};

use crate::items::{self, ItemView, Rank, Sort};
use crate::sim::{Building, Economy};
use crate::tools::Hand;

pub const SCREEN: &str = "isle_shelf";
/// The card's items section, filled by [`refresh_card`].
pub const CARD_ID: &str = "isle.sel.items";
const GRID_ID: &str = "isle.shelf.body";

/// The shelf's state, and what the card's section shows.
#[derive(Resource, Default)]
pub struct Shelf {
    pub open: bool,
    /// Choosing an item for this building's socket (building id, slot).
    picking: Option<(u32, usize)>,
    /// The shelf item picked out for its details.
    chosen: Option<u32>,
    /// The card's socket picked out for its details, and for which building.
    card_slot: Option<(u32, usize)>,
    /// What the card's section and the shelf last showed (rebuilt only when it changes).
    card_sig: String,
    shelf_sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<Shelf>();
}

fn ui_rank(r: Rank) -> UiRank {
    match r {
        Rank::Common => UiRank::Common,
        Rank::Fine => UiRank::Fine,
        Rank::Rare => UiRank::Rare,
        Rank::Storied => UiRank::Storied,
    }
}

fn sort_name(s: Sort) -> &'static str {
    match s {
        Sort::Specialist => "specialist",
        Sort::Device => "device",
        Sort::Charter => "charter",
    }
}

fn host_name(h: items::Host) -> &'static str {
    match h {
        items::Host::Union => "Trading Union",
        items::Host::Hall => "Town Hall",
        items::Host::Office => "Harbour Office",
    }
}

fn stars(n: u32) -> String {
    "★".repeat(n as usize)
}

/// "Rare device · Millers · ★★", in the item's rank colour.
fn kind_line(iv: &ItemView) -> String {
    let mut parts = vec![format!(
        "{} {}",
        ui_rank(iv.rank).name(),
        sort_name(iv.sort)
    )];
    if let Some(g) = iv.guild {
        parts.push(g.to_string());
    }
    if iv.stars > 0 {
        parts.push(stars(iv.stars));
    }
    parts.join(" · ")
}

/// The hover card of an item: what it is, where it goes, what it does and is doing.
fn tip_text(iv: &ItemView) -> String {
    let mut s = format!("[i]{}[/i]\n", iv.blurb);
    s += &format!(
        "[b]{}[/b] for the {}, on {}",
        kind_line(iv),
        host_name(iv.host),
        iv.on
    );
    for e in &iv.effects {
        s += &format!("\n· {e}");
    }
    for n in &iv.now {
        s += &format!("\n[color=#3f7a2c]Now: {n}[/color]");
    }
    if let Some(h) = iv.home {
        s += &format!(
            "\n[i]Half as strong again in {h}{}[/i]",
            if iv.at_home { " (here)" } else { "" }
        );
    }
    s
}

/// An item's socket with its hover card.
fn item_socket(iv: &ItemView, size: f32, action: &str) -> El {
    brass::socket(Some((iv.icon, ui_rank(iv.rank))), size, action).tooltip(iv.name, tip_text(iv))
}

fn ink(text: impl Into<String>, size: f32) -> El {
    let text = text.into();
    El::text(text)
        .class("ink")
        .text_style(move |t| t.size(size))
        .pick_ignore()
}

/// The lines under an item's name: kind, rules, what it does now, its climate.
fn details(iv: &ItemView) -> El {
    let rank = ui_rank(iv.rank).color();
    El::new()
        .class("col")
        .with_style(|s| s.gap(3.0))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Center))
                .child(El::new().with_style(move |s| {
                    s.size(4.0, 22.0)
                        .radius(2.0)
                        .background(engine_ui::style::Fill::Solid(rank))
                        .pick(Pick::Ignore)
                }))
                .child(
                    El::text(iv.name)
                        .class("ink_title")
                        .text_style(|t| t.size(18.0))
                        .pick_ignore(),
                ),
        )
        .child(brass::label(&kind_line(iv), true))
        .children(iv.effects.iter().map(|e| ink(format!("· {e}"), 14.0)))
        .children(iv.now.iter().map(|n| {
            El::text(format!("Now: {n}"))
                .class("ink")
                .text_style(|t| t.size(14.0).bold())
                .with_style(|s| s.color(Colors::good_ink()))
                .pick_ignore()
        }))
        .child_opt(iv.home.map(|h| {
            El::text(format!(
                "Half as strong again in {h}{}",
                if iv.at_home { ", where it is now" } else { "" }
            ))
            .class("ink")
            .text_style(|t| t.size(13.0).italic())
            .with_style(|s| s.color(Colors::ink_dim()))
            .pick_ignore()
        }))
        .child_opt(iv.next_star_s.map(|t| {
            El::text(format!(
                "Next star in {:.0} minutes of work",
                (t / 60.0).ceil()
            ))
            .class("ink")
            .text_style(|t| t.size(13.0).italic())
            .with_style(|s| s.color(Colors::ink_dim()))
            .pick_ignore()
        }))
}

/// The items section of an institution's card: reach, the sockets, the picked socket's item,
/// and complete guild sets.
fn card_section(p: &items::Panel, picked: Option<usize>) -> Vec<El> {
    let colour = Color::hex(p.host.colour());
    let mut out = vec![
        El::new()
            .class("row")
            .with_style(|s| s.gap(8.0).align_items(Align::Center))
            .child(El::new().with_style(move |s| {
                s.size(10.0, 10.0)
                    .radius(5.0)
                    .background(engine_ui::style::Fill::Solid(colour))
                    .border(1.0, Color::hex(0x2a1a0a))
                    .pick(Pick::Ignore)
            }))
            .child(brass::label(
                &format!(
                    "Reach {:.0} m · {} {} in reach",
                    p.reach_m,
                    p.in_reach,
                    if p.in_reach == 1 {
                        "building"
                    } else {
                        "buildings"
                    }
                ),
                true,
            )),
    ];
    let sockets = p.slots.iter().map(|sv| {
        let (socket, caption) = match (&sv.item, sv.open) {
            (Some(iv), _) => (
                item_socket(
                    iv,
                    58.0,
                    &format!("isle.shelf.slot:{}:{}", p.building, sv.slot),
                )
                .selected(picked == Some(sv.slot)),
                if iv.stars > 0 {
                    stars(iv.stars)
                } else {
                    ui_rank(iv.rank).name().to_string()
                },
            ),
            (None, true) => (
                brass::socket(
                    None,
                    58.0,
                    &format!("isle.shelf.pick:{}:{}", p.building, sv.slot),
                )
                .tooltip("Empty socket", "Click to choose an item from the shelf."),
                "Empty".to_string(),
            ),
            (None, false) => (
                brass::socket(None, 58.0, "")
                    .with_style(|s| s.opacity(0.4))
                    .tooltip("Locked", sv.locked.unwrap_or("Not open yet.")),
                "Locked".to_string(),
            ),
        };
        El::new()
            .class("col")
            .with_style(|s| s.gap(4.0).align_items(Align::Center))
            .child(socket)
            .child(
                El::text(caption)
                    .class("ink_label")
                    .text_style(|t| t.nowrap())
                    .pick_ignore(),
            )
    });
    out.push(
        El::new()
            .class("row")
            .with_style(|s| s.gap(14.0).justify(Justify::Center))
            .children(sockets),
    );
    let shown = picked
        .and_then(|i| p.slots.get(i))
        .and_then(|sv| sv.item.as_ref());
    match shown {
        Some(iv) => {
            let slot = picked.unwrap_or(0);
            out.push(details(iv));
            out.push(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(10.0))
                    .child(
                        El::button(
                            "Take out",
                            format!("isle.item.unsocket:{}:{slot}", p.building),
                        )
                        .class("brass_button_dark"),
                    )
                    .child(
                        El::button("Swap", format!("isle.shelf.pick:{}:{slot}", p.building))
                            .class("brass_button"),
                    ),
            );
        }
        None => out.push(
            El::text(if p.slots.iter().any(|s| s.item.is_some()) {
                "Click an item to see what it is doing."
            } else {
                "Click an empty socket to choose an item from the shelf (I)."
            })
            .class("ink")
            .text_style(|t| t.size(14.0).italic())
            .with_style(|s| s.color(Colors::ink_dim()))
            .pick_ignore(),
        ),
    }
    for set in &p.sets {
        out.push(
            El::text(format!("Guild set: {set}"))
                .class("ink")
                .text_style(|t| t.size(14.0).bold())
                .with_style(|s| s.color(Colors::good_ink()))
                .pick_ignore(),
        );
    }
    out
}

/// The selected building and its id, when it is one of the three that hold items.
fn selected_holder(world: &World) -> Option<(Entity, u32)> {
    let e = world.get_resource::<Hand>()?.selected?;
    let b = world.get::<Building>(e)?;
    items::Host::of(b.kind).map(|_| (e, b.id))
}

/// The card's empty items section (for [`refresh_card`] to fill).
pub fn card_placeholder() -> El {
    El::new()
        .class("col")
        .id(CARD_ID)
        .with_style(|s| s.gap(8.0))
}

/// Fill or update the card's items section (only when what it shows changed).
pub fn refresh_card(world: &mut World) {
    let Some((e, id)) = selected_holder(world) else {
        return;
    };
    let Some(panel) = items::panel(world, e) else {
        return;
    };
    let picked = {
        let mut s = world.resource_mut::<Shelf>();
        if s.card_slot.is_some_and(|(b, _)| b != id) {
            s.card_slot = None;
        }
        // An item just put in: pick it out.
        s.card_slot.map(|(_, i)| i)
    };
    let sig = format!(
        "{}|{picked:?}",
        serde_json::to_string(&panel).unwrap_or_default()
    );
    if world.resource::<Shelf>().card_sig == sig {
        return;
    }
    let els = card_section(&panel, picked);
    if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
        let _ = engine_ui::el::replace_children(world, node, &els);
        world.resource_mut::<Shelf>().card_sig = sig;
    }
}

/// A new card was opened: the section must be filled afresh.
pub fn card_opened(world: &mut World) {
    world.resource_mut::<Shelf>().card_sig.clear();
    refresh_card(world);
}

/// The shelf's body: the picking banner, the items, the chosen one's details, the pedlar.
fn body(world: &World) -> Vec<El> {
    let st = world.resource::<Shelf>();
    let picking = st.picking;
    let chosen = st.chosen;
    let shelf = items::shelf_view(world);
    let offers = items::offers_view(world);
    let market = world
        .get_resource::<items::Items>()
        .is_some_and(|i| i.market_open);
    let mut out = Vec::new();
    if let Some((_, s)) = picking {
        out.push(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(Align::Center))
                .child(
                    El::text(format!(
                        "Choose an item for socket {} of the selected building.",
                        s + 1
                    ))
                    .class("ink")
                    .text_style(|t| t.size(14.0).bold())
                    .with_style(|s| s.grow(1.0).shrink(1.0))
                    .pick_ignore(),
                )
                .child(El::button("Cancel", "isle.shelf.cancel").class("brass_button_dark")),
        );
    }
    if shelf.is_empty() {
        out.push(
            El::text(
                "Nothing on the shelf yet. The pedlar calls at the quay now and then, ships pick up flotsam, and specialists arrive when a town is happy.",
            )
            .class("ink")
            .text_style(|t| t.size(14.0).italic())
            .with_style(|s| s.color(Colors::ink_dim()))
            .pick_ignore(),
        );
    } else {
        let target_host = picking.and_then(|(b, _)| holder_host(world, b));
        let cells = shelf.iter().map(|iv| {
            let fits = target_host.is_none_or(|h| h == iv.host);
            let action = match picking {
                Some((b, s)) if fits => format!("isle.item.socket:{b}:{s}:{}", iv.uid),
                _ => format!("isle.shelf.item:{}", iv.uid),
            };
            El::new()
                .class("col")
                .with_style(move |s| {
                    s.gap(3.0)
                        .width(76.0)
                        .align_items(Align::Center)
                        .opacity(if fits { 1.0 } else { 0.4 })
                })
                .child(item_socket(iv, 52.0, &action).selected(chosen == Some(iv.uid)))
                .child(
                    El::text(iv.name)
                        .class("ink")
                        .text_style(|t| t.size(12.0).center())
                        .pick_ignore(),
                )
        });
        out.push(
            El::new()
                .class("row")
                .with_style(|s| s.gap(6.0).wrap(true))
                .children(cells),
        );
        if let Some(iv) = chosen.and_then(|u| shelf.iter().find(|i| i.uid == u)) {
            out.push(brass::divider(300.0));
            out.push(ink(
                format!("For the {}, on {}", host_name(iv.host), iv.on),
                13.0,
            ));
            out.push(details(iv));
            out.push(
                El::new().class("row").child(
                    El::button(
                        format!("Sell for {} coins", iv.worth),
                        format!("isle.item.sell:{}", iv.uid),
                    )
                    .class("brass_button_dark"),
                ),
            );
        }
    }
    if market {
        out.push(brass::divider(300.0));
        out.push(
            El::text("The pedlar is at the quay")
                .class("ink_title")
                .text_style(|t| t.size(18.0))
                .pick_ignore(),
        );
        if offers.is_empty() {
            out.push(ink("He has sold everything he brought.", 14.0));
        }
        for o in &offers {
            out.push(
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(10.0).align_items(Align::Center))
                    .child(item_socket(&o.item, 44.0, ""))
                    .child(
                        El::new()
                            .class("col")
                            .with_style(|s| s.gap(0.0).grow(1.0).shrink(1.0))
                            .child(
                                El::text(o.item.name)
                                    .class("ink")
                                    .text_style(|t| t.bold().size(15.0))
                                    .pick_ignore(),
                            )
                            .child(brass::label(&kind_line(&o.item), true)),
                    )
                    .child(
                        El::button(
                            format!("Buy · {}", o.price),
                            format!("isle.item.buy:{}", o.index),
                        )
                        .class("brass_button")
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.shrink(0.0))
                        .tooltip(
                            "",
                            if o.affordable {
                                "Buy it onto the shelf."
                            } else {
                                "Not enough coins."
                            },
                        )
                        .disabled(!o.affordable),
                    ),
            );
        }
    }
    out
}

/// The host of building `id`, if it is one of the three.
fn holder_host(world: &World, id: u32) -> Option<items::Host> {
    let e = world.get_resource::<Hand>()?.selected?;
    let b = world.get::<Building>(e)?;
    (b.id == id).then(|| items::Host::of(b.kind)).flatten()
}

fn el(world: &World) -> El {
    let econ = world.resource::<Economy>();
    let renown = items::renown(econ);
    let allowed = items::allowed(econ);
    let page = brass::parchment()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(22.0)
                .top(150.0)
                .width(440.0)
                .padding([18.0, 22.0])
                .gap(10.0)
                .direction(Direction::Column)
                .enter(Enter::slide(0.38, -70.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center))
                .child(brass::badge("isle_items", 52.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0))
                        .child(El::text("Items").class("ink_title").pick_ignore())
                        .child(brass::label(
                            &format!("Renown {renown:.0} · up to {allowed} of each building"),
                            true,
                        )),
                )
                .child(
                    brass::medallion("isle_close", 32.0, "isle.shelf")
                        .tooltip("", "Close [color=#6a5339](I)[/color]"),
                ),
        )
        .child(brass::divider(396.0))
        .child(
            El::new()
                .class("scroll")
                .id(GRID_ID)
                .with_style(|s| s.direction(Direction::Column).gap(10.0).max_height(620.0))
                .children(body(world)),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(page)
}

fn shelf_sig(world: &World) -> String {
    let st = world.resource::<Shelf>();
    format!(
        "{}|{}|{:?}|{:?}|{}",
        serde_json::to_string(&items::shelf_view(world)).unwrap_or_default(),
        serde_json::to_string(&items::offers_view(world)).unwrap_or_default(),
        st.picking,
        st.chosen,
        world
            .get_resource::<items::Items>()
            .is_some_and(|i| i.market_open)
    )
}

/// Open or close the shelf.
pub fn toggle(world: &mut World) {
    if world.resource::<Shelf>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        let mut s = world.resource_mut::<Shelf>();
        s.open = false;
        s.picking = None;
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
    let sig = shelf_sig(world);
    if engine_ui::screens::spawn_screen(world, screen, &el).is_ok() {
        let mut s = world.resource_mut::<Shelf>();
        s.open = true;
        s.shelf_sig = sig;
    }
}

/// Rebuild the shelf's body when what it shows changed.
pub fn refresh(world: &mut World) {
    if !world.resource::<Shelf>().open {
        return;
    }
    let sig = shelf_sig(world);
    if world.resource::<Shelf>().shelf_sig == sig {
        return;
    }
    let els = body(world);
    if let Some(e) = engine_ui::input::find_id(world, GRID_ID) {
        let _ = engine_ui::el::replace_children(world, e, &els);
        world.resource_mut::<Shelf>().shelf_sig = sig;
    }
}

/// `isle.shelf*` buttons. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    let nums =
        |rest: &str| -> Vec<u32> { rest.split(':').filter_map(|s| s.parse().ok()).collect() };
    if a == "isle.shelf" {
        toggle(world);
    } else if a == "isle.shelf.cancel" {
        world.resource_mut::<Shelf>().picking = None;
        refresh(world);
    } else if let Some(rest) = a.strip_prefix("isle.shelf.pick:") {
        let n = nums(rest);
        if let [b, s] = n[..] {
            world.resource_mut::<Shelf>().picking = Some((b, s as usize));
            if !world.resource::<Shelf>().open {
                toggle(world);
            } else {
                refresh(world);
            }
        }
    } else if let Some(rest) = a.strip_prefix("isle.shelf.slot:") {
        let n = nums(rest);
        if let [b, s] = n[..] {
            let mut st = world.resource_mut::<Shelf>();
            let pick = Some((b, s as usize));
            st.card_slot = if st.card_slot == pick { None } else { pick };
        }
        refresh_card(world);
    } else if let Some(rest) = a.strip_prefix("isle.shelf.item:") {
        if let Ok(u) = rest.parse::<u32>() {
            let mut st = world.resource_mut::<Shelf>();
            st.chosen = if st.chosen == Some(u) { None } else { Some(u) };
        }
        refresh(world);
    } else {
        return false;
    }
    true
}

/// After an `isle.item.*` action went to the game: show its result.
pub fn after_item_action(world: &mut World, a: &str) {
    if let Some(rest) = a.strip_prefix("isle.item.socket:") {
        let n: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [b, s, _] = n[..] {
            let mut st = world.resource_mut::<Shelf>();
            st.picking = None;
            st.card_slot = Some((b, s as usize));
            st.chosen = None;
        }
    } else if a.starts_with("isle.item.unsocket:") {
        world.resource_mut::<Shelf>().card_slot = None;
    } else if a.starts_with("isle.item.sell:") {
        world.resource_mut::<Shelf>().chosen = None;
    }
    refresh_card(world);
    refresh(world);
}

/// Tour shots: pick out socket `k` of the selected building's card.
pub fn pick_slot(world: &mut World, k: usize) {
    if let Some((_, id)) = selected_holder(world) {
        world.resource_mut::<Shelf>().card_slot = Some((id, k));
        refresh_card(world);
    }
}

/// A few times a second: keep the card's "now" lines and the shelf current.
pub fn update(world: &mut World) {
    refresh_card(world);
    refresh(world);
}
