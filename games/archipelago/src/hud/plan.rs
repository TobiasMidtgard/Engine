//! The build bar's tabs and the plan card: by tier (Fisherfolk, Labourers, Burghers) or by
//! type, what each building costs set out large, greyed tiles for what the company can't
//! afford yet, pad-locks on what the company has not opened yet (crate::unlocks), and a card
//! over the bar for the tile under the pointer (or the building in
//! hand) with its costs against what the island has, its upkeep, workers, chain and reach.

use engine::prelude::*;
use engine_ui::color::Color;
use engine_ui::data::UiData;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::{Interaction, Node};
use engine_ui::style::{Align, Direction, Enter, Fill, Justify, Pick, Position};

use crate::defs::{Category, DEFS, Good, Kind, Need, Tier};
use crate::sim::Economy;
use crate::tools::Tool;
use crate::unlocks::{Lock, lock_of};

pub const PLAN_ID: &str = "isle.plan";
pub const SORT_ID: &str = "isle.bar.sort";

/// One tab of the build bar.
pub struct Tab {
    pub name: &'static str,
    pub icon: &'static str,
    pub kinds: Vec<Kind>,
}

/// The bar's state: how it is sorted, the tiles shown, and the plan card's subject.
#[derive(Resource, Default)]
pub struct Plan {
    /// Six tabs by type instead of one per tier.
    pub by_type: bool,
    /// The shown tiles' nodes, to see which one the pointer is over.
    pub tiles: Vec<(Kind, Entity)>,
    /// The building the plan card shows, and what it was drawn from.
    shown: Option<Kind>,
    sig: String,
    /// Tour shots (`--hud-locks`): the people to judge the locks by, as at the start of a
    /// game, in place of the company's own.
    pub demo_people: Option<[f32; Tier::COUNT]>,
}

/// The pad-lock over tile `k` (shown while the building is not open yet).
pub fn lock_id(k: Kind) -> String {
    format!("isle.lock.{k:?}")
}

/// The key cap in a build tile's top-left corner: the keys that take it up (tools.rs),
/// written in by [`refresh_prices`].
pub fn tile_keycap(k: Kind) -> El {
    El::text(format!("{{isle.key.{k:?}}}"))
        .id(format!("isle.keycap.{k:?}"))
        .text_style(|t| t.nowrap().bold().size(11.0))
        .with_style(|s| {
            s.position(Position::Absolute)
                .top(4.0)
                .left(4.0)
                .padding([0.0, 4.0])
                .radius(3.0)
                .background(Fill::vertical(
                    Colors::lacquer_top(),
                    Colors::lacquer_bottom(),
                ))
                .border(1.0, Colors::brass().with_alpha(0.7))
                .color(Colors::cream())
                .opacity(0.0)
                .pick(Pick::Ignore)
        })
        .pick_ignore()
}

/// The pad-lock over tab `i`.
pub fn tab_lock_id(i: usize) -> String {
    format!("isle.tablock.{i}")
}

/// The pad-lock a tile or tab wears while locked, hidden until [`refresh_prices`] shows it.
pub fn padlock(id: String, size: f32, top: f32, right: f32) -> El {
    El::new()
        .id(id)
        .with_style(move |s| {
            s.position(Position::Absolute)
                .top(top)
                .right(right)
                .opacity(0.0)
                .pick(Pick::Ignore)
        })
        .child(brass::badge("isle_padlock", size))
}

/// Whether each of `kinds` is open to the company giving orders (judged by the tour shots'
/// people instead when they are set).
pub fn locks(world: &World, kinds: &[Kind]) -> Vec<Lock> {
    let me = crate::sim::acting(world);
    let econ = world.resource::<Economy>();
    match world.resource::<Plan>().demo_people {
        Some(p) => {
            let home = (0..econ.islands.len()).find(|i| econ.owns(me, *i));
            let mut e = econ.clone();
            for (i, b) in e.islands.iter_mut().enumerate() {
                b.population = if Some(i) == home {
                    p
                } else {
                    [0.0; Tier::COUNT]
                };
            }
            kinds.iter().map(|k| lock_of(&e, me, *k)).collect()
        }
        None => kinds.iter().map(|k| lock_of(econ, me, *k)).collect(),
    }
}

