//! Relations: the rival houses and where the company stands with each. A book like the
//! ledger. The left page lists every rival as a card: its shield in the house colours, name
//! and temper, the treaty, and a standing bar from bitter to warm. The right page is the
//! picked rival:
//! - a dial for the standing, and the treaty;
//! - three panels: diplomacy, trade (what passes between us, their wealth and what they buy
//!   and sell), and military (their warships and guns against ours, as a balance);
//! - what has happened of late, with what each thing did to the standing;
//! - what would please them;
//! - the moves the company can make, each saying why not when it can't.
//!
//! Data is the game's (`isle.relations`, read through the API so this compiles before the
//! rivals do); this is the UI thread's.

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::draw::DrawCtx;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Rect;
use engine_ui::style::{Align, Direction, Fill, Justify, Pick, Shadow, pct};
use engine_ui::text::{DISPLAY_FONT, TextSpec};
use serde_json::{Value, json};

pub const SCREEN: &str = "isle_relations";
const DIAL: &str = "isle_rel_dial";
const BODY_ID: &str = "isle.rel.body";

#[derive(Resource, Default)]
pub struct Relations {
    pub open: bool,
    /// The rival picked (by id).
    picked: Option<u64>,
    sig: String,
    /// The standing the dial shows (eased toward the rival's).
    dial: f32,
    dial_goal: f32,
}

pub fn install(world: &mut World) {
    world.init_resource::<Relations>();
    if let Some(mut p) = world.get_resource_mut::<engine_ui::Painters>() {
        p.add(DIAL, paint_dial);
    }
}

/// One rival, read leniently from the game's JSON.
#[derive(Clone, Debug, Default)]
struct Rival {
    id: u64,
    name: String,
    color: Color,
    emblem: String,
    temperament: String,
    standing: f32,
    treaty: String,
    /// The game's words for the treaty ("Trade pact"), when it gives them.
    treaty_text: String,
    /// Wealth, people and fleets are set numbers until the rivals play for real.
    estimate: bool,
    trade_per_min: f32,
    wealth: f32,
    people: f32,
    islands: u32,
    buys: Vec<String>,
    sells: Vec<String>,
    warships: f32,
    firepower: f32,
    our_warships: f32,
    our_firepower: f32,
    lost: f32,
    taken: f32,
    events: Vec<(f64, String, f32)>,
    wants: Vec<String>,
    actions: Vec<Move>,
}

#[derive(Clone, Debug, Default)]
struct Move {
    label: String,
    action: String,
    enabled: bool,
    why_not: String,
    args: Option<Value>,
}

