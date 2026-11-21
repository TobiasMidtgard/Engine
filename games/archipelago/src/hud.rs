//! The player's screen: the top bar (coins, balance, people, the warehouse of the island in
//! view, the date and the game speed), the build bar by group, the selected building's card
//! and the tooltips, all in the engine UI's brass kit (`engine_ui::kit::brass`): lacquered
//! plates in brass bezels, parchment cards, medallion buttons and a display serif.
//!
//! The look of these is the UI thread's; what the buttons do, and the hand on the map, are in
//! `tools.rs`. Between the two: the `isle.*` actions the buttons send (`tools::action`), the
//! `isle.*` values in `UiData` filled by [`update_data`], and [`show_card`], [`tool_changed`]
//! and [`complain`]. The map table (`hud/map.rs`), the ledger (`hud/ledger.rs`) and the trade
//! routes (`hud/routes.rs`) are parts of it.

use bevy_ecs::message::MessageCursor;
use engine::prelude::*;
use engine_ui::data::UiData;
use engine_ui::el::El;
use engine_ui::input::UiEvent;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::{Interaction, Node, Screen};
use engine_ui::screens;
use engine_ui::style::{Align, Direction, Ease, Enter, Fill, Justify, Pick, Position, pct};

use crate::camera;
use crate::defs::{Category, Good, Kind, Tier};
use crate::island;
use crate::sim::{Building, Economy, House, Producer, population};
use crate::tools::{Hand, Tool};

mod advisor;
mod alerts;
mod chains;
mod fleet;
mod free_traders;
mod house;
mod ledger;
mod map;
mod pedlar;
mod plan;
mod relations;
mod routes;
mod shelf;
mod store;
mod topbar;
mod trouble;
mod works;
mod yard;

/// The bars' own state between frames.
#[derive(Resource, Default)]
pub struct Hud {
    seen: MessageCursor<UiEvent>,
    category: usize,
    refresh: f32,
    /// The island the top bar shows.
    pub island: usize,
    info_open: bool,
    /// `--hud-demo`: after a moment, open a workshop's card and hover a build tile, so a
    /// headless tour's screenshots show the whole HUD at work.
    demo: Option<f32>,
}

const TOP: &str = "isle_top";
const BAR: &str = "isle_bar";
const INFO: &str = "isle_info";
const MAP: &str = "isle_map";

/// Game speeds the medallions set, with their icons.
const SPEEDS: [(f32, &str, &str); 4] = [
    (0.0, "isle_speed_pause", "Pause (Space)"),
    (
        1.0,
        "isle_speed_1",
        "Normal speed (+ and − change the speed)",
    ),
    (
        2.0,
        "isle_speed_2",
        "Double speed (+ and − change the speed)",
    ),
    (
        4.0,
        "isle_speed_4",
        "Four times as fast (+ and − change the speed)",
    ),
];

/// The build bar's tabs in shown order, each with its tiles in shown order for the island
/// in view (tools.rs keys tab `i` to F4 + i, and steps through a tab's tiles in this order).
pub fn bar_tabs(world: &World) -> Vec<(String, Vec<Kind>)> {
    let by_type = world
        .get_resource::<plan::Plan>()
        .is_some_and(|p| p.by_type);
    let isle = world.get_resource::<Hud>().map_or(0, |h| h.island);
    plan::tabs(by_type)
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.to_string(), plan::tab_kinds(by_type, i, isle)))
        .collect()
}

/// Show build tab `i` (clamped), as its key picks from it.
pub fn open_tab(world: &mut World, i: usize) {
    if !world.contains_resource::<Hud>() {
        return;
    }
    let n = plan::tabs(world.resource::<plan::Plan>().by_type).len();
    let i = i.min(n.saturating_sub(1));
    if world.resource::<Hud>().category != i {
        world.resource_mut::<Hud>().category = i;
        refresh_items(world);
    }
}

#[allow(dead_code)] // the ships' selection card folds it (helm.rs)
/// Fold the "Next moves" notebook away (while something else uses the bottom left), or open
/// it again.
pub fn advisor_fold(world: &mut World, folded: bool) {
    if let Some(mut a) = world.get_resource_mut::<advisor::Advisor>() {
        if a.folded == folded {
            return;
        }
        a.folded = folded;
    } else {
        return;
    }
    advisor::refresh(world);
}

pub(super) fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Road => "isle_road",
        Kind::PavedRoad => "isle_paved",
        Kind::School => "isle_school",
        Kind::Tavern => "isle_tavern",
        Kind::Harbour => "isle_anchor",
        Kind::House => "isle_house",
        Kind::Market => "isle_market",
        Kind::Chapel => "isle_chapel",
        Kind::AirshipMast => "isle_airship",
        Kind::Shipyard => "isle_shipyard",
        Kind::Chandler => "isle_chandler",
        Kind::FireWatch => "isle_fire_watch",
        Kind::WatchHouse => "isle_watch_house",
        Kind::Infirmary => "isle_infirmary",
        Kind::Theatre => "isle_theatre",
        Kind::Bank => "isle_bank",
        _ => k.def().output.map_or("isle_workshop", Good::icon),
    }
}

pub(super) fn category_icon(c: Category) -> &'static str {
    match c {
        Category::Roads => "isle_road",
        Category::Homes => "isle_house",
        Category::Production => "isle_workshop",
        Category::Farms => "isle_farm",
        Category::Public => "isle_chapel",
        Category::Harbour => "isle_anchor",
    }
}

/// The game speed medallions.
fn speed_row() -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(6.0))
        .children(SPEEDS.iter().map(|(v, icon, tip)| {
            brass::medallion(icon, 36.0, &format!("isle.speed:{v}"))
                .id(format!("isle.speed:{v}"))
                .tooltip("", *tip)
        }))
}

/// A build tile: the building's badge, its name and a price line. `compact` for a category
/// with many buildings, so the bar still fits beside the map table.
fn tile(
    icon: &str,
    name: &str,
    price: El,
    action: String,
    selected: bool,
    i: usize,
    compact: bool,
) -> El {
    let (w, badge, text) = if compact {
        (84.0, 44.0, 12.5)
    } else {
        (98.0, 52.0, 14.0)
    };
    El::new()
        .class("brass_tile")
        .on_click(action)
        .selected(selected)
        .with_style(|s| {
            s.direction(Direction::Column)
                .align_items(Align::Center)
                .gap(2.0)
                .padding([10.0, 6.0, 8.0, 6.0])
                .width(w)
                .enter(
                    Enter::rise(0.32, 22.0)
                        .delay(0.035 * i as f32)
                        .ease(Ease::Back),
                )
        })
        .child(brass::badge(icon, badge))
        .child(
            El::text(name)
                .class("ink")
                .text_style(move |t| t.bold().size(text).center().nowrap())
                .pick_ignore(),
        )
        .child(price)
}

