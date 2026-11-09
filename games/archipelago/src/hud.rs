//! The player's screen: the top bar (coins, balance, people, the warehouse of the island in
//! view), the build bar by group, the selected building's card and the speed buttons.
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
use engine_ui::node::Screen;
use engine_ui::screens;
use engine_ui::style::{Pick, Position};

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
}

const TOP: &str = "isle_top";
const BAR: &str = "isle_bar";
const INFO: &str = "isle_info";

fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Road => "isle_road",
        Kind::Harbour => "isle_anchor",
        Kind::House => "isle_house",
        Kind::Market => "coins",
        Kind::Chapel => "isle_chapel",
        Kind::AirshipMast => "isle_airship",
        _ => k.def().output.map_or("isle_workshop", Good::icon),
    }
}

fn kinds_in(c: Category) -> Vec<Kind> {
    DEFS.iter()
        .filter(|d| d.category == c)
        .map(|d| d.kind)
        .collect()
}

fn cost_text(k: Kind) -> String {
    let d = k.def();
    let mut parts = vec![format!("{} coins", d.coins)];
    for (g, n) in d.cost {
        parts.push(format!("{n} {}", g.name().to_lowercase()));
    }
    let mut s = parts.join(", ");
    if let Some((t, w)) = d.workers {
        s += &format!(". Needs {w} {}", t.name().to_lowercase());
    }
    match (d.input, d.output) {
        (Some(i), Some(o)) => {
            s += &format!(
                ". Turns {} into {}",
                i.name().to_lowercase(),
                o.name().to_lowercase()
            )
        }
        (None, Some(o)) => s += &format!(". Makes {}", o.name().to_lowercase()),
        _ => {}
    }
    if d.upkeep > 0.0 {
        s += &format!(". Upkeep {:.0}/min", d.upkeep);
    }
    format!("{}\n[color=#aab4c4]{s}.[/color]", d.blurb)
}

fn caption(text: &str) -> El {
    El::text(text)
        .class("caption")
        .text_style(|t| t.nowrap())
        .pick_ignore()
}

/// A good's icon and its warehouse count.
fn good_cell(g: Good) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(4.0).min_width(54.0))
        .tooltip(g.name(), "In the warehouse of the island in view: made and used per minute in the card on the right.")
        .child(El::icon(g.icon()).with_style(|s| s.size(24.0, 24.0).pick(Pick::Ignore)))
        .child(
            El::text(format!("{{isle.stock.{}}}", g.index()))
                .text_style(|t| t.semibold().nowrap().size(15.0))
                .pick_ignore(),
        )
}

fn top_bar() -> El {
    let stat = |icon: &str, bind: &str, tip: &str, tip_text: &str| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(6.0))
            .tooltip(tip, tip_text)
            .child(El::icon(icon).with_style(|s| s.size(26.0, 26.0).pick(Pick::Ignore)))
            .child(
                El::text(bind)
                    .text_style(|t| t.bold().nowrap().size(17.0))
                    .pick_ignore(),
            )
    };
    let goods = El::new()
        .class("row")
        .with_style(|s| s.gap(6.0))
        .children(Good::ALL.into_iter().map(good_cell));
    let speed = El::new().class("row").with_style(|s| s.gap(4.0)).children(
        [("II", "0"), ("1×", "1"), ("2×", "2"), ("4×", "4")]
            .into_iter()
            .map(|(l, v)| {
                El::button(l, format!("isle.speed:{v}"))
                    .class("ghost")
                    .id(format!("isle.speed:{v}"))
                    .with_style(|s| s.padding([4.0, 10.0]).min_width(40.0))
            }),
    );
    let bar = El::new()
        .class("glass")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(16.0)
                .right(16.0)
                .top(12.0)
                .padding([8.0, 16.0])
                .gap(22.0)
                .align_items(engine_ui::style::Align::Center)
                .radius(14.0)
        })
        .child(stat(
            "coin",
            "{isle.coins}",
            "Coins",
            "Taxes from houses, minus every building's upkeep.",
        ))
        .child(
            El::text("{isle.balance}")
                .text_style(|t| t.semibold().nowrap().size(15.0))
                .tooltip("Balance", "Coins per minute after upkeep.")
                .id("isle.balance"),
        )
        .child(stat(
            "isle_people",
            "{isle.people}",
            "People",
            "Fisherfolk and labourers on every island.",
        ))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text("{isle.island}")
                        .text_style(|t| t.bold().nowrap().size(15.0))
                        .pick_ignore(),
                )
                .child(caption("{isle.island_people}")),
        )
        .child(goods)
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(caption("{isle.clock}"))
        .child(speed);
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(bar)
}

