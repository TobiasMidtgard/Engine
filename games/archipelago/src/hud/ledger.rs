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
/// The biggest costs under the coin chart, and moving up under the people.
const DRAINS_ID: &str = "isle.led.drains";
/// The goods page's filter chips, column heads and rows (rebuilt when what they show changes).
const GOODS_ID: &str = "isle.led.goods";
const MOVING_ID: &str = "isle.led.moving";
const KEEP: usize = 360;

#[derive(Clone, Debug)]
struct Sample {
    clock_s: f64,
    coins: f64,
    /// People per island and tier.
    people: Vec<[f32; Tier::COUNT]>,
}

/// The ledger's state and the history its charts draw.
#[derive(Resource, Default)]
pub struct Ledger {
    pub open: bool,
    /// The island shown.
    pub island: usize,
    samples: VecDeque<Sample>,
    /// What the costs and the moving-up blocks last showed (rebuilt when it changes).
    drains_sig: String,
    moving_sig: String,
    goods_sig: String,
    /// Which goods the left page shows, and in what order.
    pub filter: Filter,
    pub sort: Sort,
}

/// Which goods the ledger lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    /// Asked for and not fully met, or running out.
    Short,
    Falling,
    Rising,
    /// Goods houses ask for.
    Homes,
    /// Goods buildings are built from.
    Building,
}

impl Filter {
    const ALL: [Filter; 6] = [
        Filter::All,
        Filter::Short,
        Filter::Falling,
        Filter::Rising,
        Filter::Homes,
        Filter::Building,
    ];
    fn name(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Short => "Short",
            Filter::Falling => "Falling",
            Filter::Rising => "Rising",
            Filter::Homes => "For homes",
            Filter::Building => "Building",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Short => "short",
            Filter::Falling => "falling",
            Filter::Rising => "rising",
            Filter::Homes => "homes",
            Filter::Building => "building",
        }
    }
}

/// A column the goods sort by, and which way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    pub col: Col,
    pub desc: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Col {
    /// The game's own order of goods.
    #[default]
    Usual,
    Name,
    Store,
    Net,
    Lasts,
    Met,
}

impl Col {
    fn key(self) -> &'static str {
        match self {
            Col::Usual => "usual",
            Col::Name => "name",
            Col::Store => "store",
            Col::Net => "net",
            Col::Lasts => "lasts",
            Col::Met => "met",
        }
    }
    fn from_key(s: &str) -> Option<Col> {
        [
            Col::Usual,
            Col::Name,
            Col::Store,
            Col::Net,
            Col::Lasts,
            Col::Met,
        ]
        .into_iter()
        .find(|c| c.key() == s)
    }
}

/// One good's numbers, for the filters and the sorting.
#[derive(Clone, Copy, Debug, Default)]
pub struct GoodNums {
    pub stock: f64,
    pub net: f32,
    pub used: f32,
    pub supply: f32,
    /// Minutes until the store is empty at today's pace (None: not falling).
    pub lasts: Option<f32>,
}

pub fn nums(b: &crate::sim::IslandBooks, g: Good) -> GoodNums {
    let i = g.index();
    let net = b.made_per_min[i] - b.used_per_min[i];
    GoodNums {
        stock: b.stock[i],
        net,
        used: b.used_per_min[i],
        supply: b.supply[i],
        lasts: (net < -0.05).then(|| (b.stock[i] as f32 / -net).max(0.0)),
    }
}

/// How long a store lasts, in a few characters.
pub fn lasts_text(m: f32) -> String {
    if m < 1.0 {
        "now".into()
    } else if m < 60.0 {
        format!("{m:.0} min")
    } else {
        format!("{:.0} h", m / 60.0)
    }
}

fn for_homes(g: Good) -> bool {
    Tier::ALL.iter().any(|t| {
        t.wants()
            .iter()
            .any(|w| w.need == crate::defs::Need::Good(g))
    })
}