/// `grows`: false for a farm whose crop does not grow on the island in view (shown faded at
/// the end of the row, with the reason in its tooltip).
fn item_tile(k: Kind, selected: bool, i: usize, compact: bool, grows: Option<&str>) -> El {
    let d = k.def();
    let price = plan::price(k);
    let action = format!(
        "isle.build:{}",
        serde_json::to_value(k)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default()
    );
    // (Costs, upkeep, workers and the chain are on the plan card over the bar.)
    tile(kind_icon(k), d.name, price, action, selected, i, compact)
        .id(format!("isle.build:{k:?}"))
        .child(plan::padlock(plan::lock_id(k), 36.0, 2.0, 2.0))
        .child(plan::tile_keycap(k))
        .with_style(move |s| s.opacity(if grows.is_some() { 0.48 } else { 1.0 }))
}

/// The category's build tiles for island `isle` (the one in view): farms whose crop does not
/// grow there go last, faded.
fn bar_items(category: usize, tool: Tool, isle: usize, by_type: bool) -> Vec<El> {
    let here = island::islands().get(isle);
    let grows = |k: Kind| plan::grows(k, isle);
    let all = plan::tab_kinds(by_type, category, isle);
    // Farms whose crop does not grow here go on a slim row of their own at the end.
    let (kinds, elsewhere): (Vec<Kind>, Vec<Kind>) = all.into_iter().partition(|k| grows(*k));
    let compact = kinds.len() > 8;
    let mut v: Vec<El> = kinds
        .into_iter()
        .enumerate()
        .map(|(i, k)| {
            let sel = match tool {
                Tool::Build { kind, .. } => kind == k,
                Tool::Road => k == Kind::Road,
                Tool::Paving => k == Kind::PavedRoad,
                _ => false,
            };
            let not_here = (!grows(k)).then(|| here.map_or("", |i| i.name));
            item_tile(k, sel, i, compact, not_here)
        })
        .collect();
    if !elsewhere.is_empty() {
        let name = here.map_or("", |i| i.name);
        v.push(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.gap(6.0)
                        .align_items(Align::Center)
                        .justify(Justify::Center)
                        .width(pct(100.0))
                })
                .child(
                    El::text(format!("Grows elsewhere, not on {name}:"))
                        .class("brass_label")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                )
                .children(elsewhere.into_iter().map(|k| {
                    El::new()
                        .class("brass_tile")
                        .id(format!("isle.build:{k:?}"))
                        .on_click(house::build_action(k))
                        .with_style(|s| s.padding(3.0).opacity(0.75))
                        .child(brass::badge(kind_icon(k), 34.0))
                })),
        );
    }
    v
}

/// How many full tiles tab `category` shows on island `isle`.
fn tab_len(category: usize, by_type: bool, isle: usize) -> usize {
    plan::tab_kinds(by_type, category, isle)
        .into_iter()
        .filter(|k| plan::grows(*k, isle))
        .count()
}

/// A key cap in the bar's corner of a medallion or tile: "F4", "X", "⇧N".
fn keycap(text: &str, top: Option<f32>, bottom: Option<f32>) -> El {
    El::text(text.to_string())
        .text_style(|t| t.nowrap().bold().size(11.5).center())
        .with_style(move |s| {
            let s = s
                .position(Position::Absolute)
                .right(-8.0)
                .min_width(22.0)
                .padding([1.0, 5.0])
                .radius(4.0)
                .background(Fill::vertical(
                    Colors::lacquer_top().lerp(Colors::brass_dark(), 0.25),
                    Colors::lacquer_bottom(),
                ))
                .border(1.0, Colors::brass().with_alpha(0.8))
                .color(Colors::cream())
                .pick(Pick::Ignore);
            match (top, bottom) {
                (Some(t), _) => s.top(t),
                (_, Some(b)) => s.bottom(b),
                _ => s,
            }
        })
        .pick_ignore()
}

/// One of the planning medallions beside the tabs: blueprints and stamps (tools.rs).
fn plan_medal(icon: &str, action: &str, key: &str, title: &str, tip: &str) -> El {
    brass::medallion(icon, 40.0, action)
        .id(action.to_string())
        .tooltip(title, format!("{tip}\n[b]Key[/b]  {key}"))
        .child(keycap(key, None, Some(-8.0)))
}

/// The tab medallions, the sort switch, Demolish, and the blueprint and stamp tools.
fn tab_row(category: usize, by_type: bool, demolish: bool) -> Vec<El> {
    plan::tabs(by_type)
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let key = crate::tools::tab_key(i);
            brass::medallion(t.icon, 52.0, &format!("isle.cat:{i}"))
                .id(format!("isle.cat:{i}"))
                .selected(i == category)
                .tooltip(
                    t.name,
                    format!("[b]Key[/b]  {key}, again to step through its buildings"),
                )
                .child(plan::padlock(plan::tab_lock_id(i), 26.0, -6.0, -6.0))
                .child(keycap(&key, None, Some(-8.0)))
        })
        .chain([
            El::new().with_style(|s| s.width(10.0)),
            plan::sort_switch(by_type),
            brass::medallion("isle_demolish", 40.0, "isle.tool:demolish")
                .id("isle.tool:demolish")
                .selected(demolish)
                .tooltip(
                    "Demolish",
                    "Click a building or a road to clear it. Nothing is refunded.\n[b]Key[/b]  X or Delete",
                )
                .child(keycap("X", None, Some(-8.0))),
            El::new().with_style(|s| {
                s.width(2.0)
                    .height(40.0)
                    .background(Colors::cream_dim().fade(0.35))
                    .pick(Pick::Ignore)
            }),
            plan_medal(
                "isle_blueprint",
                "isle.blueprint",
                "N",
                "Blueprints",
                "Lay plans instead of building: they cost nothing until built, and keep their ground. Click again to build for real.",
            ),
            plan_medal(
                "isle_build_all",
                "isle.blueprint.build_all",
                "⇧N",
                "Build the plans",
                "Build every blueprint the company can pay for, oldest first.",
            ),
            plan_medal(
                "isle_stamp_copy",
                "isle.stamp.copy",
                "O",
                "Copy a stamp",
                "Drag round buildings and roads to keep them in the stamp book; C copies the building under the pointer.",
            ),
            plan_medal(
                "isle_stamp",
                "isle.stamp.last",
                "⇧O",
                "Stamps",
                "Take up the last stamp from the book; [ and ] turn its pages, R turns it.",
            ),
        ])
        .collect()
}

/// The tiles row's width: one row up to ten tiles, then two even rows (so the bar stays
/// clear of the map table).
fn items_width(n: usize) -> f32 {
    if n > 10 {
        n.div_ceil(2) as f32 * 94.0
    } else {
        2000.0
    }
}

