//! The ledger: an open book on a brass-framed plate. The left page lists every good on the
//! chosen island (in store, used and made each minute as a two-way bar, how much of the
//! demand is met); the right page charts the treasury and the island's people over time and
//! shows how many jobs are filled. Opened from the map table's medallion or with L.
//!
//! Part of the HUD (hud.rs), the UI thread's.

use std::collections::VecDeque;

use engine::prelude::*;
use engine_ui::Color;
use engine_ui::data::UiData;
use engine_ui::draw::DrawCtx;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Node;
use engine_ui::node::{Rect, TextAlign};
use engine_ui::style::{Align, Direction, Fill, Justify, Pick, pct};
use engine_ui::text::TextSpec;

use crate::defs::{Good, Tier};
use crate::island;
use crate::sim::Economy;

pub const SCREEN: &str = "isle_ledger";
const COINS_PAINTER: &str = "isle_led_coins";
const PEOPLE_PAINTER: &str = "isle_led_people";
/// Game seconds between samples, and how many are kept (an hour of game time).
const SAMPLE_S: f64 = 10.0;
const KEEP: usize = 360;

#[derive(Clone, Debug)]
struct Sample {
    clock_s: f64,
    coins: f64,
    /// People per island and tier.
    people: Vec<[f32; 2]>,
}

/// The ledger's state and the history its charts draw.
#[derive(Resource, Default)]
pub struct Ledger {
    pub open: bool,
    /// The island shown.
    pub island: usize,
    samples: VecDeque<Sample>,
}

impl Ledger {
    /// Note the economy's state if a sample is due.
    pub fn record(&mut self, econ: &Economy) {
        if self
            .samples
            .back()
            .is_some_and(|s| econ.clock_s < s.clock_s + SAMPLE_S)
        {
            return;
        }
        self.samples.push_back(Sample {
            clock_s: econ.clock_s,
            coins: econ.coins,
            people: econ.islands.iter().map(|b| b.population).collect(),
        });
        while self.samples.len() > KEEP {
            self.samples.pop_front();
        }
    }
}

pub fn install(world: &mut World) {
    world.init_resource::<Ledger>();
    if let Some(mut p) = world.get_resource_mut::<engine_ui::Painters>() {
        p.add(COINS_PAINTER, paint_coins);
        p.add(PEOPLE_PAINTER, paint_people);
    }
}

fn page_title(text: &str, sub: &str) -> El {
    El::new()
        .class("col")
        .with_style(|s| s.gap(0.0))
        .child(El::text(text).class("ink_title").pick_ignore())
        .child(
            El::text(sub)
                .class("ink_label")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        )
}

/// A column heading in small caps.
fn head(text: &str, width: f32, right: bool) -> El {
    El::text(text)
        .class("ink_label")
        .text_style(move |t| {
            let t = t.nowrap();
            if right { t.align(TextAlign::Right) } else { t }
        })
        .with_style(move |s| s.width(width))
        .pick_ignore()
}

/// A good's row: badge, name, stock, a used ← → made bar, the net and the demand met.
fn good_row(g: Good) -> El {
    let i = g.index();
    let half = |id: String, color: Color, left: bool| {
        El::new()
            .with_style(move |s| {
                s.width(pct(50.0))
                    .height(pct(100.0))
                    .direction(Direction::Row)
                    .justify(if left { Justify::End } else { Justify::Start })
                    .pick(Pick::Ignore)
            })
            .child(El::new().id(id).with_style(move |s| {
                s.width(pct(0.0))
                    .height(pct(100.0))
                    .radius(if left {
                        [2.0, 0.0, 0.0, 2.0]
                    } else {
                        [0.0, 2.0, 2.0, 0.0]
                    })
                    .background(Fill::vertical(color.lerp(Color::WHITE, 0.25), color))
                    .pick(Pick::Ignore)
            }))
    };
    let bar = El::new()
        .class("brass_track")
        .with_style(|s| {
            s.width(150.0)
                .height(12.0)
                .direction(Direction::Row)
                .padding(0.0)
                .gap(0.0)
        })
        .child(half(
            format!("isle.led.used{i}"),
            Color::hex(0xa8452c),
            true,
        ))
        .child(El::new().with_style(|s| {
            s.width(1.5)
                .height(pct(100.0))
                .background(Fill::Solid(Color::hex(0x2a1a0a)))
                .pick(Pick::Ignore)
        }))
        .child(half(
            format!("isle.led.made{i}"),
            Color::hex(0x4c7a34),
            false,
        ));
    El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).align_items(Align::Center).height(30.0))
        .tooltip(g.name(), format!("{{isle.led.tip{i}}}"))
        .child(brass::badge(g.icon(), 28.0))
        .child(
            El::text(g.name())
                .class("ink")
                .text_style(|t| t.bold().nowrap())
                .with_style(|s| s.width(92.0))
                .pick_ignore(),
        )
        .child(
            El::text(format!("{{isle.led.stock{i}}}"))
                .class("ink")
                .text_style(|t| t.nowrap().tabular().align(TextAlign::Right))
                .with_style(|s| s.width(40.0))
                .pick_ignore(),
        )
        .child(bar)
        .child(
            El::text(format!("{{isle.led.net{i}}}"))
                .class("ink")
                .text_style(|t| t.nowrap().tabular().align(TextAlign::Right))
                .with_style(|s| s.width(54.0))
                .pick_ignore(),
        )
        .child(
            El::text(format!("{{isle.led.met{i}}}"))
                .class("ink")
                .text_style(|t| t.nowrap().tabular().align(TextAlign::Right))
                .with_style(|s| s.width(48.0))
                .pick_ignore(),
        )
}

