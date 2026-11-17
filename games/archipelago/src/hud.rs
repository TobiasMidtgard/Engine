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
use engine_ui::style::{Align, Direction, Ease, Enter, Justify, Pick, Position, pct};

use crate::camera;
use crate::defs::{Category, DEFS, Good, Kind, Tier};
use crate::island;
use crate::sim::{Building, Economy, House, Producer, population};
use crate::tools::{Hand, Tool};

mod ledger;
mod map;
mod routes;

/// The bars' own state between frames.
#[derive(Resource, Default)]
pub struct Hud {
    seen: MessageCursor<UiEvent>,
    category: usize,
    refresh: f32,
    /// The island the top bar shows.
    pub island: usize,
    info_open: bool,
    /// The goods the top bar shows: those of the island in view.
    goods: Vec<Good>,
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
    (1.0, "isle_speed_1", "Normal speed (1)"),
    (2.0, "isle_speed_2", "Double speed (2)"),
    (4.0, "isle_speed_4", "Four times as fast (3)"),
];

fn kind_icon(k: Kind) -> &'static str {
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
        _ => k.def().output.map_or("isle_workshop", Good::icon),
    }
}

fn category_icon(c: Category) -> &'static str {
    match c {
        Category::Roads => "isle_road",
        Category::Homes => "isle_house",
        Category::Production => "isle_workshop",
        Category::Farms => "isle_farm",
        Category::Public => "isle_chapel",
        Category::Harbour => "isle_anchor",
    }
}

fn kinds_in(c: Category) -> Vec<Kind> {
    DEFS.iter()
        .filter(|d| d.category == c)
        .map(|d| d.kind)
        .collect()
}

/// The build tile's tooltip: what it is for, then cost, workers, chain and upkeep.
fn cost_text(k: Kind) -> String {
    let d = k.def();
    let mut cost = vec![format!("{} coins", d.coins)];
    for (g, n) in d.cost {
        cost.push(format!("{n} {}", g.name().to_lowercase()));
    }
    let mut s = String::new();
    if !d.blurb.is_empty() {
        s += &format!("[i]{}[/i]\n", d.blurb);
    }
    s += &format!("[b]Cost[/b]  {}", cost.join(" · "));
    if let Some((t, w)) = d.workers {
        s += &format!("\n[b]Workers[/b]  {w} {}", t.name().to_lowercase());
    }
    match (d.input, d.output) {
        (Some(i), Some(o)) => {
            let from = match d.input2 {
                Some(j) => format!(
                    "{} and {}",
                    i.name().to_lowercase(),
                    j.name().to_lowercase()
                ),
                None => i.name().to_lowercase(),
            };
            s += &format!("\n[b]Makes[/b]  {} from {from}", o.name().to_lowercase())
        }
        (None, Some(o)) => s += &format!("\n[b]Makes[/b]  {}", o.name().to_lowercase()),
        _ => {}
    }
    if d.upkeep > 0.0 {
        s += &format!("\n[b]Upkeep[/b]  {:.0} coins a minute", d.upkeep);
    }
    s
}

/// Big number with a small-caps line under it, beside a badge.
fn stat(icon: &str, bind: &str, label_bind: &str, tip: &str, tip_text: &str) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(10.0))
        .tooltip(tip, tip_text)
        .child(brass::badge(icon, 46.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text(bind)
                        .class("brass_number")
                        .text_style(|t| t.nowrap().tabular().shadow())
                        .pick_ignore(),
                )
                .child(
                    El::text(label_bind)
                        .class("brass_label")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                ),
        )
}

/// A good in the warehouse: badge and count in a recessed well.
fn good_cell(g: Good, compact: bool) -> El {
    let well = brass::well(g.icon(), &format!("{{isle.stock.{}}}", g.index()));
    let well = if compact {
        well.with_style(|s| s.padding([2.0, 7.0, 2.0, 2.0]).gap(3.0))
    } else {
        well
    };
    well.tooltip(
        g.name(),
        "In the warehouse of the island in view. Carts bring what workshops make; houses take what they need.",
    )
}

/// Room held for the town cartouche in the middle of the top bar.
const CARTOUCHE_ROOM: f32 = 380.0;

/// Wells a row holds on either side of the town cartouche.
const WELLS_PER_ROW: usize = 6;