fn build_bar(category: usize, tool: Tool, isle: usize, by_type: bool) -> El {
    let tabs = El::new()
        .class("row")
        .id("isle.tabs")
        .with_style(|s| s.gap(14.0).align_items(Align::Center))
        .children(tab_row(category, by_type, tool == Tool::Demolish));
    let header = El::new()
        .class("row")
        .with_style(|s| s.gap(14.0).justify(Justify::Center))
        .child(brass::divider(120.0))
        .child(
            El::text("{isle.cat_name}")
                .class("brass_title")
                .text_style(|t| t.nowrap().shadow())
                .pick_ignore(),
        )
        .child(brass::divider(120.0));
    let panel = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(12.0)
                .padding([16.0, 26.0, 20.0, 26.0])
                .align_items(Align::Center)
                .min_width(560.0)
        })
        .child(header)
        .child(tabs)
        .child(
            El::new()
                .class("row")
                .id("isle.items")
                .with_style(move |s| {
                    s.gap(10.0)
                        .margin([4.0, 0.0, 0.0, 0.0])
                        .wrap(true)
                        .justify(Justify::Center)
                        .max_width(items_width(tab_len(category, by_type, isle)))
                })
                .children(bar_items(category, tool, isle, by_type)),
        );
    let hint = El::text("{isle.hint}")
        .text_style(|t| t.bold().shadow().nowrap().size(17.0))
        .with_style(|s| s.color(Colors::cream()))
        .pick_ignore();
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(0.0)
                        .right(0.0)
                        .bottom(22.0)
                        .direction(Direction::Column)
                        .align_items(Align::Center)
                        .gap(14.0)
                        .pick(Pick::Ignore)
                })
                .child(
                    El::new()
                        .id(plan::PLAN_ID)
                        .with_style(|s| s.pick(Pick::Ignore)),
                )
                .child(hint)
                .child(panel),
        )
}

/// A row of the building card: a small-caps name and its value.
fn card_row(i: usize) -> El {
    El::new()
        .class("row")
        .id(format!("isle.sel.row{i}"))
        .with_style(|s| s.gap(12.0).align_items(Align::Start))
        .child(
            El::text(format!("{{isle.sel.k{i}}}"))
                .class("ink_label")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.width(92.0).margin([3.0, 0.0, 0.0, 0.0]))
                .pick_ignore(),
        )
        .child(
            El::text(format!("{{isle.sel.v{i}}}"))
                .class("ink")
                .with_style(|s| s.grow(1.0).shrink(1.0).basis(0.0))
                .pick_ignore(),
        )
}

fn info_card(kind: Kind, ruin: bool, held: Option<(String, u32)>) -> El {
    let d = kind.def();
    // The card never runs under the map table or off the screen: its sections scroll, and
    // the header and the buttons stay put.
    let card = brass::parchment()
        .with_style(|s| {
            s.width(360.0)
                .max_height(engine_ui::style::pct(100.0))
                .padding([18.0, 22.0])
                .gap(8.0)
                .direction(Direction::Column)
                .enter(Enter::slide(0.38, 70.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center))
                .child(brass::badge(kind_icon(kind), 62.0).tooltip(d.name, d.blurb))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0).shrink(1.0))
                        .child(
                            El::text("{isle.sel.title}")
                                .class("ink_title")
                                .pick_ignore(),
                        )
                        .child(
                            El::text("{isle.sel.sub}")
                                .class("ink_label")
                                .text_style(|t| t.nowrap())
                                .pick_ignore(),
                        ),
                )
                .child(
                    brass::medallion("isle_close", 32.0, "isle.deselect")
                        .tooltip("", "Close [color=#6a5339](Esc)[/color]"),
                ),
        )
        .child(brass::divider(316.0))
        .child(card_body(kind))
        .child_opt(
            held.clone()
                .map(|(name, colour)| held_ribbon(&name, colour)),
        )
        .child_opt(held.is_none().then(|| {
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).wrap(true).margin([8.0, 0.0, 0.0, 0.0]))
                // A ruin makes nothing and pays nothing: no Pause, and Demolish clears it.
                .child_opt(d.output.filter(|_| !ruin).map(|_| {
                    El::button("{isle.sel.pause}", "isle.sel.pause").class("brass_button")
                }))
                .child_opt((kind == Kind::Harbour && !ruin).then(|| {
                    El::button("Free traders", "isle.traders")
                        .class("brass_button")
                        .tooltip(
                            "The free traders",
                            "Standing orders for raw goods, bought dearly at each of their calls here.",
                        )
                }))
                .child_opt((kind == Kind::Harbour && !ruin).then(|| {
                    El::button("Pedlar's stall", "isle.pedlar")
                        .class("brass_button")
                        .tooltip(
                            "The pedlar",
                            "His barque calls at the home quay now and then with five items for sale.\n[b]Key[/b]  M",
                        )
                }))
                .child(
                    El::button(
                        if ruin { "Clear the ruin" } else { "Demolish" },
                        "isle.sel.demolish",
                    )
                    .class("brass_button_dark"),
                )
        }));
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .right(22.0)
                        .top(140.0)
                        .bottom(map::ROOM_H)
                        .direction(Direction::Column)
                        .pick(Pick::Ignore)
                })
                .child(card),
        )
}

/// The card's sections between the header and the buttons, in a scroll that gives way first.
fn card_body(kind: Kind) -> El {
    let d = kind.def();
    El::new()
        .class("scroll")
        .with_style(|s| s.gap(8.0).shrink(1.0).padding([0.0, 4.0, 0.0, 0.0]))
        .child(trouble::card_placeholder())
        .child_opt(d.output.map(|_| works::card_placeholder()))
        .child_opt((kind == Kind::Harbour).then(store::card_placeholder))
        .child_opt(yard::builds_craft(kind).then(yard::card_placeholder))
        .child_opt((kind == Kind::House).then(house::card_placeholder))
        .children((0..6).map(card_row))
        .child_opt(crate::items::Host::of(kind).map(|_| shelf::card_placeholder()))
}

/// A rival house's building: whose it is, in place of the buttons.
fn held_ribbon(name: &str, colour: u32) -> El {
    let c = engine_ui::color::Color::hex(colour);
    El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(10.0)
                .align_items(Align::Center)
                .margin([8.0, 0.0, 0.0, 0.0])
                .padding([7.0, 12.0])
                .radius(8.0)
                .background(c.with_alpha(0.16))
                .border(1.5, c.with_alpha(0.8))
        })
        .child(
            El::new()
                .with_style(move |s| s.width(14.0).height(14.0).radius(7.0).background(c))
                .pick_ignore(),
        )
        .child(
            El::text(format!(
                "{name}'s. You can look, but only they give orders here."
            ))
            .class("ink")
            .text_style(|t| t.size(13.5))
            .with_style(|s| s.grow(1.0).shrink(1.0))
            .pick_ignore(),
        )
        .tooltip(
            name,
            "A rival house holds this island. Trade with them, or change how they see you, in Relations (P).",
        )
}

