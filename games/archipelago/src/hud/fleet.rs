//! Ships' gear: every vessel's captain and gear sockets, what they add up to, and the place to
//! put shelf items aboard, take them off, or give an order. Opened with G or from the shelf.
//!
//! Data and rules are the game's (crate::gear); this is the UI thread's.

use engine::prelude::*;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors, SocketState};
use engine_ui::style::{Align, Direction, Enter, Pick, Position};

use crate::fleet::{Roam, Routes, Vessel};
use crate::gear::{GearPanel, Kit};
use crate::island;
use crate::items::{self, ItemView, Way};
use crate::sim::Building;

use super::shelf::{details, socket_state, tip_text, ui_rank};

pub const SCREEN: &str = "isle_fleet";
const LIST_ID: &str = "isle.fleet.list";

/// The gear screen's state.
#[derive(Resource, Default)]
pub struct Fleet {
    pub open: bool,
    /// The socket picked out (vessel, slot): its item's details, or the items that fit it.
    picked: Option<(u32, usize)>,
    /// The vessel whose "sail to" list is open (the chart's next click sends her there too).
    pub voyage: Option<u32>,
    /// What the list last showed (rebuilt only when it changes).
    sig: String,
}

pub fn install(world: &mut World) {
    world.init_resource::<Fleet>();
}

/// What a vessel's gear adds up to, in words; empty when it adds nothing.
fn kit_line(k: &Kit) -> String {
    let pct = |x: f32| (x * 100.0).round();
    let mut v = Vec::new();
    if k.speed.abs() > 0.005 {
        v.push(format!("{:+.0}% speed", pct(k.speed)));
    }
    if k.hold > 0 {
        v.push(format!("+{} hold", k.hold));
    }
    if k.loads > 0.005 {
        v.push(format!("{:.0}% quicker loading", pct(k.loads)));
    }
    if k.longer > 0.005 {
        v.push(format!("{:+.0}% range", pct(k.longer)));
    }
    if k.sight > 0.005 {
        v.push(format!("+{:.0} sight", k.sight));
    }
    if k.luck > 0.005 {
        v.push(format!("+{:.0}% luck", pct(k.luck)));
    }
    if k.postage > 0.005 {
        v.push(format!("+{:.0} coins a voyage", k.postage));
    }
    if k.recharge > 0.005 {
        v.push(format!("{:.0}% faster recharge", pct(k.recharge)));
    }
    v.join(" · ")
}

/// The craft's picture, by the name the panel gives.
fn craft_icon(craft: &str) -> &'static str {
    crate::vessels::Craft::ALL
        .into_iter()
        .find(|c| c.name() == craft)
        .map_or("isle_anchor", super::yard::craft_icon)
}

fn state_id(vessel: u32, slot: usize) -> String {
    format!("isle.gsock.{vessel}.{slot}")
}

fn status_id(vessel: u32) -> String {
    format!("isle.fstat.{vessel}")
}

/// The company's vessel whose entity index is `vessel` (as the gear panel gives it).
fn vessel_entity(world: &mut World, vessel: u32) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Vessel)>();
    q.iter(world)
        .find(|(e, _)| e.index_u32() == vessel)
        .map(|(e, _)| e)
}

/// The island a point at sea is off, if it is near one; else open water.
fn waters(p: DVec2) -> String {
    let i = island::nearest_island(p.x, p.y);
    let isle = &island::islands()[i];
    if (p - isle.center).length() < isle.radius * 1.9 {
        isle.name.to_string()
    } else {
        "open water".into()
    }
}

