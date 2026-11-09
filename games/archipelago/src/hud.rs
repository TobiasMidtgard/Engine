//! The player's screen: the top bar (coins, balance, people, the warehouse of the island in
//! view, the date and the game speed), the build bar by group, the selected building's card
//! and the tooltips, all in the engine UI's brass kit (`engine_ui::kit::brass`): lacquered
//! plates in brass bezels, parchment cards, medallion buttons and a display serif.
//!
//! The look of these is the UI thread's; what the buttons do, and the hand on the map, are in
//! `tools.rs`. Between the two: the `isle.*` actions the buttons send (`tools::action`), the
//! `isle.*` values in `UiData` filled by [`update_data`], and [`show_card`], [`tool_changed`]
//! and [`complain`].

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
use crate::defs::{Category, DEFS, Good, Kind};
use crate::island;
use crate::sim::{Building, Economy, House, Producer, population};
use crate::tools::{Hand, Tool};

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
            s += &format!(
                "\n[b]Makes[/b]  {} from {}",
                o.name().to_lowercase(),
                i.name().to_lowercase()
            )
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
fn good_cell(g: Good) -> El {
    brass::well(g.icon(), &format!("{{isle.stock.{}}}", g.index())).tooltip(
        g.name(),
        "In the warehouse of the island in view. Carts bring what workshops make; houses take what they need.",
    )
}

fn top_bar() -> El {
    // Two rows of six goods.
    let row = |goods: &[Good]| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(6.0))
            .children(goods.iter().map(|g| good_cell(*g)))
    };
    let goods = El::new()
        .class("col")
        .with_style(|s| s.gap(4.0))
        .child(row(&Good::ALL[..6]))
        .child(row(&Good::ALL[6..]));
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
            "Fisherfolk and labourers on every island.",
        ))
        .child(goods)
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

/// A build tile: the building's badge, its name and a price line.
fn tile(icon: &str, name: &str, price: El, action: String, selected: bool, i: usize) -> El {
    El::new()
        .class("brass_tile")
        .on_click(action)
        .selected(selected)
        .with_style(|s| {
            s.direction(Direction::Column)
                .align_items(Align::Center)
                .gap(2.0)
                .padding([10.0, 6.0, 8.0, 6.0])
                .width(98.0)
                .enter(
                    Enter::rise(0.32, 22.0)
                        .delay(0.035 * i as f32)
                        .ease(Ease::Back),
                )
        })
        .child(brass::badge(icon, 52.0))
        .child(
            El::text(name)
                .class("ink")
                .text_style(|t| t.bold().size(14.0).center().nowrap())
                .pick_ignore(),
        )
        .child(price)
}

fn item_tile(k: Kind, selected: bool, i: usize) -> El {
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
    tile(kind_icon(k), d.name, price, action, selected, i)
        .id(format!("isle.build:{k:?}"))
        .tooltip(d.name, cost_text(k))
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
    )
    .tooltip(
        "Demolish",
        "Click a building or a road to clear it. Nothing is refunded.\n[b]Key[/b]  Delete",
    )
}

fn bar_items(category: usize, tool: Tool) -> Vec<El> {
    let c = Category::ALL[category];
    let mut v: Vec<El> = kinds_in(c)
        .into_iter()
        .enumerate()
        .map(|(i, k)| {
            let sel = match tool {
                Tool::Build { kind, .. } => kind == k,
                Tool::Road => k == Kind::Road,
                _ => false,
            };
            item_tile(k, sel, i)
        })
        .collect();
    if c == Category::Roads {
        let n = v.len();
        v.push(demolish_tile(tool == Tool::Demolish, n));
    }
    v
}

