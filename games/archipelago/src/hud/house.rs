//! The house card's people, in the town economy's terms:
//!
//! - its mood: a face, the happiness (0 to 100) on a bar banded by mood, where it is drifting
//!   and what pulls it (the tooltip);
//! - its three basic categories (food, clothing, the square): each a bar filling toward the
//!   tier's threshold, with the needs that count toward it as ringed badges beside it, and
//!   when the island asks for none yet, from how many people it will;
//! - its luxuries, which only pay and please;
//! - residents and income (rent, goodwill, needs and items in the tooltip);
//! - whether it is moving up: an arrow and the price when it is ready, the one-line reason
//!   when it waits; Move up and Move down.
//!
//! Numbers and rules are the game's (`sim::appraise`, `reports`); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::style::{Align, Fill, Justify, Pick, Position, Shadow, pct};

use crate::defs::{Cat, Kind, Need, Tier};
use crate::items::Boost;
use crate::sim::{self, Appraisal, Building, Economy, House, IslandBooks, Mood, Wait};
use crate::tools::Hand;

/// The card's people section, filled by [`refresh`].
pub const CARD_ID: &str = "isle.sel.house";
const MOOD_BAR: &str = "isle.house.mood.bar";
const MOOD_TARGET: &str = "isle.house.mood.target";
const MOOD_TEXT: &str = "isle.house.mood.text";
const READY_TEXT: &str = "isle.house.ready.text";

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
        .with_style(|s| s.gap(7.0))
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

pub(super) fn mood_icon(m: Mood) -> &'static str {
    match m {
        Mood::Angry => "isle_mood_angry",
        Mood::Unhappy => "isle_mood_unhappy",
        Mood::Content => "isle_mood_content",
        Mood::Happy => "isle_mood_happy",
        Mood::Euphoric => "isle_mood_euphoric",
    }
}

pub(super) fn mood_color(m: Mood) -> Color {
    match m {
        Mood::Angry => Color::hex(0xc8442a),
        Mood::Unhappy => Color::hex(0xe0903a),
        Mood::Content => Color::hex(0xd8b444),
        Mood::Happy => Color::hex(0x7ab444),
        Mood::Euphoric => Color::hex(0x3aa84a),
    }
}

/// What a mood does, in a line.
fn mood_means(m: Mood) -> &'static str {
    match m {
        Mood::Angry => "pays no rent, riots most and cannot move up",
        Mood::Unhappy => "fills at half speed and may riot",
        Mood::Content => "pays its rent",
        Mood::Happy => "pays goodwill on top of its rent",
        Mood::Euphoric => "pays twice the goodwill and fills fastest",
    }
}

pub(super) fn cat_icon(c: Cat) -> &'static str {
    match c {
        Cat::Food => "isle_cat_food",
        Cat::Clothing => "isle_cat_clothing",
        Cat::Square => "isle_cat_square",
        Cat::Luxury => "isle_cat_luxury",
    }
}

fn need_icon(n: Need) -> &'static str {
    match n {
        Need::Good(g) => g.icon(),
        Need::Service(k) => super::kind_icon(k),
    }
}

fn need_name(n: Need) -> &'static str {
    match n {
        Need::Good(g) => g.name(),
        Need::Service(k) => k.def().name,
    }
}

/// Everything the section shows, read once.
struct Seen {
    h: House,
    a: Appraisal,
    books: IslandBooks,
    wait_text: String,
    /// The happiness a house starts at, plus each pull ("why", points).
    pulls: Vec<(String, f32)>,
    /// The acting company's to order about (a rival's house shows no buttons).
    ours: bool,
}

