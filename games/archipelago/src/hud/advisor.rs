//! The harbour master's advice, bottom left: the few things most worth doing now, each a line
//! with its picture and the one button that does it. A shortage offers the building that
//! answers it (the view glides to the island and the building is in hand), a store running dry
//! opens the ledger on it, ready houses move up, idle works are shown. Worst first. A cross
//! puts one piece of advice away until the trouble changes, and the notebook folds to its
//! title.
//!
//! Numbers are the game's (`Economy`, `House`, `reports`); this is the UI thread's.

use std::collections::HashSet;

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Screen;
use engine_ui::style::{Align, Direction, Enter, Fill, Justify, Pick, Position, Shadow};

use crate::defs::{Cat, Good, Kind, Need, Tier};
use crate::island;
use crate::sim::{Building, Economy, House, Producer, Wait};

pub const SCREEN: &str = "isle_advice";
const LIST_ID: &str = "isle.advice";
/// Pieces of advice shown folded open.
const SHOWN: usize = 3;

#[derive(Resource, Default)]
pub struct Advisor {
    /// Put away by the player (by key; back once the advice changes).
    hidden: HashSet<String>,
    /// Folded to its title.
    pub folded: bool,
    /// Showing every piece, not only the first few.
    more: bool,
    sig: String,
}

/// One piece of advice.
#[derive(Clone, Debug)]
struct Advice {
    /// Identity, for putting it away (includes what would make it new advice).
    key: String,
    icon: &'static str,
    /// Worst first.
    weight: f32,
    title: String,
    text: String,
    button: String,
    action: String,
    /// Red (act now) or amber.
    urgent: bool,
}

pub fn install(world: &mut World) {
    world.init_resource::<Advisor>();
    let screen = Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 1,
        closable: false,
        ..Default::default()
    };
    let el = El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(El::new().id(LIST_ID).with_style(|s| {
            s.position(Position::Absolute)
                .left(22.0)
                .bottom(22.0)
                .width(420.0)
                .direction(Direction::Column)
                .gap(0.0)
                .pick(Pick::Ignore)
        }));
    if let Err(e) = engine_ui::screens::spawn_screen(world, screen, &el) {
        engine::core::log::warn!("archipelago advice: {e}");
    }
}

fn low(s: &str) -> String {
    s.to_lowercase()
}

fn settled(econ: &Economy) -> Vec<usize> {
    econ.islands
        .iter()
        .enumerate()
        .filter(|(_, b)| b.population.iter().sum::<f32>() > 0.5)
        .map(|(i, _)| i)
        .collect()
}