/// The rival house that holds building `e`, with its colour, when it isn't the player's.
fn held_by(world: &World, e: Entity) -> Option<(String, u32)> {
    if crate::build::ours(world, e) {
        return None;
    }
    let b = world.get::<Building>(e)?;
    let c = world.resource::<crate::sim::Economy>().owner(b.island)?;
    let colour = world
        .get_resource::<crate::rivals::Rivals>()
        .and_then(|r| r.0.iter().find(|x| x.id == c as u32))
        .map_or(0x8a8a8a, |r| r.colour);
    Some((crate::rivals::company_name(world, c), colour))
}

/// Orders to the selected building that only its owner may give.
fn owner_only(a: &str) -> bool {
    matches!(
        a,
        "isle.sel.pause"
            | "isle.sel.demolish"
            | "isle.house.up"
            | "isle.house.down"
            | "isle.work.first"
            | "isle.ruin.rebuild"
    ) || a.starts_with("isle.yard.order:")
        || a.starts_with("isle.yard.cancel")
        || a.starts_with("isle.sock.")
        || a.starts_with("isle.shelf.slot:")
}

/// The batch in progress, as a brass bar (its fill is sized in [`update_data_now`]).
fn hud_screen(key: &str, layer: i32) -> Screen {
    Screen {
        key: key.into(),
        modal: false,
        layer,
        closable: false,
        ..Default::default()
    }
}

/// Add the icons and the brass kit, open the bars.
pub fn setup(world: &mut World) {
    if let Some(mut lib) = world.get_resource_mut::<engine_ui::icons::IconLibrary>() {
        crate::icons::install(&mut lib);
    }
    if let Some(mut theme) = world.get_resource_mut::<engine_ui::Theme>() {
        brass::install(&mut theme);
    }
    world.init_resource::<Hud>();
    world.init_resource::<plan::Plan>();
    world.init_resource::<topbar::Slots>();
    world.init_resource::<chains::Chains>();
    world.resource_mut::<Hud>().category = 0;
    if std::env::args().any(|a| a == "--hud-demo") {
        world.resource_mut::<Hud>().demo = Some(0.0);
    }
    // `--hud-cat=k`: open the build bar on category k (tour shots).
    if let Some(k) = std::env::args()
        .find_map(|a| a.strip_prefix("--hud-cat=")?.parse::<usize>().ok())
        .filter(|k| *k < plan::tabs(world.resource::<plan::Plan>().by_type).len())
    {
        world.resource_mut::<Hud>().category = k;
    }
    update_data_now(world);
    if let Err(e) = screens::spawn_screen(
        world,
        hud_screen(TOP, 0),
        &topbar::bar(
            &world.resource::<topbar::Slots>().goods.clone(),
            speed_row(),
        ),
    ) {
        engine::core::log::warn!("archipelago top bar: {e}");
    }
    let tool = *world.resource::<Tool>();
    if let Err(e) = screens::spawn_screen(
        world,
        hud_screen(BAR, 0),
        &build_bar(
            world.resource::<Hud>().category,
            tool,
            world.resource::<Hud>().island,
            world.resource::<plan::Plan>().by_type,
        ),
    ) {
        engine::core::log::warn!("archipelago build bar: {e}");
    }
    refresh_items(world);
    map::install(world);
    if let Err(e) = screens::spawn_screen(world, hud_screen(MAP, 0), &map::panel()) {
        engine::core::log::warn!("archipelago map table: {e}");
    }
    // `--hud-map=k`: turn the map table to region k (tour shots of the far charts).
    if let Some(k) =
        std::env::args().find_map(|a| a.strip_prefix("--hud-map=")?.parse::<i32>().ok())
    {
        map::turn(world, k);
    }
    ledger::install(world);
    routes::install(world);
    house::install(world);
    fleet::install(world);
    works::install(world);
    store::install(world);
    yard::install(world);
    alerts::install(world);
    advisor::install(world);
    relations::install(world);
    trouble::install(world);
    shelf::install(world);
    pedlar::install(world);
    free_traders::install(world);
    // `--hud-shelf`: open the item shelf (tour shots).
    if std::env::args().any(|a| a == "--hud-shelf") {
        shelf::toggle(world);
    }
    // `--hud-locks`: judge the build bar's locks by the people of a new game's first years
    // (63 Fisherfolk, no Labourers or Burghers) instead of the finished town's (tour shots).
    if std::env::args().any(|a| a == "--hud-locks") {
        let mut p = [0.0; Tier::COUNT];
        p[Tier::Fisherfolk.index()] = 63.0;
        world.resource_mut::<plan::Plan>().demo_people = Some(p);
    }
    // `--hud-traders`: open the free traders' desk on the home harbour, with a standing order
    // for wool left there first (tour shots).
    if std::env::args().any(|a| a == "--hud-traders") {
        let me = crate::sim::acting(world);
        let _ = crate::traders::order(world, me, 0, Good::Wool, 60.0);
        let _ = crate::traders::order(world, me, 0, Good::Clay, 40.0);
        free_traders::toggle(world, Some(0));
    }
    // `--hud-pedlar`: open the pedlar's stall (tour shots).
    if std::env::args().any(|a| a == "--hud-pedlar") {
        pedlar::toggle(world);
    }
    // `--hud-routes`: open the trade routes with the second route picked out (tour shots).
    if std::env::args().any(|a| a == "--hud-routes") {
        routes::toggle(world);
        routes::select(world, 1);
    }
    // `--hud-ledger`: run the economy ahead a few game minutes, keeping the books, then open
    // the ledger (tour screenshots of it with real history).
    if std::env::args().any(|a| a == "--hud-ledger") {
        for _ in 0..60 {
            crate::town::prewarm(world, 10.0);
            let econ = world.resource::<Economy>().clone();
            world.resource_mut::<ledger::Ledger>().record(&econ);
        }
        let isle = world.resource::<Hud>().island;
        ledger::show(world, Some(isle));
        // `--hud-ledger-filter=short`, `--hud-ledger-sort=net`: press those (tour shots).
        if let Some(f) =
            std::env::args().find_map(|a| a.strip_prefix("--hud-ledger-filter=").map(String::from))
        {
            ledger::action(world, &format!("isle.led.filter:{f}"));
        }
        if let Some(c) =
            std::env::args().find_map(|a| a.strip_prefix("--hud-ledger-sort=").map(String::from))
        {
            ledger::action(world, &format!("isle.led.sort:{c}"));
        }
        ledger::update(world);
    }
    // `--hud-relations`: open the relations book (tour shots).
    if std::env::args().any(|a| a == "--hud-relations") {
        // `--hud-leaders`: the rival houses led by the game's first three characters.
        if std::env::args().any(|a| a == "--hud-leaders") {
            for c in 1..=3 {
                crate::characters::install(world, c, c as usize - 1);
            }
        }
        relations::toggle(world);
    }
    // `--hud-chains=Coats`: open the production chains book on that good (tour shots).
    if let Some(g) = std::env::args()
        .find_map(|a| a.strip_prefix("--hud-chains=").map(String::from))
        .and_then(|n| Good::ALL.into_iter().find(|g| format!("{g:?}") == n))
    {
        chains::toggle(world, Some(g));
    }
    // `--hud-trade-lines`: show the trade lines (tour shots).
    if std::env::args().any(|a| a == "--hud-trade-lines") {
        map::toggle_trade_lines(world);
    }
}