fn pulls(h: &House, a: &Appraisal) -> Vec<(String, f32)> {
    let tier = h.tier;
    let mut v = vec![("Every house starts at".to_string(), 40.0)];
    if a.joy > 0.0 {
        v.push(("What it has".into(), a.joy));
    }
    for c in Cat::BASIC {
        let th = tier.threshold(c);
        let p = a.points[c.index()];
        if a.asked[c.index()] && p < th {
            v.push((
                format!("{} short", c.name()),
                -sim::SHORT_GLOOM * (1.0 - p / th),
            ));
        }
    }
    if a.smoke > 0.0 {
        v.push(("Smoke from a works nearby".into(), -a.smoke));
    }
    if a.trouble > 0.0 {
        v.push(("Trouble here or nearby".into(), -a.trouble));
    }
    if a.crowding > 0.0 {
        v.push((format!("A big town of {}", tier.name()), -a.crowding));
    }
    v
}

/// The mood: face, happiness bar banded by mood with a tick where it is drifting, words.
fn mood_strip(s: &Seen) -> El {
    let h = &s.h;
    let m = h.mood();
    let c = mood_color(m);
    let mut tip = format!(
        "{} houses {}.\n\n[b]Happiness {:.0}[/b], moving toward {:.0}:",
        m.name(),
        mood_means(m),
        h.happiness,
        h.target
    );
    for (why, p) in &s.pulls {
        let col = if *p >= 0.0 { "#2f6e28" } else { "#9a2f1c" };
        tip.push_str(&format!("\n  {why}  [color={col}]{p:+.0}[/color]"));
    }
    let bands = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .top(0.0)
                .right(0.0)
                .bottom(0.0)
                .direction(engine_ui::style::Direction::Row)
                .pick(Pick::Ignore)
        })
        .children(Mood::ALL.iter().map(|m| {
            let c = mood_color(*m);
            El::new().with_style(move |s| {
                s.grow(1.0)
                    .basis(0.0)
                    .height(pct(100.0))
                    .background(Fill::Solid(c.with_alpha(0.22)))
                    .pick(Pick::Ignore)
            })
        }));
    let frac = (h.happiness / 100.0).clamp(0.0, 1.0);
    let target = (h.target / 100.0).clamp(0.0, 1.0);
    let bar = El::new()
        .class("brass_track")
        .with_style(|s| s.width(pct(100.0)).height(12.0))
        .child(bands)
        .child(El::new().id(MOOD_BAR).with_style(move |s| {
            s.width(pct(frac * 100.0))
                .height(pct(100.0))
                .radius(6.0)
                .background(Fill::vertical(c.lerp(Color::hex(0xffffff), 0.3), c))
                .pick(Pick::Ignore)
        }))
        .child(El::new().id(MOOD_TARGET).with_style(move |s| {
            s.position(Position::Absolute)
                .left(pct(target * 100.0))
                .top(-3.0)
                .bottom(-3.0)
                .width(3.0)
                .radius(1.5)
                .background(Fill::Solid(Colors::ink()))
                .pick(Pick::Ignore)
        }));
    El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).align_items(Align::Center))
        .tooltip(m.name(), tip)
        .child(
            // A round glow in the mood's colour behind the face.
            El::new()
                .with_style(move |s| {
                    s.size(46.0, 46.0)
                        .radius(23.0)
                        .shadow(Shadow::new(c.with_alpha(0.55), 12.0, 0.0))
                        .pick(Pick::Ignore)
                })
                .child(
                    El::icon(mood_icon(m)).with_style(|s| s.size(46.0, 46.0).pick(Pick::Ignore)),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(4.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.justify(Justify::SpaceBetween).align_items(Align::End))
                        .child(
                            El::text(m.name())
                                .class("ink")
                                .text_style(|t| t.size(17.0).bold().nowrap())
                                .pick_ignore(),
                        )
                        .child(
                            El::text(mood_line(h))
                                .class("ink")
                                .id(MOOD_TEXT)
                                .text_style(|t| t.size(12.5).tabular().nowrap())
                                .with_style(|s| s.color(Colors::ink_dim()))
                                .pick_ignore(),
                        ),
                )
                .child(bar),
        )
}

fn mood_line(h: &House) -> String {
    let now = h.happiness.floor();
    if h.target > h.happiness + 0.5 {
        format!("happiness {now:.0}, rising to {:.0}", h.target)
    } else if h.target < h.happiness - 0.5 {
        format!("happiness {now:.0}, falling to {:.0}", h.target)
    } else {
        format!("happiness {now:.0}")
    }
}