/// Everything worth saying now, worst first.
fn gather(world: &mut World) -> Vec<Advice> {
    let econ = world.resource::<Economy>().clone();
    let mut out = Vec::new();
    let isles = settled(&econ);
    // The island in view (and its region) matters most: the player is looking at it.
    let here = world.get_resource::<super::Hud>().map_or(0, |h| h.island);
    let region = island::islands()[here].region();
    let near = |i: usize| {
        if i == here {
            25.0
        } else if island::islands()[i].region() == region {
            10.0
        } else {
            0.0
        }
    };
    // In debt.
    if econ.debt_s > 0.0 {
        out.push(Advice {
            key: "debt".into(),
            icon: "coin",
            weight: 100.0,
            title: "The company is in debt".into(),
            text: format!(
                "{:.0} coins a minute short. The ledger shows what costs the most.",
                -econ.balance_per_min
            ),
            button: "Ledger".into(),
            action: "isle.adv.ledger:0:all".into(),
            urgent: true,
        });
    }
    // Houses and what they lack, per island.
    let houses: Vec<(Building, House)> = world
        .query::<(&Building, &House)>()
        .iter(world)
        .map(|(b, h)| (b.clone(), h.clone()))
        .collect();
    for &i in &isles {
        let books = &econ.islands[i];
        let name = island::islands()[i].name;
        // A good the island hands out running short, or about to.
        for g in Good::ALL {
            let k = g.index();
            let used = books.used_per_min[k];
            if used < 0.05 {
                continue;
            }
            let n = super::ledger::nums(books, g);
            let supply = books.supply[k];
            if supply < 0.85 {
                let fix = super::house::fixer(Need::Good(g));
                out.push(Advice {
                    key: format!("short:{i}:{}", g.name()),
                    icon: g.icon(),
                    weight: 60.0 + used * (1.0 - supply) * 4.0,
                    title: format!("{} short on {name}", g.name()),
                    text: format!(
                        "Only {:.0}% of what houses ask for: they pay less and fill slower.",
                        supply * 100.0
                    ),
                    button: fix.map_or("Ledger".into(), |k| {
                        format!("Place a {}", low(k.def().name))
                    }),
                    action: fix.map_or(format!("isle.adv.ledger:{i}:short"), |k| {
                        format!("isle.adv.build:{i}:{}", super::house::build_action(k))
                    }),
                    urgent: supply < 0.5,
                });
            } else if let Some(m) = n.lasts.filter(|m| *m < 6.0) {
                out.push(Advice {
                    key: format!("dry:{i}:{}", g.name()),
                    icon: g.icon(),
                    weight: 70.0 - m * 3.0,
                    title: format!("{} runs out on {name}", g.name()),
                    text: format!(
                        "{} at today's pace: {:.1} more used than made each minute.",
                        if m < 1.0 {
                            "Within the minute".to_string()
                        } else {
                            format!("In about {}", super::ledger::lasts_text(m))
                        },
                        -n.net
                    ),
                    button: "Ledger".into(),
                    action: format!("isle.adv.ledger:{i}:falling"),
                    urgent: m < 2.0,
                });
            }
        }
        // Houses missing a service in reach (the square).
        let mut want: std::collections::BTreeMap<Kind, u32> = Default::default();
        for (b, h) in houses.iter().filter(|(b, _)| b.island == i) {
            if h.wait != Wait::Short(Cat::Square) && h.wait != Wait::NotFull {
                continue;
            }
            for (j, w) in h.tier.wants().iter().enumerate() {
                if let Need::Service(k) = w.need
                    && w.cat == Cat::Square
                    && books.asks(h.tier, j, w)
                    && !h.has(k)
                {
                    *want.entry(k).or_default() += 1;
                }
            }
            let _ = b;
        }
        if let Some((k, n)) = want.into_iter().max_by_key(|(_, n)| *n)
            && n >= 2
        {
            out.push(Advice {
                key: format!("service:{i}:{k:?}:{}", n / 4),
                icon: super::kind_icon(k),
                weight: 50.0 + n as f32,
                title: format!("{n} houses on {name} want a {}", low(k.def().name)),
                text: "None is in reach of them: one nearby fills them and lifts their mood."
                    .into(),
                button: format!("Place a {}", low(k.def().name)),
                action: format!("isle.adv.build:{i}:{}", super::house::build_action(k)),
                urgent: false,
            });
        }
        // Ready houses.
        let ready: u32 = books.ready.iter().sum();
        if ready > 0 {
            let by: Vec<String> = Tier::ALL
                .iter()
                .filter(|t| books.ready[t.index()] > 0)
                .map(|t| format!("{} {}", books.ready[t.index()], low(t.name())))
                .collect();
            out.push(Advice {
                key: format!("ready:{i}"),
                icon: "isle_move_up",
                weight: 40.0 + ready as f32 * 0.2,
                title: format!(
                    "{ready} {} on {name} can move up",
                    if ready == 1 { "house" } else { "houses" }
                ),
                text: format!(
                    "{}. Those whose works need the people wait.",
                    capital(&by.join(", "))
                ),
                button: "Move them up".into(),
                action: format!("isle.adv.up:{i}"),
                urgent: false,
            });
        }
    }
    // Idle works that still cost.
    for d in econ.drains.iter().filter(|d| d.idle) {
        let name = island::islands()[d.island].name;
        out.push(Advice {
            key: format!("idle:{}:{:?}:{}", d.island, d.kind, d.count),
            icon: super::kind_icon(d.kind),
            weight: 30.0 + d.per_min * 0.3,
            title: format!(
                "{} idle {} on {name}",
                d.count,
                plural(d.kind.def().name, d.count)
            ),
            text: format!(
                "They cost {:.0} a minute and make nothing: pause them or find them goods.",
                d.per_min
            ),
            button: "Go there".into(),
            action: format!("isle.adv.go:{}:{:?}", d.island, d.kind),
            urgent: false,
        });
    }
    for a in &mut out {
        if let Some(i) = a
            .key
            .split(':')
            .nth(1)
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|i| *i < island::islands().len())
        {
            a.weight += near(i);
        }
    }
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn plural(name: &str, n: u32) -> String {
    let name = low(name);
    match name.strip_suffix('y') {
        _ if n == 1 || name.ends_with('s') => name,
        Some(stem) if !stem.ends_with(['a', 'e', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{name}s"),
    }
}

fn line(a: &Advice) -> El {
    let c = if a.urgent {
        Color::hex(0xf0583a)
    } else {
        Color::hex(0xf0b040)
    };
    El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(10.0)
                .align_items(Align::Center)
                .padding([8.0, 8.0, 8.0, 12.0])
                .radius(6.0)
                .background(Fill::Solid(Color::hexa(0x0a121cb0)))
                .border(1.0, c.with_alpha(0.35))
        })
        .child(El::new().with_style(move |s| {
            s.position(Position::Absolute)
                .left(3.0)
                .top(8.0)
                .bottom(8.0)
                .width(3.0)
                .radius(1.5)
                .background(Fill::Solid(c))
                .pick(Pick::Ignore)
        }))
        .child(brass::badge(a.icon, 36.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(a.title.clone())
                        .text_style(|t| t.size(15.0).bold())
                        .with_style(|s| s.color(Colors::cream()))
                        .pick_ignore(),
                )
                .child(
                    El::text(a.text.clone())
                        .text_style(|t| t.size(12.5))
                        .with_style(|s| s.color(Colors::cream_dim()))
                        .pick_ignore(),
                ),
        )
        .child(
            El::button(a.button.clone(), a.action.clone())
                .class("brass_button")
                .text_style(|t| t.nowrap().size(13.5))
                .with_style(|s| s.shrink(0.0).padding([5.0, 10.0])),
        )
        .child(
            El::text("×")
                .on_click(format!("isle.adv.hide:{}", a.key))
                .tooltip("", "Put this away until it changes")
                .text_style(|t| t.size(18.0).bold())
                .with_style(|s| s.color(Colors::cream_dim()).padding([0.0, 2.0])),
        )
}