fn item_button(k: Kind, selected: bool) -> El {
    let d = k.def();
    El::new()
        .class("card")
        .id(format!("isle.build:{k:?}"))
        .on_click(format!(
            "isle.build:{}",
            serde_json::to_value(k)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default()
        ))
        .selected(selected)
        .tooltip(d.name, cost_text(k))
        .with_style(|s| {
            s.direction(engine_ui::style::Direction::Column)
                .align_items(engine_ui::style::Align::Center)
                .gap(4.0)
                .padding([8.0, 6.0])
                .width(84.0)
        })
        .child(El::icon(kind_icon(k)).with_style(|s| s.size(38.0, 38.0).pick(Pick::Ignore)))
        .child(
            El::text(d.name)
                .text_style(|t| t.semibold().size(12.0).center().nowrap())
                .pick_ignore(),
        )
}

fn bar_items(category: usize, tool: Tool) -> Vec<El> {
    let c = Category::ALL[category];
    let mut v: Vec<El> = kinds_in(c)
        .into_iter()
        .map(|k| {
            let sel = match tool {
                Tool::Build { kind, .. } => kind == k,
                Tool::Road => k == Kind::Road,
                _ => false,
            };
            item_button(k, sel)
        })
        .collect();
    if c == Category::Roads {
        v.push(
            El::new()
                .class("card")
                .on_click("isle.tool:demolish")
                .selected(tool == Tool::Demolish)
                .tooltip(
                    "Demolish",
                    "Click a building or a road to clear it. Nothing is refunded.",
                )
                .with_style(|s| {
                    s.direction(engine_ui::style::Direction::Column)
                        .align_items(engine_ui::style::Align::Center)
                        .gap(4.0)
                        .padding([8.0, 6.0])
                        .width(84.0)
                })
                .child(
                    El::icon("isle_demolish").with_style(|s| s.size(38.0, 38.0).pick(Pick::Ignore)),
                )
                .child(
                    El::text("Demolish")
                        .text_style(|t| t.semibold().size(12.0).center())
                        .pick_ignore(),
                ),
        );
    }
    v
}

fn build_bar(category: usize, tool: Tool) -> El {
    let tabs = El::new().class("row").with_style(|s| s.gap(6.0)).children(
        Category::ALL.iter().enumerate().map(|(i, c)| {
            El::button(c.name(), format!("isle.cat:{i}"))
                .class("tab")
                .selected(i == category)
                .with_style(|s| s.padding([5.0, 12.0]))
        }),
    );
    let panel = El::new()
        .class("glass")
        .with_style(|s| {
            s.direction(engine_ui::style::Direction::Column)
                .gap(8.0)
                .padding([10.0, 14.0])
                .radius(16.0)
                .align_items(engine_ui::style::Align::Center)
        })
        .child(tabs)
        .child(
            El::new()
                .class("row")
                .id("isle.items")
                .with_style(|s| s.gap(8.0))
                .children(bar_items(category, tool)),
        );
    let hint = El::text("{isle.hint}")
        .text_style(|t| t.semibold().shadow().nowrap().size(15.0))
        .pick_ignore();
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(0.0)
                        .right(0.0)
                        .bottom(14.0)
                        .direction(engine_ui::style::Direction::Column)
                        .align_items(engine_ui::style::Align::Center)
                        .gap(10.0)
                        .pick(Pick::Ignore)
                })
                .child(hint)
                .child(panel),
        )
}

fn info_card(icon: &str) -> El {
    let line = |i: usize| {
        El::text(format!("{{isle.sel.l{i}}}"))
            .text_style(|t| t.nowrap().size(14.0))
            .pick_ignore()
    };
    let card = El::new()
        .class("panel")
        .with_style(|s| {
            s.position(Position::Absolute)
                .right(16.0)
                .top(84.0)
                .width(330.0)
                .padding(16.0)
                .gap(6.0)
                .direction(engine_ui::style::Direction::Column)
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(engine_ui::style::Align::Center))
                .child(El::icon(icon).with_style(|s| s.size(34.0, 34.0).pick(Pick::Ignore)))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.grow(1.0))
                        .child(El::text("{isle.sel.title}").class("heading").pick_ignore())
                        .child(caption("{isle.sel.sub}")),
                )
                .child(engine_ui::widgets::close_button().on_click("isle.deselect")),
        )
        .child(engine_ui::widgets::divider())
        .children((0..6).map(line))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).margin([6.0, 0.0, 0.0, 0.0]))
                .child(El::button("Pause", "isle.sel.pause").class("ghost"))
                .child(El::button("Demolish", "isle.sel.demolish").class("ghost danger")),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(card)
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