/// The tier a building belongs to: the people who work it, or the first tier that asks for
/// it as a service; roads, houses and the harbour are everyone's (the first tier).
pub fn tier_of(k: Kind) -> Tier {
    if let Some((t, _)) = k.def().workers {
        return t;
    }
    Tier::ALL
        .into_iter()
        .find(|t| t.wants().iter().any(|w| w.need == Need::Service(k)))
        .unwrap_or(Tier::Fisherfolk)
}

pub fn tier_icon(t: Tier) -> &'static str {
    match t {
        Tier::Fisherfolk => "isle_tier_fisherfolk",
        Tier::Labourers => "isle_tier_labourers",
        Tier::Burghers => "isle_tier_burghers",
        Tier::Masters => "isle_tier_masters",
        Tier::Patricians => "isle_tier_patricians",
        Tier::Riverfolk => "isle_tier_riverfolk",
        Tier::Freeholders => "isle_tier_freeholders",
        Tier::Planters => "isle_tier_planters",
        Tier::Herders => "isle_tier_herders",
        Tier::Caravaners => "isle_tier_caravaners",
        Tier::Scholars => "isle_tier_scholars",
        Tier::Trappers => "isle_tier_trappers",
        Tier::Whalers => "isle_tier_whalers",
        Tier::Wardens => "isle_tier_wardens",
    }
}

/// The tabs, in order: one per tier (each sorted by type), or one per type.
pub fn tabs(by_type: bool) -> Vec<Tab> {
    if by_type {
        return Category::ALL
            .iter()
            .map(|c| Tab {
                name: c.name(),
                icon: super::category_icon(*c),
                kinds: DEFS
                    .iter()
                    .filter(|d| d.category == *c)
                    .map(|d| d.kind)
                    .collect(),
            })
            .collect();
    }
    // (The building table names its workers by the home tiers; abroad the island's people
    // of the same rank work them, so the home tiers' tabs serve every region.)
    Tier::HOME
        .into_iter()
        .map(|t| {
            let mut kinds: Vec<Kind> = DEFS
                .iter()
                .map(|d| d.kind)
                .filter(|k| tier_of(*k) == t)
                .collect();
            kinds.sort_by_key(|k| k.def().category as usize);
            Tab {
                name: t.name(),
                icon: tier_icon(t),
                kinds,
            }
        })
        .collect()
}

/// The people who work tier `t`'s jobs on island `isle`: its own people of that rank abroad
/// (crate::sim::hands), or `t` itself where the island's ladder stops lower.
pub fn local(t: Tier, isle: usize) -> Tier {
    crate::sim::hands(t, isle).unwrap_or(t)
}

/// Tab `i`'s name and icon on island `isle`: a tier tab abroad wears the island's own people
/// of that rank (Riverfolk for the first tab on Sarrowmere).
pub fn tab_face(by_type: bool, i: usize, isle: usize) -> (&'static str, &'static str) {
    let tabs = tabs(by_type);
    let Some(tab) = tabs.get(i.min(tabs.len().saturating_sub(1))) else {
        return ("", "");
    };
    match (by_type, Tier::HOME.get(i)) {
        (false, Some(t)) => {
            let t = local(*t, isle);
            (t.name(), tier_icon(t))
        }
        _ => (tab.name, tab.icon),
    }
}

/// Tab `i`'s tiles in the order shown for island `isle`: farms whose crop does not grow
/// there go last.
pub fn tab_kinds(by_type: bool, i: usize, isle: usize) -> Vec<Kind> {
    let tabs = tabs(by_type);
    let Some(tab) = tabs.get(i.min(tabs.len().saturating_sub(1))) else {
        return Vec::new();
    };
    let mut kinds = tab.kinds.clone();
    kinds.sort_by_key(|k| !grows(*k, isle));
    kinds
}

/// Whether building `k` can go up on island `isle` (farms need the island to suit the crop).
pub fn grows(k: Kind, isle: usize) -> bool {
    !k.def().needs_fertility
        || crate::island::islands()
            .get(isle)
            .is_none_or(|i| i.fertility.allows(k))
}