/// What a vessel is doing, in words: `(kind, text)`. The kind leaves the distance out, so the
/// list is rebuilt only when what she does changes; the text moves in place.
fn status(world: &World, v: &Vessel) -> (String, String) {
    let here = DVec2::new(v.at.x, v.at.z);
    if let Some(r) = v.route {
        let name = world
            .get_resource::<Routes>()
            .and_then(|rs| rs.0.get(r).map(|r| r.name.clone()))
            .unwrap_or_else(|| "a trade".into());
        let t = format!("On the {name} route");
        return (t.clone(), t);
    }
    match &v.roam {
        Roam::Found { isle, .. } => {
            let t = format!("Bound to settle {}", island::islands()[*isle].name);
            (t.clone(), t)
        }
        Roam::Fishing { .. } => ("Fishing".into(), "Fishing for her quay".into()),
        Roam::Circling { .. } => ("Circling".into(), "Circling her mast".into()),
        Roam::Idle { at } => {
            let to = DVec2::new(at.x, at.z);
            let d = (to - here).length();
            if d > 120.0 {
                let w = waters(to);
                let k = format!("Sailing for {w}");
                (k.clone(), format!("{k} · {:.1} km to go", d / 1000.0))
            } else {
                let t = format!("Lying off {}", waters(here));
                (t.clone(), t)
            }
        }
        _ => (String::new(), String::new()),
    }
}

/// Where "sail to" island `i` takes a ship: off the company's harbour there if it has one,
/// else to the landfall a settler would make.
fn landfall(world: &mut World, i: usize) -> DVec2 {
    let mut q = world.query::<&Building>();
    if let Some(b) = q
        .iter(world)
        .find(|b| b.island == i && b.kind == crate::defs::Kind::Harbour)
    {
        return crate::fleet::mooring(b).0;
    }
    let isle = &island::islands()[i];
    island::settle_coast(isle).unwrap_or(isle.center)
}

/// Why a vessel can't be sent anywhere now, if it can't.
fn why_stays(v: &Vessel) -> Option<String> {
    if crate::settle::free(v) {
        return None;
    }
    Some(if v.craft.flies() {
        format!("{} flies her own rounds from the mast.", v.name)
    } else if v.route.is_some() {
        format!("{} is on a trade route: take her off it first.", v.name)
    } else if matches!(v.roam, Roam::Found { .. }) {
        format!("{} is bound to settle an island.", v.name)
    } else {
        format!("{} is busy with her own work.", v.name)
    })
}

/// The "sail to" list under a vessel: the islands of her waters, nearest first, settled ones
/// marked, and a hint that the chart takes a click too.
fn voyage(world: &mut World, vessel: u32) -> El {
    let col = El::new()
        .class("col")
        .with_style(|s| s.gap(6.0).padding([4.0, 8.0, 4.0, 44.0]));
    let Some(e) = vessel_entity(world, vessel) else {
        return col;
    };
    let Some(v) = world.get::<Vessel>(e).cloned() else {
        return col;
    };
    if let Some(why) = why_stays(&v) {
        return col.child(
            El::text(why)
                .class("ink")
                .text_style(|t| t.size(13.0).italic())
                .with_style(|s| s.color(Colors::ink_dim()))
                .pick_ignore(),
        );
    }
    let here = DVec2::new(v.at.x, v.at.z);
    let region = crate::globe::region_of(here.x, here.y);
    let mut isles: Vec<(usize, f64, bool)> = island::islands()
        .iter()
        .enumerate()
        .filter(|(_, i)| i.region() == region)
        .map(|(n, i)| {
            (
                n,
                (i.center - here).length(),
                crate::settle::claimed(world, n),
            )
        })
        .collect();
    isles.sort_by(|a, b| a.1.total_cmp(&b.1));
    col.child(brass::label(
        "Sail to · or click anywhere on the chart",
        true,
    ))
    .child(
        El::new()
            .class("row")
            .with_style(|s| s.gap(6.0).wrap(true))
            .children(isles.into_iter().map(|(n, d, settled)| {
                let isle = &island::islands()[n];
                El::new()
                    .class("button brass_button")
                    .on_click(format!("isle.fleet.goto:{vessel}:{n}"))
                    .with_style(|s| {
                        s.direction(Direction::Column)
                            .gap(0.0)
                            .padding([4.0, 10.0])
                            .align_items(Align::Center)
                    })
                    .tooltip(
                        isle.name,
                        if settled {
                            "Yours: she'll lie off your harbour there."
                        } else {
                            "Not settled yet: she'll stand off its best landfall, to look \
                             round. Build a harbour on its coast to settle it."
                        },
                    )
                    .child(
                        El::text(isle.name)
                            .text_style(|t| t.size(14.0).bold().nowrap())
                            .pick_ignore(),
                    )
                    .child(
                        El::text(format!(
                            "{} · {:.1} km",
                            if settled { "yours" } else { "unsettled" },
                            d / 1000.0
                        ))
                        .text_style(|t| t.size(11.0).nowrap())
                        .with_style(|s| s.opacity(0.8))
                        .pick_ignore(),
                    )
            })),
    )
}

