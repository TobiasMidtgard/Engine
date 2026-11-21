//! The cheat panel (F10, or Cheats in the Esc menu): the cheat orders of [`crate::cheats`] as
//! buttons, to test a game faster. It opens only while the game's rules allow cheats; the
//! game goes on behind it.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Screen;
use engine_ui::screens;
use engine_ui::style::{Align, Direction, Ease, Enter, Pick, Position};
use engine_ui::toast::{self, ToastKind};

use crate::cheats;
use crate::defs::Tier;
use crate::sim::{Economy, PLAYER};

/// The panel's screen.
pub const CHEATS: &str = "fv_cheats";

/// The speeds the panel offers.
const SPEEDS: [f32; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

/// Whether this game's rules allow cheats.
pub fn allowed(world: &World) -> bool {
    world.resource::<Economy>().rules.cheats
}

/// Open the panel, or close it when it is up. With cheats off, say so instead.
pub fn toggle(world: &mut World) {
    if screens::find_screen(world, CHEATS).is_some() {
        screens::hide(world, CHEATS, false);
        return;
    }
    if !allowed(world) {
        toast::notify(
            world,
            ToastKind::Info,
            "",
            "Cheats are off",
            "This game's rules leave cheats out. Turn them on under Testing when you set up a new game.",
            6.0,
        );
        return;
    }
    open(world, "Each order works at once, for your company only.");
}

fn open(world: &mut World, note: &str) {
    let screen = Screen {
        key: CHEATS.into(),
        modal: false,
        layer: 35,
        closable: false,
        ..Default::default()
    };
    screens::hide(world, CHEATS, true);
    let el = panel(world, note);
    if let Err(e) = screens::spawn_screen(world, screen, &el) {
        engine::core::log::warn!("cheats: {e}");
    }
}

/// The player's islands.
fn mine(world: &World) -> Vec<usize> {
    let econ = world.resource::<Economy>();
    (0..econ.islands.len())
        .filter(|&i| econ.owns(PLAYER, i))
        .collect()
}

/// Handle `fv.cheat.*` (false: not a cheat action).
pub fn act(world: &mut World, a: &str) -> bool {
    let Some(order) = a.strip_prefix("fv.cheat.") else {
        return false;
    };
    if order == "close" {
        screens::hide(world, CHEATS, false);
        return true;
    }
    let isles = mine(world);
    let done = match order {
        "coins" => cheats::coins(world, 10_000.0).map(|v| format!("Purse: {} coins.", v["coins"])),
        "coins_big" => {
            cheats::coins(world, 100_000.0).map(|v| format!("Purse: {} coins.", v["coins"]))
        }
        "goods" => cheats::goods(world, &isles, &[], 100.0)
            .map(|_| "Every warehouse of yours has 100 of each good, as room allows.".to_string()),
        "chart" => cheats::chart(world)
            .map(|v| format!("The fog is gone: {} islands charted.", v["islands"])),
        "unlock" => {
            let on = !world.resource::<Economy>().rules.unlocked;
            cheats::unlock(world, on).map(|_| {
                if on {
                    "Every building is open, whatever your people.".to_string()
                } else {
                    "Buildings open with your people again.".to_string()
                }
            })
        }
        "ships" => {
            cheats::ships(world).map(|v| format!("Ships finished on {} slips.", v["finished"]))
        }
        "houses" => cheats::houses(world, &isles, false, Tier::Patricians)
            .map(|v| format!("{} houses filled.", v["filled"])),
        "up" => cheats::houses(world, &isles, true, Tier::Patricians)
            .map(|v| format!("{} houses moved up a tier.", v["moved_up"])),
        "friends" => {
            let ids: Vec<u32> = world
                .get_resource::<crate::rivals::Rivals>()
                .map(|r| r.0.iter().map(|x| x.id).collect())
                .unwrap_or_default();
            if ids.is_empty() {
                Err("There are no rival houses in this game.".to_string())
            } else {
                ids.iter()
                    .try_for_each(|id| cheats::standing(world, *id, 80).map(|_| ()))
                    .map(|_| {
                        format!(
                            "{} rival houses think well of you (standing 80).",
                            ids.len()
                        )
                    })
            }
        }
        _ => match order
            .strip_prefix("speed:")
            .and_then(|s| s.parse::<f32>().ok())
        {
            Some(s) => cheats::speed(world, s).map(|_| format!("Speed {s}x.")),
            None => return true,
        },
    };
    let note = done.unwrap_or_else(|e| e);
    open(world, &note);
    true
}

fn button(label: &str, order: &str) -> El {
    El::new()
        .class("brass_button")
        .on_click(format!("fv.cheat.{order}"))
        .with_style(|s| s.justify(engine_ui::style::Justify::Start))
        .child(El::text(label).text_style(|t| t.nowrap()).pick_ignore())
}

fn panel(world: &World, note: &str) -> El {
    let unlocked = world.resource::<Economy>().rules.unlocked;
    let speed = world.resource::<Economy>().speed;
    let speeds = El::new()
        .with_style(|s| {
            s.direction(Direction::Row)
                .gap(5.0)
                .align_items(Align::Center)
        })
        .child(
            El::text("Speed")
                .text_style(|t| t.nowrap().shadow())
                .with_style(|s| s.font_size(16.0).color(Colors::cream()).width(64.0))
                .pick_ignore(),
        )
        .children(SPEEDS.iter().map(|&s| {
            El::new()
                .class("fv_chip")
                .selected((speed - s).abs() < 0.01)
                .on_click(format!("fv.cheat.speed:{s}"))
                .child(
                    El::text(format!("{s}x"))
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                )
        }));
    let body = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .width(400.0)
                .padding([18.0, 20.0, 20.0, 20.0])
                .gap(7.0)
                .enter(Enter::pop(0.18, 0.97).ease(Ease::Out))
        })
        .child(
            El::new()
                .with_style(|s| {
                    s.direction(Direction::Row)
                        .align_items(Align::Center)
                        .gap(10.0)
                        .margin([0.0, 0.0, 4.0, 0.0])
                })
                .child(brass::label("CHEATS", false))
                .child(El::new().with_style(|s| s.grow(1.0).pick(Pick::Ignore)))
                .child(crate::title::key("F10", false))
                .child(
                    El::new()
                        .class("brass_button")
                        .on_click("fv.cheat.close")
                        .child(El::text("Close").pick_ignore()),
                ),
        )
        .child(
            El::new()
                .with_style(|s| s.direction(Direction::Row).gap(6.0))
                .child(button("+10,000 coins", "coins"))
                .child(button("+100,000", "coins_big")),
        )
        .child(button("Fill the warehouses", "goods"))
        .child(button("Reveal the map", "chart"))
        .child(button(
            if unlocked {
                "Close the buildings again"
            } else {
                "Open every building"
            },
            "unlock",
        ))
        .child(button("Finish ships on the slips", "ships"))
        .child(button("Fill the houses", "houses"))
        .child(button("Move every house up a tier", "up"))
        .child(button("Befriend every rival house", "friends"))
        .child(speeds)
        .child(
            El::text(note)
                .text_style(|t| t.shadow().line_height(1.3))
                .with_style(|s| {
                    s.font_size(15.0)
                        .color(Colors::cream_dim())
                        .margin([4.0, 0.0, 0.0, 0.0])
                })
                .pick_ignore(),
        );
    El::new()
        .class("screen")
        .with_style(|s| s.pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| s.position(Position::Absolute).right(36.0).top(120.0))
                .child(body),
        )
}