fn num(v: &Value, keys: &[&str]) -> f32 {
    for k in keys {
        let mut cur = v;
        let mut ok = true;
        for part in k.split('.') {
            match cur.get(part) {
                Some(x) => cur = x,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && let Some(n) = cur.as_f64() {
            return n as f32;
        }
    }
    0.0
}

fn text(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        let mut cur = v;
        let mut ok = true;
        for part in k.split('.') {
            match cur.get(part) {
                Some(x) => cur = x,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            if let Some(s) = cur.as_str() {
                return s.to_string();
            }
            if cur.is_number() {
                return cur.to_string();
            }
        }
    }
    String::new()
}

fn list(v: &Value, keys: &[&str]) -> Vec<Value> {
    for k in keys {
        let mut cur = v;
        let mut ok = true;
        for part in k.split('.') {
            match cur.get(part) {
                Some(x) => cur = x,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && let Some(a) = cur.as_array() {
            return a.clone();
        }
    }
    Vec::new()
}

fn color_of(v: &Value) -> Color {
    match v.get("color") {
        Some(Value::Number(n)) => Color::hex(n.as_u64().unwrap_or(0x6a5a4a) as u32),
        Some(Value::String(s)) => {
            let s = s.trim_start_matches('#');
            u32::from_str_radix(s, 16).map_or(Color::hex(0x6a5a4a), Color::hex)
        }
        Some(Value::Array(a)) if a.len() >= 3 => {
            let c = |i: usize| (a[i].as_f64().unwrap_or(0.5) as f32).clamp(0.0, 1.0);
            let to = |x: f32| (x * 255.0).round() as u32;
            Color::hex(to(c(0)) << 16 | to(c(1)) << 8 | to(c(2)))
        }
        _ => Color::hex(0x6a5a4a),
    }
}

fn names(v: &[Value]) -> Vec<String> {
    v.iter()
        .filter_map(|x| {
            x.as_str()
                .map(String::from)
                .or_else(|| x.get(0).and_then(|y| y.as_str()).map(String::from))
                .or_else(|| x.get("good").and_then(|y| y.as_str()).map(String::from))
        })
        .collect()
}

fn parse(v: &Value) -> Rival {
    let events = list(v, &["events"])
        .iter()
        .map(|e| {
            if let Some(a) = e.as_array() {
                (
                    a.first().and_then(Value::as_f64).unwrap_or(0.0),
                    a.get(1).and_then(Value::as_str).unwrap_or("").to_string(),
                    a.get(2).and_then(Value::as_f64).unwrap_or(0.0) as f32,
                )
            } else {
                (
                    num(e, &["clock_s", "at"]) as f64,
                    text(e, &["text"]),
                    num(e, &["delta", "standing"]),
                )
            }
        })
        .collect();
    let actions = list(v, &["actions"])
        .iter()
        .map(|a| {
            if let Some(x) = a.as_array() {
                Move {
                    label: x.first().and_then(Value::as_str).unwrap_or("").into(),
                    action: x.get(1).and_then(Value::as_str).unwrap_or("").into(),
                    enabled: x.get(2).and_then(Value::as_bool).unwrap_or(true),
                    why_not: x.get(3).and_then(Value::as_str).unwrap_or("").into(),
                    args: None,
                }
            } else {
                Move {
                    label: text(a, &["label"]),
                    action: text(a, &["action"]),
                    enabled: a.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    why_not: text(a, &["why_not"]),
                    args: a.get("args").cloned(),
                }
            }
        })
        .collect();
    Rival {
        id: v.get("id").and_then(Value::as_u64).unwrap_or(0),
        name: text(v, &["name"]),
        color: color_of(v),
        emblem: text(v, &["emblem"]),
        temperament: text(v, &["temperament"]),
        standing: num(v, &["standing"]),
        treaty: text(v, &["treaty"]),
        treaty_text: text(v, &["treaty_name"]),
        estimate: v
            .get("placeholder")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        trade_per_min: num(v, &["economic.trade_per_min", "trade_per_min"]),
        wealth: num(v, &["economic.wealth", "wealth"]),
        people: num(v, &["economic.people", "people"]),
        islands: num(v, &["economic.islands", "islands"]) as u32,
        buys: names(&list(
            v,
            &["economic.buys", "buys", "economic.buys_from_us"],
        )),
        sells: names(&list(
            v,
            &["economic.sells", "sells", "economic.sells_to_us"],
        )),
        warships: num(v, &["military.warships", "warships"]),
        firepower: num(v, &["military.firepower", "firepower"]),
        our_warships: num(v, &["military.our_warships", "our_warships"]),
        our_firepower: num(v, &["military.our_firepower", "our_firepower"]),
        lost: num(v, &["military.lost", "lost"]),
        taken: num(v, &["military.taken", "taken"]),
        events,
        wants: list(v, &["wants"])
            .iter()
            .filter_map(|w| w.as_str().map(String::from))
            .collect(),
        actions,
    }
}

fn rivals(world: &mut World) -> Option<Vec<Rival>> {
    let v = engine::core::registry::call_api(world, "isle.relations", json!({})).ok()?;
    let arr = v
        .as_array()
        .cloned()
        .or_else(|| v.get("rivals").and_then(Value::as_array).cloned())?;
    Some(arr.iter().map(parse).collect())
}

/// The standing in a word, and its colour.
fn mood(s: f32) -> (&'static str, Color) {
    match s {
        s if s < -60.0 => ("Bitter", Color::hex(0xb03a24)),
        s if s < -20.0 => ("Cool", Color::hex(0xd0702a)),
        s if s < 20.0 => ("Wary", Color::hex(0xc8a040)),
        s if s < 60.0 => ("Cordial", Color::hex(0x7ab444)),
        _ => ("Warm", Color::hex(0x3aa84a)),
    }
}

fn treaty_icon(t: &str) -> &'static str {
    match t.to_lowercase().as_str() {
        "war" => "isle_treaty_war",
        "hostile" => "isle_treaty_hostile",
        "trade_pact" | "tradepact" | "trade pact" | "pact" => "isle_treaty_pact",
        "alliance" => "isle_treaty_alliance",
        _ => "isle_treaty_neutral",
    }
}

fn treaty_name(t: &str) -> &'static str {
    match t.to_lowercase().as_str() {
        "war" => "At war",
        "hostile" => "Hostile",
        "trade_pact" | "tradepact" | "trade pact" | "pact" => "Trade pact",
        "alliance" => "Allied",
        _ => "At peace",
    }
}

fn treaty_color(t: &str) -> Color {
    match t.to_lowercase().as_str() {
        "war" => Color::hex(0xb03a24),
        "hostile" => Color::hex(0xd0702a),
        "trade_pact" | "tradepact" | "trade pact" | "pact" => Color::hex(0x3f6e9a),
        "alliance" => Color::hex(0x3aa84a),
        _ => Color::hex(0x8a7a62),
    }
}

/// The house's shield: its colour, a gilt rim and the emblem in white.
fn shield(r: &Rival, size: f32) -> El {
    let c = r.color;
    let emblem = format!("isle_emblem_{}", r.emblem);
    El::new()
        .with_style(move |s| {
            s.size(size, size * 1.12)
                .radius(engine_ui::style::Corners([
                    size * 0.18,
                    size * 0.18,
                    size * 0.5,
                    size * 0.5,
                ]))
                .background(Fill::vertical(
                    c.lerp(Color::WHITE, 0.18),
                    c.lerp(Color::BLACK, 0.3),
                ))
                .border((size * 0.06).max(2.0), Colors::brass_light())
                .shadow(Shadow::new(
                    Color::hexa(0x00000080),
                    size * 0.12,
                    size * 0.04,
                ))
                .align_items(Align::Center)
                .justify(Justify::Center)
                .pick(Pick::Ignore)
        })
        .child(
            El::icon(&emblem)
                .with_style(move |s| s.size(size * 0.7, size * 0.7).pick(Pick::Ignore)),
        )
}

/// A standing bar from -100 to 100: a red-to-green bed with the middle marked and the fill
/// from the middle to the standing.
fn standing_bar(s: f32, width: f32) -> El {
    let (_, c) = mood(s);
    let f = (s / 100.0).clamp(-1.0, 1.0);
    let (left, w) = if f >= 0.0 {
        (50.0, f * 50.0)
    } else {
        (50.0 + f * 50.0, -f * 50.0)
    };
    El::new()
        .class("brass_track")
        .with_style(move |st| st.width(width).height(10.0).padding(0.0))
        .child(El::new().with_style(move |st| {
            st.position(engine_ui::style::Position::Absolute)
                .left(pct(left))
                .top(0.0)
                .width(pct(w))
                .height(pct(100.0))
                .radius(5.0)
                .background(Fill::vertical(c.lerp(Color::WHITE, 0.3), c))
                .pick(Pick::Ignore)
        }))
        .child(El::new().with_style(|st| {
            st.position(engine_ui::style::Position::Absolute)
                .left(pct(50.0))
                .top(-2.0)
                .bottom(-2.0)
                .width(2.0)
                .background(Fill::Solid(Colors::ink()))
                .pick(Pick::Ignore)
        }))
}

fn rival_card(r: &Rival, picked: bool) -> El {
    let (word, c) = mood(r.standing);
    let tc = treaty_color(&r.treaty);
    El::new()
        .class("row")
        .on_click(format!("isle.rel.pick:{}", r.id))
        .with_style(move |s| {
            let s = s
                .gap(12.0)
                .align_items(Align::Center)
                .padding([10.0, 12.0])
                .radius(8.0);
            if picked {
                s.background(Fill::Solid(Color::hexa(0xffffff40)))
                    .border(2.0, Colors::brass())
                    .shadow(Shadow::new(Colors::brass().with_alpha(0.4), 10.0, 0.0))
            } else {
                s.background(Fill::Solid(Color::hexa(0x00000010)))
                    .border(1.0, Color::hexa(0x2a1a0a40))
            }
        })
        .child(shield(r, 46.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(3.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(r.name.clone())
                        .class("ink")
                        .text_style(|t| t.size(17.0).bold().nowrap())
                        .pick_ignore(),
                )
                .child(
                    El::text(r.temperament.clone())
                        .class("ink")
                        .text_style(|t| t.size(12.5).italic().nowrap())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(8.0).align_items(Align::Center))
                        .child(standing_bar(r.standing, 150.0))
                        .child(
                            El::text(format!("{word} {:+.0}", r.standing))
                                .class("ink")
                                .text_style(|t| t.size(12.5).bold().tabular().nowrap())
                                .with_style(move |s| s.color(c.lerp(Colors::ink(), 0.35)))
                                .pick_ignore(),
                        ),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0).align_items(Align::Center).width(70.0))
                .child(
                    El::icon(treaty_icon(&r.treaty))
                        .with_style(|s| s.size(30.0, 30.0).pick(Pick::Ignore)),
                )
                .child(
                    El::text(treaty_name(&r.treaty))
                        .text_style(|t| t.size(11.5).bold().nowrap())
                        .with_style(move |s| s.color(tc.lerp(Colors::ink(), 0.3)))
                        .pick_ignore(),
                ),
        )
}

fn panel(icon: &str, title: &str, body: El) -> El {
    El::new()
        .class("col")
        .with_style(|s| {
            s.gap(6.0)
                .grow(1.0)
                .basis(0.0)
                .padding([10.0, 12.0])
                .radius(8.0)
                .background(Fill::Solid(Color::hexa(0x00000012)))
                .border(1.0, Color::hexa(0x2a1a0a30))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Center))
                .child(El::icon(icon).with_style(|s| s.size(30.0, 30.0).pick(Pick::Ignore)))
                .child(
                    El::text(title)
                        .class("ink_title")
                        .text_style(|t| t.size(18.0).nowrap())
                        .pick_ignore(),
                ),
        )
        .child(body)
}