fn notebook(list: &[Advice], folded: bool, more: bool) -> Vec<El> {
    let n = list.len();
    let head = El::new()
        .class("row")
        .on_click("isle.adv.fold")
        .with_style(|s| {
            s.gap(10.0)
                .align_items(Align::Center)
                .justify(Justify::SpaceBetween)
                .padding([8.0, 14.0])
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Center))
                .child(
                    El::icon("isle_compass").with_style(|s| s.size(24.0, 24.0).pick(Pick::Ignore)),
                )
                .child(
                    El::text("Next moves")
                        .class("brass_title")
                        .text_style(|t| t.size(19.0).nowrap().shadow())
                        .pick_ignore(),
                )
                .child_opt((n > 0).then(|| {
                    El::text(n.to_string())
                        .text_style(|t| t.size(12.5).bold().tabular())
                        .with_style(|s| {
                            s.color(Color::hex(0x1a1008))
                                .padding([1.0, 7.0])
                                .radius(9.0)
                                .background(Fill::Solid(Colors::brass_light()))
                        })
                        .pick_ignore()
                })),
        )
        .child(
            El::text(if folded { "▸ open" } else { "▾ fold" })
                .class("brass_label")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        );
    let mut body = El::new()
        .class("col")
        .with_style(|s| s.gap(6.0).padding([0.0, 10.0, 10.0, 10.0]));
    if !folded {
        if list.is_empty() {
            body = body.child(
                El::text("All is well: nothing needs you just now.")
                    .text_style(|t| t.size(13.5).italic())
                    .with_style(|s| s.color(Colors::cream_dim()).padding([2.0, 6.0]))
                    .pick_ignore(),
            );
        }
        let take = if more {
            list.len()
        } else {
            SHOWN.min(list.len())
        };
        body = body.children(list.iter().take(take).map(line));
        if list.len() > SHOWN {
            body = body.child(
                El::text(if more {
                    "show fewer".to_string()
                } else {
                    format!("and {} more", list.len() - SHOWN)
                })
                .class("brass_label")
                .on_click("isle.adv.more")
                .with_style(|s| s.padding([2.0, 8.0])),
            );
        }
    }
    vec![
        brass::plate()
            .with_style(|s| {
                s.direction(Direction::Column)
                    .gap(0.0)
                    .padding(0.0)
                    .shadow(Shadow::new(Color::hexa(0x00000090), 14.0, 4.0))
                    .enter(Enter::default())
            })
            .child(head)
            .child(body),
    ]
}