/// One vessel: its picture, name and what its gear adds, then the captain's socket and the
/// gear sockets; under it, when one of its sockets is picked, the item or the items that fit.
fn row(
    p: &GearPanel,
    picked: Option<usize>,
    shelf: &[ItemView],
    status: &str,
    sailing: Option<El>,
    stays: Option<String>,
) -> El {
    let kit = kit_line(&p.kit);
    let open = sailing.is_some();
    let dim = stays.is_some();
    let sockets = p.slots.iter().map(|gs| {
        let captain = gs.kind == "captain";
        let size = if captain { 50.0 } else { 42.0 };
        let action = format!("isle.fleet.pick:{}:{}", p.vessel, gs.slot);
        let socket = match &gs.item {
            Some(iv) => brass::socket_with(
                Some((iv.icon, ui_rank(iv.rank))),
                size,
                &action,
                &socket_state(iv),
                &state_id(p.vessel, gs.slot),
            )
            .tooltip(iv.name, tip_text(iv)),
            None => brass::socket(None, size, &action).tooltip(
                if captain {
                    "No captain"
                } else {
                    "Empty gear slot"
                },
                if captain {
                    "Click to sign on a captain from the shelf."
                } else {
                    "Click to fit gear from the shelf."
                },
            ),
        };
        let lit = picked == Some(gs.slot);
        El::new()
            .class("col")
            .with_style(move |s| {
                s.gap(2.0)
                    .align_items(Align::Center)
                    .padding([3.0, 3.0])
                    .radius(8.0)
                    .background(engine_ui::style::Fill::Solid(if lit {
                        Colors::brass().with_alpha(0.25)
                    } else {
                        Colors::brass().with_alpha(0.0)
                    }))
            })
            .child(socket)
            .child_opt(captain.then(|| {
                El::text("Captain")
                    .class("ink_label")
                    .text_style(|t| t.nowrap())
                    .pick_ignore()
            }))
    });
    let mut col = El::new()
        .class("col")
        .with_style(|s| s.gap(6.0).padding([8.0, 6.0]))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(10.0).align_items(Align::Center))
                .child(brass::badge(craft_icon(p.craft), 38.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).width(178.0).shrink(0.0))
                        .child(
                            El::text(p.name.clone())
                                .class("ink")
                                .text_style(|t| t.size(16.0).bold().nowrap())
                                .pick_ignore(),
                        )
                        .child(brass::label(p.craft, true))
                        .child_opt((!status.is_empty()).then(|| {
                            El::text(status)
                                .class("ink")
                                .id(status_id(p.vessel))
                                .text_style(|t| t.size(13.0).italic())
                                .with_style(|s| s.color(Colors::ink_dim()))
                                .pick_ignore()
                        }))
                        .child_opt((!kit.is_empty()).then(|| {
                            El::text(kit)
                                .class("ink")
                                .text_style(|t| t.size(13.0).bold())
                                .with_style(|s| s.color(Colors::good_ink()))
                                .pick_ignore()
                        })),
                )
                .child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(4.0).align_items(Align::Start).shrink(0.0))
                        .children(sockets),
                )
                .child(El::new().with_style(|s| s.grow(1.0)))
                .child(
                    brass::medallion("isle_compass", 32.0, &format!("isle.fleet.voyage:{}", p.vessel))
                        .with_style(move |s| s.opacity(if dim { 0.45 } else { 1.0 }))
                        .tooltip(
                            if open { "Close the list" } else { "Sail to…" },
                            stays.unwrap_or_else(|| {
                                "Send her to an island, or to any point you click on the chart, to look round under the fog.".into()
                            }),
                        ),
                ),
        );
    if let Some(list) = sailing {
        col = col.child(list);
    }
    if let Some(slot) = picked {
        let item = p.slots.get(slot).and_then(|g| g.item.as_ref());
        col = col.child(match item {
            Some(iv) => {
                let pressable = matches!(iv.way, Way::Order { .. } | Way::Consumable { .. })
                    && iv.later.is_none();
                El::new()
                    .class("col")
                    .with_style(|s| s.gap(6.0).padding([4.0, 8.0, 4.0, 44.0]))
                    .child(details(iv))
                    .child(
                        El::new()
                            .class("row")
                            .with_style(|s| s.gap(10.0))
                            .child_opt(pressable.then(|| {
                                El::button(
                                    if iv.ready {
                                        "Give the order"
                                    } else if iv.acting {
                                        "Under way"
                                    } else {
                                        "Recharging"
                                    },
                                    format!("isle.gear.use:{}:{slot}", p.vessel),
                                )
                                .class("brass_button")
                                .disabled(!iv.ready)
                            }))
                            .child(
                                El::button(
                                    "Take ashore",
                                    format!("isle.gear.unsocket:{}:{slot}", p.vessel),
                                )
                                .class("brass_button_dark"),
                            ),
                    )
            }
            None => {
                let fits: Vec<&ItemView> = p
                    .fits
                    .iter()
                    .filter(|(s, _)| *s == slot)
                    .filter_map(|(_, uid)| shelf.iter().find(|iv| iv.uid == *uid))
                    .collect();
                let mut pick = El::new()
                    .class("col")
                    .with_style(|s| s.gap(6.0).padding([4.0, 8.0, 4.0, 44.0]));
                pick = pick.child(brass::label(
                    if fits.is_empty() {
                        "Nothing on the shelf fits this slot"
                    } else if slot == 0 {
                        "Sign on a captain"
                    } else {
                        "Fit gear from the shelf"
                    },
                    true,
                ));
                pick.child(
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(6.0).wrap(true))
                        .children(fits.into_iter().map(|iv| {
                            El::new()
                                .class("col")
                                .with_style(|s| s.gap(2.0).width(70.0).align_items(Align::Center))
                                .child(
                                    brass::socket_with(
                                        Some((iv.icon, ui_rank(iv.rank))),
                                        44.0,
                                        &format!("isle.gear.socket:{}:{slot}:{}", p.vessel, iv.uid),
                                        &socket_state(iv),
                                        "",
                                    )
                                    .tooltip(iv.name, tip_text(iv)),
                                )
                                .child(
                                    El::text(iv.name)
                                        .class("ink")
                                        .text_style(|t| t.size(12.0).center())
                                        .pick_ignore(),
                                )
                        })),
                )
            }
        });
    }
    col
}