fn stat(label: &str, value: String, color: Option<Color>) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.justify(Justify::SpaceBetween).align_items(Align::Center))
        .child(brass::label(label, true))
        .child(
            El::text(value)
                .class("ink")
                .text_style(|t| t.size(14.0).bold().tabular().nowrap())
                .with_style(move |s| match color {
                    Some(c) => s.color(c),
                    None => s,
                })
                .pick_ignore(),
        )
}

fn good_chips(label: &str, goods: &[String]) -> Option<El> {
    if goods.is_empty() {
        return None;
    }
    Some(
        El::new()
            .class("row")
            .with_style(|s| s.gap(4.0).align_items(Align::Center).wrap(true))
            .child(brass::label(label, true))
            .children(goods.iter().filter_map(|n| {
                crate::defs::Good::from_name(n).map(|g| {
                    El::icon(g.icon())
                        .tooltip(g.name(), "")
                        .with_style(|s| s.size(22.0, 22.0))
                })
            })),
    )
}

/// Their strength against ours, as a balance: our share from the left in blue, theirs from
/// the right in their colour.
fn balance(ours: f32, theirs: f32, c: Color) -> El {
    let total = (ours + theirs).max(0.001);
    let us = (ours / total).clamp(0.0, 1.0);
    El::new()
        .class("brass_track")
        .with_style(|s| {
            s.width(pct(100.0))
                .height(12.0)
                .padding(0.0)
                .direction(Direction::Row)
        })
        .child(El::new().with_style(move |s| {
            s.width(pct(us * 100.0))
                .height(pct(100.0))
                .radius([6.0, 0.0, 0.0, 6.0])
                .background(Fill::vertical(Color::hex(0x6a9ad0), Color::hex(0x2c5a86)))
                .pick(Pick::Ignore)
        }))
        .child(El::new().with_style(move |s| {
            s.grow(1.0)
                .height(pct(100.0))
                .radius([0.0, 6.0, 6.0, 0.0])
                .background(Fill::vertical(c.lerp(Color::WHITE, 0.25), c))
                .pick(Pick::Ignore)
        }))
}

