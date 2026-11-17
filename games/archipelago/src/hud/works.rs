//! A works' card as a picture: a ribbon saying whether it runs (and what stops it), the goods
//! going in ringed by what the warehouse holds of them, the good coming out with its rate,
//! this batch's progress, and meters for the workers and the site.
//!
//! Numbers are the game's (`Producer`, `Def`, items' `Boost`); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Justify, Pick, Shadow, pct};

use crate::defs::{Good, Kind};
use crate::items::Boost;
use crate::sim::{Building, Economy, Producer, Status};
use crate::tools::Hand;

use super::house::{meter, ring};

/// The card's works section, filled by [`refresh`].
pub const CARD_ID: &str = "isle.sel.works";

/// What the section last showed (rebuilt only when it changes).
#[derive(Resource, Default)]
pub struct WorksCard {
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<WorksCard>();
}

pub fn card_placeholder() -> El {
    El::new()
        .class("col")
        .id(CARD_ID)
        .with_style(|s| s.gap(8.0))
}

/// Everything the section shows, gathered from the world.
struct Shown {
    kind: Kind,
    status: Status,
    efficiency: f32,
    progress: f32,
    made: u32,
    /// The goods a batch takes (items may swap them), with what the warehouse holds.
    inputs: Vec<(Good, f64)>,
    output: Good,
    /// Goods a minute at today's pace.
    rate: f32,
    /// Share of its jobs filled, and the tier and number it asks for.
    staff: Option<(f32, crate::defs::Tier, u32)>,
    site: f32,
    upkeep: f32,
    faster: f32,
    island: &'static str,
}

/// The ribbon's colour and the picture of what stops it.
fn status_look(s: &Shown) -> (Color, &'static str) {
    let green = Color::hex(0x4f9a3c);
    let amber = Color::hex(0xd99a2a);
    let red = Color::hex(0xb03a24);
    match s.status {
        Status::Working if s.efficiency >= 0.99 => (green, s.output.icon()),
        Status::Working => (amber, s.output.icon()),
        Status::NoWorkers if s.efficiency > 0.0 => (amber, "isle_people"),
        Status::NoWorkers => (red, "isle_people"),
        Status::NoInput => (
            red,
            s.inputs
                .iter()
                .find(|(_, n)| *n < 1.0)
                .or(s.inputs.first())
                .map_or("isle_workshop", |(g, _)| g.icon()),
        ),
        Status::NoRoad => (red, "isle_road"),
        Status::Full => (amber, "isle_anchor"),
        Status::NoTrees => (red, "isle_logs"),
        Status::NoWilds => (red, "isle_furs"),
        Status::NoFields => (red, "isle_farm"),
        Status::Paused => (Color::hex(0x6a5f4c), "isle_workshop"),
    }
}

fn ribbon(s: &Shown) -> El {
    let (c, icon) = status_look(s);
    let text = match s.status {
        Status::NoInput => {
            let short: Vec<&str> = s
                .inputs
                .iter()
                .filter(|(_, n)| *n < 1.0)
                .map(|(g, _)| g.name())
                .collect();
            if short.is_empty() {
                Status::NoInput.text().to_string()
            } else {
                format!("Waiting for {}", short.join(" and ").to_lowercase())
            }
        }
        st => st.text().to_string(),
    };
    El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(10.0)
                .align_items(Align::Center)
                .padding([5.0, 14.0, 5.0, 6.0])
                .radius(18.0)
                .background(c.with_alpha(0.16))
                .border(1.5, c.with_alpha(0.75))
        })
        .child(brass::badge(icon, 30.0).with_style(move |s| s.border(2.0, c)))
        .child(
            El::text(text)
                .class("ink")
                .text_style(|t| t.size(16.0).bold().nowrap())
                .with_style(move |s| s.grow(1.0).color(c.scale(0.7)))
                .pick_ignore(),
        )
        .child(
            El::text(format!("{:.0}%", s.efficiency * 100.0))
                .class("ink")
                .text_style(|t| t.size(18.0).bold().tabular().nowrap())
                .with_style(move |s| s.color(c.scale(0.7)))
                .pick_ignore(),
        )
}

/// A good going in: ringed green while the warehouse has some, red when it has none.
fn input_badge(g: Good, stock: f64, island: &str) -> El {
    let met = if stock >= 1.0 { 1.0 } else { 0.0 };
    let c = ring(met);
    El::new()
        .class("col")
        .with_style(|s| s.gap(2.0).align_items(Align::Center))
        .tooltip(
            g.name(),
            format!("One a batch, taken from the warehouse.\n{stock:.0} in store on {island}."),
        )
        .child(brass::badge(g.icon(), 44.0).with_style(move |s| {
            s.border(2.5, c)
                .shadow(Shadow::new(c.with_alpha(0.45), 6.0, 0.0))
        }))
        .child(
            El::text(format!("{stock:.0} in store"))
                .class("ink")
                .text_style(|t| t.size(13.0).bold().tabular().nowrap())
                .with_style(move |s| {
                    s.color(if met > 0.0 {
                        Colors::ink_dim()
                    } else {
                        Colors::bad_ink()
                    })
                })
                .pick_ignore(),
        )
}