fn rule() -> El {
    El::new().with_style(|s| {
        s.height(1.0)
            .width(engine_ui::style::pct(100.0))
            .background(engine_ui::style::Fill::Solid(
                Colors::brass_dark().with_alpha(0.35),
            ))
            .pick(Pick::Ignore)
    })
}

/// The vessels, those with anything aboard first.
fn panels(world: &mut World) -> Vec<GearPanel> {
    let mut v = crate::gear::fleet(world);
    v.sort_by_key(|p| !p.slots.iter().any(|g| g.item.is_some()));
    v
}

fn body(world: &mut World) -> Vec<El> {
    let picked = world.resource::<Fleet>().picked;
    let shelf = items::shelf_view(world);
    let list = panels(world);
    if list.is_empty() {
        return vec![
            El::text("No vessels yet. Build a harbour and a shipyard.")
                .class("ink")
                .text_style(|t| t.size(14.0).italic())
                .with_style(|s| s.color(Colors::ink_dim()))
                .pick_ignore(),
        ];
    }
    let open = world.resource::<Fleet>().voyage;
    let mut out = Vec::new();
    for (i, p) in list.iter().enumerate() {
        if i > 0 {
            out.push(rule());
        }
        let mine = picked.filter(|(v, _)| *v == p.vessel).map(|(_, s)| s);
        let v = vessel_entity(world, p.vessel).and_then(|e| world.get::<Vessel>(e).cloned());
        let status = v.as_ref().map(|v| status(world, v).1).unwrap_or_default();
        let stays = v.as_ref().and_then(why_stays);
        let sailing = (open == Some(p.vessel)).then(|| voyage(world, p.vessel));
        out.push(row(p, mine, &shelf, &status, sailing, stays));
    }
    out
}