fn for_building(g: Good) -> bool {
    crate::defs::DEFS
        .iter()
        .any(|d| d.cost.iter().any(|(c, _)| *c == g))
}

fn passes(f: Filter, g: Good, n: &GoodNums) -> bool {
    match f {
        Filter::All => true,
        Filter::Short => (n.used > 0.01 && n.supply < 0.99) || n.lasts.is_some_and(|m| m < 10.0),
        Filter::Falling => n.net < -0.05,
        Filter::Rising => n.net > 0.05,
        Filter::Homes => for_homes(g),
        Filter::Building => for_building(g),
    }
}

/// The goods listed, filtered and sorted. `nums` is per good index (empty: no numbers yet).
fn goods_shown(goods: &[Good], nums: &[GoodNums], f: Filter, sort: Sort) -> Vec<Good> {
    let get = |g: Good| nums.get(g.index()).copied().unwrap_or_default();
    let mut v: Vec<Good> = goods
        .iter()
        .copied()
        .filter(|g| passes(f, *g, &get(*g)))
        .collect();
    let key = |g: &Good| -> f64 {
        let n = get(*g);
        match sort.col {
            Col::Usual | Col::Name => 0.0,
            Col::Store => n.stock,
            Col::Net => n.net as f64,
            Col::Lasts => n.lasts.map_or(f64::MAX, |m| m as f64),
            Col::Met => {
                if n.used > 0.01 {
                    n.supply as f64
                } else {
                    2.0
                }
            }
        }
    };
    match sort.col {
        Col::Usual => {}
        Col::Name => v.sort_by_key(|g| g.name()),
        _ => v.sort_by(|a, b| key(a).total_cmp(&key(b))),
    }
    if sort.desc {
        v.reverse();
    }
    v
}

/// A column head that sorts by it: its words, an arrow when it is the one sorted by.
fn sort_head(text: &str, width: f32, right: bool, col: Col, sort: Sort) -> El {
    let on = sort.col == col;
    let arrow = match (on, sort.desc) {
        (true, false) => " ▲",
        (true, true) => " ▼",
        _ => "",
    };
    El::text(format!("{text}{arrow}"))
        .class("ink_label")
        .on_click(format!("isle.led.sort:{}", col.key()))
        .tooltip(
            "",
            if on {
                "Click to turn the order round"
            } else {
                "Click to sort by this"
            },
        )
        .text_style(move |t| {
            let t = t.nowrap();
            if right { t.align(TextAlign::Right) } else { t }
        })
        .with_style(move |s| {
            let s = s.width(width);
            if on { s.color(Colors::ink()) } else { s }
        })
}