fn ago(clock: f64, at: f64) -> String {
    let s = (clock - at).max(0.0);
    if s < 60.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{:.0} min ago", s / 60.0)
    } else {
        format!("{:.0} h ago", s / 3600.0)
    }
}

fn detail(r: &Rival, clock: f64) -> El {
    let (word, c) = mood(r.standing);
    let tc = treaty_color(&r.treaty);
    let head = El::new()
        .class("row")
        .with_style(|s| s.gap(16.0).align_items(Align::Center))
        .child(shield(r, 80.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(2.0).grow(1.0).shrink(1.0).basis(0.0))
                .child(
                    El::text(r.name.clone())
                        .class("ink_title")
                        .text_style(|t| t.size(30.0))
                        .pick_ignore(),
                )
                .child(
                    El::text(r.temperament.clone())
                        .class("ink")
                        .text_style(|t| t.size(14.0).italic())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(move |s| {
                            s.gap(8.0)
                                .align_items(Align::Center)
                                .padding([4.0, 12.0, 4.0, 6.0])
                                .radius(14.0)
                                .margin([6.0, 0.0, 0.0, 0.0])
                                .background(Fill::Solid(tc.with_alpha(0.16)))
                                .border(1.0, tc.with_alpha(0.6))
                        })
                        .child(
                            El::icon(treaty_icon(&r.treaty))
                                .with_style(|s| s.size(24.0, 24.0).pick(Pick::Ignore)),
                        )
                        .child(
                            El::text(if r.treaty_text.is_empty() {
                                treaty_name(&r.treaty).to_string()
                            } else {
                                r.treaty_text.clone()
                            })
                            .class("ink")
                            .text_style(|t| t.size(14.0).bold().nowrap())
                            .pick_ignore(),
                        ),
                ),
        )
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0).align_items(Align::Center))
                .child(
                    El::new()
                        .paint(DIAL)
                        .with_style(|s| s.size(190.0, 112.0).pick(Pick::Ignore)),
                )
                .child(
                    El::text(format!("{word}  {:+.0}", r.standing))
                        .class("ink")
                        .text_style(|t| t.size(16.0).bold().tabular().nowrap())
                        .with_style(move |s| s.color(c.lerp(Colors::ink(), 0.3)))
                        .pick_ignore(),
                ),
        );
    // Diplomacy: the treaty, how the standing has moved of late.
    let moved: f32 = r.events.iter().map(|e| e.2).sum();
    let diplomacy = El::new()
        .class("col")
        .with_style(|s| s.gap(5.0))
        .child(stat(
            "treaty",
            treaty_name(&r.treaty).into(),
            Some(tc.lerp(Colors::ink(), 0.3)),
        ))
        .child(stat(
            "of late",
            format!("{moved:+.0}"),
            Some(if moved >= 0.0 {
                Colors::good_ink()
            } else {
                Colors::bad_ink()
            }),
        ))
        .child(stat("standing", format!("{:+.0} of 100", r.standing), None));
    let trade_c = if r.trade_per_min >= 0.0 {
        Colors::good_ink()
    } else {
        Colors::bad_ink()
    };
    let trade = El::new()
        .class("col")
        .with_style(|s| s.gap(5.0))
        .child(stat(
            "with us",
            format!("{:+.0} a min", r.trade_per_min),
            Some(trade_c),
        ))
        .child(stat(
            if r.estimate {
                "their wealth (est.)"
            } else {
                "their wealth"
            },
            super::fmt_int(r.wealth as f64),
            None,
        ))
        .child(stat(
            "people · islands",
            format!("{} · {}", super::fmt_int(r.people as f64), r.islands),
            None,
        ))
        .child_opt(good_chips("buy", &r.buys))
        .child_opt(good_chips("sell", &r.sells));
    let theirs = r.firepower.max(r.warships);
    let ours = r.our_firepower.max(r.our_warships);
    let edge = if ours + theirs < 0.01 {
        ("No warships on either side", Colors::ink_dim())
    } else if ours > theirs * 1.25 {
        ("We hold the seas", Colors::good_ink())
    } else if theirs > ours * 1.25 {
        ("They outgun us", Colors::bad_ink())
    } else {
        ("Evenly matched", Colors::ink())
    };
    let military = El::new()
        .class("col")
        .with_style(|s| s.gap(5.0))
        .child(stat(
            "warships",
            format!("{:.0} ours · {:.0} theirs", r.our_warships, r.warships),
            None,
        ))
        .child(stat(
            "guns",
            format!("{:.0} · {:.0}", r.our_firepower, r.firepower),
            None,
        ))
        .child(balance(ours, theirs, r.color))
        .child(
            El::text(edge.0)
                .class("ink")
                .text_style(|t| t.size(13.0).bold().nowrap())
                .with_style(move |s| s.color(edge.1))
                .pick_ignore(),
        )
        .child_opt((r.lost + r.taken > 0.0).then(|| {
            stat(
                "lately lost · taken",
                format!("{:.0} · {:.0}", r.lost, r.taken),
                None,
            )
        }));
    let panels = El::new()
        .class("row")
        .with_style(|s| s.gap(12.0).align_items(Align::Stretch))
        .child(panel("isle_rel_diplomacy", "Diplomacy", diplomacy))
        .child(panel("isle_rel_trade", "Trade", trade))
        .child(panel("isle_rel_military", "Military", military));
    // Of late.
    let mut events = El::new()
        .class("col")
        .with_style(|s| s.gap(4.0).grow(1.0).basis(0.0))
        .child(brass::label("Of late", true));
    if r.events.is_empty() {
        events = events.child(
            El::text("Nothing yet.")
                .class("ink")
                .text_style(|t| t.italic().size(13.0))
                .pick_ignore(),
        );
    }
    for (at, t, d) in r.events.iter().take(6) {
        let dc = if *d >= 0.0 {
            Colors::good_ink()
        } else {
            Colors::bad_ink()
        };
        events = events.child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).align_items(Align::Start))
                .child(
                    El::text(if d.abs() < 0.5 {
                        "·".into()
                    } else {
                        format!("{d:+.0}")
                    })
                    .class("ink")
                    .text_style(|t| t.size(13.0).bold().tabular().nowrap())
                    .with_style(move |s| s.color(dc).width(34.0))
                    .pick_ignore(),
                )
                .child(
                    El::text(t.clone())
                        .class("ink")
                        .text_style(|t| t.size(13.0))
                        .with_style(|s| s.grow(1.0).shrink(1.0).basis(0.0))
                        .pick_ignore(),
                )
                .child(
                    El::text(ago(clock, *at))
                        .class("ink")
                        .text_style(|t| t.size(11.5).nowrap())
                        .with_style(|s| s.color(Colors::ink_dim()))
                        .pick_ignore(),
                ),
        );
    }
    let mut wants = El::new()
        .class("col")
        .with_style(|s| s.gap(4.0).width(250.0))
        .child(brass::label("What would please them", true));
    for w in r.wants.iter().take(5) {
        wants = wants.child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(6.0).align_items(Align::Start))
                .child(
                    El::text("◆")
                        .text_style(|t| t.size(10.0))
                        .with_style(|s| s.color(Colors::brass()).margin([3.0, 0.0, 0.0, 0.0]))
                        .pick_ignore(),
                )
                .child(
                    El::text(w.clone())
                        .class("ink")
                        .text_style(|t| t.size(13.0))
                        .with_style(|s| s.grow(1.0).shrink(1.0).basis(0.0))
                        .pick_ignore(),
                ),
        );
    }
    let lower = El::new()
        .class("row")
        .with_style(|s| s.gap(18.0).align_items(Align::Start))
        .child(events)
        .child(wants);
    let moves = El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).wrap(true).justify(Justify::End))
        .children(r.actions.iter().enumerate().map(|(k, m)| {
            let warlike = m.action.ends_with(".war");
            let b = El::button(m.label.clone(), format!("isle.rel.do:{}:{k}", r.id))
                .class(if warlike {
                    "brass_button_dark"
                } else {
                    "brass_button"
                })
                .text_style(|t| t.nowrap())
                .disabled(!m.enabled);
            if m.enabled || m.why_not.is_empty() {
                b
            } else {
                b.tooltip(m.label.clone(), m.why_not.clone())
            }
        }));
    El::new()
        .class("col")
        .with_style(|s| s.gap(14.0))
        .child(head)
        .child(brass::divider(640.0))
        .child(panels)
        .child(lower)
        .child(moves)
}