fn sig(world: &mut World) -> String {
    let picked = world.resource::<Fleet>().picked;
    let mut list = panels(world);
    for p in &mut list {
        for g in &mut p.slots {
            if let Some(iv) = &mut g.item {
                iv.left_s = 0.0;
                iv.cool_s = 0.0;
                iv.fill = 0.0;
            }
        }
    }
    let open = world.resource::<Fleet>().voyage;
    let doing: Vec<String> = {
        let mut q = world.query::<&Vessel>();
        let vs: Vec<Vessel> = q.iter(world).cloned().collect();
        vs.iter()
            .map(|v| {
                format!(
                    "{}:{}:{}",
                    v.name,
                    status(world, v).0,
                    crate::settle::free(v)
                )
            })
            .collect()
    };
    let claims: String = (0..island::islands().len())
        .map(|i| {
            if crate::settle::claimed(world, i) {
                '1'
            } else {
                '0'
            }
        })
        .collect();
    format!(
        "{}|{}|{picked:?}|{open:?}|{}|{claims}",
        serde_json::to_string(&list).unwrap_or_default(),
        serde_json::to_string(&items::shelf_view(world)).unwrap_or_default(),
        doing.join(",")
    )
}

fn el(world: &mut World) -> El {
    let n = crate::gear::fleet(world).len();
    let left = if world.resource::<super::shelf::Shelf>().open {
        478.0
    } else {
        22.0
    };
    let page = brass::parchment()
        .with_style(move |s| {
            s.position(Position::Absolute)
                .left(left)
                .top(150.0)
                .width(560.0)
                .padding([18.0, 22.0])
                .gap(10.0)
                .direction(Direction::Column)
                .enter(Enter::slide(0.38, -70.0))
        })
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(12.0).align_items(Align::Center))
                .child(brass::badge("isle_anchor", 52.0))
                .child(
                    El::new()
                        .class("col")
                        .with_style(|s| s.gap(0.0).grow(1.0))
                        .child(El::text("Ships' gear").class("ink_title").pick_ignore())
                        .child(brass::label(
                            &format!(
                                "{n} vessels · a captain and gear aboard each · click a socket"
                            ),
                            true,
                        )),
                )
                .child(
                    brass::medallion("isle_close", 32.0, "isle.fleet")
                        .tooltip("", "Close [color=#6a5339](G)[/color]"),
                ),
        )
        .child(brass::divider(516.0))
        .child(
            El::new()
                .class("scroll")
                .id(LIST_ID)
                .with_style(|s| s.direction(Direction::Column).gap(2.0).max_height(760.0))
                .children(body(world)),
        );
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(page)
}