/// A need's small badge, its ring split green for the share supplied and red for the rest
/// (dim when the island does not ask for it yet). Clicking a need that is short picks up the
/// building that answers it, ready to place.
fn need_chip(n: Need, met: f32, asked: bool, tip: String) -> El {
    let ring = brass::SplitRing {
        frac: met,
        idle: !asked,
        width: 3.0,
    };
    let chip = brass::split_badge(need_icon(n), 28.0, &ring, "").tooltip(need_name(n), tip);
    match fixer(n).filter(|_| asked && met < 0.99) {
        Some(k) => chip.on_click(build_action(k)),
        None => chip,
    }
}

/// The building that answers a need: its service, or the first works that makes the good.
pub(super) fn fixer(n: Need) -> Option<Kind> {
    match n {
        Need::Service(k) => Some(k),
        Need::Good(g) => crate::defs::DEFS
            .iter()
            .find(|d| d.output == Some(g))
            .map(|d| d.kind),
    }
}

/// The build menu's action for `k`.
pub(super) fn build_action(k: Kind) -> String {
    format!(
        "isle.build:{}",
        serde_json::to_value(k)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default()
    )
}

fn chip_tip(s: &Seen, j: usize) -> String {
    let w = &s.h.tier.wants()[j];
    let l = &s.a.lines[j];
    let mut t = String::new();
    if !l.asked {
        let paused = matches!(w.need, Need::Good(g) if s.books.paused[g.index()]);
        if paused {
            t.push_str("[color=#9a2f1c]Paused on this island: nobody gets it.[/color]\n");
        } else {
            t.push_str(&format!(
                "Asked for once the island has {:.0} {}.\n",
                w.unlock,
                s.h.tier.name().to_lowercase()
            ));
        }
    } else {
        t.push_str(&format!("Supplied {:.0}% now.\n", l.met * 100.0));
    }
    if w.basic() {
        t.push_str(&format!(
            "[b]{:.0}[/b] points toward {}, brings {:.0} people, pays {:.1} coins a minute",
            w.points,
            w.cat.name().to_lowercase(),
            w.people,
            w.coins
        ));
    } else {
        t.push_str(&format!("A luxury: pays {:.1} coins a minute", w.coins));
    }
    if w.joy > 0.0 {
        t.push_str(&format!(", +{:.0} happiness", w.joy));
    }
    t.push('.');
    if l.asked
        && l.met < 0.99
        && let Some(k) = fixer(w.need)
    {
        t.push_str(&format!(
            "\n[color=#2f6e28][b]Click[/b] to place a {}.[/color]",
            k.def().name.to_lowercase()
        ));
    }
    t
}