fn body(list: &[Rival], picked: Option<u64>, clock: f64) -> Vec<El> {
    if list.is_empty() {
        return vec![
            El::new()
                .class("col")
                .with_style(|s| s.gap(10.0).align_items(Align::Center).padding(40.0))
                .child(
                    El::icon("isle_relations")
                        .with_style(|s| s.size(64.0, 64.0).opacity(0.5).pick(Pick::Ignore)),
                )
                .child(
                    El::text("No rival houses met yet. They come with the far waters.")
                        .class("ink")
                        .text_style(|t| t.size(16.0).italic())
                        .pick_ignore(),
                ),
        ];
    }
    let pick = picked
        .and_then(|p| list.iter().find(|r| r.id == p))
        .unwrap_or(&list[0]);
    let left = brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(10.0)
                .padding([18.0, 18.0])
                .width(430.0)
                .radius([4.0, 0.0, 0.0, 4.0])
        })
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(El::text("Rival houses").class("ink_title").pick_ignore())
                .child(
                    El::text(format!("{} met · best of terms first", list.len()))
                        .class("ink_label")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                ),
        )
        .children({
            let mut v: Vec<&Rival> = list.iter().collect();
            v.sort_by(|a, b| b.standing.total_cmp(&a.standing));
            v.into_iter()
                .map(|r| rival_card(r, r.id == pick.id))
                .collect::<Vec<_>>()
        });
    let spine = El::new().with_style(|s| {
        s.width(14.0)
            .background(Fill::Linear {
                from: Color::hex(0x6f5530),
                to: Color::hex(0x2a1a0a),
                angle_deg: 0.0,
            })
            .pick(Pick::Ignore)
    });
    let right = brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .padding([20.0, 24.0])
                .width(720.0)
                .radius([0.0, 4.0, 4.0, 0.0])
        })
        .child(detail(pick, clock));
    vec![
        El::new()
            .class("row")
            .with_style(|s| s.gap(0.0).align_items(Align::Stretch))
            .child(left)
            .child(spine)
            .child(right),
    ]
}