/// Rebuild the notebook when what it says changes.
pub fn refresh(world: &mut World) {
    if !world.contains_resource::<Advisor>() {
        return;
    }
    let all = gather(world);
    let hidden = world.resource::<Advisor>().hidden.clone();
    let list: Vec<Advice> = all
        .into_iter()
        .filter(|a| !hidden.contains(&a.key))
        .collect();
    let (folded, more) = {
        let a = world.resource::<Advisor>();
        (a.folded, a.more)
    };
    let sig = format!(
        "{folded}{more}|{}",
        list.iter()
            .map(|a| format!("{}|{}|{}", a.key, a.title, a.text))
            .collect::<Vec<_>>()
            .join(";")
    );
    if world.resource::<Advisor>().sig == sig {
        return;
    }
    let Some(node) = engine_ui::input::find_id(world, LIST_ID) else {
        return;
    };
    let _ = engine_ui::el::replace_children(world, node, &notebook(&list, folded, more));
    world.resource_mut::<Advisor>().sig = sig;
}

/// `isle.adv.*` buttons. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    let Some(rest) = a.strip_prefix("isle.adv.") else {
        return false;
    };
    let (verb, args) = rest.split_once(':').unwrap_or((rest, ""));
    match verb {
        "fold" => {
            let mut s = world.resource_mut::<Advisor>();
            s.folded = !s.folded;
        }
        "more" => {
            let mut s = world.resource_mut::<Advisor>();
            s.more = !s.more;
        }
        "hide" => {
            world
                .resource_mut::<Advisor>()
                .hidden
                .insert(args.to_string());
        }
        "build" => {
            // "<island>:isle.build:<kind>": glide to the island and pick the building up.
            if let Some((i, build)) = args.split_once(':')
                && let Ok(i) = i.parse::<usize>()
            {
                look_island(world, i);
                crate::tools::action(world, build);
            }
        }
        "ledger" => {
            let mut p = args.split(':');
            let i = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let f = p.next().unwrap_or("all");
            super::ledger::open_filtered(world, i, f);
        }
        "up" => {
            if let Ok(i) = args.parse::<usize>() {
                super::ledger::up_all(world, i);
            }
        }
        "go" => {
            let mut p = args.split(':');
            let i: usize = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let kind = p.next().unwrap_or("");
            let found = world
                .query::<(Entity, &Building, &Producer)>()
                .iter(world)
                .filter(|(_, b, pr)| {
                    b.island == i && format!("{:?}", b.kind) == kind && pr.idle_s > 0.0
                })
                .map(|(e, b, _)| (e, b.clone()))
                .next();
            if let Some((e, b)) = found {
                let at = crate::grid::footprint_center(b.kind, b.origin, b.rot);
                super::map::look_at(world, at);
                world.resource_mut::<crate::tools::Hand>().selected = Some(e);
                super::show_card(world);
            } else {
                look_island(world, i);
            }
        }
        _ => return false,
    }
    refresh(world);
    true
}

fn look_island(world: &mut World, i: usize) {
    let c = island::islands()[i].center;
    // The home town's harbour rather than the middle of the hills, when it is this island.
    let at = world
        .query::<&Building>()
        .iter(world)
        .find(|b| b.island == i && b.kind == Kind::Harbour)
        .map(|b| crate::grid::footprint_center(b.kind, b.origin, b.rot))
        .unwrap_or(c);
    super::map::look_at(world, at);
}