fn chart(painter: &str, h: f32) -> El {
    El::new()
        .paint(painter)
        .with_style(move |s| s.width(pct(100.0)).height(h).pick(Pick::Ignore))
}

fn legend(color: Color, text: &str) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(6.0).align_items(Align::Center))
        .child(El::new().with_style(move |s| {
            s.size(14.0, 4.0)
                .radius(2.0)
                .background(Fill::Solid(color))
                .pick(Pick::Ignore)
        }))
        .child(El::text(text).class("ink_label").pick_ignore())
}

fn workforce(t: Tier) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).align_items(Align::Center))
        .child(
            El::text(t.name())
                .class("ink")
                .text_style(|x| x.bold().nowrap())
                .with_style(|s| s.width(92.0))
                .pick_ignore(),
        )
        .child(
            El::new()
                .class("brass_track")
                .with_style(|s| s.width(220.0).height(12.0))
                .child(
                    El::new()
                        .class("brass_fill")
                        .id(format!("isle.led.work{}", t.index()))
                        .with_style(|s| s.width(pct(0.0))),
                ),
        )
        .child(
            El::text(format!("{{isle.led.work{}}}", t.index()))
                .class("ink")
                .text_style(|x| x.nowrap().tabular())
                .pick_ignore(),
        )
}