fn el() -> El {
    let header = El::new()
        .class("row")
        .with_style(|s| s.gap(18.0).align_items(Align::Center))
        .child(brass::divider(70.0))
        .child(
            El::text("Relations")
                .class("brass_title")
                .text_style(|t| t.nowrap().shadow().size(30.0))
                .pick_ignore(),
        )
        .child(brass::divider(70.0))
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(
            El::text("where the company stands with every rival house")
                .class("brass_label")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        )
        .child(
            brass::medallion("isle_close", 36.0, "isle.relations")
                .tooltip("", "Close [color=#c9b07a](P or Esc)[/color]"),
        );
    let book = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(14.0)
                .padding([16.0, 24.0, 22.0, 24.0])
                .enter(engine_ui::style::Enter::pop(0.35, 0.94))
        })
        .child(header)
        .child(El::new().id(BODY_ID).class("col"));
    El::new()
        .with_style(|s| {
            s.fill_parent()
                .background(Fill::Solid(Color::hexa(0x0a0f1650)))
                .align_items(Align::Center)
                .justify(Justify::Center)
                .padding([96.0, 0.0, 0.0, 0.0])
                .pick(Pick::Block)
        })
        .child(book)
}

/// Open the book, or close it.
pub fn toggle(world: &mut World) {
    if world.resource::<Relations>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        world.resource_mut::<Relations>().open = false;
        return;
    }
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    if engine_ui::screens::spawn_screen(world, screen, &el()).is_ok() {
        let mut r = world.resource_mut::<Relations>();
        r.open = true;
        r.sig.clear();
    }
    refresh(world);
}