/// One basic category: its picture, a bar toward the threshold, its needs.
fn category(s: &Seen, c: Cat) -> El {
    let tier = s.h.tier;
    let th = tier.threshold(c);
    let p = s.a.points[c.index()];
    let asked = s.a.asked[c.index()];
    let frac = if th > 0.0 { p / th } else { 1.0 };
    let met = asked && p >= th - 1e-4;
    let fill = if met {
        Color::hex(0x4f9a3c)
    } else if frac > 0.05 {
        Color::hex(0xd99a2a)
    } else {
        Color::hex(0xb03a24)
    };
    let wants: Vec<usize> = tier
        .wants()
        .iter()
        .enumerate()
        .filter(|(_, w)| w.cat == c)
        .map(|(j, _)| j)
        .collect();
    let value = if asked {
        format!("{p:.1} of {th:.0}")
    } else {
        // When the first need of it is asked for.
        wants
            .iter()
            .map(|j| tier.wants()[*j].unlock)
            .min_by(f32::total_cmp)
            .map_or("none yet".into(), |u| {
                format!("from {u:.0} {}", tier.name().to_lowercase())
            })
    };
    let tip = format!(
        "{} counts once its needs' points reach [b]{th:.0}[/b] (each need's points times how well it is supplied). Below that it weighs on the house and keeps it from moving up.",
        c.name()
    );
    El::new()
        .class("row")
        .with_style(|s| s.gap(8.0).align_items(Align::Center))
        .child(
            El::new()
                .tooltip(c.name(), tip)
                .with_style(|s| s.size(32.0, 32.0))
                .child(El::icon(cat_icon(c)).with_style(move |s| {
                    s.size(32.0, 32.0)
                        .opacity(if asked { 1.0 } else { 0.5 })
                        .pick(Pick::Ignore)
                })),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.justify(Justify::SpaceBetween).align_items(Align::End))
                        .child(
                            El::text(c.name())
                                .class("ink")
                                .text_style(|t| t.size(14.0).bold().nowrap())
                                .pick_ignore(),
                        )
                        .child(
                            El::text(value)
                                .class("ink")
                                .text_style(|t| t.size(12.5).tabular().nowrap())
                                .with_style(move |s| {
                                    s.color(if met {
                                        Colors::good_ink()
                                    } else if asked {
                                        Colors::ink()
                                    } else {
                                        Colors::ink_dim()
                                    })
                                })
                                .pick_ignore(),
                        ),
                )
                .child(
                    El::new()
                        .class("brass_track")
                        .with_style(|s| s.width(pct(100.0)).height(10.0))
                        .child(El::new().with_style(move |s| {
                            s.width(pct(frac.clamp(0.0, 1.0) * 100.0))
                                .height(pct(100.0))
                                .radius(5.0)
                                .background(Fill::vertical(
                                    fill.lerp(Color::hex(0xffffff), 0.28),
                                    fill,
                                ))
                                .pick(Pick::Ignore)
                        })),
                ),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(3.0).align_items(Align::Center))
                .children(wants.iter().map(|j| {
                    let w = &tier.wants()[*j];
                    let l = &s.a.lines[*j];
                    need_chip(w.need, l.met, l.asked, chip_tip(s, *j))
                })),
        )
}

fn luxuries(s: &Seen) -> Option<El> {
    let tier = s.h.tier;
    let list: Vec<usize> = tier
        .wants()
        .iter()
        .enumerate()
        .filter(|(_, w)| !w.basic())
        .map(|(j, _)| j)
        .collect();
    if list.is_empty() {
        return None;
    }
    let pays: f32 = list.iter().map(|j| s.a.lines[*j].coins).sum::<f32>();
    Some(
        El::new()
            .class("row")
            .with_style(|s| s.gap(8.0).align_items(Align::Center))
            .child(
                El::icon(cat_icon(Cat::Luxury))
                    .tooltip(
                        "Luxuries",
                        "Nobody moves in or out for them: they pay and they please.",
                    )
                    .with_style(|s| s.size(32.0, 32.0)),
            )
            .child(
                El::new()
                    .class("col")
                    .with_style(|s| s.gap(0.0).width(64.0))
                    .child(
                        El::text("Luxury")
                            .class("ink")
                            .text_style(|t| t.size(14.0).bold().nowrap())
                            .pick_ignore(),
                    )
                    .child(
                        El::text(format!("+{pays:.1} a min"))
                            .class("ink")
                            .text_style(|t| t.size(12.5).tabular().nowrap())
                            .with_style(move |s| {
                                s.color(if pays > 0.01 {
                                    Colors::good_ink()
                                } else {
                                    Colors::ink_dim()
                                })
                            })
                            .pick_ignore(),
                    ),
            )
            .child(
                El::new()
                    .class("row")
                    .with_style(|s| {
                        s.gap(3.0)
                            .wrap(true)
                            .grow(1.0)
                            .shrink(1.0)
                            .justify(Justify::End)
                    })
                    .children(list.iter().map(|j| {
                        let w = &tier.wants()[*j];
                        let l = &s.a.lines[*j];
                        need_chip(w.need, l.met, l.asked, chip_tip(s, *j))
                    })),
            ),
    )
}