/// Add the icons, open the bars.
pub fn setup(world: &mut World) {
    if let Some(mut lib) = world.get_resource_mut::<engine_ui::icons::IconLibrary>() {
        crate::icons::install(&mut lib);
    }
    world.init_resource::<Hud>();
    update_data_now(world);
    if let Err(e) = screens::spawn_screen(world, hud_screen(TOP, 0), &top_bar()) {
        engine::core::log::warn!("archipelago top bar: {e}");
    }
    let tool = *world.resource::<Tool>();
    if let Err(e) = screens::spawn_screen(world, hud_screen(BAR, 0), &build_bar(1, tool)) {
        engine::core::log::warn!("archipelago build bar: {e}");
    }
    world.resource_mut::<Hud>().category = 1;
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
    if screens::spawn_screen(world, hud_screen(INFO, 1), &info_card(kind_icon(kind))).is_ok() {
        world.resource_mut::<Hud>().info_open = true;
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
    }
}

fn refresh_items(world: &mut World) {
    let (cat, tool) = (world.resource::<Hud>().category, *world.resource::<Tool>());
    if let Some(e) = engine_ui::input::find_id(world, "isle.items") {
        let _ = engine_ui::el::replace_children(world, e, &bar_items(cat, tool));
    }
}

fn fmt_int(x: f64) -> String {
    engine_ui::data::group_thousands(format!("{:.0}", x))
}

/// The calendar: a game minute is a month, from spring 1820.
fn clock(s: f64) -> String {
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
    let m = (s / 60.0) as u64 + 3;
    format!("{} {}", MONTHS[(m % 12) as usize], 1820 + m / 12)
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
    let mut lines: Vec<String> = vec![String::new(); 6];
    let (mut title, mut sub) = (String::new(), String::new());
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
            lines[0] = format!("Residents: {:.1} of {:.0}", h.residents, h.tier.capacity());
            let books = &econ.islands[b.island];
            let need_list: Vec<String> = needs[..basic]
                .iter()
                .map(|g| format!("{} {:.0}%", g.name(), books.supply[g.index()] * 100.0))
                .collect();
            lines[1] = format!("Needs: {}", need_list.join(", "));
            let svc: Vec<String> = h
                .tier
                .services()
                .iter()
                .enumerate()
                .map(|(i, k)| format!("{} {}", k.def().name, if h.services[i] { "✓" } else { "✗" }))
                .collect();
            lines[2] = format!("Services: {}", svc.join(", "));
            lines[3] = format!("Taxes: {:.1} coins/min", h.residents * h.tier.tax() / 1.0);
            lines[4] = match h.tier.next() {
                Some((next, cost)) => {
                    let c: Vec<String> = cost
                        .iter()
                        .map(|(g, n)| format!("{n} {}", g.name().to_lowercase()))
                        .collect();
                    format!(
                        "Moves up to {} when full and content ({})",
                        next.name(),
                        c.join(", ")
                    )
                }
                None => "Top tier for now".into(),
            };
            lines[5] = format!("Content for {:.0} s", h.content_s);
        } else if let Some(p) = world.get::<Producer>(e) {
            lines[0] = format!("Status: {}", p.status.text());
            lines[1] = format!("Output: {:.0}%", p.efficiency * 100.0);
            lines[2] = match (d.input, d.output) {
                (Some(i), Some(o)) => {
                    format!("{} → {}, one every {:.0} s", i.name(), o.name(), d.cycle_s)
                }
                (None, Some(o)) => format!("{}, one every {:.0} s", o.name(), d.cycle_s),
                _ => String::new(),
            };
            lines[3] = d.workers.map_or(String::new(), |(t, n)| {
                format!("Workers: {n} {}", t.name().to_lowercase())
            });
            lines[4] = format!("Made so far: {}", p.made);
            lines[5] = format!("Upkeep: {:.0} coins/min", d.upkeep);
        } else {
            lines[0] = d.blurb.to_string();
            if b.kind == Kind::Harbour {
                let books = &econ.islands[b.island];
                lines[1] = format!("Warehouse room: {:.0} of each good", books.capacity);
            }
        }
    }
    let total: f32 = econ.islands.iter().map(population).sum();
    let books = &econ.islands[isle];
    {
        let mut data = world.resource_mut::<UiData>();
        data.set("isle.coins", fmt_int(econ.coins));
        let b = econ.balance_per_min;
        data.set(
            "isle.balance",
            format!("{}{:.0} / min", if b >= 0.0 { "+" } else { "−" }, b.abs()),
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
        data.set("isle.hint", hint);
        data.set("isle.sel.title", title);
        data.set("isle.sel.sub", sub);
        for (i, l) in lines.into_iter().enumerate() {
            data.set(&format!("isle.sel.l{i}"), l);
        }
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
}