/// The goods wells in two rows at most, six to a row; `side` 0 is the block left of the town
/// cartouche, 1 the block right of it (goods past twelve). The wells are smaller when the
/// right block is in use.
fn goods_rows(goods: &[Good], side: usize) -> Vec<El> {
    let compact = goods.len() > 2 * WELLS_PER_ROW;
    let part = if side == 0 {
        &goods[..goods.len().min(2 * WELLS_PER_ROW)]
    } else {
        &goods[goods.len().min(2 * WELLS_PER_ROW)..]
    };
    part.chunks(WELLS_PER_ROW)
        .map(|row| {
            El::new()
                .class("row")
                .with_style(move |s| s.gap(if compact { 4.0 } else { 6.0 }))
                .children(row.iter().map(|g| good_cell(*g, compact)))
        })
        .collect()
}

/// The top bar, with the wells for `goods` (the goods kept, made or used on the island in
/// view; they change as the view moves to another island).
fn top_bar(goods: &[Good]) -> El {
    let block = |side: usize| {
        El::new()
            .class("col")
            .id(if side == 0 {
                "isle.goods"
            } else {
                "isle.goods2"
            })
            .with_style(|s| s.gap(4.0))
            .children(goods_rows(goods, side))
    };
    let left = block(0);
    // Goods past twelve: a second block just right of the town cartouche (an overlay row with
    // the cartouche's room held free in the middle).
    let right = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(0.0)
                .bottom(0.0)
                .align_items(Align::Center)
                .pick(Pick::Ignore)
        })
        .child(El::new().with_style(|s| s.grow(1.0).basis(0.0).pick(Pick::Ignore)))
        .child(El::new().with_style(|s| s.width(CARTOUCHE_ROOM).pick(Pick::Ignore)))
        .child(
            El::new()
                .with_style(|s| s.grow(1.0).basis(0.0).pick(Pick::Ignore))
                .child(block(1)),
        );
    let speed = El::new()
        .class("row")
        .with_style(|s| s.gap(8.0))
        .children(SPEEDS.iter().map(|(v, icon, tip)| {
            brass::medallion(icon, 42.0, &format!("isle.speed:{v}"))
                .id(format!("isle.speed:{v}"))
                .tooltip("", *tip)
        }));
    let bar = El::new()
        .class("brass_bar")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(14.0)
                .right(14.0)
                .top(10.0)
                .min_height(76.0)
                .padding([8.0, 22.0])
                .gap(22.0)
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
            "Every tier, on every island.",
        ))
        .child(left)
        .child(right)
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0).align_items(Align::End))
                .child(
                    El::text("{isle.clock}")
                        .class("brass_title")
                        .text_style(|t| t.nowrap().shadow())
                        .with_style(|s| s.font_size(20.0))
                        .pick_ignore(),
                )
                .child(
                    El::text("{isle.season}")
                        .class("brass_label")
                        .text_style(|t| t.nowrap())
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
                .top(80.0)
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::new()
                .class("cartouche")
                .tooltip(
                    "Island in view",
                    "The top bar shows this island's warehouse.",
                )
                .with_style(|s| {
                    s.min_width(230.0)
                        .enter(Enter::rise(0.5, -24.0).delay(0.15))
                })
                .child(
                    El::text("{isle.island}")
                        .class("ink_title")
                        .text_style(|t| t.nowrap())
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
    let price = El::new()
        .class("row")
        .with_style(|s| s.gap(3.0).justify(Justify::Center).pick(Pick::Ignore))
        .child(El::icon("coin").with_style(|s| s.size(14.0, 14.0).pick(Pick::Ignore)))
        .child(
            El::text(d.coins.to_string())
                .text_style(|t| t.size(13.0).nowrap().tabular())
                .with_style(|s| s.color(Colors::ink_dim()))
                .pick_ignore(),
        );
    let action = format!(
        "isle.build:{}",
        serde_json::to_value(k)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default()
    );
    let mut tip = cost_text(k);
    if let Some(isle) = grows {
        tip.push_str(&format!("\n[i]Does not grow on {isle}.[/i]"));
    }
    tile(kind_icon(k), d.name, price, action, selected, i, compact)
        .id(format!("isle.build:{k:?}"))
        .with_style(move |s| s.opacity(if grows.is_some() { 0.5 } else { 1.0 }))
        .tooltip(d.name, tip)
}

fn demolish_tile(selected: bool, i: usize) -> El {
    let price = El::text("free")
        .text_style(|t| t.size(13.0).nowrap().italic())
        .with_style(|s| s.color(Colors::ink_dim()))
        .pick_ignore();
    tile(
        "isle_demolish",
        "Demolish",
        price,
        "isle.tool:demolish".into(),
        selected,
        i,
        false,
    )
    .tooltip(
        "Demolish",
        "Click a building or a road to clear it. Nothing is refunded.\n[b]Key[/b]  Delete",
    )
}

/// The category's build tiles for island `isle` (the one in view): farms whose crop does not
/// grow there go last, faded.
fn bar_items(category: usize, tool: Tool, isle: usize) -> Vec<El> {
    let c = Category::ALL[category];
    let here = island::islands().get(isle);
    let grows = |k: Kind| !k.def().needs_fertility || here.is_none_or(|i| i.fertility.allows(k));
    let mut kinds = kinds_in(c);
    kinds.sort_by_key(|k| !grows(*k));
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
    if c == Category::Roads {
        let n = v.len();
        v.push(demolish_tile(tool == Tool::Demolish, n));
    }
    v
}

fn build_bar(category: usize, tool: Tool, isle: usize) -> El {
    let tabs = El::new().class("row").with_style(|s| s.gap(14.0)).children(
        Category::ALL.iter().enumerate().map(|(i, c)| {
            brass::medallion(category_icon(*c), 52.0, &format!("isle.cat:{i}"))
                .id(format!("isle.cat:{i}"))
                .selected(i == category)
                .tooltip(c.name(), "")
        }),
    );
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
                .with_style(|s| s.gap(10.0).margin([4.0, 0.0, 0.0, 0.0]))
                .children(bar_items(category, tool, isle)),
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

fn info_card(kind: Kind) -> El {
    let d = kind.def();
    // What goes in and comes out, as badges.
    let chain = match (d.input, d.output) {
        (Some(i), Some(o)) => Some(
            El::new()
                .class("row")
                .with_style(|s| {
                    s.gap(10.0)
                        .justify(Justify::Center)
                        .align_items(Align::Center)
                })
                .child(brass::badge(i.icon(), 42.0).tooltip(i.name(), "Taken from the warehouse."))
                .children(d.input2.map(|j| {
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(10.0).align_items(Align::Center))
                        .child(
                            El::text("+")
                                .class("ink_title")
                                .text_style(|t| t.size(20.0))
                                .pick_ignore(),
                        )
                        .child(
                            brass::badge(j.icon(), 42.0)
                                .tooltip(j.name(), "Taken from the warehouse, with the first."),
                        )
                }))
                .child(El::icon("isle_arrow").with_style(|s| s.size(34.0, 20.0).pick(Pick::Ignore)))
                .child(brass::badge(o.icon(), 50.0).tooltip(o.name(), "Carted to the warehouse.")),
        ),
        (None, Some(o)) => Some(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).justify(Justify::Center))
                .child(brass::badge(o.icon(), 50.0).tooltip(o.name(), "Carted to the warehouse.")),
        ),
        _ => None,
    };
    let card = brass::parchment()
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(22.0)
                .top(140.0)
                .width(360.0)
                .padding([18.0, 22.0])
                .gap(8.0)
                .direction(Direction::Column)
                .enter(Enter::slide(0.38, 70.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center))
                .child(brass::badge(kind_icon(kind), 62.0))
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
        .child_opt(chain)
        .child_opt(d.output.map(|_| progress_bar()))
        .children((0..6).map(card_row))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).margin([8.0, 0.0, 0.0, 0.0]))
                .child_opt(d.output.map(|_| {
                    El::button("{isle.sel.pause}", "isle.sel.pause").class("brass_button")
                }))
                .child(El::button("Demolish", "isle.sel.demolish").class("brass_button_dark")),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(card)
}