/// The tool changed: show it selected in the build bar.
pub fn tool_changed(world: &mut World) {
    refresh_items(world);
}

/// Open the card for the selected building (tools.rs [`Hand`]), or close it.
pub fn show_card(world: &mut World) {
    if world.resource::<Hud>().info_open {
        screens::hide(world, INFO, true);
        world.resource_mut::<Hud>().info_open = false;
    }
    let Some(kind) = world
        .resource::<Hand>()
        .selected
        .and_then(|e| world.get::<Building>(e))
        .map(|b| b.kind)
    else {
        return;
    };
    update_data_now(world);
    let ruin = world
        .resource::<Hand>()
        .selected
        .is_some_and(|e| world.get::<crate::incidents::Ruin>(e).is_some());
    let held = world
        .resource::<Hand>()
        .selected
        .and_then(|e| held_by(world, e));
    if screens::spawn_screen(world, hud_screen(INFO, 1), &info_card(kind, ruin, held)).is_ok() {
        world.resource_mut::<Hud>().info_open = true;
        update_data_now(world);
        shelf::card_opened(world);
        house::card_opened(world);
        works::card_opened(world);
        store::card_opened(world);
        yard::card_opened(world);
        trouble::card_opened(world);
    }
}

/// Tell the player why something can't be done.
pub fn complain(world: &mut World, title: &str, text: &str) {
    engine_ui::toast::notify(
        world,
        engine_ui::toast::ToastKind::Bad,
        "",
        title,
        text,
        3.0,
    );
}

/// The bars' buttons: the build bar's groups here, the rest to the hand (tools.rs).
pub fn read_actions(world: &mut World) {
    if !world.contains_resource::<Hud>() {
        return;
    }
    let mut seen = std::mem::take(&mut world.resource_mut::<Hud>().seen);
    let events = engine_ui::input::read_events(world, &mut seen);
    world.resource_mut::<Hud>().seen = seen;
    for ev in events {
        let UiEvent::Action {
            action: a, entity, ..
        } = ev
        else {
            continue;
        };
        if let Some(i) = a
            .strip_prefix("isle.cat:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            let n = plan::tabs(world.resource::<plan::Plan>().by_type).len();
            world.resource_mut::<Hud>().category = i.min(n - 1);
            refresh_items(world);
        } else if a == "isle.bar.sort" {
            let tab = plan::toggle_sort(world);
            world.resource_mut::<Hud>().category = tab;
            refresh_tabs(world);
            refresh_items(world);
        } else if let Some(i) = a
            .strip_prefix("isle.led:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            ledger::show(world, Some(i.min(island::islands().len() - 1)));
            ledger::update(world);
        } else if a == "isle.ledger" {
            toggle_ledger(world);
        } else if a == "isle.routes" {
            toggle_routes(world);
        } else if let Some(i) = a
            .strip_prefix("isle.route:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            routes::select(world, i);
        } else if a == "isle.map" {
            map::clicked(world, entity);
        } else if a == "isle.map.next" || a == "isle.map.prev" {
            map::turn(world, if a.ends_with("next") { 1 } else { -1 });
            update_data_now(world);
        } else if a == "isle.home" {
            map::home(world);
        } else if a == "isle.route.chart" {
            routes::chart_clicked(world);
        } else if a.starts_with("isle.route.") {
            let opened = a.starts_with("isle.route.open");
            crate::tools::action(world, &a);
            if opened {
                routes::pick_last(world);
            } else {
                routes::refresh(world);
            }
        } else if owner_only(&a)
            && let Some((name, _)) = world
                .resource::<Hand>()
                .selected
                .and_then(|e| held_by(world, e))
        {
            complain(
                world,
                &format!("{name}'s"),
                "Only its owner gives orders here.",
            );
        } else if a.starts_with("isle.build:") && world.resource::<chains::Chains>().open {
            // A link of the chains book: put it in hand and clear the view for placing it.
            chains::built_from_book(world);
            crate::tools::action(world, &a);
        } else if topbar::action(world, &a)
            || chains::action(world, &a)
            || shelf::action(world, &a)
            || pedlar::action(world, &a)
            || free_traders::action(world, &a)
            || fleet::action(world, &a)
            || alerts::action(world, &a)
            || advisor::action(world, &a)
            || relations::action(world, &a)
            || map::action(world, &a)
            || house::action(world, &a)
            || ledger::action(world, &a)
            || works::action(world, &a)
            || yard::action(world, &a)
        {
        } else if a.starts_with("isle.gear.") {
            crate::tools::action(world, &a);
            fleet::after_gear_action(world, &a);
        } else if a.starts_with("isle.item.") {
            crate::tools::action(world, &a);
            shelf::after_item_action(world, &a);
            pedlar::after_item_action(world, &a);
        } else {
            crate::tools::action(world, &a);
        }
        update_data_now(world);
    }
}

/// Open the trade routes (closing the ledger), or close them.
fn toggle_routes(world: &mut World) {
    if world.resource::<ledger::Ledger>().open {
        ledger::show(world, None);
    }
    routes::toggle(world);
}

/// Open the ledger on the island in view (closing the routes), or close it.
fn toggle_ledger(world: &mut World) {
    if world.resource::<routes::RouteBook>().open {
        routes::toggle(world);
    }
    if world.resource::<ledger::Ledger>().open {
        ledger::show(world, None);
    } else {
        let isle = world.resource::<Hud>().island;
        ledger::show(world, Some(isle));
        ledger::update(world);
    }
}