/// The tab the building is on.
pub fn tab_of(by_type: bool, k: Kind) -> usize {
    tabs(by_type)
        .iter()
        .position(|t| t.kinds.contains(&k))
        .unwrap_or(0)
}

/// What the company giving orders pays in coins for `k` under this game's rules.
pub fn coins(world: &World, k: Kind) -> f64 {
    crate::build::price(world, k, crate::sim::acting(world))
}

/// What building `k` still lacks on island `isle` at `price` coins: coins first, then each
/// good.
pub fn short(econ: &Economy, isle: usize, k: Kind, price: f64) -> Vec<(Option<Good>, f64)> {
    let d = k.def();
    let mut out = Vec::new();
    let coins = econ.coins_of(crate::sim::PLAYER);
    if coins < price {
        out.push((None, price - coins.max(0.0)));
    }
    if let Some(b) = econ.islands.get(isle) {
        for (g, n) in d.cost {
            let have = b.get(*g);
            if have < *n as f64 {
                out.push((Some(*g), *n as f64 - have.max(0.0)));
            }
        }
    }
    out
}

fn price_id(k: Kind) -> String {
    format!("isle.price.{k:?}")
}

fn good_price_id(k: Kind, j: usize) -> String {
    format!("isle.cost.{k:?}.{j}")
}

/// A tile's price: coins on top, then each good it takes, every number bound so it turns
/// red while the island is short of it.
pub fn price(k: Kind) -> El {
    let d = k.def();
    let coins = El::new()
        .class("row")
        .with_style(|s| s.gap(3.0).justify(Justify::Center).pick(Pick::Ignore))
        .child(El::icon("coin").with_style(|s| s.size(16.0, 16.0).pick(Pick::Ignore)))
        .child(
            El::text(format!("{{{}}}", price_id(k)))
                .text_style(|t| t.size(14.5).bold().nowrap().tabular())
                .with_style(|s| s.color(Colors::ink()))
                .pick_ignore(),
        );
    let goods = (!d.cost.is_empty()).then(|| {
        El::new()
            .class("row")
            .with_style(|s| s.gap(5.0).justify(Justify::Center).pick(Pick::Ignore))
            .children(d.cost.iter().enumerate().map(|(j, (g, _))| {
                El::new()
                    .class("row")
                    .with_style(|s| s.gap(1.0).pick(Pick::Ignore))
                    .child(El::icon(g.icon()).with_style(|s| s.size(15.0, 15.0).pick(Pick::Ignore)))
                    .child(
                        El::text(format!("{{{}}}", good_price_id(k, j)))
                            .text_style(|t| t.size(12.5).bold().nowrap().tabular())
                            .with_style(|s| s.color(Colors::ink_dim()))
                            .pick_ignore(),
                    )
            }))
    });
    El::new()
        .class("col")
        .with_style(|s| s.gap(1.0).align_items(Align::Center).pick(Pick::Ignore))
        .child(coins)
        .child_opt(goods)
}

fn hex(c: Color) -> String {
    c.to_hex()
}