/// The book.
pub fn el(shown: usize) -> El {
    let tabs = El::new().class("row").with_style(|s| s.gap(8.0)).children(
        island::islands().iter().enumerate().map(|(i, isle)| {
            El::button(isle.name, format!("isle.led:{i}"))
                .class(if i == shown {
                    "brass_button"
                } else {
                    "brass_button_dark"
                })
                .id(format!("isle.led:{i}"))
        }),
    );
    let header = El::new()
        .class("row")
        .with_style(|s| s.gap(18.0).align_items(Align::Center))
        .child(brass::divider(70.0))
        .child(
            El::text("Ledger")
                .class("brass_title")
                .text_style(|t| t.nowrap().shadow().size(30.0))
                .pick_ignore(),
        )
        .child(brass::divider(70.0))
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(tabs)
        .child(
            brass::medallion("isle_close", 36.0, "isle.ledger")
                .tooltip("", "Close [color=#c9b07a](L or Esc)[/color]"),
        );
    let left = brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(2.0)
                .padding([18.0, 22.0, 18.0, 24.0])
                .width(560.0)
                .radius([4.0, 0.0, 0.0, 4.0])
        })
        .child(page_title(
            "{isle.led.goods_title}",
            "made and used each minute, lately",
        ))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.justify(Justify::Center))
                .child(brass::divider(320.0)),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).margin([4.0, 0.0, 4.0, 0.0]))
                .child(head("good", 130.0, false))
                .child(head("store", 40.0, true))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.width(150.0).justify(Justify::SpaceBetween))
                        .child(head("used", 60.0, false))
                        .child(head("made", 60.0, true)),
                )
                .child(head("a minute", 54.0, true))
                .child(head("need met", 48.0, true)),
        )
        .children(Good::ALL.into_iter().map(good_row));
    let spine = El::new().with_style(|s| {
        s.width(14.0)
            .background(Fill::Linear {
                from: Color::hex(0x6f5530),
                to: Color::hex(0x2a1a0a),
                angle_deg: 0.0,
            })
            .shadow(engine_ui::style::Shadow::new(
                Color::hexa(0x00000080),
                6.0,
                0.0,
            ))
            .pick(Pick::Ignore)
    });
    let right = brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(8.0)
                .padding([18.0, 24.0, 18.0, 22.0])
                .width(480.0)
                .radius([0.0, 4.0, 4.0, 0.0])
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.justify(Justify::SpaceBetween).align_items(Align::End))
                .child(page_title("Treasury", "{isle.led.since}"))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).align_items(Align::End))
                        .child(
                            El::text("{isle.coins}")
                                .class("ink_title")
                                .text_style(|t| t.tabular().size(26.0))
                                .pick_ignore(),
                        )
                        .child(El::text("{isle.led.flow}").class("ink").pick_ignore()),
                ),
        )
        .child(chart(COINS_PAINTER, 150.0))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.justify(Justify::Center))
                .child(brass::divider(320.0)),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.justify(Justify::SpaceBetween).align_items(Align::End))
                .child(page_title("{isle.led.people_title}", "{isle.led.people}"))
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(14.0))
                        .child(legend(FISHERFOLK, "fisherfolk"))
                        .child(legend(LABOURERS, "labourers")),
                ),
        )
        .child(chart(PEOPLE_PAINTER, 120.0))
        .child(
            El::text("Jobs filled")
                .class("ink_label")
                .with_style(|s| s.margin([6.0, 0.0, 0.0, 0.0]))
                .pick_ignore(),
        )
        .child(workforce(Tier::Fisherfolk))
        .child(workforce(Tier::Labourers));
    let book = brass::plate()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(14.0)
                .padding([16.0, 24.0, 22.0, 24.0])
                .enter(engine_ui::style::Enter::pop(0.35, 0.94))
        })
        .child(header)
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(0.0).align_items(Align::Stretch))
                .child(left)
                .child(spine)
                .child(right),
        );
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

const FISHERFOLK: Color = Color {
    r: 0.17,
    g: 0.33,
    b: 0.48,
    a: 1.0,
};
const LABOURERS: Color = Color {
    r: 0.62,
    g: 0.24,
    b: 0.13,
    a: 1.0,
};

fn set_width(world: &mut World, id: &str, frac: f32) {
    if let Some(e) = engine_ui::input::find_id(world, id)
        && let Some(mut n) = world.get_mut::<Node>(e)
    {
        let w = Some(pct((frac * 100.0).clamp(0.0, 100.0)));
        if n.style.width != w {
            n.style.width = w;
        }
    }
}

fn signed(x: f32) -> String {
    if x.abs() < 0.05 {
        "·".into()
    } else if x > 0.0 {
        format!("[color=#2f6e28]+{x:.1}[/color]")
    } else {
        format!("[color=#9a2f1c]−{:.1}[/color]", -x)
    }
}