/// Rebuild the pages when what they show changes; ease the dial.
pub fn refresh(world: &mut World) {
    if !world.resource::<Relations>().open {
        return;
    }
    let list = rivals(world).unwrap_or_default();
    let clock = world.resource::<crate::sim::Economy>().clock_s;
    let picked = world.resource::<Relations>().picked;
    let pick = picked
        .and_then(|p| list.iter().find(|r| r.id == p))
        .or(list.first());
    if let Some(p) = pick {
        world.resource_mut::<Relations>().dial_goal = p.standing;
    }
    let sig = format!(
        "{picked:?}|{}|{}",
        list.iter()
            .map(|r| format!(
                "{}:{:.0}:{}:{:.0}:{}:{}",
                r.id,
                r.standing,
                r.treaty,
                r.trade_per_min,
                r.events.len(),
                r.actions.iter().map(|a| a.enabled as u8).sum::<u8>()
            ))
            .collect::<Vec<_>>()
            .join(","),
        (clock / 60.0).floor()
    );
    if world.resource::<Relations>().sig == sig {
        return;
    }
    let els = body(&list, picked, clock);
    if let Some(e) = engine_ui::input::find_id(world, BODY_ID) {
        let _ = engine_ui::el::replace_children(world, e, &els);
        world.resource_mut::<Relations>().sig = sig;
    }
}

/// `isle.rel.*` and `isle.relations`. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a == "isle.relations" {
        toggle(world);
        return true;
    }
    if let Some(id) = a
        .strip_prefix("isle.rel.pick:")
        .and_then(|s| s.parse::<u64>().ok())
    {
        world.resource_mut::<Relations>().picked = Some(id);
        refresh(world);
        return true;
    }
    if let Some(rest) = a.strip_prefix("isle.rel.do:") {
        let mut p = rest.split(':');
        let id: Option<u64> = p.next().and_then(|s| s.parse().ok());
        let k: Option<usize> = p.next().and_then(|s| s.parse().ok());
        if let (Some(id), Some(k)) = (id, k) {
            let list = rivals(world).unwrap_or_default();
            if let Some(m) = list
                .iter()
                .find(|r| r.id == id)
                .and_then(|r| r.actions.get(k))
                .cloned()
            {
                do_move(world, id, &m);
            }
        }
        refresh(world);
        return true;
    }
    false
}