/// Fill the shown tiles' prices (red where short) and grey out what can't be built now.
pub fn refresh_prices(world: &mut World, kinds: &[Kind], isle: usize) {
    let econ = world.resource::<Economy>().clone();
    let prices: Vec<f64> = kinds.iter().map(|&k| coins(world, k)).collect();
    let red = hex(Colors::bad_ink());
    let mut faded: Vec<(Kind, bool)> = Vec::new();
    {
        let mut data = world.resource_mut::<UiData>();
        for (&k, &cost) in kinds.iter().zip(&prices) {
            let d = k.def();
            let lacks = short(&econ, isle, k, cost);
            let coins_short = lacks.iter().any(|(g, _)| g.is_none());
            data.set(
                &price_id(k),
                if coins_short {
                    format!("[color={red}]{cost:.0}[/color]")
                } else {
                    format!("{cost:.0}")
                },
            );
            for (j, (g, n)) in d.cost.iter().enumerate() {
                let lack = lacks.iter().any(|(s, _)| *s == Some(*g));
                data.set(
                    &good_price_id(k, j),
                    if lack {
                        format!("[color={red}]{n}[/color]")
                    } else {
                        n.to_string()
                    },
                );
            }
            faded.push((k, !lacks.is_empty() || !grows(k, isle)));
        }
    }
    let locked: Vec<bool> = locks(world, kinds).iter().map(|l| l.locked).collect();
    // The key caps.
    let keys: Vec<(Kind, Option<String>)> = kinds
        .iter()
        .map(|k| (*k, crate::tools::hotkey(world, *k)))
        .collect();
    for (k, key) in keys {
        let id = format!("isle.key.{k:?}");
        let text = key.clone().unwrap_or_default();
        {
            let mut data = world.resource_mut::<UiData>();
            if data.get_str(&id) != Some(text.as_str()) {
                data.set(&id, text);
            }
        }
        if let Some(e) = engine_ui::input::find_id(world, &format!("isle.keycap.{k:?}")) {
            set_opacity(world, e, if key.is_some() { 1.0 } else { 0.0 });
        }
    }
    let tiles = world.resource::<Plan>().tiles.clone();
    for (k, e) in tiles {
        let Some(at) = faded.iter().position(|(x, _)| *x == k) else {
            continue;
        };
        let shut = locked[at];
        let want = if shut {
            0.62
        } else if faded[at].1 {
            0.48
        } else {
            1.0
        };
        set_opacity(world, e, want);
        if let Some(l) = engine_ui::input::find_id(world, &lock_id(k)) {
            set_opacity(world, l, if shut { 1.0 } else { 0.0 });
        }
    }
    // A tab whose every building is locked wears a pad-lock too.
    let by_type = world.resource::<Plan>().by_type;
    for i in 0..tabs(by_type).len() {
        let ks: Vec<Kind> = tab_kinds(by_type, i, isle)
            .into_iter()
            .filter(|k| grows(*k, isle))
            .collect();
        let shut = !ks.is_empty() && locks(world, &ks).iter().all(|l| l.locked);
        if let Some(l) = engine_ui::input::find_id(world, &tab_lock_id(i)) {
            set_opacity(world, l, if shut { 1.0 } else { 0.0 });
        }
    }
}

fn set_opacity(world: &mut World, e: Entity, want: f32) {
    if let Some(mut n) = world.get_mut::<Node>(e)
        && n.style.opacity != Some(want)
    {
        n.style.opacity = Some(want);
    }
}

/// Remember the tile nodes after the bar's tiles were rebuilt.
pub fn note_tiles(world: &mut World, kinds: &[Kind]) {
    let tiles: Vec<(Kind, Entity)> = kinds
        .iter()
        .filter_map(|k| {
            engine_ui::input::find_id(world, &format!("isle.build:{k:?}")).map(|e| (*k, e))
        })
        .collect();
    world.resource_mut::<Plan>().tiles = tiles;
}

/// The plan card's subject: the tile under the pointer, else the building in hand.
fn subject(world: &World) -> Option<Kind> {
    let hovered = world.resource::<Plan>().tiles.iter().find_map(|(k, e)| {
        world
            .get::<Interaction>(*e)
            .is_some_and(|i| i.hovered)
            .then_some(*k)
    });
    hovered.or(match *world.resource::<Tool>() {
        Tool::Build { kind, .. } => Some(kind),
        _ => None,
    })
}

/// Every frame: show, change or clear the plan card.
pub fn update(world: &mut World, isle: usize) {
    if !world.contains_resource::<Plan>() {
        return;
    }
    let k = subject(world);
    let econ = world.resource::<Economy>();
    let sig = match k {
        Some(k) => format!(
            "{k:?}|{isle}|{:?}|{}",
            short(econ, isle, k, coins(world, k))
                .iter()
                .map(|(g, n)| (*g, (*n).ceil() as i64))
                .collect::<Vec<_>>(),
            locks(world, &[k])[0].why
        ),
        None => String::new(),
    };
    if world.resource::<Plan>().sig == sig {
        return;
    }
    let card = k.map(|k| card(world, k, isle));
    {
        let mut p = world.resource_mut::<Plan>();
        p.sig = sig;
        p.shown = k;
    }
    if let Some(e) = engine_ui::input::find_id(world, PLAN_ID) {
        let els: Vec<El> = card.into_iter().collect();
        let _ = engine_ui::el::replace_children(world, e, &els);
    }
}

