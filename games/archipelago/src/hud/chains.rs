//! The production chains book (K): pick a good on the left, and its whole chain is drawn as
//! a tree that reads left to right, raw goods first, the good itself last. Each link is the
//! building that makes it, with its rate and how many of it one building of the next link
//! needs (×2 means two sheep farms keep one weaver busy). Click any link to put that building
//! in hand. Deep chains (four or five links, two or three inputs each) nest the same way.

use engine::prelude::*;
use engine_ui::color::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Direction, Enter, Justify, Pick, Position, Shadow};

use crate::defs::{DEFS, Good, Kind, Need, Tier};

pub const SCREEN: &str = "isle_chains";
const TREE_ID: &str = "isle.chain.tree";

#[derive(Resource, Default)]
pub struct Chains {
    pub open: bool,
    pub picked: Option<Good>,
}

/// The building that makes good `g`.
pub fn maker(g: Good) -> Option<Kind> {
    DEFS.iter().find(|d| d.output == Some(g)).map(|d| d.kind)
}

/// What a building needs per batch.
fn inputs(k: Kind) -> Vec<Good> {
    let d = k.def();
    d.input.into_iter().chain(d.input2).collect()
}

/// Goods made per minute by one building at full strength.
fn rate(k: Kind) -> f32 {
    60.0 / k.def().cycle_s.max(0.1)
}

/// The goods worth a chain, grouped: what each tier asks for, then building materials and
/// the rest.
fn groups() -> Vec<(String, Vec<Good>)> {
    let mut seen: Vec<Good> = Vec::new();
    let mut out = Vec::new();
    for t in Tier::ALL {
        let gs: Vec<Good> = t
            .wants()
            .iter()
            .filter_map(|w| match w.need {
                Need::Good(g) => Some(g),
                _ => None,
            })
            .filter(|g| !seen.contains(g))
            .collect();
        seen.extend(&gs);
        if !gs.is_empty() {
            out.push((format!("{} want", t.name()), gs));
        }
    }
    let building: Vec<Good> = [Good::Planks, Good::Bricks, Good::Glass]
        .into_iter()
        .filter(|g| !seen.contains(g))
        .collect();
    seen.extend(&building);
    out.push(("For building".into(), building));
    let rest: Vec<Good> = Good::ALL
        .into_iter()
        .filter(|g| !seen.contains(g) && maker(*g).is_some())
        .collect();
    if !rest.is_empty() {
        out.push(("Everything else".into(), rest));
    }
    out
}

/// One link: the building, the good it makes, its rate, and how many of it the next link
/// needs.
fn link(g: Good, k: Option<Kind>, times: Option<f32>, root: bool) -> El {
    let gold = Colors::brass();
    let (title, sub) = match k {
        Some(k) => (
            k.def().name.to_string(),
            format!("{} · {:.1} a minute", g.name(), rate(k)),
        ),
        None => (g.name().to_string(), "nobody makes it".into()),
    };
    let tier = k.map(super::plan::tier_of);
    let mut card = El::new()
        .class("brass_tile")
        .with_style(move |s| {
            s.direction(Direction::Row)
                .gap(8.0)
                .align_items(Align::Center)
                .padding([6.0, 12.0, 6.0, 6.0])
                .min_width(196.0)
        })
        .child(
            El::new()
                .with_style(|s| s.pick(Pick::Ignore))
                .child(brass::badge(
                    k.map_or(g.icon(), super::kind_icon),
                    if root { 50.0 } else { 42.0 },
                ))
                .child(
                    El::new()
                        .with_style(|s| {
                            s.position(Position::Absolute)
                                .right(-6.0)
                                .bottom(-4.0)
                                .pick(Pick::Ignore)
                        })
                        .child(brass::badge(g.icon(), 22.0)),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0).grow(1.0).pick(Pick::Ignore))
                .child(
                    El::text(title)
                        .class("ink")
                        .text_style(|t| t.bold().size(14.5).nowrap())
                        .pick_ignore(),
                )
                .child(
                    El::text(sub)
                        .class("ink")
                        .text_style(|t| t.size(12.0).nowrap())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                ),
        )
        .child_opt(tier.map(|t| {
            brass::badge(super::plan::tier_icon(t), 24.0)
                .tooltip(t.name(), "The people who work it.")
        }))
        .child_opt(times.filter(|x| (*x - 1.0).abs() > 0.05).map(|x| {
            El::text(format!("×{}", fmt_times(x)))
                .text_style(|t| t.bold().size(16.0).nowrap().tabular())
                .with_style(|s| {
                    s.color(Colors::cream())
                        .padding([2.0, 8.0])
                        .radius(10.0)
                        .background(Color::hex(0x7a4a14))
                        .pick(Pick::Ignore)
                })
                .pick_ignore()
        }));
    if root {
        card = card.with_style(move |s| {
            s.border(2.0, gold)
                .shadow(Shadow::new(gold.with_alpha(0.5), 10.0, 0.0))
        });
    }
    match k {
        Some(k) => card.on_click(super::house::build_action(k)).tooltip(
            k.def().name,
            format!(
                "Makes {}.{}\n[i]Click to place one.[/i]",
                g.name().to_lowercase(),
                times
                    .filter(|x| (*x - 1.0).abs() > 0.05)
                    .map(|x| format!(
                        " About {} of these keep one of the next link busy.",
                        fmt_times(x)
                    ))
                    .unwrap_or_default()
            ),
        ),
        None => card,
    }
}