/// Open or close the gear screen.
pub fn toggle(world: &mut World) {
    if world.resource::<Fleet>().open {
        engine_ui::screens::hide(world, SCREEN, true);
        let mut f = world.resource_mut::<Fleet>();
        f.open = false;
        f.picked = None;
        f.voyage = None;
        return;
    }
    let screen = engine_ui::node::Screen {
        key: SCREEN.into(),
        modal: false,
        layer: 2,
        closable: false,
        ..Default::default()
    };
    let el = el(world);
    let s = sig(world);
    if engine_ui::screens::spawn_screen(world, screen, &el).is_ok() {
        let mut f = world.resource_mut::<Fleet>();
        f.open = true;
        f.sig = s;
    }
}

/// Rebuild the list when what it shows changed; move the sockets' clocks every tick.
pub fn refresh(world: &mut World) {
    if !world.resource::<Fleet>().open {
        return;
    }
    let s = sig(world);
    if world.resource::<Fleet>().sig != s {
        let els = body(world);
        if let Some(e) = engine_ui::input::find_id(world, LIST_ID) {
            let _ = engine_ui::el::replace_children(world, e, &els);
            world.resource_mut::<Fleet>().sig = s;
        }
    }
    for p in crate::gear::fleet(world) {
        if let Some(e) = vessel_entity(world, p.vessel)
            && let Some(v) = world.get::<Vessel>(e).cloned()
        {
            let text = status(world, &v).1;
            if let Some(n) = engine_ui::input::find_id(world, &status_id(p.vessel))
                && let Some(mut t) = world.get_mut::<engine_ui::node::Text>(n)
                && t.text != text
            {
                t.text = text;
            }
        }
        for g in &p.slots {
            if let Some(iv) = &g.item
                && let Some(e) = engine_ui::input::find_id(world, &state_id(p.vessel, g.slot))
                && let Some(mut st) = world.get_mut::<SocketState>(e)
            {
                let new = socket_state(iv);
                if *st != new {
                    *st = new;
                }
            }
        }
    }
}

/// `isle.fleet*` buttons. Returns false for anything else.
pub fn action(world: &mut World, a: &str) -> bool {
    if a == "isle.fleet" {
        toggle(world);
    } else if let Some(rest) = a.strip_prefix("isle.fleet.pick:") {
        let n: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [v, s] = n[..] {
            let mut f = world.resource_mut::<Fleet>();
            let pick = Some((v, s as usize));
            f.picked = if f.picked == pick { None } else { pick };
        }
        refresh(world);
    } else if let Some(rest) = a.strip_prefix("isle.fleet.voyage:") {
        if let Ok(v) = rest.parse::<u32>() {
            let mut f = world.resource_mut::<Fleet>();
            f.voyage = if f.voyage == Some(v) { None } else { Some(v) };
        }
        refresh(world);
    } else if let Some(rest) = a.strip_prefix("isle.fleet.goto:") {
        let n: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [v, i] = n[..]
            && (i as usize) < island::islands().len()
        {
            let to = landfall(world, i as usize);
            send(world, v, to, island::islands()[i as usize].name);
        }
    } else {
        return false;
    }
    true
}

/// Send vessel `v` to `to`, and say so (or why not).
fn send(world: &mut World, v: u32, to: DVec2, place: &str) {
    let Some(e) = vessel_entity(world, v) else {
        return;
    };
    let name = world
        .get::<Vessel>(e)
        .map(|v| v.name.clone())
        .unwrap_or_default();
    let (kind, title, text) = match crate::settle::sail_to(world, e, to) {
        Ok(()) => (
            engine_ui::toast::ToastKind::Info,
            format!("{name} sails"),
            format!("Bound for {place}."),
        ),
        Err(why) => (
            engine_ui::toast::ToastKind::Warn,
            "She stays".to_string(),
            why,
        ),
    };
    if world.get_resource::<engine_ui::theme::Theme>().is_some() {
        engine_ui::toast::notify(world, kind, "isle_compass", &title, &text, 5.0);
    }
    world.resource_mut::<Fleet>().voyage = None;
    refresh(world);
}