/// A cost chip: the picture, how many, and how many more are needed when short.
fn cost_chip(icon: &str, need: String, lack: Option<f64>) -> El {
    let ok = lack.is_none();
    El::new()
        .class("row")
        .with_style(move |s| {
            s.gap(6.0)
                .align_items(Align::Center)
                .padding([4.0, 10.0, 4.0, 4.0])
                .radius(18.0)
                .background(if ok {
                    Color::hexa(0x4f9a3c22)
                } else {
                    Color::hexa(0xb03a2426)
                })
                .border(
                    1.2,
                    if ok {
                        Color::hexa(0x4f9a3c88)
                    } else {
                        Color::hexa(0xb03a24aa)
                    },
                )
                .pick(Pick::Ignore)
        })
        .child(brass::badge(icon, 30.0))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0).pick(Pick::Ignore))
                .child(
                    El::text(need)
                        .class("ink")
                        .text_style(|t| t.size(19.0).bold().nowrap().tabular())
                        .pick_ignore(),
                )
                .child_opt(lack.map(|n| {
                    El::text(format!("{:.0} short", n.ceil()))
                        .class("ink")
                        .text_style(|t| t.size(12.0).bold().nowrap())
                        .with_style(|s| s.color(Colors::bad_ink()))
                        .pick_ignore()
                })),
        )
}

/// A small fact line: picture, label and value.
fn fact(icon: &str, label: &str, value: String) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.gap(8.0).align_items(Align::Center).pick(Pick::Ignore))
        .child(El::icon(icon).with_style(|s| s.size(22.0, 22.0).pick(Pick::Ignore)))
        .child(
            El::text(label)
                .class("ink_label")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.width(78.0))
                .pick_ignore(),
        )
        .child(
            El::text(value)
                .class("ink")
                .text_style(|t| t.size(14.5))
                .with_style(|s| s.grow(1.0).shrink(1.0))
                .pick_ignore(),
        )
}