fn fmt_times(x: f32) -> String {
    if (x - x.round()).abs() < 0.05 {
        format!("{:.0}", x.round())
    } else {
        format!("{x:.1}")
    }
}

/// Good `g`'s chain as a tree: its inputs' chains stacked on the left, joined by a bracket
/// to the link that makes it. `times` is how many of this link one of the next needs.
fn tree(g: Good, times: Option<f32>, root: bool, depth: usize) -> El {
    let k = maker(g);
    let node = link(g, k, times, root);
    let ins = k.map(inputs).unwrap_or_default();
    if ins.is_empty() || depth > 6 {
        return node;
    }
    let me = k.map(rate).unwrap_or(1.0);
    let kids: Vec<El> = ins
        .iter()
        .map(|i| {
            let each = maker(*i).map(rate).unwrap_or(1.0);
            tree(*i, Some(me / each.max(0.01)), false, depth + 1)
        })
        .collect();
    let line = Colors::ink().with_alpha(0.55);
    El::new()
        .class("row")
        .with_style(|s| s.gap(0.0).align_items(Align::Center))
        .child(
            El::new()
                .class("col")
                .with_style(move |s| s.gap(10.0).padding([0.0, 14.0, 0.0, 0.0]).border(0.0, line))
                .children(kids.into_iter().map(move |c| {
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(0.0).align_items(Align::Center))
                        .child(c)
                        .child(El::new().with_style(move |s| {
                            s.width(14.0)
                                .height(2.5)
                                .background(line)
                                .pick(Pick::Ignore)
                        }))
                }))
                .child_opt((ins.len() > 1).then(|| {
                    El::new().with_style(move |s| {
                        s.position(Position::Absolute)
                            .right(12.0)
                            .top(28.0)
                            .bottom(28.0)
                            .width(2.5)
                            .background(line)
                            .pick(Pick::Ignore)
                    })
                })),
        )
        .child(El::new().with_style(move |s| {
            s.width(20.0)
                .height(2.5)
                .background(line)
                .pick(Pick::Ignore)
        }))
        .child(El::icon("isle_arrow").with_style(|s| s.size(26.0, 16.0).pick(Pick::Ignore)))
        .child(node)
}

fn good_button(g: Good, picked: bool) -> El {
    El::new()
        .class("brass_tile")
        .on_click(format!("isle.chain.pick:{}", g.index()))
        .selected(picked)
        .with_style(|s| {
            s.direction(Direction::Column)
                .align_items(Align::Center)
                .gap(1.0)
                .padding([5.0, 3.0])
                .width(78.0)
        })
        .child(brass::badge(g.icon(), 34.0))
        .child(
            El::text(g.name())
                .class("ink")
                .text_style(|t| t.size(11.5).bold().center().nowrap())
                .pick_ignore(),
        )
}

const PICKER_ID: &str = "isle.chain.picker";

fn picker(picked: Good) -> El {
    El::new()
        .class("col")
        .id(PICKER_ID)
        .with_style(|s| s.gap(10.0).width(268.0))
        .children(groups().into_iter().map(move |(name, gs)| {
            El::new()
                .class("col")
                .with_style(|s| s.gap(4.0))
                .child(brass::label(&name, true))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(6.0).wrap(true))
                        .children(gs.into_iter().map(move |g| good_button(g, g == picked))),
                )
        }))
}

