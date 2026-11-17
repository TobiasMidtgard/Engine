//! The house card's needs as a picture: one badge per need (goods, then services), ringed
//! green when it is met, amber when partly, red when missing, with the people and coins a
//! minute it brings under it; luxuries (no people) in their own row. Plus meters for the
//! residents and the income.
//!
//! Numbers are the game's (`Tier::wants`, `House`); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Fill, Justify, Pick, Shadow, pct};

use crate::defs::Need;
use crate::sim::{Building, Economy, House};
use crate::tools::Hand;

/// The card's needs section, filled by [`refresh`].
pub const CARD_ID: &str = "isle.sel.house";

/// What the section last showed (rebuilt only when it changes).
#[derive(Resource, Default)]
pub struct HouseCard {
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<HouseCard>();
}

pub fn card_placeholder() -> El {
    El::new()
        .class("col")
        .id(CARD_ID)
        .with_style(|s| s.gap(6.0))
}

/// One need as it stands for this house.
struct Shown {
    icon: &'static str,
    name: String,
    /// 0..1: how well it is met.
    met: f32,
    people: f32,
    coins: f32,
}

pub(super) fn ring(met: f32) -> Color {
    if met >= 0.99 {
        Color::hex(0x4f9a3c)
    } else if met > 0.05 {
        Color::hex(0xd99a2a)
    } else {
        Color::hex(0xb03a24)
    }
}

/// A need's badge: ringed in its state, the coins it brings (or could) under it.
fn need_badge(n: &Shown) -> El {
    let c = ring(n.met);
    let earned = n.coins * n.met;
    let tip = if n.people > 0.0 {
        format!(
            "Brings {:.0} people into a full house and pays {:.1} coins a minute.\nMet {:.0}% now: {:+.1} coins a minute.",
            n.people,
            n.coins,
            n.met * 100.0,
            earned
        )
    } else {
        format!(
            "A luxury: nobody leaves without it, it only pays, {:.1} coins a minute.\nMet {:.0}% now: {:+.1} coins a minute.",
            n.coins,
            n.met * 100.0,
            earned
        )
    };
    let missing = n.met <= 0.05;
    El::new()
        .class("col")
        .with_style(|s| s.gap(2.0).align_items(Align::Center).width(42.0))
        .tooltip(&n.name, tip)
        .child(
            El::new()
                .class("brass_badge")
                .with_style(move |s| {
                    s.size(36.0, 36.0)
                        .border(2.5, c)
                        .shadow(Shadow::new(c.with_alpha(0.45), 6.0, 0.0))
                        .opacity(if missing { 0.7 } else { 1.0 })
                })
                .child(El::icon(n.icon).with_style(move |s| {
                    s.size(26.0, 26.0)
                        .opacity(if missing { 0.45 } else { 1.0 })
                        .pick(Pick::Ignore)
                })),
        )
        .child(
            El::text(if missing {
                format!("+{:.1}", n.coins)
            } else {
                format!("+{earned:.1}")
            })
            .class("ink")
            .text_style(|t| t.size(13.0).bold().tabular().nowrap())
            .with_style(move |s| {
                s.color(if missing {
                    Colors::ink_dim()
                } else {
                    Colors::good_ink()
                })
            })
            .pick_ignore(),
        )
        .child_opt((n.people > 0.0).then(|| {
            El::text(format!("{:.0} ppl", n.people))
                .class("ink_label")
                .text_style(|t| t.nowrap())
                .pick_ignore()
        }))
}

/// A labelled meter: `label` on the left, `value` on the right, the bar between.
pub(super) fn meter(label: &str, frac: f32, value: String, fill: Color) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).align_items(Align::Center))
        .child(brass::label(label, true).with_style(|s| s.width(72.0)))
        .child(
            El::new()
                .class("brass_track")
                .with_style(|s| s.grow(1.0).height(12.0))
                .child(El::new().with_style(move |s| {
                    s.width(pct((frac.clamp(0.0, 1.0)) * 100.0))
                        .height(pct(100.0))
                        .radius(6.0)
                        .background(Fill::vertical(fill.lerp(Color::hex(0xffffff), 0.25), fill))
                        .pick(Pick::Ignore)
                })),
        )
        .child(
            El::text(value)
                .class("ink")
                .text_style(|t| t.size(14.0).bold().tabular().nowrap())
                .with_style(|s| s.width(92.0))
                .pick_ignore(),
        )
}

fn section(h: &House, needs: &[Shown]) -> Vec<El> {
    let (basic, luxury): (Vec<&Shown>, Vec<&Shown>) = needs.iter().partition(|n| n.people > 0.0);
    let row = |list: Vec<&Shown>| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(3.0).wrap(true).justify(Justify::Center))
            .children(list.into_iter().map(need_badge))
    };
    let mut out = vec![
        meter(
            "Residents",
            h.residents / h.tier.capacity().max(1.0),
            format!("{:.0} of {:.0}", h.residents, h.tier.capacity()),
            Color::hex(0x3f6e9a),
        ),
        meter(
            "Income",
            h.income / h.tier.full_income().max(0.01),
            format!("{:.1} of {:.1}", h.income, h.tier.full_income()),
            Color::hex(0xb8892a),
        ),
        brass::label("Needs · people and coins a minute each brings", true),
        row(basic),
    ];
    if !luxury.is_empty() {
        out.push(brass::label("Luxuries · they only pay", true));
        out.push(row(luxury));
    }
    out
}

/// Fill or update the section for the selected house.
pub fn refresh(world: &mut World) {
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return;
    };
    let (Some(h), Some(b)) = (world.get::<House>(e).cloned(), world.get::<Building>(e)) else {
        return;
    };
    let island = b.island;
    let Some(books) = world.resource::<Economy>().islands.get(island).cloned() else {
        return;
    };
    let services = h.tier.services();
    let shown: Vec<Shown> = h
        .tier
        .wants()
        .iter()
        .map(|w| match w.need {
            Need::Good(g) => Shown {
                icon: g.icon(),
                name: g.name().to_string(),
                met: books.supply[g.index()].clamp(0.0, 1.0),
                people: w.people,
                coins: w.coins,
            },
            Need::Service(k) => Shown {
                icon: super::kind_icon(k),
                name: k.def().name.to_string(),
                met: services
                    .iter()
                    .position(|s| *s == k)
                    .and_then(|i| h.services.get(i))
                    .map_or(0.0, |on| if *on { 1.0 } else { 0.0 }),
                people: w.people,
                coins: w.coins,
            },
        })
        .collect();
    let sig = format!(
        "{e:?}|{:.0}|{:.1}|{}",
        h.residents,
        h.income,
        shown
            .iter()
            .map(|n| format!("{:.2}", n.met))
            .collect::<Vec<_>>()
            .join(",")
    );
    if world.resource::<HouseCard>().sig == sig {
        return;
    }
    let els = section(&h, &shown);
    if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
        let _ = engine_ui::el::replace_children(world, node, &els);
        world.resource_mut::<HouseCard>().sig = sig;
    }
}

/// A new card was opened: fill the section afresh.
pub fn card_opened(world: &mut World) {
    world.resource_mut::<HouseCard>().sig.clear();
    refresh(world);
}