fn build_bar(category: usize, tool: Tool) -> El {
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
                .with_style(|s| s.gap(12.0).margin([4.0, 0.0, 0.0, 0.0]))
                .children(bar_items(category, tool)),
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
                .with_style(|s| s.gap(10.0).justify(Justify::Center))
                .child(brass::badge(i.icon(), 42.0).tooltip(i.name(), "Taken from the warehouse."))
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
    update_data_now(world);
    if let Err(e) = screens::spawn_screen(world, hud_screen(TOP, 0), &top_bar()) {
        engine::core::log::warn!("archipelago top bar: {e}");
    }
    let tool = *world.resource::<Tool>();
    if let Err(e) = screens::spawn_screen(world, hud_screen(BAR, 0), &build_bar(2, tool)) {
        engine::core::log::warn!("archipelago build bar: {e}");
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
        let UiEvent::Action { action: a, .. } = ev else {
            continue;
        };
        if let Some(i) = a
            .strip_prefix("isle.cat:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            world.resource_mut::<Hud>().category = i.min(Category::ALL.len() - 1);
            refresh_items(world);
        } else {
            crate::tools::action(world, &a);
        }
        update_data_now(world);
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
    if let Some(e) = engine_ui::input::find_id(world, "isle.items") {
        let _ = engine_ui::el::replace_children(world, e, &bar_items(cat, tool));
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
            let need_list: Vec<String> = needs[..basic]
                .iter()
                .map(|g| {
                    let pct = books.supply[g.index()] * 100.0;
                    let s = format!("{} {:.0}%", g.name(), pct);
                    if pct >= 99.0 { good(&s) } else { bad(&s) }
                })
                .collect();
            rows[1] = ("Needs".into(), need_list.join(", "));
            let svc: Vec<String> = h
                .tier
                .services()
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    if h.services[i] {
                        good(&format!("{} ✓", k.def().name))
                    } else {
                        bad(&format!("{} ✗", k.def().name))
                    }
                })
                .collect();
            rows[2] = ("Services".into(), svc.join(", "));
            rows[3] = (
                "Taxes".into(),
                format!("{:.1} coins a minute", h.residents * h.tier.tax()),
            );
            rows[4] = (
                "Next tier".into(),
                match h.tier.next() {
                    Some((next, cost)) => {
                        let c: Vec<String> = cost
                            .iter()
                            .map(|(g, n)| format!("{n} {}", g.name().to_lowercase()))
                            .collect();
                        format!("{} when full and content ({})", next.name(), c.join(", "))
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
        data.set(
            "isle.island_people",
            format!(
                "{:.0} fisherfolk · {:.0} labourers",
                books.population[0], books.population[1]
            ),
        );
        for g in Good::ALL {
            data.set(
                &format!("isle.stock.{}", g.index()),
                format!("{:.0}", books.get(g)),
            );
        }
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
    if let Some(mut h) = world.get_resource_mut::<Hud>() {
        h.island = isle;
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
    if due {
        world.resource_mut::<Hud>().refresh = 0.2;
        update_data_now(world);
    }
    demo_step(world, dt);
}

/// `--hud-demo`: open the first weaver's card at 2 s and keep the pointer on a build tile.
fn demo_step(world: &mut World, dt: f32) {
    let Some(t) = world.resource::<Hud>().demo else {
        return;
    };
    let t = t + dt;
    world.resource_mut::<Hud>().demo = Some(t);
    if t > 2.0 && !world.resource::<Hud>().info_open {
        let weaver = {
            let mut q = world.query::<(Entity, &Building)>();
            q.iter(world)
                .find(|(_, b)| b.kind == Kind::Weaver)
                .or_else(|| q.iter(world).find(|(_, b)| b.kind.def().output.is_some()))
                .map(|(e, _)| e)
        };
        if let Some(e) = weaver {
            world.resource_mut::<Hand>().selected = Some(e);
            show_card(world);
        }
    }
    if t > 2.5
        && let Some(e) = engine_ui::input::find_id(world, "isle.build:Sawmill")
        && let Some(c) = world.get::<engine_ui::node::ComputedNode>(e)
    {
        let p = c.rect.center() + Vec2::new(8.0, 10.0);
        world.write_message(engine::input::InputEvent::CursorMoved { position: p });
    }
}