/// The filter chips, the column heads and a row per good shown.
fn goods_table(shown: &[Good], f: Filter, sort: Sort, all: &[Good]) -> Vec<El> {
    let chips = El::new()
        .class("row")
        .with_style(|s| {
            s.gap(6.0)
                .wrap(true)
                .align_items(Align::Center)
                .margin([2.0, 0.0, 4.0, 0.0])
        })
        .child(brass::label("show", true))
        .children(Filter::ALL.iter().map(|k| {
            let on = *k == f;
            El::button(k.name(), format!("isle.led.filter:{}", k.key()))
                .class(if on {
                    "brass_button"
                } else {
                    "brass_button_dark"
                })
                .text_style(|t| t.size(13.0).nowrap())
                .with_style(|s| s.padding([3.0, 10.0]))
        }));
    let heads = El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).margin([4.0, 0.0, 4.0, 0.0]))
        .child(sort_head("good", 130.0, false, Col::Name, sort))
        .child(sort_head("store", 40.0, true, Col::Store, sort))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.width(150.0).justify(Justify::SpaceBetween))
                .child(head("used", 60.0, false))
                .child(head("made", 60.0, true)),
        )
        .child(sort_head("a min", 46.0, true, Col::Net, sort))
        .child(sort_head("lasts", 46.0, true, Col::Lasts, sort))
        .child(sort_head("met", 40.0, true, Col::Met, sort));
    let mut out = vec![chips, heads];
    out.extend(shown.iter().copied().map(good_row));
    if shown.is_empty() {
        out.push(
            El::text("No goods here are like that just now.")
                .class("ink")
                .text_style(|t| t.italic().size(14.0))
                .with_style(|s| s.color(Colors::ink_dim()).margin([10.0, 0.0, 0.0, 40.0]))
                .pick_ignore(),
        );
    }
    out.extend(others(all));
    out
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
                .with_style(|s| s.width(46.0))
                .pick_ignore(),
        )
        .child(
            El::text(format!("{{isle.led.lasts{i}}}"))
                .class("ink")
                .text_style(|t| t.nowrap().tabular().align(TextAlign::Right).size(14.0))
                .with_style(|s| s.width(46.0))
                .pick_ignore(),
        )
        .child(
            El::text(format!("{{isle.led.met{i}}}"))
                .class("ink")
                .text_style(|t| t.nowrap().tabular().align(TextAlign::Right))
                .with_style(|s| s.width(40.0))
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
        .child(
            El::text(text)
                .class("ink_label")
                .text_style(|t| t.nowrap())
                .pick_ignore(),
        )
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
/// `goods`: the goods kept, made or used on the shown island (the others are listed in one
/// line under the table).
pub fn el(shown: usize, settled: &[bool], goods: &[Good]) -> El {
    // Tabs for the shown island's region: its settled islands (and the shown one).
    let region = island::islands()[shown].region();
    let tabs = El::new().class("row").with_style(|s| s.gap(8.0)).children(
        island::islands()
            .iter()
            .enumerate()
            .filter(|(i, isle)| {
                isle.region() == region
                    && (*i == shown || settled.get(*i).copied().unwrap_or(false))
            })
            .map(|(i, isle)| {
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
                .class("col")
                .id(GOODS_ID)
                .with_style(|s| s.gap(2.0))
                .children(goods_table(
                    &goods_shown(goods, &[], Filter::All, Sort::default()),
                    Filter::All,
                    Sort::default(),
                    goods,
                )),
        );
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
        .child(chart(COINS_PAINTER, 130.0))
        .child(
            El::new()
                .class("col")
                .id(DRAINS_ID)
                .with_style(|s| s.gap(4.0)),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.justify(Justify::Center))
                .child(brass::divider(320.0)),
        )
        .child(page_title("{isle.led.people_title}", "{isle.led.people}"))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(16.0).justify(Justify::End))
                .child(legend(FISHERFOLK, "fisherfolk"))
                .child(legend(LABOURERS, "labourers"))
                .child(legend(BURGHERS, "burghers")),
        )
        .child(chart(PEOPLE_PAINTER, 120.0))
        .child(
            El::text("Jobs filled")
                .class("ink_label")
                .with_style(|s| s.margin([6.0, 0.0, 0.0, 0.0]))
                .pick_ignore(),
        )
        .child(workforce(Tier::Fisherfolk))
        .child(workforce(Tier::Labourers))
        .child(workforce(Tier::Burghers))
        .child(
            El::new()
                .class("col")
                .id(MOVING_ID)
                .with_style(|s| s.gap(6.0).margin([6.0, 0.0, 0.0, 0.0])),
        );
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
const BURGHERS: Color = Color {
    r: 0.25,
    g: 0.48,
    b: 0.17,
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
    {
        let all = island_goods(&econ, shown);
        let n: Vec<GoodNums> = Good::ALL.iter().map(|g| nums(&books, *g)).collect();
        let (f, sort) = {
            let l = world.resource::<Ledger>();
            (l.filter, l.sort)
        };
        let list = goods_shown(&all, &n, f, sort);
        let sig = format!("{shown}|{f:?}|{sort:?}|{list:?}|{all:?}");
        if world.resource::<Ledger>().goods_sig != sig
            && let Some(e) = engine_ui::input::find_id(world, GOODS_ID)
        {
            let _ = engine_ui::el::replace_children(world, e, &goods_table(&list, f, sort, &all));
            world.resource_mut::<Ledger>().goods_sig = sig;
        }
    }
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
                "{:.0} fisherfolk · {:.0} labourers · {:.0} burghers",
                books.population[0], books.population[1], books.population[2]
            ),
        );
        d.set("isle.led.since", format!("since {}", clock(since)));
        d.set(
            "isle.led.flow",
            {
                // The fleet's wages and stores apart from the buildings' upkeep.
                let fleet = econ.fleet_upkeep as f64;
                let ashore = (econ.upkeep_per_min - fleet).max(0.0);
                let fleet_part = if fleet >= 0.5 {
                    format!(" · fleet [color=#9a2f1c]−{fleet:.0}[/color]")
                } else {
                    String::new()
                };
                format!(
                    "taxes [color=#2f6e28]+{:.0}[/color] · buildings [color=#9a2f1c]−{ashore:.0}[/color]{fleet_part} a minute",
                    econ.taxes_per_min
                )
            },
        );
        for g in Good::ALL {
            let i = g.index();
            let (made, used) = (books.made_per_min[i], books.used_per_min[i]);
            d.set(
                &format!("isle.led.stock{i}"),
                format!("{:.0}", books.stock[i]),
            );
            d.set(&format!("isle.led.net{i}"), signed(made - used));
            d.set(
                &format!("isle.led.lasts{i}"),
                match nums(&books, g).lasts {
                    Some(m) if m < 10.0 => {
                        format!("[color=#9a2f1c][b]{}[/b][/color]", lasts_text(m))
                    }
                    Some(m) => lasts_text(m),
                    None => "—".into(),
                },
            );
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
    let sig = drains_sig(&econ);
    if world.resource::<Ledger>().drains_sig != sig
        && let Some(n) = engine_ui::input::find_id(world, DRAINS_ID)
    {
        let _ = engine_ui::el::replace_children(world, n, &drains(&econ));
        world.resource_mut::<Ledger>().drains_sig = sig;
    }
    let sig = format!("{shown}|{:?}|{}", books.ready, books.auto_up);
    if world.resource::<Ledger>().moving_sig != sig
        && let Some(n) = engine_ui::input::find_id(world, MOVING_ID)
    {
        let _ = engine_ui::el::replace_children(world, n, &moving(&books, name));
        world.resource_mut::<Ledger>().moving_sig = sig;
    }
}