/// The chart was clicked at `at`: if a "sail to" list is open, send that vessel there and
/// return true (the view stays where it is).
pub fn chart_clicked(world: &mut World, at: DVec2) -> bool {
    let Some(v) = world
        .get_resource::<Fleet>()
        .filter(|f| f.open)
        .and_then(|f| f.voyage)
    else {
        return false;
    };
    // A click on land means that island: she stands off it instead of running ashore.
    let (to, place) = match island::island_at(at.x, at.y) {
        Some(i) => (landfall(world, i), island::islands()[i].name.to_string()),
        None => match waters(at).as_str() {
            "open water" => (at, "open water".to_string()),
            w => (at, format!("the waters off {w}")),
        },
    };
    send(world, v, to, &place);
    true
}

/// After an `isle.gear.*` action: show the result at once.
pub fn after_gear_action(world: &mut World, a: &str) {
    // A fitted item: keep its socket picked, now showing the item.
    if let Some(rest) = a.strip_prefix("isle.gear.socket:") {
        let n: Vec<u32> = rest.split(':').filter_map(|s| s.parse().ok()).collect();
        if let [v, s, _] = n[..] {
            world.resource_mut::<Fleet>().picked = Some((v, s as usize));
        }
    }
    refresh(world);
}

/// `--hud-fleet=k`: open the gear screen with socket `k` picked on the first vessel that has
/// an order or a consumable there (or the first vessel).
pub fn demo(world: &mut World, k: usize) {
    if !world.resource::<Fleet>().open {
        toggle(world);
    }
    let list = panels(world);
    let pressable = |p: &&GearPanel| {
        p.slots
            .get(k)
            .and_then(|g| g.item.as_ref())
            .is_some_and(|iv| matches!(iv.way, Way::Order { .. } | Way::Consumable { .. }))
    };
    if let Some(p) = list.iter().find(pressable).or(list.first()) {
        world.resource_mut::<Fleet>().picked = Some((p.vessel, k));
        // `--hud-fleet-use`: give that order, to show it under way.
        if std::env::args().any(|a| a == "--hud-fleet-use") {
            crate::tools::action(world, &format!("isle.gear.use:{}:{k}", p.vessel));
        }
    }
    let free = |world: &mut World| -> Vec<u32> {
        panels(world)
            .iter()
            .map(|p| p.vessel)
            .filter(|v| {
                vessel_entity(world, *v)
                    .and_then(|e| world.get::<Vessel>(e))
                    .is_some_and(crate::settle::free)
            })
            .collect()
    };
    // The tour's ships are all on routes: stand down the first two ships for the shots.
    let wants = std::env::args().any(|a| a == "--hud-voyage" || a.starts_with("--hud-voyage-go="));
    while wants && free(world).len() < 2 {
        let busy = {
            let mut q = world.query::<(Entity, &Vessel)>();
            q.iter(world)
                .find(|(_, v)| v.route.is_some() && !v.craft.flies())
                .map(|(e, _)| e)
        };
        let Some(e) = busy else {
            break;
        };
        if crate::orders::unassign(world, e).is_err() {
            break;
        }
    }
    // `--hud-voyage-go=i`: send the first free vessel to island `i`.
    if let Some(i) =
        std::env::args().find_map(|a| a.strip_prefix("--hud-voyage-go=")?.parse::<u32>().ok())
        && let Some(v) = free(world).first().copied()
    {
        action(world, &format!("isle.fleet.goto:{v}:{i}"));
    }
    // `--hud-voyage`: open the "sail to" list of the first free vessel.
    if std::env::args().any(|a| a == "--hud-voyage")
        && let Some(v) = free(world).first().copied()
    {
        world.resource_mut::<Fleet>().voyage = Some(v);
    }
    refresh(world);
}