/// The HUD's own keys: L the ledger, T the trade routes, H home, M the pedlar's stall, Esc
/// closes the book that is open.
fn read_keys(world: &mut World) {
    let typing = world
        .get_resource::<engine_ui::UiInputCapture>()
        .is_some_and(|c| c.keyboard);
    if typing {
        return;
    }
    let Some(keys) = world.get_resource::<ButtonInput<KeyCode>>() else {
        return;
    };
    let (l, h, t, i, g, esc) = (
        keys.just_pressed(KeyCode::L),
        keys.just_pressed(KeyCode::H),
        keys.just_pressed(KeyCode::T),
        keys.just_pressed(KeyCode::I),
        keys.just_pressed(KeyCode::G),
        keys.just_pressed(KeyCode::Escape),
    );
    let (p, v, k, m) = (
        keys.just_pressed(KeyCode::P),
        keys.just_pressed(KeyCode::V),
        keys.just_pressed(KeyCode::K),
        keys.just_pressed(KeyCode::M),
    );
    if m || (esc && world.resource::<pedlar::Stall>().open) {
        pedlar::toggle(world);
    }
    if esc && world.resource::<free_traders::Desk>().open {
        free_traders::toggle(world, None);
    }
    if k || (esc && world.resource::<chains::Chains>().open) {
        chains::toggle(world, None);
    }
    if p || (esc && world.resource::<relations::Relations>().open) {
        relations::toggle(world);
    }
    if v {
        map::toggle_trade_lines(world);
    }
    if g || (esc && world.resource::<fleet::Fleet>().open) {
        fleet::toggle(world);
    }
    if i || (esc && world.resource::<shelf::Shelf>().open) {
        shelf::toggle(world);
    }
    if l || (esc && world.resource::<ledger::Ledger>().open) {
        toggle_ledger(world);
    }
    if t || (esc && world.resource::<routes::RouteBook>().open) {
        toggle_routes(world);
    }
    if h {
        map::home(world);
    }
}

/// Mark the node with this id selected (or not).
fn set_selected(world: &mut World, id: &str, on: bool) {
    if let Some(e) = engine_ui::input::find_id(world, id)
        && let Some(mut it) = world.get_mut::<Interaction>(e)
        && it.selected != on
    {
        it.selected = on;
    }
}

fn refresh_items(world: &mut World) {
    let (cat, tool) = (world.resource::<Hud>().category, *world.resource::<Tool>());
    let isle = world.resource::<Hud>().island;
    let by_type = world.resource::<plan::Plan>().by_type;
    if let Some(e) = engine_ui::input::find_id(world, "isle.items") {
        let _ = engine_ui::el::replace_children(world, e, &bar_items(cat, tool, isle, by_type));
        let w = Some(engine_ui::style::Val::Px(items_width(tab_len(
            cat, by_type, isle,
        ))));
        if let Some(mut n) = world.get_mut::<Node>(e)
            && n.style.max_width != w
        {
            n.style.max_width = w;
        }
    }
    let kinds = plan::tab_kinds(by_type, cat, isle);
    plan::note_tiles(world, &kinds);
    plan::refresh_prices(world, &kinds, isle);
    for i in 0..plan::tabs(by_type).len() {
        set_selected(world, &format!("isle.cat:{i}"), i == cat);
    }
    set_selected(world, "isle.tool:demolish", tool == Tool::Demolish);
    refresh_planning(world);
}

/// Light the blueprint and stamp medallions while their tool or mode is on.
fn refresh_planning(world: &mut World) {
    let tool = *world.resource::<Tool>();
    let blue = crate::blueprint::mode(world);
    set_selected(world, "isle.blueprint", blue);
    set_selected(world, "isle.stamp.copy", tool == Tool::Copy);
    set_selected(world, "isle.stamp.last", matches!(tool, Tool::Stamp { .. }));
}

/// Rebuild the tab medallions (after the sort changes).
fn refresh_tabs(world: &mut World) {
    let cat = world.resource::<Hud>().category;
    let by_type = world.resource::<plan::Plan>().by_type;
    if let Some(e) = engine_ui::input::find_id(world, "isle.tabs") {
        let demolish = *world.resource::<Tool>() == Tool::Demolish;
        let _ = engine_ui::el::replace_children(world, e, &tab_row(cat, by_type, demolish));
    }
}

fn fmt_int(x: f64) -> String {
    engine_ui::data::group_thousands(format!("{:.0}", x))
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The calendar: a game minute is a month, from spring 1820.
fn month_index(s: f64) -> u64 {
    (s / 60.0) as u64 + 3
}

fn clock(s: f64) -> String {
    let m = month_index(s);
    format!("{} {}", MONTHS[(m % 12) as usize], 1820 + m / 12)
}

fn season(s: f64) -> &'static str {
    match month_index(s) % 12 {
        2..=4 => "spring",
        5..=7 => "summer",
        8..=10 => "autumn",
        _ => "winter",
    }
}