/// The plan card for building `k` on island `isle`.
pub fn card(world: &World, k: Kind, isle: usize) -> El {
    let d = k.def();
    let econ = world.resource::<Economy>();
    let cost = coins(world, k);
    let lacks = short(econ, isle, k, cost);
    let lack_of = |g: Option<Good>| lacks.iter().find(|(x, _)| *x == g).map(|(_, n)| *n);
    let here = crate::island::islands().get(isle).map_or("", |i| i.name);
    let tier = local(tier_of(k), isle);
    let mut chips = vec![cost_chip("coin", format!("{cost:.0}"), lack_of(None))];
    for (g, n) in d.cost {
        chips.push(cost_chip(g.icon(), n.to_string(), lack_of(Some(*g))));
    }
    let mut facts: Vec<El> = Vec::new();
    if d.upkeep > 0.0 {
        facts.push(fact(
            "coin",
            "Upkeep",
            format!("{:.0} coins a minute while it runs", d.upkeep),
        ));
    }
    if let Some((t, n)) = d.workers {
        facts.push(fact(
            "isle_people",
            "Workers",
            format!("{n} {}", local(t, isle).name().to_lowercase()),
        ));
    }
    if let Some(o) = d.output {
        let from: Vec<&str> = d
            .input
            .into_iter()
            .chain(d.input2)
            .map(|g| g.name())
            .collect();
        let rate = 60.0 / d.cycle_s.max(0.1);
        facts.push(fact(
            o.icon(),
            "Makes",
            if from.is_empty() {
                format!("{} · {rate:.1} a minute", o.name())
            } else {
                format!(
                    "{} from {} · {rate:.1} a minute",
                    o.name(),
                    from.join(" and ").to_lowercase()
                )
            },
        ));
    }
    if d.reach > 0 && d.output.is_none() {
        facts.push(fact(
            "isle_house",
            "Reach",
            format!("houses within {} cells", d.reach),
        ));
    }
    facts.push(fact(
        "isle_workshop",
        "Takes",
        format!("{} by {} cells", d.size.0, d.size.1),
    ));
    let lock = locks(world, &[k]).remove(0);
    let verdict = if lock.locked {
        Some(format!("Not open yet. {}", lock.why))
    } else if !grows(k, isle) {
        Some(format!(
            "Does not grow on {here}: settle an island that suits it."
        ))
    } else if lacks.is_empty() {
        None
    } else {
        let parts: Vec<String> = lacks
            .iter()
            .map(|(g, n)| match g {
                None => format!("{:.0} coins", n.ceil()),
                Some(g) => format!("{:.0} {}", n.ceil(), g.name().to_lowercase()),
            })
            .collect();
        Some(format!("Short of {} on {here}.", parts.join(", ")))
    };
    let ready = verdict.is_none();
    brass::parchment()
        .with_style(|s| {
            s.direction(Direction::Column)
                .gap(8.0)
                .padding([14.0, 20.0, 16.0, 20.0])
                .width(470.0)
                .pick(Pick::Ignore)
                .enter(Enter::rise(0.18, 10.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center).pick(Pick::Ignore))
                .child(brass::badge(super::kind_icon(k), 56.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0).shrink(1.0).pick(Pick::Ignore))
                        .child(El::text(d.name).class("ink_title").pick_ignore())
                        .child(
                            El::text(format!("{} · {}", tier.name(), d.category.name()))
                                .class("ink_label")
                                .text_style(|t| t.nowrap())
                                .pick_ignore(),
                        ),
                )
                .child(brass::badge(tier_icon(tier), 34.0)),
        )
        .child_opt((!d.blurb.is_empty()).then(|| {
            El::text(d.blurb)
                .class("ink")
                .text_style(|t| t.italic().size(14.0))
                .with_style(|s| s.color(Colors::ink_dim()))
                .pick_ignore()
        }))
        .child(brass::label("Costs", true))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0).wrap(true).pick(Pick::Ignore))
                .children(chips),
        )
        .child(brass::divider(240.0))
        .children(facts)
        .child_opt(lock.locked.then(|| {
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(Align::Center).pick(Pick::Ignore))
                .child(brass::badge("isle_padlock", 34.0))
                .child(brass::meter(300.0, lock.progress))
                .child(
                    El::text(format!("{:.0}%", lock.progress * 100.0))
                        .class("ink")
                        .text_style(|t| t.size(14.0).bold().tabular().nowrap())
                        .pick_ignore(),
                )
        }))
        .child(
            El::text(verdict.unwrap_or_else(|| format!("You can build it on {here} now.")))
                .class("ink")
                .text_style(|t| t.size(14.5).bold())
                .with_style(move |s| {
                    s.color(if ready {
                        Colors::good_ink()
                    } else {
                        Colors::bad_ink()
                    })
                })
                .pick_ignore(),
        )
}

/// `isle.bar.sort`: by tier or by type; the bar opens on the tab that holds the building in
/// hand.
pub fn toggle_sort(world: &mut World) -> usize {
    let by_type = {
        let mut p = world.resource_mut::<Plan>();
        p.by_type = !p.by_type;
        p.by_type
    };
    match *world.resource::<Tool>() {
        Tool::Build { kind, .. } => tab_of(by_type, kind),
        _ => 0,
    }
}

/// The sort switch at the end of the tabs.
pub fn sort_switch(by_type: bool) -> El {
    brass::medallion(
        if by_type {
            "isle_sort_type"
        } else {
            "isle_sort_tier"
        },
        40.0,
        "isle.bar.sort",
    )
    .id(SORT_ID)
    .tooltip(
        if by_type { "Sorted by type" } else { "Sorted by tier" },
        if by_type {
            "Click to sort the buildings by the tier of people they serve: Fisherfolk, Labourers, Burghers."
        } else {
            "Click to sort the buildings by type: roads, homes, workshops, farms, public, harbour."
        },
    )
}