fn do_move(world: &mut World, id: u64, m: &Move) {
    let mut args = m.args.clone().unwrap_or_else(|| json!({}));
    if let Some(o) = args.as_object_mut() {
        o.entry("rival").or_insert(json!(id));
        if m.action.ends_with(".gift") {
            o.entry("coins").or_insert(json!(1000));
        }
    }
    let r = engine::core::registry::call_api(world, &m.action, args);
    let (kind, text) = match r {
        Ok(v) => (
            engine_ui::toast::ToastKind::Good,
            v.get("text")
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| format!("{}: done.", m.label)),
        ),
        Err(e) => (engine_ui::toast::ToastKind::Warn, e),
    };
    engine_ui::toast::notify(world, kind, "isle_relations", "Relations", &text, 5.0);
}

/// The standing dial: a half ring from bitter (left) to warm (right) in five bands, ticks,
/// and a brass needle easing to the standing.
fn paint_dial(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let r = ctx.rect;
    let px = ctx.px;
    let (val, goal) = {
        let s = world.resource::<Relations>();
        (s.dial, s.dial_goal)
    };
    // Ease the needle when the painter runs each frame; it lands on the standing either way.
    let v = if (goal - val).abs() > 0.5 {
        val + (goal - val) * 0.18
    } else {
        goal
    };
    world.resource_mut::<Relations>().dial = v;
    let c = Vec2::new(r.x + r.w * 0.5, r.y + r.h - 22.0 * px);
    let rad = (r.w * 0.5 - 12.0 * px).min(r.h - 34.0 * px);
    let pi = std::f32::consts::PI;
    let bands = [
        Color::hex(0xb03a24),
        Color::hex(0xd0702a),
        Color::hex(0xc8a040),
        Color::hex(0x7ab444),
        Color::hex(0x3aa84a),
    ];
    let th = 14.0 * px;
    ctx.ring(
        c,
        Vec2::splat(rad),
        th + 4.0 * px,
        Colors::outline(),
        -pi * 0.5,
        pi,
        0.0,
    );
    for (k, col) in bands.iter().enumerate() {
        let start = -pi * 0.5 + pi * k as f32 / 5.0;
        ctx.ring(
            c,
            Vec2::splat(rad),
            th,
            *col,
            start + 0.01,
            pi / 5.0 - 0.02,
            0.0,
        );
    }
    // Ticks every 20.
    for k in 0..=10 {
        let a = -pi * 0.5 + pi * k as f32 / 10.0;
        let d = Vec2::new(a.sin(), -a.cos());
        let (inner, w) = if k % 5 == 0 {
            (rad - th * 1.4, 2.2)
        } else {
            (rad - th * 0.95, 1.2)
        };
        ctx.line(
            c + d * inner,
            c + d * (rad - th * 0.55),
            w * px,
            Colors::outline(),
            0.0,
        );
    }
    // The needle.
    let f = ((v + 100.0) / 200.0).clamp(0.0, 1.0);
    let a = -pi * 0.5 + pi * f;
    let d = Vec2::new(a.sin(), -a.cos());
    let tip = c + d * (rad + th * 0.2);
    ctx.line(c, tip, 6.0 * px, Colors::outline(), 0.0);
    ctx.line(c, tip, 3.0 * px, Colors::brass_light(), 0.0);
    let (_, mc) = mood(v);
    ctx.glow(tip, 10.0 * px, mc.with_alpha(0.6), 1.6);
    let hub = 9.0 * px;
    ctx.rect(
        Rect::new(c.x - hub, c.y - hub, hub * 2.0, hub * 2.0),
        Colors::brass(),
        hub,
        2.0 * px,
        Colors::outline(),
    );
    for (t, x) in [("bitter", 0.0f32), ("warm", 1.0)] {
        let spec = TextSpec {
            text: t.into(),
            size: 11.0 * px,
            weight: 700,
            color: Colors::ink_dim(),
            family: Some(DISPLAY_FONT.into()),
            wrap: false,
            ..Default::default()
        };
        let p = Vec2::new(c.x + (x * 2.0 - 1.0) * rad, c.y + 4.0 * px);
        ctx.text_spec(&spec, p, Vec2::new(0.5, 0.0));
    }
}