/// Fill the bars' values now.
fn update_data_now(world: &mut World) {
    let focus = {
        let mut q = world.query::<&camera::StrategyCam>();
        q.iter(world).next().map(|c| c.focus)
    };
    let isle = focus.map_or(0, |f| {
        island::island_at(f.x, f.z).unwrap_or_else(|| island::nearest_island(f.x, f.z))
    });
    let econ = world.resource::<Economy>().clone();
    let hint = world
        .get_resource::<Hand>()
        .map(|h| h.hint.clone())
        .unwrap_or_default();
    let selected = world.get_resource::<Hand>().and_then(|h| h.selected);
    let mut rows: Vec<(String, String)> = vec![(String::new(), String::new()); 6];
    let (mut title, mut sub) = (String::new(), String::new());
    let mut progress: Option<f32> = None;
    let mut paused = false;
    if let Some(e) = selected
        && let Some(b) = world.get::<Building>(e).cloned()
    {
        let d = b.kind.def();
        title = d.name.to_string();
        sub = format!(
            "{} · {}",
            island::islands()[b.island].name,
            d.category.name()
        );
        if let Some(h) = world.get::<House>(e) {
            sub = format!(
                "{} house · {}",
                h.tier.name(),
                island::islands()[b.island].name
            );
            // Mood, needs by category, residents, income and moving up: hud/house.rs.
        } else if let Some(p) = world.get::<Producer>(e) {
            paused = p.paused;
            progress = Some(p.progress);
            // Status, chain, workers, site and upkeep: the works section (hud/works.rs).
        } else {
            // (The three item holders show their sockets instead: the card stays short.)
            // (The harbour shows its warehouse instead: hud/store.rs.)
            if crate::items::Host::of(b.kind).is_none()
                && b.kind != Kind::Harbour
                && !yard::builds_craft(b.kind)
            {
                rows[0] = ("About".into(), d.blurb.to_string());
            }
        }
    }
    let (ships, airships) = {
        let mut q = world.query::<&crate::fleet::Vessel>();
        q.iter(world)
            .filter(|v| v.commanded())
            .fold((0, 0), |(s, a), v| {
                if v.craft == crate::vessels::Craft::Airship {
                    (s, a + 1)
                } else {
                    (s + 1, a)
                }
            })
    };
    let region = map::region_label(world);
    // The player's own islands only: a rival's people are not the company's.
    let total: f32 = econ
        .islands
        .iter()
        .enumerate()
        .filter(|(i, _)| econ.owner(*i).is_none_or(|c| c == crate::sim::PLAYER))
        .map(|(_, b)| population(b))
        .sum();
    let rival_here = econ
        .owner(isle)
        .filter(|c| *c != crate::sim::PLAYER)
        .map(|c| crate::rivals::company_name(world, c));
    let books = &econ.islands[isle];
    let category = world.get_resource::<Hud>().map_or(2, |h| h.category);
    let tab_name = {
        let by_type = world
            .get_resource::<plan::Plan>()
            .is_some_and(|p| p.by_type);
        let tabs = plan::tabs(by_type);
        tabs[category.min(tabs.len() - 1)].name
    };
    {
        let mut data = world.resource_mut::<UiData>();
        data.set("isle.coins", fmt_int(econ.coins));
        let b = econ.balance_per_min;
        data.set(
            "isle.balance",
            format!(
                "[color={}]{}{:.0} a minute[/color]",
                if b >= 0.0 { "#9be08a" } else { "#ff8a70" },
                if b >= 0.0 { "+" } else { "−" },
                b.abs()
            ),
        );
        data.set("isle.people", fmt_int(total as f64));
        data.set(
            "isle.island",
            match &rival_here {
                Some(n) => format!("{} · {n}'s", island::islands()[isle].name),
                None => island::islands()[isle].name.to_string(),
            },
        );
        data.set("isle.island_people", {
            // Every tier that lives here, lowest first (fisherfolk alone on a new isle).
            let tiers: Vec<String> = Tier::ALL
                .into_iter()
                .filter(|t| books.population[*t as usize] >= 0.5 || *t == Tier::Fisherfolk)
                .map(|t| {
                    format!(
                        "{:.0} {}",
                        books.population[t as usize],
                        t.name().to_lowercase()
                    )
                })
                .collect();
            tiers.join(" · ")
        });
        data.set(
            "isle.map_note",
            format!(
                "{ships} {} · {airships} {} aloft",
                if ships == 1 { "ship" } else { "ships" },
                if airships == 1 { "airship" } else { "airships" }
            ),
        );
        data.set("isle.map_region", region.0);
        data.set("isle.map_blurb", region.1);
        data.set("isle.clock", clock(econ.clock_s));
        data.set("isle.season", season(econ.clock_s));
        data.set("isle.cat_name", tab_name);
        data.set("isle.hint", hint);
        data.set("isle.sel.title", title);
        data.set("isle.sel.sub", sub);
        data.set("isle.sel.pause", if paused { "Resume" } else { "Pause" });
        for (i, (k, v)) in rows.iter().enumerate() {
            data.set(&format!("isle.sel.k{i}"), k.clone());
            data.set(&format!("isle.sel.v{i}"), v.clone());
        }
    }
    // Hide empty card rows, size the progress bar, light the current speed.
    for (i, (k, _)) in rows.iter().enumerate() {
        if let Some(e) = engine_ui::input::find_id(world, &format!("isle.sel.row{i}"))
            && let Some(mut n) = world.get_mut::<Node>(e)
        {
            let display = if k.is_empty() {
                engine_ui::style::Display::None
            } else {
                engine_ui::style::Display::Flex
            };
            if n.style.display != Some(display) {
                n.style.display = Some(display);
            }
        }
    }
    if let Some(p) = progress
        && let Some(e) = engine_ui::input::find_id(world, "isle.sel.progress")
        && let Some(mut n) = world.get_mut::<Node>(e)
    {
        let w = Some(pct((p * 100.0).clamp(0.0, 100.0)));
        if n.style.width != w {
            n.style.width = w;
        }
    }
    for (v, ..) in SPEEDS {
        set_selected(world, &format!("isle.speed:{v}"), econ.speed == v);
    }
    let moved = world.get_resource_mut::<Hud>().is_some_and(|mut h| {
        let moved = h.island != isle;
        h.island = isle;
        moved
    });
    // Another island in view: its crops decide which farms the bar offers first.
    if moved {
        refresh_items(world);
    }
    topbar::fill(world, &econ, isle);
}

/// Refresh the bars a few times a second.
pub fn update_data(world: &mut World) {
    if !world.contains_resource::<Hud>() {
        return;
    }
    let dt = world.resource::<Time>().real_delta_secs();
    let due = {
        let mut h = world.resource_mut::<Hud>();
        h.refresh -= dt;
        h.refresh <= 0.0
    };
    read_keys(world);
    let isle = world.resource::<Hud>().island;
    plan::update(world, isle);
    if due {
        world.resource_mut::<Hud>().refresh = 0.2;
        update_data_now(world);
        {
            let by_type = world.resource::<plan::Plan>().by_type;
            let cat = world.resource::<Hud>().category;
            let kinds = plan::tab_kinds(by_type, cat, isle);
            plan::refresh_prices(world, &kinds, isle);
        }
        refresh_planning(world);
        ledger::update(world);
        map::update(world, 0.2);
        shelf::update(world);
        pedlar::update(world);
        free_traders::update(world);
        let here = pedlar::here(world);
        set_selected(world, map::PEDLAR_MEDAL, here);
        house::refresh(world);
        works::refresh(world);
        store::refresh(world);
        yard::refresh(world);
        fleet::refresh(world);
        alerts::refresh(world);
        advisor::refresh(world);
        relations::refresh(world);
        trouble::refresh(world);
    }
    demo_step(world, dt);
}

/// The build tile `--hud-demo` hovers: `--hud-tile=PavedRoad`, or the sawmill.
fn demo_tile() -> String {
    let k = std::env::args()
        .find_map(|a| a.strip_prefix("--hud-tile=").map(String::from))
        .unwrap_or_else(|| "Sawmill".into());
    format!("isle.build:{k}")
}

fn demo_place(world: &mut World, want: &str) {
    let Some(kind) = crate::defs::DEFS
        .iter()
        .map(|d| d.kind)
        .find(|k| format!("{k:?}") == want)
    else {
        return;
    };
    let harbour = {
        let mut q = world.query::<&Building>();
        q.iter(world)
            .find(|b| b.kind == Kind::Harbour && b.island == 0)
            .cloned()
    };
    let Some(h) = harbour else {
        return;
    };
    let at = crate::fleet::mooring(&h).0;
    let t = crate::grid::cell_of(at.x, at.y);
    let (w, d) = kind.def().size;
    let half = IVec2::new(w / 2, d / 2);
    for r in 4i32..60 {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dz.abs() != r {
                    continue;
                }
                let origin = t + IVec2::new(dx, dz) - half;
                for rot in 0..4u8 {
                    if crate::grid::check(world.resource::<crate::grid::Grid>(), kind, origin, rot)
                        .is_err()
                    {
                        continue;
                    }
                    if let Ok(e) = crate::build::place(world, kind, origin, rot, false) {
                        if crate::town::join_to_roads(world, e) {
                            return;
                        }
                        crate::build::demolish(world, e);
                    }
                }
            }
        }
    }
}