/// "bakery" -> "bakeries", "weaver" -> "weavers".
fn plural(name: &str, n: u32) -> String {
    let name = name.to_lowercase();
    match name.strip_suffix('y') {
        _ if n == 1 || name.ends_with('s') => name,
        Some(stem) if !stem.ends_with(['a', 'e', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{name}s"),
    }
}

fn drains_sig(econ: &Economy) -> String {
    let d: Vec<String> = econ
        .drains
        .iter()
        .map(|d| {
            format!(
                "{:?}{}{}{}{:.0}",
                d.kind, d.island, d.count, d.idle, d.per_min
            )
        })
        .collect();
    format!(
        "{}|{}|{}",
        d.join(","),
        (econ.debt_s / 60.0).floor(),
        econ.auto_paused
    )
}

/// The three biggest costs: each a building's picture, how many, idle or running, where,
/// and what they cost a minute; above them a red line while the company is in debt.
fn drains(econ: &Economy) -> Vec<El> {
    let mut out = Vec::new();
    if econ.drains.is_empty() {
        return out;
    }
    out.push(
        El::new()
            .class("row")
            .with_style(|s| s.justify(Justify::SpaceBetween).align_items(Align::End))
            .child(brass::label("Biggest costs · all islands", true))
            .child(brass::label("a minute", true)),
    );
    if econ.debt_s > 0.0 {
        let soon = if econ.debt_s < crate::sim::DEBT_PAUSE_S {
            "idle works pause themselves after two minutes"
        } else {
            "idle works are pausing themselves"
        };
        out.push(
            El::text(format!(
                "In debt for {}: {soon}.",
                if econ.debt_s < 90.0 {
                    format!("{:.0} s", econ.debt_s)
                } else {
                    format!("{:.0} min", econ.debt_s / 60.0)
                }
            ))
            .class("ink")
            .text_style(|t| t.size(13.0).bold())
            .with_style(|s| s.color(Colors::bad_ink()))
            .pick_ignore(),
        );
    } else if econ.auto_paused > 0 {
        out.push(
            El::text(format!(
                "{} idle works paused themselves while in debt: resume them from their cards.",
                econ.auto_paused
            ))
            .class("ink")
            .text_style(|t| t.size(12.5))
            .with_style(|s| s.color(Colors::ink_dim()))
            .pick_ignore(),
        );
    }
    let top = econ.drains.iter().map(|d| d.per_min).fold(1.0f32, f32::max);
    for d in &econ.drains {
        let name = d.kind.def().name;
        let isle = island::islands()[d.island].name;
        let tip = if d.idle {
            format!(
                "{} {} on {isle} stand idle. Idle works cost half after a minute: pause them (a quarter) or find them goods and roads.",
                d.count,
                plural(name, d.count)
            )
        } else {
            format!(
                "{} {} on {isle}, running at full upkeep.",
                d.count,
                plural(name, d.count)
            )
        };
        let frac = d.per_min / top;
        let idle = d.idle;
        out.push(
            El::new()
                .class("row")
                .tooltip(name, tip)
                .with_style(|s| s.gap(8.0).align_items(Align::Center))
                .child(brass::badge(super::kind_icon(d.kind), 28.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(2.0).grow(1.0).shrink(1.0).basis(0.0))
                        .child(
                            El::new()
                                .class("row")
                                .with_style(|s| s.gap(6.0).align_items(Align::Center))
                                .child(
                                    El::text(format!("{} {}", d.count, plural(name, d.count)))
                                        .class("ink")
                                        .text_style(|t| t.size(14.0).bold().nowrap())
                                        .pick_ignore(),
                                )
                                .child_opt(idle.then(|| {
                                    El::text("idle")
                                        .text_style(|t| t.size(11.5).bold().nowrap())
                                        .with_style(|s| {
                                            s.color(Color::hex(0xfff4d8))
                                                .padding([1.0, 6.0])
                                                .radius(4.0)
                                                .background(Fill::Solid(Color::hex(0xc07a1a)))
                                        })
                                        .pick_ignore()
                                }))
                                .child(
                                    El::text(format!("on {isle}"))
                                        .class("ink")
                                        .text_style(|t| t.size(12.5).nowrap())
                                        .with_style(|s| s.color(Colors::ink_dim()))
                                        .pick_ignore(),
                                ),
                        )
                        .child(
                            El::new()
                                .class("brass_track")
                                .with_style(|s| s.width(pct(100.0)).height(9.0))
                                .child(El::new().with_style(move |s| {
                                    let c = if idle {
                                        Color::hex(0xc07a1a)
                                    } else {
                                        Color::hex(0x9a2f1c)
                                    };
                                    s.width(pct(frac.clamp(0.0, 1.0) * 100.0))
                                        .height(pct(100.0))
                                        .radius(3.0)
                                        .background(Fill::Solid(c))
                                        .pick(Pick::Ignore)
                                })),
                        ),
                )
                .child(
                    El::text(format!("−{:.0}", d.per_min))
                        .class("ink")
                        .text_style(|t| t.size(15.0).bold().tabular().nowrap())
                        .with_style(|s| s.color(Colors::bad_ink()).width(44.0))
                        .pick_ignore(),
                ),
        );
    }
    out
}

/// Moving up on this island: how many houses are ready, by tier, a button that moves them
/// all (those whose tier can spare the people), and the switch that lets them move on their
/// own.
fn moving(books: &crate::sim::IslandBooks, name: &str) -> Vec<El> {
    let total: u32 = books.ready.iter().sum();
    let by_tier: Vec<String> = Tier::ALL
        .iter()
        .filter(|t| books.ready[t.index()] > 0)
        .map(|t| format!("{} {}", books.ready[t.index()], t.name().to_lowercase()))
        .collect();
    let words = if total == 0 {
        "No house is ready to move up".to_string()
    } else {
        format!(
            "{total} {} ready to move up: {}",
            if total == 1 { "house" } else { "houses" },
            by_tier.join(", ")
        )
    };
    vec![
        El::new()
            .class("row")
            .with_style(|s| s.gap(10.0).align_items(Align::Center))
            .child(El::icon("isle_move_up").with_style(move |s| {
                s.size(34.0, 34.0)
                    .opacity(if total > 0 { 1.0 } else { 0.45 })
                    .pick(Pick::Ignore)
            }))
            .child(
                El::text(words)
                    .class("ink")
                    .text_style(|t| t.size(14.0).bold())
                    .with_style(|s| s.grow(1.0).shrink(1.0).basis(0.0))
                    .pick_ignore(),
            )
            .child(
                El::button(
                    if total > 0 {
                        format!("Move up all ready ({total})")
                    } else {
                        "Move up all ready".into()
                    },
                    "isle.led.up_all",
                )
                .class("brass_button")
                .text_style(|t| t.nowrap())
                .disabled(total == 0)
                .tooltip(
                    "Move up all ready",
                    "Moves every ready house up, the longest ready first, skipping those whose works would lose the people.",
                ),
            ),
        brass::switch(
            books.auto_up,
            &format!("Houses on {name} move up on their own"),
            "isle.led.auto_up",
            true,
        )
        .tooltip(
            "Move up on their own",
            "Off: a ready house waits for your order, so no works lose their hands by surprise. On: ready houses move up by themselves, one at a time, when their tier can spare them.",
        ),
    ]
}

/// Move every ready house on island `i` up (those whose works can spare the people) and say
/// what happened.
pub fn up_all(world: &mut World, i: usize) {
    let (moved, waiting) = crate::reports::move_up_all(world, i);
    let left: u32 = waiting.values().sum();
    let why: Vec<String> = waiting
        .iter()
        .map(|(k, n)| {
            let w = match k.as_str() {
                "would_empty_works" => "their works need the people",
                "no_materials" => "the warehouse is short of building goods",
                "needs_paving" => "they want a paved street",
                _ => "they are not ready",
            };
            format!("{n}: {w}")
        })
        .collect();
    let text = match (moved, left) {
        (m, 0) => format!("{m} {} moved up.", if m == 1 { "house" } else { "houses" }),
        (0, _) => format!("None could go ({}).", why.join("; ")),
        (m, l) => format!(
            "{m} moved up; {l} still {} ({}).",
            if l == 1 { "waits" } else { "wait" },
            why.join("; ")
        ),
    };
    engine_ui::toast::notify(
        world,
        if moved > 0 {
            engine_ui::toast::ToastKind::Good
        } else {
            engine_ui::toast::ToastKind::Warn
        },
        "isle_move_up",
        "Moving up",
        &text,
        5.0,
    );
}

/// Open the ledger on island `i` with filter `f` ("short", "falling", ...).
pub fn open_filtered(world: &mut World, i: usize, f: &str) {
    let filter = Filter::ALL
        .into_iter()
        .find(|x| x.key() == f)
        .unwrap_or_default();
    world.resource_mut::<Ledger>().filter = filter;
    show(world, Some(i.min(island::islands().len() - 1)));
    update(world);
}

/// `isle.led.up_all` and `isle.led.auto_up` on the island shown. Returns false for anything
/// else.
pub fn action(world: &mut World, a: &str) -> bool {
    let i = world.resource::<Ledger>().island;
    if let Some(k) = a.strip_prefix("isle.led.filter:") {
        if let Some(f) = Filter::ALL.into_iter().find(|f| f.key() == k) {
            world.resource_mut::<Ledger>().filter = f;
        }
        update(world);
        return true;
    }
    if let Some(c) = a.strip_prefix("isle.led.sort:").and_then(Col::from_key) {
        {
            let mut l = world.resource_mut::<Ledger>();
            // Again on the same column turns the order round; a third time goes back to usual.
            l.sort = match (l.sort.col == c, l.sort.desc) {
                (false, _) => Sort {
                    col: c,
                    // Numbers read best biggest first; names from A.
                    desc: !matches!(c, Col::Name | Col::Lasts | Col::Met),
                },
                (true, d) if d == matches!(c, Col::Name | Col::Lasts | Col::Met) => Sort::default(),
                (true, d) => Sort { col: c, desc: !d },
            };
        }
        update(world);
        return true;
    }
    match a {
        "isle.led.up_all" => up_all(world, i),
        "isle.led.auto_up" => {
            let on = world.resource::<Economy>().islands[i].auto_up;
            crate::reports::auto_up(world, i, !on);
        }
        _ => return false,
    }
    update(world);
    true
}

/// The goods worth a row for island `i`: kept, made or used there (every good when fewer than
/// three are, early on).
pub(super) fn island_goods(econ: &Economy, i: usize) -> Vec<Good> {
    let Some(b) = econ.islands.get(i) else {
        return Good::ALL.to_vec();
    };
    let v: Vec<Good> = Good::ALL
        .into_iter()
        .filter(|g| {
            let n = g.index();
            b.stock[n] > 0.5 || b.made_per_min[n] > 0.01 || b.used_per_min[n] > 0.01
        })
        .collect();
    if v.len() < 3 { Good::ALL.to_vec() } else { v }
}

/// One line for the goods not kept on this island: their icons, faded.
fn others(goods: &[Good]) -> Option<El> {
    let rest: Vec<Good> = Good::ALL
        .into_iter()
        .filter(|g| !goods.contains(g))
        .collect();
    (!rest.is_empty()).then(|| {
        El::new()
            .class("row")
            .with_style(|s| {
                s.gap(4.0)
                    .align_items(Align::Center)
                    .margin([8.0, 0.0, 0.0, 0.0])
            })
            .child(
                El::text("Not kept here")
                    .class("ink_label")
                    .text_style(|t| t.nowrap())
                    .with_style(|s| s.margin([0.0, 6.0, 0.0, 0.0]))
                    .pick_ignore(),
            )
            .children(rest.into_iter().map(|g| {
                El::icon(g.icon())
                    .tooltip(g.name(), "Nobody keeps, makes or uses it here yet.")
                    .with_style(|s| s.size(20.0, 20.0).opacity(0.45))
            }))
    })
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
    {
        // A new page: everything is drawn afresh.
        let mut l = world.resource_mut::<Ledger>();
        l.island = i;
        l.goods_sig.clear();
        l.drains_sig.clear();
        l.moving_sig.clear();
    }
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    let econ = world.resource::<Economy>();
    let settled: Vec<bool> = econ
        .islands
        .iter()
        .enumerate()
        .map(|(k, b)| {
            econ.owner(k).is_none_or(|c| c == crate::sim::PLAYER)
                && (b.harbours > 0 || b.population.iter().sum::<f32>() > 0.0)
        })
        .collect();
    let goods = island_goods(world.resource::<Economy>(), i);
    if engine_ui::screens::spawn_screen(world, screen, &el(i, &settled, &goods)).is_ok() {
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
    let series = [
        (tier(0), FISHERFOLK),
        (tier(1), LABOURERS),
        (tier(2), BURGHERS),
    ];
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