/// Fill the ledger's values (cheap; called with the bars' refresh).
pub fn update(world: &mut World) {
    let clock = super::clock;
    let econ = world.resource::<Economy>().clone();
    {
        let mut l = world.resource_mut::<Ledger>();
        l.record(&econ);
    }
    if !world.resource::<Ledger>().open {
        return;
    }
    let shown = world
        .resource::<Ledger>()
        .island
        .min(econ.islands.len() - 1);
    let since = world
        .resource::<Ledger>()
        .samples
        .front()
        .map(|s| s.clock_s)
        .unwrap_or(econ.clock_s);
    let books = econ.islands[shown].clone();
    let name = island::islands()[shown].name;
    // Bars share one scale so goods compare at a glance.
    let top = Good::ALL
        .iter()
        .map(|g| books.made_per_min[g.index()].max(books.used_per_min[g.index()]))
        .fold(1.0f32, f32::max);
    {
        let mut d = world.resource_mut::<UiData>();
        d.set("isle.led.goods_title", format!("Goods of {name}"));
        d.set("isle.led.people_title", format!("People of {name}"));
        d.set(
            "isle.led.people",
            format!(
                "{:.0} fisherfolk · {:.0} labourers",
                books.population[0], books.population[1]
            ),
        );
        d.set("isle.led.since", format!("since {}", clock(since)));
        d.set(
            "isle.led.flow",
            format!(
                "taxes [color=#2f6e28]+{:.0}[/color] · upkeep [color=#9a2f1c]−{:.0}[/color] a minute",
                econ.taxes_per_min, econ.upkeep_per_min
            ),
        );
        for g in Good::ALL {
            let i = g.index();
            let (made, used) = (books.made_per_min[i], books.used_per_min[i]);
            d.set(
                &format!("isle.led.stock{i}"),
                format!("{:.0}", books.stock[i]),
            );
            d.set(&format!("isle.led.net{i}"), signed(made - used));
            let met = if used > 0.01 {
                let p = books.supply[i] * 100.0;
                let c = if p >= 99.0 {
                    "#2f6e28"
                } else if p >= 70.0 {
                    "#8a5a12"
                } else {
                    "#9a2f1c"
                };
                format!("[color={c}]{p:.0}%[/color]")
            } else {
                "—".into()
            };
            d.set(&format!("isle.led.met{i}"), met);
            d.set(
                &format!("isle.led.tip{i}"),
                format!(
                    "[b]In store[/b]  {:.0} of {:.0}\n[b]Made[/b]  {made:.1} a minute\n[b]Used[/b]  {used:.1} a minute",
                    books.stock[i], books.capacity
                ),
            );
        }
        for t in Tier::ALL {
            let k = t.index();
            d.set(
                &format!("isle.led.work{k}"),
                format!("{:.0}% of {:.0}", books.workforce[k] * 100.0, books.jobs[k]),
            );
        }
    }
    for g in Good::ALL {
        let i = g.index();
        set_width(
            world,
            &format!("isle.led.made{i}"),
            books.made_per_min[i] / top,
        );
        set_width(
            world,
            &format!("isle.led.used{i}"),
            books.used_per_min[i] / top,
        );
    }
    for t in Tier::ALL {
        let k = t.index();
        let w = if books.jobs[k] > 0.0 {
            books.workforce[k]
        } else {
            0.0
        };
        set_width(world, &format!("isle.led.work{k}"), w);
    }
}

/// Open the ledger on `island`, or close it.
pub fn show(world: &mut World, island: Option<usize>) {
    let was_open = world.resource::<Ledger>().open;
    if was_open {
        engine_ui::screens::hide(world, SCREEN, true);
        world.resource_mut::<Ledger>().open = false;
    }
    let Some(i) = island else {
        return;
    };
    world.resource_mut::<Ledger>().island = i;
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    if engine_ui::screens::spawn_screen(world, screen, &el(i)).is_ok() {
        world.resource_mut::<Ledger>().open = true;
    }
}