/// The batch in progress, as a brass bar (its fill is sized in [`update_data_now`]).
fn progress_bar() -> El {
    El::new()
        .class("brass_track")
        .with_style(|s| s.width(pct(100.0)).height(12.0))
        .child(
            El::new()
                .class("brass_fill")
                .id("isle.sel.progress")
                .with_style(|s| s.width(pct(0.0))),
        )
}

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
    world.resource_mut::<Hud>().category = 2;
    if std::env::args().any(|a| a == "--hud-demo") {
        world.resource_mut::<Hud>().demo = Some(0.0);
    }
    // `--hud-cat=k`: open the build bar on category k (tour shots).
    if let Some(k) = std::env::args()
        .find_map(|a| a.strip_prefix("--hud-cat=")?.parse::<usize>().ok())
        .filter(|k| *k < Category::ALL.len())
    {
        world.resource_mut::<Hud>().category = k;
    }
    update_data_now(world);
    if let Err(e) = screens::spawn_screen(
        world,
        hud_screen(TOP, 0),
        &top_bar(&world.resource::<Hud>().goods.clone()),
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
        ),
    ) {
        engine::core::log::warn!("archipelago build bar: {e}");
    }
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
        ledger::update(world);
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
    if screens::spawn_screen(world, hud_screen(INFO, 1), &info_card(kind)).is_ok() {
        world.resource_mut::<Hud>().info_open = true;
        update_data_now(world);
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
            world.resource_mut::<Hud>().category = i.min(Category::ALL.len() - 1);
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
        } else if a.starts_with("isle.route.") {
            let opened = a.starts_with("isle.route.open");
            crate::tools::action(world, &a);
            if opened {
                routes::pick_last(world);
            } else {
                routes::refresh(world);
            }
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

/// The HUD's own keys: L the ledger, T the trade routes, H home, Esc closes either book.
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
    let (l, h, t, esc) = (
        keys.just_pressed(KeyCode::L),
        keys.just_pressed(KeyCode::H),
        keys.just_pressed(KeyCode::T),
        keys.just_pressed(KeyCode::Escape),
    );
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
    if let Some(e) = engine_ui::input::find_id(world, "isle.items") {
        let _ = engine_ui::el::replace_children(world, e, &bar_items(cat, tool, isle));
    }
    for i in 0..Category::ALL.len() {
        set_selected(world, &format!("isle.cat:{i}"), i == cat);
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
    let good = |s: &str| format!("[color=#2f6e28]{s}[/color]");
    let bad = |s: &str| format!("[color=#9a2f1c]{s}[/color]");
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
            let (needs, basic) = h.tier.needs();
            // What each need is worth to this tier, in coins a minute.
            let worth = |n: crate::defs::Need| {
                h.tier
                    .wants()
                    .iter()
                    .find(|w| w.need == n)
                    .map_or(0.0, |w| w.coins)
            };
            sub = format!(
                "{} house · {}",
                h.tier.name(),
                island::islands()[b.island].name
            );
            rows[0] = (
                "Residents".into(),
                format!("{:.1} of {:.0}", h.residents, h.tier.capacity()),
            );
            let books = &econ.islands[b.island];
            let _ = basic;
            let need_list: Vec<String> = needs
                .iter()
                .map(|g| {
                    let met = books.supply[g.index()].min(1.0);
                    let s = format!(
                        "{} +{:.1}",
                        g.name(),
                        worth(crate::defs::Need::Good(*g)) * met
                    );
                    if met >= 0.99 { good(&s) } else { bad(&s) }
                })
                .collect();
            rows[1] = ("Needs".into(), need_list.join(", "));
            let svc: Vec<String> = h
                .tier
                .services()
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    let coins = worth(crate::defs::Need::Service(*k));
                    if h.services[i] {
                        good(&format!("{} +{coins:.1}", k.def().name))
                    } else {
                        bad(&format!("{} ✗", k.def().name))
                    }
                })
                .collect();
            rows[2] = ("Services".into(), svc.join(", "));
            rows[3] = (
                "Income".into(),
                format!(
                    "{:.1} of {:.1} coins a minute",
                    h.income,
                    h.tier.full_income()
                ),
            );
            rows[4] = (
                "Next tier".into(),
                match h.tier.next() {
                    Some((next, cost)) => {
                        let c: Vec<String> = cost
                            .iter()
                            .map(|(g, n)| format!("{n} {}", g.name().to_lowercase()))
                            .collect();
                        let road = if next.wants_paving() && !h.paved {
                            ", needs a paved road at the door"
                        } else {
                            ""
                        };
                        format!(
                            "{} when full and content ({}){road}",
                            next.name(),
                            c.join(", ")
                        )
                    }
                    None => "Top tier for now".into(),
                },
            );
            rows[5] = ("Content".into(), format!("for {:.0} s", h.content_s));
        } else if let Some(p) = world.get::<Producer>(e) {
            paused = p.paused;
            progress = Some(p.progress);
            let status = p.status.text().to_string();
            rows[0] = (
                "Status".into(),
                if p.paused {
                    bad("Paused")
                } else if p.efficiency > 0.5 {
                    good(&status)
                } else {
                    bad(&status)
                },
            );
            rows[1] = ("Output".into(), format!("{:.0}%", p.efficiency * 100.0));
            rows[2] = (
                "Cycle".into(),
                format!("one every {:.0} s at full strength", d.cycle_s),
            );
            rows[3] = d.workers.map_or(Default::default(), |(t, n)| {
                ("Workers".into(), format!("{n} {}", t.name().to_lowercase()))
            });
            rows[4] = ("Made".into(), format!("{} so far", p.made));
            rows[5] = ("Upkeep".into(), format!("{:.0} coins a minute", d.upkeep));
        } else {
            rows[0] = ("About".into(), d.blurb.to_string());
            if b.kind == Kind::Harbour {
                let books = &econ.islands[b.island];
                rows[1] = (
                    "Storage".into(),
                    format!("{:.0} of each good", books.capacity),
                );
            }
        }
    }
    let (ships, airships) = {
        let mut q = world.query::<&crate::fleet::Vessel>();
        q.iter(world).fold((0, 0), |(s, a), v| {
            if v.craft == crate::vessels::Craft::Airship {
                (s, a + 1)
            } else {
                (s + 1, a)
            }
        })
    };
    let region = map::region_label(world);
    let total: f32 = econ.islands.iter().map(population).sum();
    let books = &econ.islands[isle];
    let category = world.get_resource::<Hud>().map_or(2, |h| h.category);
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
        data.set("isle.island", island::islands()[isle].name);
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
        for g in Good::ALL {
            data.set(
                &format!("isle.stock.{}", g.index()),
                format!("{:.0}", books.get(g)),
            );
        }
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
        data.set(
            "isle.cat_name",
            Category::ALL[category.min(Category::ALL.len() - 1)].name(),
        );
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
    // The top bar's goods follow the island's: rebuild the wells when that set changes.
    let goods = ledger::island_goods(&econ, isle);
    if world
        .get_resource::<Hud>()
        .is_some_and(|h| h.goods != goods)
    {
        for (side, id) in ["isle.goods", "isle.goods2"].into_iter().enumerate() {
            if let Some(e) = engine_ui::input::find_id(world, id) {
                let _ = engine_ui::el::replace_children(world, e, &goods_rows(&goods, side));
            }
        }
        world.resource_mut::<Hud>().goods = goods;
    }
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
    if due {
        world.resource_mut::<Hud>().refresh = 0.2;
        update_data_now(world);
        ledger::update(world);
        map::update(world, 0.2);
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

/// `--hud-demo`: open the first weaver's (or `--hud-card=Kind`'s) card at 2 s and keep the pointer on a build tile.
fn demo_step(world: &mut World, dt: f32) {
    let Some(t) = world.resource::<Hud>().demo else {
        return;
    };
    let t = t + dt;
    world.resource_mut::<Hud>().demo = Some(t);
    if t > 2.0
        && !world.resource::<Hud>().info_open
        && !world.resource::<ledger::Ledger>().open
        && !world.resource::<routes::RouteBook>().open
    {
        // `--hud-card=Saltery` shows that kind's card instead (by its name in the code).
        let want = std::env::args()
            .find_map(|a| a.strip_prefix("--hud-card=").map(String::from))
            .unwrap_or_else(|| "Weaver".into());
        let weaver = {
            let mut q = world.query::<(Entity, &Building)>();
            q.iter(world)
                .find(|(_, b)| format!("{:?}", b.kind) == want)
                .or_else(|| q.iter(world).find(|(_, b)| b.kind == Kind::Weaver))
                .or_else(|| q.iter(world).find(|(_, b)| b.kind.def().output.is_some()))
                .map(|(e, _)| e)
        };
        if let Some(e) = weaver {
            world.resource_mut::<Hand>().selected = Some(e);
            show_card(world);
        }
    }
    if t > 2.5
        && let Some(e) = engine_ui::input::find_id(world, &demo_tile())
        && let Some(c) = world.get::<engine_ui::node::ComputedNode>(e)
    {
        let p = c.rect.center() + Vec2::new(8.0, 10.0);
        world.write_message(engine::input::InputEvent::CursorMoved { position: p });
    }
}