fn income_tip(s: &Seen) -> String {
    let h = &s.h;
    let needs = (h.income - h.rent - h.goodwill).max(0.0);
    format!(
        "[b]Rent[/b]  {:.1}\n[b]Goodwill[/b]  {:.1} (happy houses pay it, euphoric ones twice)\n[b]Needs and items[/b]  {needs:.1}\n\nA full, euphoric house with every need met pays {:.1}.",
        h.rent,
        h.goodwill,
        h.tier.full_income()
    )
}

/// The icon beside a waiting house's reason.
fn wait_icon(w: Wait, m: Mood) -> &'static str {
    match w {
        Wait::None => "isle_move_up",
        Wait::Top => "isle_house",
        Wait::Trouble => "isle_alert_fire",
        Wait::NotFull => "isle_people",
        Wait::Short(c) => cat_icon(c),
        Wait::Angry => mood_icon(m),
        Wait::NeedsPaving => "isle_paved",
        Wait::NoMaterials => "isle_bricks",
        Wait::WouldEmptyWorks => "isle_workshop",
    }
}

/// Moving up: ready (arrow, price, Move up) or the reason it waits; and Move down.
fn moving(s: &Seen) -> El {
    let h = &s.h;
    let next = h.tier.next();
    let ready = h.ready && next.is_some();
    let green = Color::hex(0x4f9a3c);
    let (c, icon, title) = if ready {
        (
            green,
            "isle_move_up",
            format!("Ready to become {}", next.map_or("", |(t, _)| t.name())),
        )
    } else if next.is_none() {
        (
            Color::hex(0x6a5a4a),
            "isle_house",
            format!("{} are the top tier", h.tier.name()),
        )
    } else {
        (
            Color::hex(0xd99a2a),
            wait_icon(h.wait, h.mood()),
            "Not moving up yet".to_string(),
        )
    };
    let cost: Vec<El> = next
        .map(|(_, cost)| {
            cost.iter()
                .map(|(g, n)| {
                    let short = s.books.get(*g) + 1e-6 < *n as f64;
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(2.0).align_items(Align::Center))
                        .child(
                            El::icon(g.icon())
                                .with_style(|s| s.size(18.0, 18.0).pick(Pick::Ignore)),
                        )
                        .child(
                            El::text(format!("{n:.0}"))
                                .class("ink")
                                .text_style(|t| t.size(13.0).bold().tabular().nowrap())
                                .with_style(move |s| {
                                    s.color(if short {
                                        Colors::bad_ink()
                                    } else {
                                        Colors::ink()
                                    })
                                })
                                .pick_ignore(),
                        )
                })
                .collect()
        })
        .unwrap_or_default();
    // A ready house that still waits (no materials, its works need the people) says why.
    let line = if ready && h.wait == Wait::None {
        ready_line(h, s.books.auto_up)
    } else {
        s.wait_text.clone()
    };
    let paving = next.is_some_and(|(t, _)| t.wants_paving()) && !h.paved;
    let banner = El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(10.0)
                .align_items(Align::Center)
                .padding([7.0, 10.0, 7.0, 10.0])
                .radius(8.0)
                .background(Fill::Solid(c.with_alpha(0.14)))
                .border(1.0, c.with_alpha(0.5))
        })
        .child(El::icon(icon).with_style(move |s| s.size(36.0, 36.0).pick(Pick::Ignore)))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(title)
                        .class("ink")
                        .text_style(|t| t.size(14.5).bold().nowrap())
                        .with_style(move |s| {
                            s.color(if ready {
                                Colors::good_ink()
                            } else {
                                Colors::ink()
                            })
                        })
                        .pick_ignore(),
                )
                .child(
                    El::text(line)
                        .class("ink")
                        .id(READY_TEXT)
                        .text_style(|t| t.size(12.5))
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                )
                .child_opt((!cost.is_empty()).then(|| {
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(8.0).align_items(Align::Center).wrap(true))
                        .child(brass::label("costs", true))
                        .children(cost)
                        .child_opt(paving.then(|| {
                            El::new()
                                .class("row")
                                .with_style(|s| s.gap(3.0).align_items(Align::Center))
                                .child(
                                    El::icon("isle_paved")
                                        .with_style(|s| s.size(18.0, 18.0).pick(Pick::Ignore)),
                                )
                                .child(
                                    El::text("paving")
                                        .class("ink")
                                        .text_style(|t| t.size(12.5).nowrap())
                                        .with_style(|s| s.color(Colors::bad_ink()))
                                        .pick_ignore(),
                                )
                        }))
                })),
        );
    let up_tip = match next {
        Some((t, _)) if ready => format!(
            "Move it up to {} now: pays the building goods. If the works of the tier it leaves need these people, it says so.",
            t.name()
        ),
        Some(_) => format!("Not ready: {}.", s.wait_text),
        None => "The top tier.".into(),
    };
    let down = h.tier != Tier::Fisherfolk && s.ours;
    let buttons = El::new()
        .class("row")
        .with_style(|s| s.gap(8.0))
        .child_opt(next.filter(|_| s.ours).map(|_| {
            El::button("Move up", "isle.house.up")
                .class("brass_button")
                .text_style(|t| t.nowrap())
                .disabled(!ready)
                .tooltip("Move up", up_tip)
        }))
        .child_opt(down.then(|| {
            El::button("Move down", "isle.house.down")
                .class("brass_button_dark")
                .text_style(|t| t.nowrap())
                .tooltip(
                    "Move down",
                    format!(
                        "Back to {}: gives their works the hands back. People beyond the smaller house move out and nothing is refunded.",
                        Tier::ALL[h.tier.index() - 1].name()
                    ),
                )
        }));
    El::new()
        .class("col")
        .with_style(|s| s.gap(6.0))
        .child(banner)
        .child(buttons)
}