fn page(picked: Good) -> Vec<El> {
    let total = count(picked);
    vec![
        El::new()
            .class("row")
            .with_style(|s| s.gap(10.0).align_items(Align::Center))
            .child(brass::badge(picked.icon(), 40.0))
            .child(
                El::text(picked.name())
                    .class("ink_title")
                    .text_style(|t| t.nowrap())
                    .pick_ignore(),
            )
            .child(
                El::text(format!(
                    "{total} buildings in the chain · click one to place it"
                ))
                .class("ink_label")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
            ),
        El::new()
            .class("scroll")
            .with_style(|s| {
                s.max_height(640.0)
                    .padding([6.0, 4.0])
                    .justify(Justify::Center)
            })
            .child(tree(picked, None, true, 0)),
    ]
}

/// How many links a chain has.
fn count(g: Good) -> usize {
    match maker(g) {
        Some(k) => 1 + inputs(k).into_iter().map(count).sum::<usize>(),
        None => 0,
    }
}

fn el(picked: Good) -> El {
    let book = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(12.0)
                .padding([16.0, 22.0, 20.0, 22.0])
                .max_width(1560.0)
                .enter(Enter::rise(0.3, 30.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).align_items(Align::Center))
                .child(brass::divider(70.0))
                .child(
                    El::text("Production chains")
                        .class("brass_title")
                        .text_style(|t| t.nowrap().shadow().size(30.0))
                        .pick_ignore(),
                )
                .child(brass::divider(70.0))
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    brass::medallion("isle_close", 36.0, "isle.chains")
                        .tooltip("", "Close [color=#c9b07a](K or Esc)[/color]"),
                ),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).align_items(Align::Start))
                .child(
                    brass::parchment()
                        .with_style(|s| s.padding(12.0))
                        .child(picker(picked)),
                )
                .child(
                    brass::parchment()
                        .id(TREE_ID)
                        .with_style(|s| {
                            s.padding([14.0, 18.0])
                                .gap(10.0)
                                .direction(Direction::Column)
                                .min_width(760.0)
                                .min_height(420.0)
                        })
                        .children(page(picked)),
                ),
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

/// Open the book (on good `g`, or the last one shown), or close it.
pub fn toggle(world: &mut World, g: Option<Good>) {
    if world.resource::<Chains>().open && g.is_none() {
        engine_ui::screens::hide(world, SCREEN, true);
        world.resource_mut::<Chains>().open = false;
        return;
    }
    if world.resource::<Chains>().open {
        engine_ui::screens::hide(world, SCREEN, true);
    }
    let picked = g
        .or(world.resource::<Chains>().picked)
        .unwrap_or(Good::Clothes);
    world.resource_mut::<Chains>().picked = Some(picked);
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    if engine_ui::screens::spawn_screen(world, screen, &el(picked)).is_ok() {
        world.resource_mut::<Chains>().open = true;
    }
}

/// `isle.chains`, `isle.chain.pick:<good>`, and a click on a link (it puts the building in
/// hand and closes the book). Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a == "isle.chains" {
        toggle(world, None);
        return true;
    }
    if let Some(g) = a
        .strip_prefix("isle.chain.pick:")
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|i| Good::ALL.get(i).copied())
    {
        world.resource_mut::<Chains>().picked = Some(g);
        // In place: the picker's highlight and the tree.
        let groups = vec![picker(g)];
        if let Some(e) = engine_ui::input::find_id(world, PICKER_ID)
            && let Some(parent) = world.get::<ChildOf>(e).map(|c| c.parent())
        {
            let _ = engine_ui::el::replace_children(world, parent, &groups);
        }
        if let Some(e) = engine_ui::input::find_id(world, TREE_ID) {
            let _ = engine_ui::el::replace_children(world, e, &page(g));
        }
        return true;
    }
    if let Some(g) = a
        .strip_prefix("isle.chain.open:")
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|i| Good::ALL.get(i).copied())
    {
        toggle(world, Some(g));
        return true;
    }
    false
}

/// A link was clicked while the book was open: close it so the map is free to build on.
pub fn built_from_book(world: &mut World) {
    if world.resource::<Chains>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        world.resource_mut::<Chains>().open = false;
    }
}