/// `--hud-demo`: open the first weaver's (or `--hud-card=Kind`'s) card at 2 s and keep the pointer on a build tile.
fn demo_step(world: &mut World, dt: f32) {
    let Some(t) = world.resource::<Hud>().demo else {
        return;
    };
    let t = t + dt;
    world.resource_mut::<Hud>().demo = Some(t);
    // `--hud-place=Shipyard`: put one of that kind up by the home harbour first (for card
    // shots of kinds the tour town lacks).
    if t - dt <= 0.0
        && let Some(want) =
            std::env::args().find_map(|a| a.strip_prefix("--hud-place=").map(String::from))
    {
        demo_place(world, &want);
    }
    if t > 2.0
        && !world.resource::<Hud>().info_open
        && !world.resource::<ledger::Ledger>().open
        && !world.resource::<routes::RouteBook>().open
    {
        // `--hud-card=Saltery` shows that kind's card instead (by its name in the code).
        let want = std::env::args()
            .find_map(|a| a.strip_prefix("--hud-card=").map(String::from))
            .unwrap_or_else(|| "Weaver".into());
        // `--hud-card=House:Burghers`: a house of that tier.
        let (want, tier) = match want.split_once(':') {
            Some((k, t)) => (k.to_string(), Some(t.to_string())),
            None => (want, None),
        };
        // `--hud-card-rival`: a works on a rival house's island. The tour's sandbox has no
        // rivals, so for the picture its second settled island is handed to the first house.
        let rival = std::env::args().any(|a| a == "--hud-card-rival").then(|| {
            {
                let mut econ = world.resource_mut::<crate::sim::Economy>();
                if econ.owner(0) == Some(crate::sim::PLAYER)
                    && let Some(k) = (1..econ.islands.len()).find(|k| econ.islands[*k].harbours > 0)
                    && !econ
                        .islands
                        .iter()
                        .any(|b| b.harbours > 0 && b.owner != crate::sim::PLAYER)
                {
                    econ.islands[k].owner = 1;
                }
            }
            let es: Vec<Entity> = world
                .query_filtered::<Entity, With<Building>>()
                .iter(world)
                .collect();
            es.into_iter().find(|e| {
                !crate::build::ours(world, *e)
                    && world
                        .get::<Building>(*e)
                        .is_some_and(|b| b.kind.def().output.is_some())
            })
        });
        let weaver = if let Some(found) = rival {
            found
        } else {
            let mut q = world.query::<(Entity, &Building, Option<&House>)>();
            q.iter(world)
                .find(|(_, b, h)| {
                    format!("{:?}", b.kind) == want
                        && tier
                            .as_ref()
                            .is_none_or(|t| h.is_some_and(|h| format!("{:?}", h.tier) == *t))
                })
                .or_else(|| {
                    q.iter(world)
                        .find(|(_, b, _)| format!("{:?}", b.kind) == want)
                })
                .or_else(|| q.iter(world).find(|(_, b, _)| b.kind == Kind::Weaver))
                .or_else(|| {
                    q.iter(world)
                        .find(|(_, b, _)| b.kind.def().output.is_some())
                })
                .map(|(e, ..)| e)
        };
        if let Some(e) = weaver {
            world.resource_mut::<Hand>().selected = Some(e);
            show_card(world);
            // `--hud-slot=k`: pick out an item socket on an institution's card.
            if let Some(k) =
                std::env::args().find_map(|a| a.strip_prefix("--hud-slot=")?.parse::<usize>().ok())
            {
                shelf::pick_slot(world, k);
            }
            // `--hud-yard=airship,biplane`: order those at the yard whose card is open.
            if let Some(list) =
                std::env::args().find_map(|a| a.strip_prefix("--hud-yard=").map(String::from))
            {
                for c in list.split(',') {
                    crate::tools::action(world, &format!("isle.yard.order:{c}"));
                }
            }
            // `--hud-design=craft[:fittings]`: a craft's designer open on the yard card.
            if let Some(spec) =
                std::env::args().find_map(|a| a.strip_prefix("--hud-design=").map(String::from))
            {
                yard::demo(world, &spec);
            }
            // `--hud-fleet=k`: the ships' gear with the first vessel's socket k picked.
            if let Some(k) =
                std::env::args().find_map(|a| a.strip_prefix("--hud-fleet=")?.parse::<usize>().ok())
            {
                fleet::demo(world, k);
            }
            // `--hud-burn=Kind`: burn the first building of that kind down (ruin shots).
            if let Some(want) =
                std::env::args().find_map(|a| a.strip_prefix("--hud-burn=").map(String::from))
            {
                let e = {
                    let mut q = world.query::<(Entity, &Building)>();
                    q.iter(world)
                        .find(|(_, b)| format!("{:?}", b.kind) == want)
                        .map(|(e, _)| e)
                };
                if let Some(e) = e {
                    crate::incidents::burn_now(world, e);
                }
            }
            // `--hud-alerts`: start a fire, a riot and a sickness or two (shots of the alerts).
            if std::env::args().any(|a| a == "--hud-alerts") {
                use crate::incidents::{self, Trouble};
                let picks: Vec<(Entity, Building)> = {
                    let mut q = world.query::<(Entity, &Building)>();
                    q.iter(world).map(|(e, b)| (e, b.clone())).collect()
                };
                let mut used = Vec::new();
                for (kind, isle, what) in [
                    (Kind::Bakery, 0, Trouble::Fire),
                    (Kind::House, 0, Trouble::Riot),
                    (Kind::House, 1, Trouble::Sickness),
                    (Kind::Brewery, 0, Trouble::Fire),
                    (Kind::House, 0, Trouble::Sickness),
                ] {
                    if let Some((e, b)) = picks
                        .iter()
                        .find(|(e, b)| b.kind == kind && b.island == isle && !used.contains(e))
                    {
                        used.push(*e);
                        incidents::start(world, *e, b, what, false);
                    }
                }
                if std::env::args().any(|a| a == "--hud-alert-go") {
                    alerts::demo_go(world);
                }
            }
            // `--hud-reveal=i`: lift the fog off island `i`, as if a ship had looked round it
            // (shots of an unsettled island on the chart).
            if let Some(i) = std::env::args()
                .find_map(|a| a.strip_prefix("--hud-reveal=")?.parse::<usize>().ok())
                && let Some(isle) = island::islands().get(i)
                && let Some(mut fog) = world.get_resource_mut::<crate::fog::FogOfWar>()
            {
                fog.reveal_map(isle.center.x, isle.center.y, isle.radius * 1.8);
            }
        }
    }
    // `--hud-route-island=i`: keep the pointer over island `i` on the routes chart.
    if t > 2.5
        && let Some(i) = std::env::args()
            .find_map(|a| a.strip_prefix("--hud-route-island=")?.parse::<usize>().ok())
        && let Some(p) = routes::demo_point(world, i)
    {
        world.write_message(engine::input::InputEvent::CursorMoved { position: p });
    } else if t > 2.5
        && let Some(e) = engine_ui::input::find_id(world, &demo_tile())
        && let Some(c) = world.get::<engine_ui::node::ComputedNode>(e)
    {
        let p = c.rect.center() + Vec2::new(8.0, 10.0);
        world.write_message(engine::input::InputEvent::CursorMoved { position: p });
    }
}