/// A ready house's line: it waits for the player's order (or moves itself, on an island
/// set to), and for how long it has been ready.
fn ready_line(h: &House, auto_up: bool) -> String {
    if auto_up {
        format!("Moves up on its own · ready for {}", since(h.content_s))
    } else {
        format!("Waiting for your order · ready for {}", since(h.content_s))
    }
}

fn since(s: f32) -> String {
    if s < 90.0 {
        format!("{s:.0} s")
    } else {
        format!("{:.0} min", s / 60.0)
    }
}

fn section(s: &Seen) -> Vec<El> {
    let h = &s.h;
    let mut out = vec![
        mood_strip(s),
        brass::label("Needs · points toward each threshold", true),
    ];
    out.extend(Cat::BASIC.iter().map(|c| category(s, *c)));
    out.extend(luxuries(s));
    out.push(brass::divider(316.0));
    out.push(meter(
        "Residents",
        h.residents / s.a.size.max(1.0),
        format!("{:.0} of {:.0}", h.residents.floor(), s.a.size),
        Color::hex(0x3f6e9a),
    ));
    out.push(
        meter(
            "Income",
            h.income / h.tier.full_income().max(0.01),
            format!("{:.1} a min", h.income),
            Color::hex(0xb8892a),
        )
        .tooltip("Income", income_tip(s)),
    );
    out.push(moving(s));
    out
}

fn seen(world: &mut World, e: Entity) -> Option<Seen> {
    let h = world.get::<House>(e)?.clone();
    let b = world.get::<Building>(e)?.clone();
    let boost = world.get::<Boost>(e).cloned();
    let books = world.resource::<Economy>().islands.get(b.island)?.clone();
    let a = sim::appraise(&h, &books, boost.as_ref());
    let wait_text = crate::reports::wait_text(&h, &a, &books);
    let pulls = pulls(&h, &a);
    let ours = crate::build::ours(world, e);
    Some(Seen {
        h,
        a,
        books,
        wait_text,
        pulls,
        ours,
    })
}