/// One line chart on the right page: dotted guides with values, the series as ink lines over
/// a wash, the latest value marked.
fn line_chart(
    world: &mut World,
    ctx: &mut DrawCtx,
    series: &[(Vec<(f64, f32)>, Color)],
    fmt: fn(f32) -> String,
) {
    let r = ctx.rect;
    let px = ctx.px;
    let ink = Colors::ink();
    let faint = Colors::ink().with_alpha(0.28);
    let pts: Vec<&(f64, f32)> = series.iter().flat_map(|(s, _)| s.iter()).collect();
    if series.iter().all(|(s, _)| s.len() < 2) {
        ctx.text(
            "The ledger fills in as time passes.",
            14.0,
            400,
            Colors::ink_dim(),
            r.center(),
            Vec2::splat(0.5),
        );
        return;
    }
    let (t0, t1) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (lo, hi) = pts
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let lo = if lo >= 0.0 { 0.0 } else { lo };
    let span = (hi - lo).max(1.0);
    // Round the top to a tidy step.
    let step = {
        let raw = span / 3.0;
        let mag = 10f32.powf(raw.log10().floor());
        let n = raw / mag;
        mag * if n <= 1.0 {
            1.0
        } else if n <= 2.0 {
            2.0
        } else if n <= 5.0 {
            5.0
        } else {
            10.0
        }
    };
    let hi = lo + (span / step).ceil() * step;
    let left = 46.0 * px;
    let bottom = 18.0 * px;
    let top = 10.0 * px;
    let plot = Rect::new(
        r.x + left,
        r.y + top,
        r.w - left - 6.0 * px,
        r.h - bottom - top,
    );
    let x_of = |t: f64| plot.x + ((t - t0) / (t1 - t0).max(1e-6)) as f32 * plot.w;
    let y_of = |v: f32| plot.y + plot.h - (v - lo) / (hi - lo) * plot.h;
    // Guides.
    let mut v = lo;
    while v <= hi + step * 0.01 {
        let y = y_of(v);
        ctx.line(
            Vec2::new(plot.x, y),
            Vec2::new(plot.x + plot.w, y),
            1.0 * px,
            faint,
            4.0 * px,
        );
        let spec = TextSpec {
            text: fmt(v),
            size: 11.5 * px,
            color: Colors::ink_dim(),
            tabular: true,
            wrap: false,
            ..Default::default()
        };
        ctx.text_spec(&spec, Vec2::new(plot.x - 6.0 * px, y), Vec2::new(1.0, 0.5));
        v += step;
    }
    ctx.line(
        Vec2::new(plot.x, plot.y + plot.h),
        Vec2::new(plot.x + plot.w, plot.y + plot.h),
        1.2 * px,
        ink.with_alpha(0.7),
        0.0,
    );
    // Dates under the ends.
    let clock = super::clock;
    for (t, a) in [(t0, 0.0), (t1, 1.0)] {
        let spec = TextSpec {
            text: clock(t),
            size: 11.5 * px,
            color: Colors::ink_dim(),
            wrap: false,
            ..Default::default()
        };
        ctx.text_spec(
            &spec,
            Vec2::new(x_of(t), plot.y + plot.h + 3.0 * px),
            Vec2::new(a, 0.0),
        );
    }
    for (k, (s, color)) in series.iter().enumerate() {
        if s.len() < 2 {
            continue;
        }
        // A wash under the first series, as thin columns.
        if k == 0 {
            for w in s.windows(2) {
                let (xa, xb) = (x_of(w[0].0), x_of(w[1].0));
                let y = y_of(w[0].1.max(w[1].1).min(hi)).min(y_of(lo));
                let col = Rect::new(xa, y, (xb - xa).max(1.0) + 0.5, y_of(lo) - y);
                ctx.rect(col, color.with_alpha(0.13), 0.0, 0.0, Color::TRANSPARENT);
            }
        }
        for w in s.windows(2) {
            let a = Vec2::new(x_of(w[0].0), y_of(w[0].1));
            let b = Vec2::new(x_of(w[1].0), y_of(w[1].1));
            ctx.line(a, b, 2.2 * px, *color, 0.0);
        }
        let last = s[s.len() - 1];
        let p = Vec2::new(x_of(last.0), y_of(last.1));
        ctx.glow(p, 8.0 * px, color.with_alpha(0.35), 2.0);
        ctx.rect(
            Rect::new(p.x - 3.0 * px, p.y - 3.0 * px, 6.0 * px, 6.0 * px),
            *color,
            3.0 * px,
            1.0 * px,
            Color::hex(0xf3e6c4),
        );
    }
    let _ = world;
}

fn short(v: f32) -> String {
    let a = v.abs();
    if a >= 10_000.0 {
        format!("{:.0}k", v / 1000.0)
    } else if a >= 1000.0 {
        format!("{:.1}k", v / 1000.0)
    } else {
        format!("{v:.0}")
    }
}

fn paint_coins(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let s: Vec<(f64, f32)> = world
        .resource::<Ledger>()
        .samples
        .iter()
        .map(|s| (s.clock_s, s.coins as f32))
        .collect();
    line_chart(world, ctx, &[(s, Color::hex(0x8a5a12))], short);
}

fn paint_people(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let l = world.resource::<Ledger>();
    let i = l.island;
    let tier = |k: usize| -> Vec<(f64, f32)> {
        l.samples
            .iter()
            .map(|s| (s.clock_s, s.people.get(i).map_or(0.0, |p| p[k])))
            .collect()
    };
    let series = [(tier(0), FISHERFOLK), (tier(1), LABOURERS)];
    line_chart(world, ctx, &series, short);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_every_ten_game_seconds_and_keeps_an_hour() {
        let mut l = Ledger::default();
        let mut econ = Economy::default();
        for s in 0..10_000 {
            econ.clock_s = s as f64;
            l.record(&econ);
        }
        assert_eq!(l.samples.len(), KEEP);
        let gaps: Vec<f64> = l
            .samples
            .iter()
            .zip(l.samples.iter().skip(1))
            .map(|(a, b)| b.clock_s - a.clock_s)
            .collect();
        assert!(
            gaps.iter().all(|g| (*g - SAMPLE_S).abs() < 1e-9),
            "{gaps:?}"
        );
    }
}