fn chain(s: &Shown) -> El {
    let mut row = El::new().class("row").with_style(|st| {
        st.gap(8.0)
            .justify(Justify::Center)
            .align_items(Align::Start)
    });
    for (i, (g, n)) in s.inputs.iter().enumerate() {
        if i > 0 {
            row = row.child(
                El::text("+")
                    .class("ink_title")
                    .text_style(|t| t.size(20.0))
                    .with_style(|st| st.margin([10.0, 0.0, 0.0, 0.0]))
                    .pick_ignore(),
            );
        }
        row = row.child(input_badge(*g, *n, s.island));
    }
    if !s.inputs.is_empty() {
        row = row.child(El::icon("isle_arrow").with_style(|st| {
            st.size(34.0, 20.0)
                .margin([14.0, 2.0, 0.0, 2.0])
                .pick(Pick::Ignore)
        }));
    }
    let gold = Colors::brass();
    let faster = if s.faster > 0.0 {
        format!("\nItems make it {:.0}% faster.", s.faster * 100.0)
    } else {
        String::new()
    };
    row.child(
        El::new()
            .class("col")
            .with_style(|st| st.gap(2.0).align_items(Align::Center))
            .tooltip(
                s.output.name(),
                format!(
                    "Carted to the warehouse.\n{:.1} a minute at today's pace, {} made so far.{faster}",
                    s.rate, s.made
                ),
            )
            .child(brass::badge(s.output.icon(), 52.0).with_style(move |st| {
                st.border(2.5, gold)
                    .shadow(Shadow::new(gold.with_alpha(0.5), 8.0, 0.0))
            }))
            .child(
                El::text(format!("{:.1} a minute", s.rate))
                    .class("ink")
                    .text_style(|t| t.size(13.0).bold().tabular().nowrap())
                    .with_style(move |st| {
                        st.color(if s.rate > 0.0 {
                            Colors::good_ink()
                        } else {
                            Colors::ink_dim()
                        })
                    })
                    .pick_ignore(),
            )
            .child(
                El::text(format!("{} made", s.made))
                    .class("ink")
                    .text_style(|t| t.size(12.0).tabular().nowrap())
                    .with_style(|st| st.color(Colors::ink_dim()))
                    .pick_ignore(),
            ),
    )
}

/// This batch's progress, sized by the card's own refresh (`isle.sel.progress`).
fn progress(p: f32) -> El {
    El::new()
        .class("brass_track")
        .with_style(|s| s.width(pct(100.0)).height(10.0))
        .child(
            El::new()
                .class("brass_fill")
                .id("isle.sel.progress")
                .with_style(move |s| s.width(pct((p * 100.0).clamp(0.0, 100.0)))),
        )
}

fn section(s: &Shown) -> Vec<El> {
    let mut out = vec![ribbon(s), chain(s), progress(s.progress)];
    if let Some((staff, _, n)) = s.staff {
        out.push(meter(
            "Workers",
            staff,
            format!("{:.0} of {n}", staff * n as f32),
            Color::hex(0x3f6e9a),
        ));
    }
    if s.site < 0.995 || matches!(s.kind, Kind::Woodcutter | Kind::TrapperLodge) {
        let label = match s.kind {
            Kind::Woodcutter => "Trees",
            Kind::TrapperLodge => "Wilds",
            _ => "Fields",
        };
        out.push(meter(
            label,
            s.site,
            format!("{:.0}%", s.site * 100.0),
            Color::hex(0x4f8a3c),
        ));
    }
    let workers = s.staff.map_or(String::new(), |(_, t, n)| {
        format!(" · {n} {}", t.name().to_lowercase())
    });
    let cycle = s.kind.def().cycle_s;
    out.push(
        El::text(format!(
            "One every {cycle:.0} s at full strength{workers} · upkeep {:.0} a minute",
            s.upkeep
        ))
        .class("ink")
        .text_style(|t| t.size(13.0).italic())
        .with_style(|st| st.color(Colors::ink_dim()))
        .pick_ignore(),
    );
    out
}

/// Fill or update the section for the selected works.
pub fn refresh(world: &mut World) {
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return;
    };
    let (Some(p), Some(b)) = (world.get::<Producer>(e).cloned(), world.get::<Building>(e)) else {
        return;
    };
    let kind = b.kind;
    let d = kind.def();
    let Some(output) = d.output else {
        return;
    };
    let isle = b.island;
    let boost = world.get::<Boost>(e).cloned().unwrap_or_default();
    let Some(books) = world.resource::<Economy>().islands.get(isle).cloned() else {
        return;
    };
    let inputs: Vec<(Good, f64)> = d
        .input
        .into_iter()
        .chain(d.input2.filter(|_| !boost.spare2))
        .map(|g| {
            let g = boost.input(g);
            (g, books.get(g))
        })
        .collect();
    let staff = d
        .workers
        .map(|(t, n)| (books.workforce[t.index()].clamp(0.0, 1.0), t, n));
    let shown = Shown {
        kind,
        status: p.status,
        efficiency: p.efficiency,
        progress: p.progress,
        made: p.made,
        inputs,
        output,
        rate: 60.0 / d.cycle_s.max(0.1) * p.efficiency * (1.0 + boost.faster),
        staff,
        site: p.site,
        upkeep: (d.upkeep - boost.thrift).max(0.0),
        faster: boost.faster,
        island: crate::island::islands()[isle].name,
    };
    let sig = format!(
        "{e:?}|{:?}|{:.2}|{}|{}|{:?}|{:.2}|{:.1}",
        shown.status,
        shown.efficiency,
        shown.made,
        shown
            .inputs
            .iter()
            .map(|(g, n)| format!("{}:{n:.0}", g.index()))
            .collect::<Vec<_>>()
            .join(","),
        shown.staff.map(|s| (s.0 * 100.0).round()),
        shown.site,
        shown.upkeep,
    );
    if world.resource::<WorksCard>().sig == sig {
        return;
    }
    let els = section(&shown);
    if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
        let _ = engine_ui::el::replace_children(world, node, &els);
        world.resource_mut::<WorksCard>().sig = sig;
    }
}

/// A new card was opened: fill the section afresh.
pub fn card_opened(world: &mut World) {
    world.resource_mut::<WorksCard>().sig.clear();
    refresh(world);
}