/// Fill or update the section for the selected house.
pub fn refresh(world: &mut World) {
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return;
    };
    let Some(s) = seen(world, e) else {
        return;
    };
    let h = &s.h;
    let tier = h.tier;
    let lines: String = (0..tier.wants().len())
        .map(|j| {
            let l = &s.a.lines[j];
            format!("{}{:.2}", l.asked as u8, l.met)
        })
        .collect::<Vec<_>>()
        .join(",");
    let sig = format!(
        "{e:?}|{tier:?}|{:?}|{}|{:?}|{}|{:.0}|{:.0}|{:.1}|{:.1}|{:.1}|{:.0}|{:?}|{}|{lines}",
        h.mood(),
        h.ready,
        h.wait,
        h.paved,
        h.residents.floor(),
        s.a.size,
        h.income,
        h.rent,
        h.goodwill,
        h.target,
        s.a.points,
        s.wait_text,
    );
    if world.resource::<HouseCard>().sig != sig {
        let els = section(&s);
        if let Some(node) = engine_ui::input::find_id(world, CARD_ID) {
            let _ = engine_ui::el::replace_children(world, node, &els);
            world.resource_mut::<HouseCard>().sig = sig;
        }
    }
    // The happiness drifts a point every few seconds: move it in place.
    let frac = (h.happiness / 100.0).clamp(0.0, 1.0);
    if let Some(n) = engine_ui::input::find_id(world, MOOD_BAR)
        && let Some(mut node) = world.get_mut::<engine_ui::node::Node>(n)
    {
        let w = Some(pct(frac * 100.0));
        if node.style.width != w {
            node.style.width = w;
        }
    }
    if let Some(n) = engine_ui::input::find_id(world, MOOD_TEXT)
        && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(n)
    {
        let text = mood_line(h);
        if t.text != text {
            t.text = text;
        }
    }
    if h.ready
        && h.wait == Wait::None
        && let Some(n) = engine_ui::input::find_id(world, READY_TEXT)
        && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(n)
    {
        let text = ready_line(h, s.books.auto_up);
        if t.text != text {
            t.text = text;
        }
    }
}

/// A new card was opened: fill the section afresh.
pub fn card_opened(world: &mut World) {
    world.resource_mut::<HouseCard>().sig.clear();
    refresh(world);
}

/// `isle.house.up` and `isle.house.down` on the selected house. Returns false for anything
/// else.
pub fn action(world: &mut World, a: &str) -> bool {
    let up = match a {
        "isle.house.up" => true,
        "isle.house.down" => false,
        _ => return false,
    };
    let Some(e) = world.get_resource::<Hand>().and_then(|h| h.selected) else {
        return true;
    };
    let before = world.get::<House>(e).map(|h| h.tier);
    let r = if up {
        crate::reports::move_up(world, e)
    } else {
        crate::reports::move_down(world, e).map(|()| None)
    };
    match r {
        Ok(warning) => {
            let after = world.get::<House>(e).map(|h| h.tier);
            if let Some(t) = after
                && after != before
            {
                let (icon, what) = if up {
                    ("isle_move_up", "moved up")
                } else {
                    ("isle_move_down", "moved down")
                };
                let (kind, text) = match &warning {
                    Some(w) => (
                        engine_ui::toast::ToastKind::Warn,
                        format!("It is a {} house now. {w}", t.name().to_lowercase()),
                    ),
                    None => (
                        engine_ui::toast::ToastKind::Good,
                        format!("It is a {} house now.", t.name().to_lowercase()),
                    ),
                };
                engine_ui::toast::notify(
                    world,
                    kind,
                    icon,
                    &format!("A house {what}"),
                    &text,
                    if warning.is_some() { 7.0 } else { 4.0 },
                );
            }
            super::show_card(world);
        }
        Err(msg) => {
            engine_ui::toast::notify(
                world,
                engine_ui::toast::ToastKind::Warn,
                if up { "isle_move_up" } else { "isle_move_down" },
                if up {
                    "Can't move up"
                } else {
                    "Can't move down"
                },
                &msg,
                5.0,
            );
        }
    }
    true
}
